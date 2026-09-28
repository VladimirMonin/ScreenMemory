//! Bounded, stream-independent IPC byte framing. The trusted session owner chooses
//! the phase; this module does not authenticate peers or dispatch JSON methods.
use serde::Serialize;
use std::{
    fmt, io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{Mutex, OwnedSemaphorePermit, Semaphore},
    time::{timeout_at, Instant},
};
#[cfg(test)]
static PAYLOAD_ALLOCATIONS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub const APPLICATION_LIMIT: usize = 4 * 1024 * 1024;
pub const HANDSHAKE_LIMIT: usize = 8 * 1024;
pub const HOST_BUFFER_BUDGET: usize = 16 * 1024 * 1024;
pub const MAX_JSON_DEPTH: usize = 64;
/// Maximum polled sends (one active plus at most seven queued) per writer.
pub const WRITER_SEND_CAPACITY: usize = 8;

/// Chosen only by the trusted connection state machine, never by incoming JSON.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Handshake,
    Application,
}
impl Phase {
    fn limit(self) -> usize {
        match self {
            Self::Handshake => HANDSHAKE_LIMIT,
            Self::Application => APPLICATION_LIMIT,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FrameError {
    FrameTooLarge,
    InvalidLength,
    ProtocolTruncatedFrame,
    InvalidUtf8,
    InvalidJson,
    JsonTooDeep,
    BudgetClosed,
    QueueFull,
    Io,
    DeadlineExceeded,
    Poisoned,
}
impl fmt::Display for FrameError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.code())
    }
}
impl std::error::Error for FrameError {}
impl FrameError {
    pub fn code(self) -> &'static str {
        match self {
            Self::FrameTooLarge => "FRAME_TOO_LARGE",
            Self::InvalidLength => "INVALID_REQUEST",
            Self::ProtocolTruncatedFrame => "PROTOCOL_TRUNCATED_FRAME",
            Self::InvalidUtf8 => "INVALID_UTF8",
            Self::InvalidJson => "INVALID_REQUEST",
            Self::JsonTooDeep => "JSON_TOO_DEEP",
            Self::BudgetClosed => "BUFFER_BUDGET_CLOSED",
            Self::QueueFull => "WRITER_QUEUE_FULL",
            Self::Io => "TRANSPORT_IO",
            Self::DeadlineExceeded => "HANDSHAKE_RECEIVE_TIMEOUT",
            Self::Poisoned => "TRANSPORT_POISONED",
        }
    }
}

/// Clone this *one* instance across all connections/readers/writers of a host.
/// Permits account for raw owned frame buffers, not parsed AST or process RSS.
#[derive(Clone, Debug)]
pub struct BufferBudget(Arc<Semaphore>);
impl Default for BufferBudget {
    fn default() -> Self {
        Self::new()
    }
}
impl BufferBudget {
    pub fn new() -> Self {
        Self(Arc::new(Semaphore::new(HOST_BUFFER_BUDGET)))
    }
    pub fn available(&self) -> usize {
        self.0.available_permits()
    }
    async fn reserve(&self, bytes: usize) -> Result<OwnedSemaphorePermit, FrameError> {
        self.0
            .clone()
            .acquire_many_owned(bytes as u32)
            .await
            .map_err(|_| FrameError::BudgetClosed)
    }
}

/// Bytes deliberately have no Debug/Display/Serialize implementation.
pub struct Frame {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes
    }
}

