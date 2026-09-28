//! Narrow Win32 security and peer ABI. No impersonation/thread-token state crosses awaits.
use crate::PipeError;
use std::{
    ffi::c_void,
    os::windows::{
        ffi::OsStrExt,
        io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle},
    },
    path::Path,
    ptr,
};
use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, LocalFree, HANDLE},
    Security::{
        Authorization::{
            ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW,
            ConvertStringSidToSidW, GetSecurityInfo, SetSecurityInfo, SE_KERNEL_OBJECT,
        },
        EqualSid, GetAce, GetSecurityDescriptorDacl, GetTokenInformation, TokenGroups, TokenUser,
        ACL, DACL_SECURITY_INFORMATION, PSID, SECURITY_ATTRIBUTES, TOKEN_GROUPS, TOKEN_QUERY,
        TOKEN_USER,
    },
    Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING},
    System::{
        Pipes::{
            GetNamedPipeClientProcessId, GetNamedPipeClientSessionId, GetNamedPipeServerProcessId,
            GetNamedPipeServerSessionId,
        },
        RemoteDesktop::ProcessIdToSessionId,
        SystemServices::SE_GROUP_LOGON_ID,
        Threading::{
            GetCurrentProcess, GetCurrentProcessId, OpenProcess, OpenProcessToken,
            QueryFullProcessImageNameW, PROCESS_QUERY_LIMITED_INFORMATION,
        },
    },
};

