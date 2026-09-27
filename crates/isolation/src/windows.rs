//! Narrow direct bindings to the Windows SDK kernel32 ABI (no downloaded Win32 crate).
//! All OS/unsafe operations stay in this module. No network path is probed.
use crate::RootError;
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

// Metadata-only FILE_READ_ATTRIBUTES does NOT pin directory rename on NTFS.
// FILE_LIST_DIRECTORY (0x1) grants directory read access; retained handles
// with no FILE_SHARE_DELETE reject ordinary leaf/ancestor renames.
const FILE_LIST_DIRECTORY: u32 = 0x1;
const FILE_READ_ATTRIBUTES: u32 = 0x80;
const DIRECTORY_READ: u32 = FILE_LIST_DIRECTORY | FILE_READ_ATTRIBUTES;
// No FILE_SHARE_DELETE: every retained ancestor and root is rename/delete pinned.
// A conflicting pre-existing DELETE handle makes resolution fail closed.
const SHARE_PINNED: u32 = 0x1 | 0x2;
const OPEN_EXISTING: u32 = 3;
const OPEN_ALWAYS: u32 = 4;
const READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const LOCK_EXCLUSIVE_NOWAIT: u32 = 0x1 | 0x2; // LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK
const DIRECTORY_OPEN: u32 = 0x0200_0000 | 0x0020_0000; // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
const REPARSE_POINT: u32 = 0x400;
const DIRECTORY: u32 = 0x10;
const DRIVE_FIXED: u32 = 3;
type Handle = *mut c_void;
const INVALID: Handle = -1isize as Handle;
#[repr(C)]
#[allow(non_snake_case)]
struct FileInfo {
    dwFileAttributes: u32,
    ftCreationTime: [u32; 2],
    ftLastAccessTime: [u32; 2],
    ftLastWriteTime: [u32; 2],
    dwVolumeSerialNumber: u32,
    nFileSizeHigh: u32,
    nFileSizeLow: u32,
    nNumberOfLinks: u32,
    nFileIndexHigh: u32,
    nFileIndexLow: u32,
}
#[link(name = "kernel32")]
extern "system" {
    fn CreateFileW(
        name: *const u16,
        access: u32,
        share: u32,
        security: *const c_void,
        creation: u32,
        flags: u32,
        template: Handle,
    ) -> Handle;
    fn CloseHandle(handle: Handle) -> i32;
    fn LockFileEx(
        handle: Handle,
        flags: u32,
        reserved: u32,
        low: u32,
        high: u32,
        offset: *mut Overlapped,
    ) -> i32;
    fn GetFileInformationByHandle(handle: Handle, info: *mut FileInfo) -> i32;
    fn GetFinalPathNameByHandleW(handle: Handle, buffer: *mut u16, len: u32, flags: u32) -> u32;
    fn GetVolumeInformationW(
        root: *const u16,
        volume: *mut u16,
        volume_len: u32,
        serial: *mut u32,
        max_component: *mut u32,
        flags: *mut u32,
        fs: *mut u16,
        fs_len: u32,
    ) -> i32;
    fn GetDriveTypeW(root: *const u16) -> u32;
}
// minwinbase.h OVERLAPPED: two ULONG_PTR, two DWORD offsets, HANDLE event (x64).
#[repr(C)]
struct Overlapped {
    internal: usize,
    internal_high: usize,
    offset: u32,
    offset_high: u32,
    event: Handle,
}
struct OwnedHandle(Handle);
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}

