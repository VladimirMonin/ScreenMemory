//! Sealed Windows named-pipe byte endpoint. Not a host, session, auth or dispatcher.
#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("sm-pipe requires Windows x64");
mod native;
#[cfg(test)]
mod security_tests;
use sha2::{Digest, Sha256};
use sm_isolation::NamespaceLayout;
use std::{
    collections::HashSet,
    future::Future,
    os::windows::io::AsRawHandle,
    path::{Path, PathBuf},
    pin::Pin,
    sync::mpsc::{self, Receiver, SyncSender},
    task::{Context, Poll},
};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tokio::net::windows::named_pipe::{NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions};

/// Fixed safe error category plus immediate Win32 code. No raw OS error strings or identity bytes.
#[derive(Debug)]
pub struct PipeError {
    pub code: &'static str,
    pub win32: Option<u32>,
}
impl PipeError {
    fn policy(code: &'static str) -> Self {
        Self { code, win32: None }
    }
    fn native(code: &'static str, win32: u32) -> Self {
        Self {
            code,
            win32: Some(win32),
        }
    }
    fn io(code: &'static str, error: std::io::Error) -> Self {
        Self {
            code,
            win32: error.raw_os_error().map(|v| v as u32),
        }
    }
}

/// Construct only from trusted, already-resolved installed/test composition configuration.
/// Never accept executable path from wire, environment, search PATH or a remote caller.
pub struct TrustedServerImage {
    path: PathBuf,
}
impl TrustedServerImage {
    pub fn from_configuration(path: &Path) -> Result<Self, PipeError> {
        if !path.is_absolute() || !path.is_file() {
            return Err(PipeError::policy("TRUSTED_IMAGE_INVALID"));
        }
        Ok(Self {
            path: std::fs::canonicalize(path)
                .map_err(|e| PipeError::io("TRUSTED_IMAGE_RESOLVE", e))?,
        })
    }
}

/// Identity is derived from opened, pinned root volume/file IDs and logon SID; never a path.
/// Equality across root spelling/case aliases follows sm-isolation's native identity.
pub struct Endpoint {
    name: String,
}
impl Endpoint {
    pub fn for_layout(layout: &NamespaceLayout) -> Result<Self, PipeError> {
        let security = native::Security::current()?;
        let mut hash = Sha256::new();
        hash.update(b"ScreenMemory.pipe.v1/root/logon");
        hash.update(layout.environment.endpoint_prefix().as_bytes());
        hash.update(layout.data.volume_serial().to_le_bytes());
        hash.update(layout.data.file_identity().to_le_bytes());
        hash.update(security.session_discriminator());
        let digest = hash.finalize();
        let suffix = digest[..16]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Ok(Self {
            name: format!(
                r"\\.\pipe\{}.s{}.{}",
                layout.environment.endpoint_prefix(),
                native::current_session_id()?,
                suffix
            ),
        })
    }
}