pub(crate) const BOOTSTRAP: u32 = 0x0016_019f; // logon SID only; duplex + synchronize + WRITE_DAC. No DELETE/WRITE_OWNER.
pub(crate) const FINAL: u32 = 0x0010_0083; // logon SID read/write data, read attributes, synchronize; no instance creation.
struct Handle(HANDLE);
impl Drop for Handle {
    fn drop(&mut self) {
        unsafe {
            CloseHandle(self.0);
        }
    }
}
struct Local(*mut c_void);
impl Drop for Local {
    fn drop(&mut self) {
        unsafe {
            LocalFree(self.0);
        }
    }
}
fn failure(code: &'static str) -> PipeError {
    PipeError::native(code, unsafe { GetLastError() })
}
fn wide(s: &str) -> Vec<u16> {
    std::ffi::OsStr::new(s)
        .encode_wide()
        .chain(Some(0))
        .collect()
}
fn process_token(process: HANDLE) -> Result<Handle, PipeError> {
    let mut token = ptr::null_mut();
    if unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) } == 0 {
        return Err(failure("TOKEN_OPEN"));
    }
    Ok(Handle(token))
}
fn token_info(token: HANDLE, kind: i32) -> Result<Vec<u64>, PipeError> {
    let mut len = 0;
    unsafe {
        GetTokenInformation(token, kind, ptr::null_mut(), 0, &mut len);
    }
    if !(16..=65536).contains(&len) {
        return Err(PipeError::policy("TOKEN_SIZE"));
    }
    let mut buffer = vec![0u64; (len as usize).div_ceil(8)];
    if unsafe { GetTokenInformation(token, kind, buffer.as_mut_ptr().cast(), len, &mut len) } == 0 {
        return Err(failure("TOKEN_INFO"));
    }
    Ok(buffer)
}
fn logon_sid(token: HANDLE) -> Result<Vec<u8>, PipeError> {
    let info = token_info(token, TokenGroups)?;
    let groups = info.as_ptr().cast::<TOKEN_GROUPS>();
    let count = unsafe { (*groups).GroupCount } as usize;
    if count > 1024
        || std::mem::offset_of!(TOKEN_GROUPS, Groups)
            + count * std::mem::size_of::<windows_sys::Win32::Security::SID_AND_ATTRIBUTES>()
            > info.len() * 8
    {
        return Err(PipeError::policy("GROUP_COUNT"));
    }
    let entries = unsafe { (*groups).Groups.as_ptr() };
    for index in 0..count {
        let entry = unsafe { &*entries.add(index) };
        if entry.Attributes & (SE_GROUP_LOGON_ID as u32) == (SE_GROUP_LOGON_ID as u32) {
            return copy_sid(entry.Sid);
        }
    }
    Err(PipeError::policy("LOGON_SID_MISSING"))
}
fn copy_sid(sid: PSID) -> Result<Vec<u8>, PipeError> {
    let len = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) } as usize;
    if !(8..=68).contains(&len) {
        return Err(PipeError::policy("SID_SIZE"));
    }
    let mut bytes = vec![0; len];
    if unsafe { windows_sys::Win32::Security::CopySid(len as u32, bytes.as_mut_ptr().cast(), sid) }
        == 0
    {
        return Err(failure("SID_COPY"));
    }
    Ok(bytes)
}
fn token_user(token: HANDLE) -> Result<Vec<u8>, PipeError> {
    let info = token_info(token, TokenUser)?;
    copy_sid(unsafe { (*info.as_ptr().cast::<TOKEN_USER>()).User.Sid })
}
fn sid_text(sid: &[u8]) -> Result<String, PipeError> {
    let mut raw = ptr::null_mut();
    if unsafe { ConvertSidToStringSidW(sid.as_ptr().cast_mut().cast(), &mut raw) } == 0 {
        return Err(failure("SID_TEXT"));
    }
    let text = Local(raw.cast());
    let mut n = 0;
    while n < 256 && unsafe { *raw.add(n) } != 0 {
        n += 1;
    }
    if n == 256 {
        return Err(PipeError::policy("SID_TEXT_SIZE"));
    }
    String::from_utf16(unsafe { std::slice::from_raw_parts(text.0.cast(), n) })
        .map_err(|_| PipeError::policy("SID_TEXT_INVALID"))
}
fn owner_rights_sid() -> Result<Local, PipeError> {
    let mut raw = ptr::null_mut();
    if unsafe { ConvertStringSidToSidW(wide("S-1-3-4").as_ptr(), &mut raw) } == 0 {
        return Err(failure("OWNER_RIGHTS_SID"));
    }
    Ok(Local(raw))
}
pub(crate) struct Security {
    initial: Local,
    sealed: Local,
    logon: Vec<u8>,
    owner_rights: Local,
}
impl Security {
    pub(crate) fn current() -> Result<Self, PipeError> {
        let token = process_token(unsafe { GetCurrentProcess() })?;
        let logon = logon_sid(token.0)?;
        let sid = sid_text(&logon)?;
        let initial = Self::descriptor(&format!(
            "D:P(A;;0x{BOOTSTRAP:08x};;;{sid})(A;;0x00000000;;;OW)"
        ))?;
        let sealed = Self::descriptor(&format!(
            "D:P(A;;0x{FINAL:08x};;;{sid})(A;;0x00000000;;;OW)"
        ))?;
        Ok(Self {
            initial,
            sealed,
            logon,
            owner_rights: owner_rights_sid()?,
        })
    }
    fn descriptor(sddl: &str) -> Result<Local, PipeError> {
        let mut raw = ptr::null_mut();
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                wide(sddl).as_ptr(),
                1,
                &mut raw,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(failure("DESCRIPTOR_CREATE"));
        }
        Ok(Local(raw))
    }
    fn acl(descriptor: *mut c_void) -> Result<*mut ACL, PipeError> {
        let mut present = 0;
        let mut defaulted = 0;
        let mut acl = ptr::null_mut();
        if unsafe { GetSecurityDescriptorDacl(descriptor, &mut present, &mut acl, &mut defaulted) }
            == 0
        {
            return Err(failure("DACL_QUERY"));
        }
        if present == 0 || acl.is_null() || defaulted != 0 {
            return Err(PipeError::policy("DACL_INVALID"));
        }
        Ok(acl)
    }
    pub(crate) fn attributes(&self) -> SECURITY_ATTRIBUTES {
        SECURITY_ATTRIBUTES {
            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.initial.0,
            bInheritHandle: 0,
        }
    }
    pub(crate) fn seal(&self, server: &NamedPipeServer) -> Result<(), PipeError> {
        let acl = Self::acl(self.sealed.0)?;
        let code = unsafe {
            SetSecurityInfo(
                server.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                acl,
                ptr::null(),
            )
        };
        if code != 0 {
            return Err(PipeError::native("SEAL", code));
        }
        self.verify(server)
    }
    pub(crate) fn verify(&self, server: &NamedPipeServer) -> Result<(), PipeError> {
        let mut raw = ptr::null_mut();
        let mut acl = ptr::null_mut();
        let code = unsafe {
            GetSecurityInfo(
                server.as_raw_handle(),
                SE_KERNEL_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut acl,
                ptr::null_mut(),
                &mut raw,
            )
        };
        if code != 0 {
            return Err(PipeError::native("SEAL_READBACK", code));
        }
        let descriptor = Local(raw);
        if descriptor.0.is_null() || acl.is_null() {
            return Err(PipeError::policy("SEAL_NULL"));
        }
        if unsafe { (*acl).AceCount } != 2 {
            return Err(PipeError::policy("SEAL_ACE_COUNT"));
        }
        for (index, sid, mask) in [
            (0, self.logon.as_ptr().cast_mut().cast(), FINAL),
            (1, self.owner_rights.0, 0),
        ] {
            let mut ace = ptr::null_mut();
            if unsafe { GetAce(acl, index, &mut ace) } == 0 {
                return Err(failure("SEAL_ACE_READ"));
            }
            let p = ace.cast::<u8>();
            // ACCESS_ALLOWED_ACE: ACE_HEADER 4 bytes, ACCESS_MASK 4 bytes, SID starts at offset 8.
            if unsafe { *p } != 0
                || unsafe { p.add(4).cast::<u32>().read_unaligned() } != mask
                || unsafe { EqualSid(p.add(8).cast(), sid) } == 0
            {
                return Err(PipeError::policy("SEAL_ACE_MISMATCH"));
            }
        }
        Ok(())
    }
    pub(crate) fn session_discriminator(&self) -> &[u8] {
        &self.logon
    }
}
fn process_identity(
    pid: u32,
    expected_logon: &[u8],
    expected_user: &[u8],
    expected_session: u32,
) -> Result<Handle, PipeError> {
    if pid == 0 {
        return Err(PipeError::policy("PEER_PID"));
    }
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(pid, &mut session) } == 0 {
        return Err(failure("PEER_PROCESS_SESSION"));
    }
    if session != expected_session {
        return Err(PipeError::policy("PEER_SESSION"));
    }
    let raw = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
    if raw.is_null() {
        return Err(failure("PEER_PROCESS_OPEN"));
    }
    let process = Handle(raw);
    let token = process_token(process.0)?;
    if logon_sid(token.0)? != expected_logon || token_user(token.0)? != expected_user {
        return Err(PipeError::policy("PEER_TOKEN"));
    }
    Ok(process)
}
pub(crate) fn current_session_id() -> Result<u32, PipeError> {
    let mut session = 0;
    if unsafe { ProcessIdToSessionId(GetCurrentProcessId(), &mut session) } == 0 {
        return Err(failure("LOCAL_SESSION"));
    }
    Ok(session)
}
fn current_identity() -> Result<(Vec<u8>, Vec<u8>, u32), PipeError> {
    let token = process_token(unsafe { GetCurrentProcess() })?;
    Ok((
        logon_sid(token.0)?,
        token_user(token.0)?,
        current_session_id()?,
    ))
}
pub(crate) fn verify_client(server: &NamedPipeServer) -> Result<(), PipeError> {
    let (logon, user, session) = current_identity()?;
    let raw = server.as_raw_handle();
    let mut pid = 0;
    let mut pipe_session = 0;
    if unsafe { GetNamedPipeClientProcessId(raw, &mut pid) } == 0
        || unsafe { GetNamedPipeClientSessionId(raw, &mut pipe_session) } == 0
    {
        return Err(failure("CLIENT_PIPE_PEER"));
    }
    if pipe_session != session {
        return Err(PipeError::policy("CLIENT_SESSION"));
    }
    let process = process_identity(pid, &logon, &user, session)?;
    let mut again = 0;
    if unsafe { GetNamedPipeClientProcessId(raw, &mut again) } == 0 || again != pid {
        return Err(PipeError::policy("CLIENT_PID_CHANGED"));
    }
    drop(process);
    Ok(())
}
pub(crate) fn open_minimal(name: &str) -> Result<NamedPipeClient, PipeError> {
    let raw = unsafe {
        CreateFileW(
            wide(name).as_ptr(),
            FINAL,
            0,
            ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | 0x0010_0000 | 0x0001_0000,
            ptr::null_mut(),
        )
    };
    if raw == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(failure("CLIENT_OPEN"));
    }
    // Handle is owned immediately, including registration failure. Tokio's from_raw_handle
    // consumes it on both success and error (PollEvented owns the mio handle).
    let owned = unsafe { OwnedHandle::from_raw_handle(raw) };
    unsafe { NamedPipeClient::from_raw_handle(owned.into_raw_handle()) }
        .map_err(|e| PipeError::io("CLIENT_REGISTER", e))
}