pub struct Reader<R> {
    stream: R,
    budget: BufferBudget,
    poisoned: Arc<AtomicBool>,
}
impl<R: AsyncRead + Unpin> Reader<R> {
    pub fn new(stream: R, budget: BufferBudget) -> Self {
        Self {
            stream,
            budget,
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }

    /// A clean EOF between frames returns None. An EOF inside a frame is an error.
    /// Handshake deadline is absolute from this invocation, including permit wait.
    pub async fn receive(&mut self, phase: Phase) -> Result<Option<Frame>, FrameError> {
        if phase == Phase::Handshake {
            let deadline = Instant::now() + std::time::Duration::from_secs(3);
            timeout_at(deadline, self.receive_inner(phase))
                .await
                .map_err(|_| FrameError::DeadlineExceeded)?
        } else {
            self.receive_inner(phase).await
        }
    }
    async fn receive_inner(&mut self, phase: Phase) -> Result<Option<Frame>, FrameError> {
        if self.is_poisoned() {
            return Err(FrameError::Poisoned);
        }
        // Abandoning a receive at any await (including a budget wait) makes the
        // stream position uncertain. Even clean timeout must not permit reuse.
        let mut operation = WriteInProgress {
            poisoned: self.poisoned.clone(),
            finished: false,
        };
        let mut prefix = [0u8; 4];
        let first = self
            .stream
            .read(&mut prefix[..1])
            .await
            .map_err(|_| FrameError::Io)?;
        if first == 0 {
            operation.finished = true;
            return Ok(None);
        }
        self.stream
            .read_exact(&mut prefix[1..])
            .await
            .map_err(map_read_error)?;
        let length = u32::from_le_bytes(prefix) as usize;
        if length == 0 {
            return Err(FrameError::InvalidLength);
        }
        if length > phase.limit() {
            return Err(FrameError::FrameTooLarge);
        }
        // Acquire before Vec allocation. On cancellation, IO error or parse failure,
        // the permit and buffer drop together. No read-ahead holds unaccounted data.
        let permit = self.budget.reserve(length).await?;
        #[cfg(test)]
        PAYLOAD_ALLOCATIONS.fetch_add(1, Ordering::SeqCst);
        let mut bytes = vec![0; length];
        self.stream
            .read_exact(&mut bytes)
            .await
            .map_err(map_read_error)?;
        validate(&bytes)?;
        operation.finished = true;
        Ok(Some(Frame {
            bytes,
            _permit: permit,
        }))
    }
}
fn map_read_error(error: io::Error) -> FrameError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        FrameError::ProtocolTruncatedFrame
    } else {
        FrameError::Io
    }
}

fn validate(bytes: &[u8]) -> Result<(), FrameError> {
    let text = std::str::from_utf8(bytes).map_err(|_| FrameError::InvalidUtf8)?;
    // Non-recursive lexical depth guard before serde allocates a parsed tree.
    // Escaped quotes/brackets inside strings are inert; serde checks all syntax.
    let (mut in_string, mut escaped, mut depth) = (false, false, 0usize);
    for byte in bytes {
        if in_string {
            if escaped {
                escaped = false;
            } else if *byte == b'\\' {
                escaped = true;
            } else if *byte == b'"' {
                in_string = false;
            }
        } else {
            match *byte {
                b'"' => in_string = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return Err(FrameError::JsonTooDeep);
                    }
                }
                b'}' | b']' => {
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
        }
    }
    serde_json::from_str::<serde_json::Value>(text).map_err(|_| FrameError::InvalidJson)?;
    Ok(())
}