/// Owns all 16 server handles, never creates another after sealing. No publishing before seal.
/// A checked-out connection owns its handle until synchronously returned to the bounded pool.
enum SlotReturn {
    Ready(NamedPipeServer),
    Lost,
}
pub struct Listener {
    idle: Vec<NamedPipeServer>,
    reused: HashSet<usize>,
    returned: Receiver<SlotReturn>,
    return_to: SyncSender<SlotReturn>,
    failed: bool,
}
impl Listener {
    pub fn bind(endpoint: Endpoint) -> Result<Self, PipeError> {
        Self::bind_inner(endpoint, None)
    }
    fn bind_inner(endpoint: Endpoint, stop_before: Option<usize>) -> Result<Self, PipeError> {
        let security = native::Security::current()?;
        let mut options = ServerOptions::new();
        options
            .pipe_mode(PipeMode::Byte)
            .reject_remote_clients(true)
            .max_instances(16)
            .write_dac(true);
        let attributes = security.attributes();
        let mut idle = Vec::with_capacity(16);
        for index in 0..16 {
            if stop_before == Some(index) {
                return Err(PipeError::policy("PARTIAL_STARTUP_TEST"));
            }
            options.first_pipe_instance(index == 0);
            // SAFETY: SECURITY_ATTRIBUTES + self-relative descriptor remain live throughout the
            // synchronous CreateNamedPipeW call; no handles are published while bootstrap rights exist.
            let pipe = unsafe {
                options.create_with_security_attributes_raw(
                    &endpoint.name,
                    (&attributes as *const windows_sys::Win32::Security::SECURITY_ATTRIBUTES)
                        .cast_mut()
                        .cast(),
                )
            }
            .map_err(|e| PipeError::io("PIPE_CREATE", e))?;
            idle.push(pipe);
        }
        for pipe in &idle {
            security.seal(pipe)?;
        }
        let (return_to, returned) = mpsc::sync_channel(16);
        Ok(Self {
            idle,
            reused: HashSet::new(),
            returned,
            return_to,
            failed: false,
        })
    }
    pub async fn accept(&mut self) -> Result<Connection, PipeError> {
        while let Ok(slot) = self.returned.try_recv() {
            match slot {
                SlotReturn::Ready(pipe) => {
                    self.reused.insert(pipe.as_raw_handle() as usize);
                    self.idle.push(pipe);
                }
                SlotReturn::Lost => self.failed = true,
            }
        }
        if self.failed {
            return Err(PipeError::policy("PIPE_SLOT_LOST"));
        }
        if self.idle.is_empty() {
            return Err(PipeError::policy("CLIENT_LIMIT"));
        }
        // All 16 precreated instances can be connected by Windows while one accept is polled.
        // No unbounded application queue, extra pipe creation or network fallback.
        let mut connects = self
            .idle
            .iter()
            .map(|pipe| Box::pin(pipe.connect()))
            .collect::<Vec<_>>();
        let (index, result) = std::future::poll_fn(|cx| {
            for (i, connecting) in connects.iter_mut().enumerate() {
                if let Poll::Ready(result) = Pin::as_mut(connecting).poll(cx) {
                    return Poll::Ready((i, result));
                }
            }
            Poll::Pending
        })
        .await;
        drop(connects);
        if let Err(error) = result {
            self.failed = true;
            return Err(PipeError::io("PIPE_CONNECT", error));
        }
        let pipe = self.idle.swap_remove(index);
        if let Err(error) = native::verify_client(&pipe) {
            if let Err(e) = pipe.disconnect() {
                self.failed = true;
                return Err(PipeError::io("PEER_REJECT_DISCONNECT", e));
            }
            self.idle.push(pipe);
            return Err(error);
        }
        let first_reused_read = self.reused.remove(&(pipe.as_raw_handle() as usize));
        Ok(Connection {
            pipe: Some(pipe),
            return_to: self.return_to.clone(),
            first_reused_read,
        })
    }
    pub fn idle_count(&self) -> usize {
        self.idle.len()
    }
}

/// Byte stream for sm-framing; connection drop disconnects before returning the same handle.
/// A slot disconnect failure permanently poisons that slot rather than loosening its descriptor.
pub struct Connection {
    pipe: Option<NamedPipeServer>,
    return_to: SyncSender<SlotReturn>,
    first_reused_read: bool,
}
impl Connection {
    pub fn stream(&mut self) -> &mut Self {
        self
    }
}
impl AsyncRead for Connection {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<std::io::Result<()>> {
        let retry = self.first_reused_read;
        let result = Pin::new(self.pipe.as_mut().expect("connection alive")).poll_read(cx, buf);
        match result {
            Poll::Pending => Poll::Pending,
            Poll::Ready(Err(e))
                if retry && e.raw_os_error() == Some(233) && buf.filled().is_empty() =>
            {
                self.first_reused_read = false;
                Pin::new(self.pipe.as_mut().expect("connection alive")).poll_read(cx, buf)
            }
            result => {
                self.first_reused_read = false;
                result
            }
        }
    }
}
impl AsyncWrite for Connection {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<std::io::Result<usize>> {
        Pin::new(self.pipe.as_mut().expect("connection alive")).poll_write(cx, buf)
    }
    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(self.pipe.as_mut().expect("connection alive")).poll_flush(cx)
    }
    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<std::io::Result<()>> {
        Pin::new(self.pipe.as_mut().expect("connection alive")).poll_shutdown(cx)
    }
}
impl Drop for Connection {
    fn drop(&mut self) {
        if let Some(pipe) = self.pipe.take() {
            if pipe.disconnect().is_ok() {
                let _ = self.return_to.try_send(SlotReturn::Ready(pipe));
            } else {
                drop(pipe);
                let _ = self.return_to.try_send(SlotReturn::Lost);
            }
            // Failure: own handle closed; listener refuses future accepts, never relaxes ACL.
        }
    }
}

/// Client receives only a byte stream after OS peer and trusted image checks.
/// Credential handshake and application dispatch are owned by a future host/session slice.
pub fn open(endpoint: &Endpoint, image: &TrustedServerImage) -> Result<NamedPipeClient, PipeError> {
    let pipe = native::open_minimal(&endpoint.name)?;
    native::verify_server(&pipe, &image.path)?;
    Ok(pipe)
}
