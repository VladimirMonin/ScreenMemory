//! Narrow direct bindings to the Windows SDK kernel32 ABI (no downloaded Win32 crate).
//! All OS/unsafe operations stay in this module. No network path is probed.
use crate::{RootError, StagingError};
use std::ffi::{c_void, OsStr};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
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
const CREATE_NEW: u32 = 1;
const READ_WRITE: u32 = 0x8000_0000 | 0x4000_0000;
const READ_ONLY: u32 = 0x8000_0000 | FILE_READ_ATTRIBUTES;
const MOVEFILE_WRITE_THROUGH: u32 = 0x8;
const OPEN_REPARSE_POINT: u32 = 0x0020_0000;
const LOCK_EXCLUSIVE_NOWAIT: u32 = 0x1 | 0x2; // LOCKFILE_FAIL_IMMEDIATELY | LOCKFILE_EXCLUSIVE_LOCK
const DIRECTORY_OPEN: u32 = 0x0200_0000 | 0x0020_0000; // BACKUP_SEMANTICS | OPEN_REPARSE_POINT
pub(super) const REPARSE_POINT: u32 = 0x400;
pub(super) const DIRECTORY: u32 = 0x10;
// Windows SDK minwinbase.h FILE_INFO_BY_HANDLE_CLASS, not a path-based FindFirstFile.
const FILE_FULL_DIRECTORY_INFO: u32 = 14;
const FILE_FULL_DIRECTORY_RESTART_INFO: u32 = 15;
const NO_MORE_FILES: i32 = 18;
const MAX_STAGING_ENTRIES: usize = 4096;
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
    fn GetFileInformationByHandleEx(
        handle: Handle,
        info_class: u32,
        buffer: *mut c_void,
        buffer_size: u32,
    ) -> i32;
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
    fn MoveFileExW(source: *const u16, destination: *const u16, flags: u32) -> i32;
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
// Kernel HANDLE values are process-wide and may be moved to a single owner thread;
// no borrowed thread-affine state or concurrent mutable access crosses this edge.
unsafe impl Send for OwnedHandle {}
// Shared references only inspect immutable metadata; the owner cannot close the
// OS handle until all borrowed scoped children end. Win32 HANDLE is process-wide.
unsafe impl Sync for OwnedHandle {}
impl Drop for OwnedHandle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl OwnedHandle {
    fn into_file(self) -> std::fs::File {
        let raw = self.0;
        std::mem::forget(self);
        // A successfully validated exclusively-created file transfers sole handle ownership.
        unsafe { std::fs::File::from_raw_handle(raw) }
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

/// Require the opened child to be on the same volume and directly under the
/// retained parent identity, not merely under a matching pathname.
fn pinned_child(parent: &Directory, child: &Directory) -> Result<(), StagingError> {
    if child.volume != parent.volume
        || child.ancestors.len() != parent.ancestors.len() + 1
        || child.ancestors[..parent.ancestors.len()] != parent.ancestors
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(())
}

pub(super) fn staging_directory(data: &Directory) -> Result<Directory, StagingError> {
    sibling_directory(data, "staging")
}

pub(super) fn assets_directory(data: &Directory) -> Result<Directory, StagingError> {
    sibling_directory(data, "assets")
}

fn sibling_directory(data: &Directory, leaf: &str) -> Result<Directory, StagingError> {
    let parent = data.path.parent().ok_or(StagingError::UnsafeEntry)?;
    let path = parent.join(leaf);
    // First initialization is create_new, never adopt a different existing entry.
    match std::fs::create_dir(&path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(staging_io(error)),
    }
    let root = Directory::open(&path).map_err(|error| match error {
        RootError::Inaccessible => StagingError::AccessDenied,
        _ => StagingError::UnsafeEntry,
    })?;
    if root.volume != data.volume
        || root.ancestors.len() != data.ancestors.len()
        || root.ancestors[..data.ancestors.len() - 1] != data.ancestors[..data.ancestors.len() - 1]
        || root.file == data.file
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(root)
}

/// WinBase.h FILE_FULL_DIR_INFO: fixed header ends at offset 68 (FileName).
/// GetFileInformationByHandleEx queries the retained directory HANDLE itself; it
/// neither resolves a pathname nor opens child contents. NTFS names are <= 255 UTF-16
/// units, so 4096 bytes can hold at least one complete record per native call.
/// The first 4097th non-dot record fails closed. NTFS may also return . and ..;
/// reserve two additional native queries for those records and one for EOF.
pub(super) fn enumerate_staging(
    root: &Directory,
    mut visit: impl FnMut(&[u16], u32) -> Result<(), StagingError>,
) -> Result<(), StagingError> {
    let handle = root._handles.last().ok_or(StagingError::UnsafeEntry)?.0;
    let mut count = 0usize;
    for query in 0..=MAX_STAGING_ENTRIES + 2 {
        // Aligned storage, zeroed on every call; no uninitialized bytes or raw paths escape.
        let mut storage = [0u64; 512];
        let ok = unsafe {
            GetFileInformationByHandleEx(
                handle,
                if query == 0 {
                    FILE_FULL_DIRECTORY_RESTART_INFO
                } else {
                    FILE_FULL_DIRECTORY_INFO
                },
                storage.as_mut_ptr().cast(),
                std::mem::size_of_val(&storage) as u32,
            )
        };
        if ok == 0 {
            return if std::io::Error::last_os_error().raw_os_error() == Some(NO_MORE_FILES) {
                Ok(())
            } else {
                Err(StagingError::EnumerationIncomplete)
            };
        }
        let bytes = unsafe {
            std::slice::from_raw_parts(
                storage.as_ptr().cast::<u8>(),
                std::mem::size_of_val(&storage),
            )
        };
        let mut at = 0usize;
        loop {
            if at + 68 > bytes.len() {
                return Err(StagingError::EnumerationIncomplete);
            }
            let field = |offset: usize| -> u32 {
                u32::from_ne_bytes(bytes[at + offset..at + offset + 4].try_into().unwrap())
            };
            let next = field(0) as usize;
            let attributes = field(56);
            let name_len = field(60) as usize;
            let end = at.checked_add(68).and_then(|n| n.checked_add(name_len));
            if name_len == 0
                || name_len > 510
                || !name_len.is_multiple_of(2)
                || end.is_none_or(|n| n > bytes.len())
                || (next != 0
                    && (next < 68 + name_len || !next.is_multiple_of(8) || at + next > bytes.len()))
            {
                return Err(StagingError::EnumerationIncomplete);
            }
            let end = end.unwrap();
            let name: Vec<u16> = bytes[at + 68..end]
                .as_chunks::<2>()
                .0
                .iter()
                .map(|pair| u16::from_ne_bytes(*pair))
                .collect();
            if name != [b'.' as u16] && name != [b'.' as u16, b'.' as u16] {
                if count == MAX_STAGING_ENTRIES {
                    return Err(StagingError::EnumerationIncomplete);
                }
                count += 1;
                visit(&name, attributes)?;
            }
            if next == 0 {
                break;
            }
            at += next;
        }
    }
    Err(StagingError::EnumerationIncomplete)
}

pub(super) fn create_operation_directory(
    root: &Directory,
    canonical_id: &str,
) -> Result<Directory, StagingError> {
    let path = root.path.join(canonical_id);
    std::fs::create_dir(&path).map_err(staging_io)?;
    open_operation_directory(root, canonical_id)
}

// Only fixed or type-validated single components reach this helper. Existing
// directories are reopened/pinned, never trusted merely because create_dir collided.
pub(super) fn ensure_child_directory(
    root: &Directory,
    leaf: &str,
) -> Result<Directory, StagingError> {
    match std::fs::create_dir(root.path.join(leaf)) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(error) => return Err(staging_io(error)),
    }
    open_operation_directory(root, leaf)
}

fn staging_io(error: std::io::Error) -> StagingError {
    match error.raw_os_error() {
        Some(2 | 3) => StagingError::Missing,
        Some(80 | 183) => StagingError::AlreadyExists,
        Some(112 | 39) => StagingError::Full,
        Some(5) => StagingError::AccessDenied,
        _ => StagingError::UnsafeEntry,
    }
}

pub(super) fn open_operation_directory(
    root: &Directory,
    canonical_id: &str,
) -> Result<Directory, StagingError> {
    let operation =
        Directory::open(&root.path.join(canonical_id)).map_err(|error| match error {
            RootError::Missing => StagingError::Missing,
            RootError::Inaccessible => StagingError::AccessDenied,
            _ => StagingError::UnsafeEntry,
        })?;
    pinned_child(root, &operation)?;
    Ok(operation)
}

pub(super) fn create_temp(root: &Directory, leaf: &str) -> Result<std::fs::File, StagingError> {
    let path = wide(root.path.join(leaf).as_os_str());
    let raw = unsafe {
        CreateFileW(
            path.as_ptr(),
            READ_WRITE,
            SHARE_PINNED,
            std::ptr::null(),
            CREATE_NEW,
            OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID {
        return Err(staging_io(std::io::Error::last_os_error()));
    }
    let handle = OwnedHandle(raw);
    let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
    if unsafe { GetFileInformationByHandle(handle.0, info.as_mut_ptr()) } == 0 {
        return Err(StagingError::UnsafeEntry);
    }
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) != 0
        || info.nNumberOfLinks != 1
        || info.dwVolumeSerialNumber != root.volume
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(handle.into_file())
}

fn file_info(handle: Handle, root: &Directory) -> Result<FileInfo, StagingError> {
    let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
    if unsafe { GetFileInformationByHandle(handle, info.as_mut_ptr()) } == 0 {
        return Err(staging_io(std::io::Error::last_os_error()));
    }
    let info = unsafe { info.assume_init() };
    if info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) != 0
        || info.nNumberOfLinks != 1
        || info.dwVolumeSerialNumber != root.volume
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(info)
}

fn identity(info: &FileInfo) -> (u32, u64) {
    (
        info.dwVolumeSerialNumber,
        (u64::from(info.nFileIndexHigh) << 32) | u64::from(info.nFileIndexLow),
    )
}

// OPEN_REPARSE_POINT + no DELETE sharing retains the file against ordinary
// substitution while inspecting it. No caller-provided pathname enters this function.
fn inspect_fixed(root: &Directory, leaf: &str) -> Result<(std::fs::File, u64), StagingError> {
    let name = wide(root.path.join(leaf).as_os_str());
    let raw = unsafe {
        CreateFileW(
            name.as_ptr(),
            READ_ONLY,
            SHARE_PINNED,
            std::ptr::null(),
            OPEN_EXISTING,
            OPEN_REPARSE_POINT,
            std::ptr::null_mut(),
        )
    };
    if raw == INVALID {
        return Err(staging_io(std::io::Error::last_os_error()));
    }
    let handle = OwnedHandle(raw);
    let info = file_info(handle.0, root)?;
    let len = (u64::from(info.nFileSizeHigh) << 32) | u64::from(info.nFileSizeLow);
    Ok((handle.into_file(), len))
}

pub(super) fn open_fixed(
    root: &Directory,
    leaf: &str,
) -> Result<(std::fs::File, u64), StagingError> {
    inspect_fixed(root, leaf)
}

pub(super) fn publish_temp(
    root: &Directory,
    source: &str,
    target: &str,
    file: std::fs::File,
) -> Result<(), StagingError> {
    let expected = identity(&file_info(file.as_raw_handle(), root)?);
    let (source_handle, _) = inspect_fixed(root, source)?;
    if identity(&file_info(source_handle.as_raw_handle(), root)?) != expected {
        return Err(StagingError::UnsafeEntry);
    }
    // Refuse an existing target, especially a symlink/reparse point. MoveFileExW
    // without REPLACE_EXISTING is the final no-overwrite authority, not this preflight.
    match inspect_fixed(root, target) {
        Ok(_) => return Err(StagingError::AlreadyExists),
        Err(StagingError::Missing) => {}
        Err(error) => return Err(error),
    }
    drop(source_handle);
    drop(file);
    let target_leaf = target;
    let source = wide(root.path.join(source).as_os_str());
    let target = wide(root.path.join(target).as_os_str());
    // Both names are fixed children of the same pinned NTFS directory. Never use
    // COPY_ALLOWED or REPLACE_EXISTING. MoveFileExW is pathname-based after close:
    // a malicious same-user process retaining DELETE access can race this gap;
    // this capability is not a sandbox against that process.
    if unsafe { MoveFileExW(source.as_ptr(), target.as_ptr(), MOVEFILE_WRITE_THROUGH) } == 0 {
        return Err(staging_io(std::io::Error::last_os_error()));
    }
    let (published, _) = inspect_fixed(root, target_leaf)?;
    if identity(&file_info(published.as_raw_handle(), root)?) != expected {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(())
}

/// Pinned existing entries prevent replacement between preflight and SQLite open.
/// No name supplied by a caller can become a database/sidecar write target.
pub(super) struct DatabaseEntry {
    pub path: PathBuf,
    pub existed: bool,
    _pins: Vec<OwnedHandle>,
}
impl DatabaseEntry {
    pub(super) fn inspect(root: &Directory) -> Result<Self, crate::LeaseError> {
        let path = root.path.join("library.sqlite3");
        let mut pins = Vec::new();
        let mut existed = false;
        for (index, suffix) in ["", "-wal", "-shm", "-journal"].iter().enumerate() {
            let name = wide(
                root.path
                    .join(format!("library.sqlite3{suffix}"))
                    .as_os_str(),
            );
            let raw = unsafe {
                CreateFileW(
                    name.as_ptr(),
                    FILE_READ_ATTRIBUTES,
                    SHARE_PINNED,
                    std::ptr::null(),
                    OPEN_EXISTING,
                    OPEN_REPARSE_POINT,
                    std::ptr::null_mut(),
                )
            };
            if raw == INVALID {
                if std::io::Error::last_os_error().raw_os_error() == Some(2) {
                    continue;
                }
                return Err(crate::LeaseError::UnsafeEntry);
            }
            let handle = OwnedHandle(raw);
            let mut info = std::mem::MaybeUninit::<FileInfo>::uninit();
            if unsafe { GetFileInformationByHandle(handle.0, info.as_mut_ptr()) } == 0 {
                return Err(crate::LeaseError::UnsafeEntry);
            }
            let info = unsafe { info.assume_init() };
            if info.dwFileAttributes & (REPARSE_POINT | DIRECTORY) != 0
                || info.nNumberOfLinks != 1
                || info.dwVolumeSerialNumber != root.volume
            {
                return Err(crate::LeaseError::UnsafeEntry);
            }
            if index == 0 {
                existed = true;
            }
            pins.push(handle);
        }
        // Stray sidecars are not a valid fresh initialization state.
        if !existed && !pins.is_empty() {
            return Err(crate::LeaseError::UnsafeEntry);
        }
        Ok(Self {
            path,
            existed,
            _pins: pins,
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