pub(super) struct Directory {
    pub path: PathBuf,
    pub volume: u32,
    pub file: u64,
    pub ancestors: Vec<(u32, u64)>,
    // The full chain, not just leaf: dropping this owner releases every rename pin.
    _handles: Vec<OwnedHandle>,
}
fn wide(s: &OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}
fn classify(err: std::io::Error) -> RootError {
    match err.raw_os_error() {
        Some(2 | 3) => RootError::Missing,
        Some(5 | 32) => RootError::Inaccessible,
        _ => RootError::Unsupported,
    }
}
impl Directory {
    pub(super) fn open(path: &Path) -> Result<Self, RootError> {
        let text = path.as_os_str().to_string_lossy();
        let volume_root = PathBuf::from(&text[..3]);
        let drive = wide(volume_root.as_os_str());
        // Require local fixed drive and NTFS BEFORE opening any component.
        if unsafe { GetDriveTypeW(drive.as_ptr()) } != DRIVE_FIXED {
            return Err(RootError::Unsupported);
        }
        let mut fs = [0u16; 32];
        if unsafe {
            GetVolumeInformationW(
                drive.as_ptr(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                fs.as_mut_ptr(),
                fs.len() as u32,
            )
        } == 0
        {
            return Err(classify(std::io::Error::last_os_error()));
        }
        let fs_name =
            String::from_utf16_lossy(&fs[..fs.iter().position(|c| *c == 0).unwrap_or(fs.len())]);
        if fs_name != "NTFS" {
            return Err(RootError::Unsupported);
        }
        let mut current = volume_root.clone();
        let mut ancestors = Vec::new();
        let mut last = None;
        // Each prefix is opened as a reparse point itself, not followed; rejection
        // happens before descending into it. Hold all handles until final resolution.
        let mut handles = Vec::new();
        for component in std::iter::once(None).chain(path.components().skip(2).map(Some)) {
            if let Some(component) = component {
                current.push(component.as_os_str());
            }
            let w = wide(current.as_os_str());
            let raw = unsafe {
                CreateFileW(
                    w.as_ptr(),
                    DIRECTORY_READ,
                    SHARE_PINNED,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    DIRECTORY_OPEN,
                    std::ptr::null_mut(),
                )
            };
            if raw == INVALID {
                return Err(classify(std::io::Error::last_os_error()));
            }
            let handle = OwnedHandle(raw);
            let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
            if unsafe { GetFileInformationByHandle(handle.0, info.as_mut_ptr()) } == 0 {
                return Err(classify(std::io::Error::last_os_error()));
            }
            let info = unsafe { info.assume_init() };
            if info.dwFileAttributes & REPARSE_POINT != 0 {
                return Err(RootError::Reparse);
            }
            if info.dwFileAttributes & DIRECTORY == 0 {
                return Err(RootError::Invalid);
            }
            let identity = (
                info.dwVolumeSerialNumber,
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            );
            ancestors.push(identity);
            last = Some(identity);
            handles.push(handle);
        }
        // Validate the entire chain again after ALL prefixes are pinned. Earlier
        // lookup races cannot authorize a leaf reached via a substituted ancestor.
        let mut verify = volume_root.clone();
        for (index, component) in std::iter::once(None)
            .chain(path.components().skip(2).map(Some))
            .enumerate()
        {
            if let Some(component) = component {
                verify.push(component.as_os_str());
            }
            let w = wide(verify.as_os_str());
            let raw = unsafe {
                CreateFileW(
                    w.as_ptr(),
                    DIRECTORY_READ,
                    SHARE_PINNED,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    DIRECTORY_OPEN,
                    std::ptr::null_mut(),
                )
            };
            if raw == INVALID {
                return Err(classify(std::io::Error::last_os_error()));
            }
            let check = OwnedHandle(raw);
            let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
            if unsafe { GetFileInformationByHandle(check.0, info.as_mut_ptr()) } == 0 {
                return Err(classify(std::io::Error::last_os_error()));
            }
            let info = unsafe { info.assume_init() };
            let id = (
                info.dwVolumeSerialNumber,
                (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
            );
            if info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) != DIRECTORY
                || id != ancestors[index]
            {
                return Err(RootError::Reparse);
            }
        }
        let handle = handles.last().ok_or(RootError::Invalid)?;
        let mut buf = vec![0u16; 256];
        let len = loop {
            // Return length is UTF-16 code units (excluding NUL on success).
            // A too-small buffer returns required length including NUL; retry
            // with a bounded buffer rather than accepting truncated paths.
            let len = unsafe {
                GetFinalPathNameByHandleW(handle.0, buf.as_mut_ptr(), buf.len() as u32, 0)
            } as usize;
            if len == 0 {
                return Err(classify(std::io::Error::last_os_error()));
            }
            if len < buf.len() {
                break len;
            }
            if len >= 32768 {
                return Err(RootError::Unsupported);
            }
            buf.resize(len.max(buf.len() * 2).min(32768), 0);
        };
        if buf[len] != 0 || buf[..len].contains(&0) {
            return Err(RootError::Unsupported);
        }
        let canonical = String::from_utf16(&buf[..len]).map_err(|_| RootError::Unsupported)?;
        let canonical = canonical
            .strip_prefix("\\\\?\\")
            .ok_or(RootError::Unsupported)?;
        if !canonical.as_bytes().get(1).is_some_and(|c| *c == b':') {
            return Err(RootError::Unsupported);
        }
        let (volume, file) = last.ok_or(RootError::Invalid)?;
        Ok(Self {
            path: PathBuf::from(canonical),
            volume,
            file,
            ancestors,
            _handles: handles,
        })
    }
}

/// Only a consumed, pinned Directory may open the lock. Never expose a pathname
/// or raw HANDLE as a write capability. Closing the handle releases the OS lock.
pub(super) struct NativeLease {
    _file: OwnedHandle,
}
impl NativeLease {
    pub(super) fn acquire(root: &Directory) -> Result<Self, crate::LeaseError> {
        let name = wide(root.path.join("host.lock").as_os_str());
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                READ_WRITE,
                SHARE_PINNED,
                std::ptr::null(),
                OPEN_ALWAYS,
                OPEN_REPARSE_POINT,
                std::ptr::null_mut(),
            )
        };
        if raw == INVALID {
            return Err(crate::LeaseError::UnsafeEntry);
        }
        let file = OwnedHandle(raw);
        let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
        if unsafe { GetFileInformationByHandle(file.0, info.as_mut_ptr()) } == 0 {
            return Err(crate::LeaseError::UnsafeEntry);
        }
        let info = unsafe { info.assume_init() };
        if info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) != 0
            || info.nNumberOfLinks != 1
            || info.dwVolumeSerialNumber != root.volume
        {
            return Err(crate::LeaseError::UnsafeEntry);
        }
        // Offsets [0,1); zeroed OVERLAPPED, synchronous nonblocking LockFileEx.
        let mut offset = Overlapped {
            internal: 0,
            internal_high: 0,
            offset: 0,
            offset_high: 0,
            event: std::ptr::null_mut(),
        };
        if unsafe { LockFileEx(file.0, LOCK_EXCLUSIVE_NOWAIT, 0, 1, 0, &mut offset) } == 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(33) {
                Err(crate::LeaseError::Busy)
            } else {
                Err(crate::LeaseError::UnsafeEntry)
            };
        }
        Ok(Self { _file: file })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t02b_07_permission_error_mapping_without_acl_changes() {
        assert_eq!(std::mem::size_of::<FileInfo>(), 52);
        assert_eq!(std::mem::align_of::<FileInfo>(), 4);
        assert_eq!(std::mem::size_of::<Handle>(), 8);
        assert_eq!(std::mem::size_of::<Overlapped>(), 32);
        assert_eq!(std::mem::align_of::<Overlapped>(), 8);
        assert!(matches!(
            classify(std::io::Error::from_raw_os_error(5)),
            RootError::Inaccessible
        ));
        assert!(matches!(
            classify(std::io::Error::from_raw_os_error(3)),
            RootError::Missing
        ));
    }
}