struct CappedJson {
    bytes: Vec<u8>,
    max: usize,
    exceeded: bool,
}
impl io::Write for CappedJson {
    fn write(&mut self, input: &[u8]) -> io::Result<usize> {
        if input.len() > self.max - self.bytes.len() {
            self.exceeded = true;
            return Err(io::Error::other("bounded serialization"));
        }
        self.bytes.extend_from_slice(input);
        Ok(input.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// A non-waiting admission gate bounds polled sends to eight per serialized writer:
/// one active and at most seven queued. Unpolled caller-owned futures are outside
/// this internal queue. Admitted waiters hold no frame; the shared raw buffer
/// budget still applies independently during serialization and sending.
/// Cancellation once IO begins poisons this path even if zero bytes were written:
/// a subsequent caller can never append onto a possibly partial frame.
pub struct Writer<W> {
    stream: Arc<Mutex<W>>,
    admission: Arc<Semaphore>,
    budget: BufferBudget,
    poisoned: Arc<AtomicBool>,
}
impl<W> Clone for Writer<W> {
    fn clone(&self) -> Self {
        Self {
            stream: self.stream.clone(),
            admission: self.admission.clone(),
            budget: self.budget.clone(),
            poisoned: self.poisoned.clone(),
        }
    }
}
struct WriteInProgress {
    poisoned: Arc<AtomicBool>,
    finished: bool,
}
impl Drop for WriteInProgress {
    fn drop(&mut self) {
        if !self.finished {
            self.poisoned.store(true, Ordering::Release);
        }
    }
}
impl<W: AsyncWrite + Unpin> Writer<W> {
    pub fn new(stream: W, budget: BufferBudget) -> Self {
        Self {
            stream: Arc::new(Mutex::new(stream)),
            admission: Arc::new(Semaphore::new(WRITER_SEND_CAPACITY)),
            budget,
            poisoned: Arc::new(AtomicBool::new(false)),
        }
    }
    pub fn is_poisoned(&self) -> bool {
        self.poisoned.load(Ordering::Acquire)
    }
    pub async fn send<T: Serialize>(&self, phase: Phase, value: &T) -> Result<(), FrameError> {
        if self.is_poisoned() {
            return Err(FrameError::Poisoned);
        }
        // No async admission wait: overflow never enters the mutex or budget queue,
        // and never serializes the caller's payload. RAII releases on every exit.
        let _admission = self
            .admission
            .clone()
            .try_acquire_owned()
            .map_err(|_| FrameError::QueueFull)?;
        let mut stream = self.stream.lock().await;
        if self.is_poisoned() {
            return Err(FrameError::Poisoned);
        }
        let _permit = self.budget.reserve(phase.limit()).await?;
        let mut capped = CappedJson {
            bytes: Vec::new(),
            max: phase.limit(),
            exceeded: false,
        };
        if serde_json::to_writer(&mut capped, value).is_err() {
            return Err(if capped.exceeded {
                FrameError::FrameTooLarge
            } else {
                FrameError::InvalidJson
            });
        }
        if capped.bytes.is_empty() {
            return Err(FrameError::InvalidLength);
        }
        validate(&capped.bytes)?;
        let len = (capped.bytes.len() as u32).to_le_bytes();
        let mut operation = WriteInProgress {
            poisoned: self.poisoned.clone(),
            finished: false,
        };
        stream.write_all(&len).await.map_err(|_| FrameError::Io)?;
        stream
            .write_all(&capped.bytes)
            .await
            .map_err(|_| FrameError::Io)?;
        stream.flush().await.map_err(|_| FrameError::Io)?;
        operation.finished = true;
        Ok(())
    }
}

#[cfg(test)]
mod allocation_proof {
    use super::*;
    use tokio::{
        io::{duplex, AsyncWriteExt},
        time::{advance, Duration},
    };

    #[tokio::test(start_paused = true)]
    async fn no_payload_allocation_before_shared_permit_including_deadline() {
        let budget = BufferBudget::new();
        let held = budget.reserve(HOST_BUFFER_BUDGET).await.unwrap();
        let initial = PAYLOAD_ALLOCATIONS.load(Ordering::SeqCst);
        let (mut tx, rx) = duplex(16);
        tx.write_all(&(HANDSHAKE_LIMIT as u32).to_le_bytes())
            .await
            .unwrap();
        let task = tokio::spawn({
            let budget = budget.clone();
            async move { Reader::new(rx, budget).receive(Phase::Handshake).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(PAYLOAD_ALLOCATIONS.load(Ordering::SeqCst), initial);
        advance(Duration::from_secs(3)).await;
        assert!(matches!(
            task.await.unwrap(),
            Err(FrameError::DeadlineExceeded)
        ));
        assert_eq!(PAYLOAD_ALLOCATIONS.load(Ordering::SeqCst), initial);
        drop(held);
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    }
}
