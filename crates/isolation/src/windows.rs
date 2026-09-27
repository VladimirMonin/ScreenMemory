//! Narrow direct bindings to the Windows SDK kernel32 ABI (no downloaded Win32 crate).
//! All OS/unsafe operations stay in this module. No network path is probed.
use crate::RootError;
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};

const READ_ATTRIBUTES: u32 = 0x80;
const SHARE_ALL: u32 = 0x7;
const OPEN_EXISTING: u32 = 3;
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
    _handle: OwnedHandle,
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
        let mut current = volume_root;
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
                    READ_ATTRIBUTES,
                    SHARE_ALL,
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
        let handle = handles.pop().ok_or(RootError::Invalid)?;
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
            _handle: handle,
        })
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
