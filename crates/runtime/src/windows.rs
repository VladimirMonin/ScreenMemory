use crate::RuntimeError;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE},
    System::JobObjects::*,
};

pub(crate) fn system_root() -> Result<std::path::PathBuf, RuntimeError> {
    use std::os::windows::ffi::OsStringExt;
    let mut buffer = [0u16; 512];
    let n = unsafe {
        windows_sys::Win32::System::SystemInformation::GetWindowsDirectoryW(
            buffer.as_mut_ptr(),
            buffer.len() as u32,
        )
    } as usize;
    if n == 0 || n >= buffer.len() {
        return Err(RuntimeError::settled("SYSTEM_ROOT_UNAVAILABLE"));
    }
    let path = std::path::PathBuf::from(std::ffi::OsString::from_wide(&buffer[..n]));
    if !path.is_absolute() {
        return Err(RuntimeError::settled("SYSTEM_ROOT_UNAVAILABLE"));
    }
    Ok(path)
}

pub(crate) struct Job(HANDLE);
// The owned job handle may move with its supervisor; Windows handles are thread-independent.
unsafe impl Send for Job {}
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Job {
    pub(crate) fn new_and_assign(child: &tokio::process::Child) -> Result<Self, RuntimeError> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(RuntimeError::settled("CONTAINMENT_UNAVAILABLE"));
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.BasicLimitInformation.ActiveProcessLimit = 1;
            info.ProcessMemoryLimit = 512 * 1024 * 1024;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
                || AssignProcessToJobObject(
                    handle,
                    child
                        .raw_handle()
                        .ok_or_else(|| RuntimeError::settled("CONTAINMENT_UNAVAILABLE"))?
                        as HANDLE,
                ) == 0
            {
                return Err(RuntimeError::settled("CONTAINMENT_UNAVAILABLE"));
            }
            Ok(job)
        }
    }
    pub(crate) fn empty(&self) -> bool {
        let end = std::time::Instant::now() + std::time::Duration::from_secs(3);
        loop {
            let mut info: JOBOBJECT_BASIC_ACCOUNTING_INFORMATION = unsafe { std::mem::zeroed() };
            if unsafe {
                QueryInformationJobObject(
                    self.0,
                    JobObjectBasicAccountingInformation,
                    (&mut info as *mut JOBOBJECT_BASIC_ACCOUNTING_INFORMATION).cast(),
                    std::mem::size_of_val(&info) as u32,
                    std::ptr::null_mut(),
                )
            } == 0
            {
                return false;
            }
            if info.ActiveProcesses == 0 {
                return true;
            }
            if std::time::Instant::now() >= end {
                return false;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }
}