pub(crate) fn verify_server(
    client: &NamedPipeClient,
    expected_image: &Path,
) -> Result<(), PipeError> {
    let (logon, user, session) = current_identity()?;
    let raw = client.as_raw_handle();
    let mut pid = 0;
    let mut pipe_session = 0;
    if unsafe { GetNamedPipeServerProcessId(raw, &mut pid) } == 0
        || unsafe { GetNamedPipeServerSessionId(raw, &mut pipe_session) } == 0
    {
        return Err(failure("SERVER_PIPE_PEER"));
    }
    if pipe_session != session {
        return Err(PipeError::policy("SERVER_SESSION"));
    }
    let process = process_identity(pid, &logon, &user, session)?;
    let mut image = vec![0u16; 32768];
    let mut length = image.len() as u32;
    if unsafe { QueryFullProcessImageNameW(process.0, 0, image.as_mut_ptr(), &mut length) } == 0 {
        return Err(failure("SERVER_IMAGE"));
    }
    let image = std::path::PathBuf::from(
        String::from_utf16(&image[..length as usize])
            .map_err(|_| PipeError::policy("SERVER_IMAGE_INVALID"))?,
    );
    // Both canonicalizations must succeed; trusted expected_image is provided by composition,
    // never by a pipe frame, PATH or CWD. No image bytes enter diagnostics.
    let actual =
        std::fs::canonicalize(image).map_err(|e| PipeError::io("SERVER_IMAGE_RESOLVE", e))?;
    let expected = std::fs::canonicalize(expected_image)
        .map_err(|e| PipeError::io("TRUSTED_IMAGE_RESOLVE", e))?;
    if actual != expected {
        return Err(PipeError::policy("SERVER_IMAGE_MISMATCH"));
    }
    let mut again = 0;
    if unsafe { GetNamedPipeServerProcessId(raw, &mut again) } == 0 || again != pid {
        return Err(PipeError::policy("SERVER_PID_CHANGED"));
    }
    Ok(())
}
