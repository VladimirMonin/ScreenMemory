#![cfg(windows)]
use super::*;
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    os::windows::{ffi::OsStrExt, io::AsRawHandle},
    ptr,
};
use windows_sys::Win32::{
    Foundation::{CloseHandle, GetLastError, INVALID_HANDLE_VALUE},
    Storage::FileSystem::{CreateFileW, FILE_FLAG_OVERLAPPED, OPEN_EXISTING, PIPE_ACCESS_DUPLEX},
    System::Pipes::{
        CreateNamedPipeW, GetNamedPipeHandleStateW, GetNamedPipeInfo, PIPE_READMODE_MESSAGE,
        PIPE_REJECT_REMOTE_CLIENTS, PIPE_SERVER_END, PIPE_TYPE_MESSAGE, PIPE_WAIT,
    },
};

#[tokio::test]
async fn s01_pipe_03_final_descriptor_and_actual_creation_denial() {
    let parent = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
    fs::create_dir_all(&parent).unwrap();
    let parent = fs::canonicalize(parent).unwrap();
    let parent = std::path::PathBuf::from(parent.to_string_lossy().trim_start_matches(r"\\?\"));
    let root = parent.join(format!(
        "t06b-security-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    fs::create_dir(&root).unwrap();
    for name in ["data", "config", "cache", "webview"] {
        fs::create_dir(root.join(name)).unwrap();
    }
    let layout = LibraryRootResolver::resolve(RootSource::Test {
        root: &root,
        forbidden_donor_roots: &[],
    })
    .unwrap();
    let endpoint = Endpoint::for_layout(&layout).unwrap();
    assert_eq!(
        Listener::bind_inner(endpoint, Some(3)).err().unwrap().code,
        "PARTIAL_STARTUP_TEST"
    );
    let listener = Listener::bind(Endpoint::for_layout(&layout).unwrap()).unwrap();
    let endpoint = Endpoint::for_layout(&layout).unwrap();
    let security = native::Security::current().unwrap();
    for server in &listener.idle {
        security.verify(server).unwrap();
    }
    let handle = listener.idle[0].as_raw_handle();
    let mut mode = 0;
    let mut instances = 0;
    let state = unsafe {
        GetNamedPipeHandleStateW(
            handle,
            &mut mode,
            &mut instances,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            0,
        )
    };
    assert_ne!(state, 0);
    assert_eq!(mode & (PIPE_WAIT | PIPE_READMODE_MESSAGE), 0);
    let mut info = 0;
    let mut max = 0;
    assert_ne!(
        unsafe {
            GetNamedPipeInfo(
                handle,
                &mut info,
                ptr::null_mut(),
                ptr::null_mut(),
                &mut max,
            )
        },
        0
    );
    assert_eq!(info & PIPE_SERVER_END, PIPE_SERVER_END);
    assert_eq!(info & PIPE_TYPE_MESSAGE, 0);
    assert_eq!(max, 16);
    // SDK readback may not surface PIPE_REJECT_REMOTE_CLIENTS; creation requests it unconditionally.
    let remote_readback = mode & PIPE_REJECT_REMOTE_CLIENTS;
    println!("S01-PIPE-03 mode_remote_bit={remote_readback}");
    let name = std::ffi::OsStr::new(&endpoint.name)
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let attributes = security.attributes();
    let extra = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED | 0x40000,
            PIPE_REJECT_REMOTE_CLIENTS,
            16,
            4096,
            4096,
            0,
            &attributes,
        )
    };
    assert_eq!(extra, INVALID_HANDLE_VALUE);
    assert_eq!(unsafe { GetLastError() }, 5);
    for denied in [0x40000, 0x80000, 0x40000000] {
        let raw = unsafe {
            CreateFileW(
                name.as_ptr(),
                denied,
                0,
                ptr::null(),
                OPEN_EXISTING,
                FILE_FLAG_OVERLAPPED,
                ptr::null_mut(),
            )
        };
        if raw != INVALID_HANDLE_VALUE {
            unsafe { CloseHandle(raw) };
            panic!("forbidden access accepted: mask={denied}");
        }
        assert_eq!(unsafe { GetLastError() }, 5);
    }
    // Drop only our own handles before deleting the fixture; no pipe namespace sweep.
    drop(listener);
    drop(layout);
    fs::remove_dir_all(root).unwrap();
}
