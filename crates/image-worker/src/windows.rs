use crate::wire::{self, Request, Response};
use sm_image::{sha256, ImageError, Options, Report, ABI, MAX_INPUT_BYTES};
use std::{
    fs::{File, OpenOptions},
    io::{Read, Write},
    os::windows::{
        fs::{MetadataExt, OpenOptionsExt},
        io::AsRawHandle,
        process::CommandExt,
    },
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JobObjectBasicAccountingInformation,
            JobObjectExtendedLimitInformation, QueryInformationJobObject, SetInformationJobObject,
            JOBOBJECT_BASIC_ACCOUNTING_INFORMATION, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JOB_OBJECT_LIMIT_ACTIVE_PROCESS, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
            JOB_OBJECT_LIMIT_PROCESS_MEMORY,
        },
        Threading::WaitForSingleObject,
    },
};

static BUSY: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Default)]
pub struct Cancellation {
    cancel: Arc<AtomicBool>,
    executing: Arc<AtomicBool>,
}
impl Cancellation {
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn is_executing(&self) -> bool {
        self.executing.load(Ordering::Acquire)
    }
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Acquire)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WorkerError {
    Unavailable,
    UntrustedPackage,
    Overloaded,
    InvalidRequest,
    Protocol,
    Codec(ImageError),
    Crashed,
    Timeout,
    Cancelled,
    TerminationUnproven,
}
/// Verified private result. Constructor is inaccessible to wire consumers. Still
/// not Saved, a DB witness or permission to overwrite a canonical.
pub struct WorkerResult {
    bytes: Vec<u8>,
    report: Report,
    pub operation_id: uuid::Uuid,
}
impl WorkerResult {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes
    }
    pub fn report(&self) -> &Report {
        &self.report
    }
    pub fn into_parts(self) -> (Vec<u8>, Report) {
        (self.bytes, self.report)
    }
}
/// Created only from a trusted package manifest, never a user/RPC path. The
/// read-only, write-denying pinned handle survives all child operations.
pub struct TrustedWorker {
    path: PathBuf,
    _pin: File,
}
struct Job(HANDLE);
impl Drop for Job {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl Job {
    fn new() -> Result<Self, WorkerError> {
        unsafe {
            let handle = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if handle.is_null() {
                return Err(WorkerError::Unavailable);
            }
            let job = Self(handle);
            let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = std::mem::zeroed();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
                | JOB_OBJECT_LIMIT_PROCESS_MEMORY
                | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
            info.BasicLimitInformation.ActiveProcessLimit = 1;
            info.ProcessMemoryLimit = 1280 * 1024 * 1024;
            if SetInformationJobObject(
                handle,
                JobObjectExtendedLimitInformation,
                (&info as *const JOBOBJECT_EXTENDED_LIMIT_INFORMATION).cast(),
                std::mem::size_of_val(&info) as u32,
            ) == 0
            {
                return Err(WorkerError::Unavailable);
            }
            Ok(job)
        }
    }
    fn no_active_children(&self) -> bool {
        let end = Instant::now() + Duration::from_secs(5);
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
            if Instant::now() >= end {
                return false;
            }
            thread::sleep(Duration::from_millis(2));
        }
    }
    fn assign(&self, child: &Child) -> Result<(), WorkerError> {
        if unsafe { AssignProcessToJobObject(self.0, child.as_raw_handle() as HANDLE) } == 0 {
            Err(WorkerError::Unavailable)
        } else {
            Ok(())
        }
    }
}
struct Permit {
    release: bool,
}
impl Drop for Permit {
    fn drop(&mut self) {
        if self.release {
            BUSY.store(false, Ordering::Release);
        }
    }
}
struct OwnedChild(Child);
impl OwnedChild {
    fn stop(&mut self) -> Result<(), WorkerError> {
        if self
            .0
            .try_wait()
            .map_err(|_| WorkerError::TerminationUnproven)?
            .is_none()
        {
            let _ = self.0.kill();
        }
        // OS wait proves native work stopped; a timeout is NOT that proof.
        if unsafe { WaitForSingleObject(self.0.as_raw_handle() as HANDLE, 5000) } != WAIT_OBJECT_0 {
            return Err(WorkerError::TerminationUnproven);
        }
        self.0
            .wait()
            .map_err(|_| WorkerError::TerminationUnproven)?;
        Ok(())
    }
}
impl Drop for OwnedChild {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}
impl TrustedWorker {
    pub fn from_manifest(path: &Path, expected_sha256: [u8; 32]) -> Result<Self, WorkerError> {
        if !path.is_absolute()
            || path
                .file_name()
                .is_none_or(|n| n != "shot-image-worker.exe")
        {
            return Err(WorkerError::UntrustedPackage);
        }
        for ancestor in path.ancestors() {
            let md =
                std::fs::symlink_metadata(ancestor).map_err(|_| WorkerError::UntrustedPackage)?;
            if md.file_attributes() & 0x400 != 0 {
                return Err(WorkerError::UntrustedPackage);
            }
        }
        let mut pin = OpenOptions::new()
            .read(true)
            .share_mode(1)
            .open(path)
            .map_err(|_| WorkerError::UntrustedPackage)?;
        let length = pin
            .metadata()
            .map_err(|_| WorkerError::UntrustedPackage)?
            .len();
        if length == 0 || length > 64 * 1024 * 1024 {
            return Err(WorkerError::UntrustedPackage);
        }
        let mut bytes = Vec::new();
        Read::by_ref(&mut pin)
            .take(length + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| WorkerError::UntrustedPackage)?;
        if bytes.len() as u64 != length || sha256(&bytes) != expected_sha256 {
            return Err(WorkerError::UntrustedPackage);
        }
        Ok(Self {
            path: path.into(),
            _pin: pin,
        })
    }
    /// Blocking API: caller runs this on its image lane, not UI/event/SQL thread.
    /// One heavy worker process in this host at a time; overload is fail-fast.
    pub fn execute(
        &self,
        operation_id: uuid::Uuid,
        input: Vec<u8>,
        options: Options,
        cancel: &Cancellation,
        deadline: Duration,
    ) -> Result<WorkerResult, WorkerError> {
        if operation_id.is_nil()
            || input.is_empty()
            || input.len() > MAX_INPUT_BYTES
            || deadline.is_zero()
            || deadline > Duration::from_secs(180)
        {
            return Err(WorkerError::InvalidRequest);
        }
        if cancel.cancelled() {
            return Err(WorkerError::Cancelled);
        }
        if BUSY
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return Err(WorkerError::Overloaded);
        }
        let mut permit = Permit { release: true };
        let mut end = Instant::now() + deadline.min(Duration::from_secs(30));
        let job = Job::new()?;
        let child = Command::new(&self.path)
            .creation_flags(0x08000000)
            .env_clear()
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| WorkerError::Unavailable)?;
        let mut child = OwnedChild(child);
        if let Err(e) = job.assign(&child.0) {
            if child.stop().is_err() {
                permit.release = false;
                return Err(WorkerError::TerminationUnproven);
            }
            return Err(e);
        }
        let mut stdin = child.0.stdin.take().ok_or(WorkerError::Unavailable)?;
        let mut stdout = child.0.stdout.take().ok_or(WorkerError::Unavailable)?;
        let input_sha256 = sha256(&input);
        let request = Request {
            abi: ABI.into(),
            operation_id,
            input_sha256,
            input_length: input.len() as u64,
            options: options.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(2);
        let writer_tx = tx.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let writer = thread::spawn(move || {
            let result = ready_rx
                .recv()
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::BrokenPipe))
                .and_then(|()| wire::write_header(&mut stdin, &request))
                .and_then(|()| stdin.write_all(&input));
            drop(stdin);
            let _ = writer_tx.send(Event::Write(result.is_ok()));
        });
        let reader = thread::spawn(move || {
            let result = wire::read_hello(&mut stdout)
                .and_then(|()| {
                    let _ = tx.send(Event::Hello);
                    wire::read_accepted(&mut stdout, operation_id, input_sha256)
                })
                .and_then(|()| {
                    let _ = tx.send(Event::Accepted);
                    wire::read_encoding(&mut stdout, operation_id)
                })
                .and_then(|()| {
                    let _ = tx.send(Event::Encoding);
                    wire::read_response(&mut stdout)
                });
            let _ = tx.send(Event::Read(Box::new(result)));
        });
        let outcome = loop {
            if cancel.cancelled() {
                break Err(WorkerError::Cancelled);
            }
            if Instant::now() >= end {
                break Err(WorkerError::Timeout);
            }
            match rx.recv_timeout(Duration::from_millis(10)) {
                Ok(Event::Write(false)) => break Err(WorkerError::Crashed),
                Ok(Event::Write(true)) => {}
                Ok(Event::Hello) => {
                    if ready_tx.try_send(()).is_err() {
                        break Err(WorkerError::Protocol);
                    }
                }
                Ok(Event::Accepted) => cancel.executing.store(true, Ordering::Release),
                Ok(Event::Encoding) => {
                    end = Instant::now()
                        + deadline.min(
                            if options.profile == sm_image::OutputProfile::AvifQuality90 {
                                Duration::from_secs(180)
                            } else {
                                Duration::from_secs(30)
                            },
                        );
                }
                Ok(Event::Read(result)) => break (*result).map_err(|_| WorkerError::Protocol),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break Err(WorkerError::Crashed),
            }
        };
        drop(ready_tx);
        let mut outcome = outcome;
        if outcome.is_ok() {
            loop {
                match child.0.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success() {
                            outcome = Err(WorkerError::Crashed);
                        }
                        break;
                    }
                    Ok(None) if Instant::now() < end => thread::sleep(Duration::from_millis(2)),
                    Ok(None) => {
                        outcome = Err(WorkerError::Timeout);
                        break;
                    }
                    Err(_) => {
                        outcome = Err(WorkerError::Unavailable);
                        break;
                    }
                }
            }
        }
        if child.stop().is_err() || !job.no_active_children() {
            permit.release = false;
            return Err(WorkerError::TerminationUnproven);
        }
        cancel.executing.store(false, Ordering::Release);
        // After process exit there are no child handles left to hold either pipe.
        if writer.join().is_err() || reader.join().is_err() {
            return Err(WorkerError::Protocol);
        }
        let (response, bytes) = outcome?;
        if response.abi != ABI || response.operation_id != operation_id {
            return Err(WorkerError::Protocol);
        }
        match (response.report, response.error) {
            (None, Some(e)) if bytes.is_empty() && response.output_length == 0 => {
                Err(WorkerError::Codec(e))
            }
            (Some(report), None) => {
                if report.source_sha256 != input_sha256
                    || report.output_sha256 != sha256(&bytes)
                    || report.byte_length != bytes.len() as u64
                    || report.byte_length != response.output_length
                    || report.method != options.method
                    || report.output_format != options.profile.format()
                    || report.profile_fingerprint != options.profile.fingerprint()
                    || report.dimensions.byte_len().is_err()
                {
                    return Err(WorkerError::Protocol);
                }
                let magic = match report.output_format {
                    sm_image::Format::Png => bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
                    sm_image::Format::Jpeg => bytes.starts_with(b"\xff\xd8\xff"),
                    sm_image::Format::Webp => {
                        bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP")
                    }
                    sm_image::Format::Avif => bytes.get(4..8) == Some(b"ftyp"),
                    _ => false,
                };
                if !magic {
                    return Err(WorkerError::Protocol);
                }
                Ok(WorkerResult {
                    bytes,
                    report,
                    operation_id,
                })
            }
            _ => Err(WorkerError::Protocol),
        }
    }
}
enum Event {
    Hello,
    Accepted,
    Encoding,
    Write(bool),
    Read(Box<std::io::Result<(Response, Vec<u8>)>>),
}
