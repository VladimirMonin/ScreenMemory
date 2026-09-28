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
use zeroize::Zeroize;

/// Private unit-test instrumentation for raw frame buffers. `libtest` runs every
/// unit test on its own harness thread and these tests use the current-thread
/// runtime, so a test observes exactly its own buffers; per-test deltas stay
/// deterministic even though several tests allocate frames in one process. Wipe
/// checks are recorded from `FrameBuffer::drop` after the volatile zeroization and
/// while the allocation is still alive: no freed memory is ever inspected.
#[cfg(test)]
mod test_proof {
    use std::cell::Cell;

    thread_local! {
        static PAYLOAD_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
        static WIPES: Cell<usize> = const { Cell::new(0) };
        static UNWIPED: Cell<usize> = const { Cell::new(0) };
    }

    pub(super) fn record_allocation() {
        PAYLOAD_ALLOCATIONS.with(|count| count.set(count.get() + 1));
    }

    pub(super) fn allocations() -> usize {
        PAYLOAD_ALLOCATIONS.with(Cell::get)
    }

    pub(super) fn record_zeroized(bytes: &[u8]) {
        WIPES.with(|wipes| wipes.set(wipes.get() + 1));
        if bytes.iter().any(|byte| *byte != 0) {
            UNWIPED.with(|unwiped| unwiped.set(unwiped.get() + 1));
        }
    }

    pub(super) fn wipe_snapshot() -> (usize, usize) {
        (WIPES.with(Cell::get), UNWIPED.with(Cell::get))
    }
}

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

/// Sole owner of one raw inbound frame buffer and of the shared-budget permit that
/// accounts for its bytes. `Drop` wipes the live window with `zeroize` (volatile
/// writes plus a fence, which the optimizer may not delete the way it may delete
/// `fill(0)`) before the allocation is released, so the wipe covers every receive
/// exit: partial and truncated reads, transport IO errors, invalid UTF-8/JSON, the
/// handshake deadline, caller cancellation, and the ordinary drop of a delivered
/// frame. Deserialized serde copies never live here; they are a separate documented
/// limitation.
struct FrameBuffer {
    bytes: Vec<u8>,
    _permit: OwnedSemaphorePermit,
}
impl FrameBuffer {
    fn new(bytes: Vec<u8>, permit: OwnedSemaphorePermit) -> Self {
        Self {
            bytes,
            _permit: permit,
        }
    }
    fn wipe(&mut self) {
        // `vec![0; length]` never reallocates here, so the initialized window is the
        // only place frame bytes can live; spare capacity is zeroized as well.
        self.bytes.as_mut_slice().zeroize();
        self.bytes.spare_capacity_mut().zeroize();
    }
}
impl Drop for FrameBuffer {
    fn drop(&mut self) {
        self.wipe();
        #[cfg(test)]
        test_proof::record_zeroized(&self.bytes);
    }
}

/// Bytes deliberately have no Debug/Display/Serialize implementation.
pub struct Frame {
    buffer: FrameBuffer,
}
impl Frame {
    pub fn as_bytes(&self) -> &[u8] {
        &self.buffer.bytes
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
        // the permit and buffer drop together inside `FrameBuffer`, which zeroizes
        // the raw bytes it owns before releasing them. No read-ahead holds
        // unaccounted data.
        let permit = self.budget.reserve(length).await?;
        #[cfg(test)]
        test_proof::record_allocation();
        let mut buffer = FrameBuffer::new(vec![0; length], permit);
        self.stream
            .read_exact(&mut buffer.bytes)
            .await
            .map_err(map_read_error)?;
        validate(&buffer.bytes)?;
        operation.finished = true;
        Ok(Some(Frame { buffer }))
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
    serde_json::from_str::<serde::de::IgnoredAny>(text).map_err(|_| FrameError::InvalidJson)?;
    Ok(())
}

struct CappedJson {
    bytes: Vec<u8>,
    max: usize,
    exceeded: bool,
}
impl Drop for CappedJson {
    fn drop(&mut self) {
        self.bytes.zeroize();
    }
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
        let initial = test_proof::allocations();
        let (mut tx, rx) = duplex(16);
        tx.write_all(&(HANDSHAKE_LIMIT as u32).to_le_bytes())
            .await
            .unwrap();
        let task = tokio::spawn({
            let budget = budget.clone();
            async move { Reader::new(rx, budget).receive(Phase::Handshake).await }
        });
        tokio::task::yield_now().await;
        assert_eq!(test_proof::allocations(), initial);
        advance(Duration::from_secs(3)).await;
        assert!(matches!(
            task.await.unwrap(),
            Err(FrameError::DeadlineExceeded)
        ));
        assert_eq!(test_proof::allocations(), initial);
        drop(held);
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    }
}

/// Deterministic synthetic credential-canary regressions for the raw receive
/// buffer. The canary is not a credential, is never printed, and never leaves this
/// process: the tests prove only that its bytes are absent from the live allocation
/// at wipe time, before the allocator may reuse that memory.
#[cfg(test)]
mod secret_wipe {
    use super::{
        test_proof, BufferBudget, Frame, FrameError, Phase, Reader, HANDSHAKE_LIMIT,
        HOST_BUFFER_BUDGET,
    };
    use tokio::{
        io::{duplex, AsyncWriteExt},
        time::{advance, Duration},
    };

    const CANARY: &str = "CANARY-NOT-A-REAL-SECRET";

    fn canary_body() -> String {
        format!("{{\"credential\":\"{CANARY}\",\"client_kind\":\"cli\"}}")
    }

    async fn feed_prefix_and_partial(
        body: &[u8],
        partial: usize,
    ) -> (tokio::io::DuplexStream, tokio::io::DuplexStream) {
        let (mut tx, rx) = duplex(HANDSHAKE_LIMIT + 4);
        tx.write_all(&(body.len() as u32).to_le_bytes())
            .await
            .unwrap();
        tx.write_all(&body[..partial]).await.unwrap();
        (tx, rx)
    }

    /// Rejected handshake reads (malformed JSON, invalid UTF-8, truncated body and
    /// truncated length header) wipe the canary buffer before it is released.
    #[tokio::test]
    async fn s01_ipc_handshake_canary_wiped_on_every_rejected_read() {
        let budget = BufferBudget::new();
        let (before_wipes, before_unwiped) = test_proof::wipe_snapshot();
        let malformed = format!("{{\"credential\":\"{CANARY}\"");
        let mut invalid_utf8 = format!("{{\"credential\":\"{CANARY}").into_bytes();
        invalid_utf8.push(0xff);
        for (body, code) in [
            (malformed.as_bytes(), FrameError::InvalidJson),
            (invalid_utf8.as_slice(), FrameError::InvalidUtf8),
            (b"{x}".as_slice(), FrameError::InvalidJson),
        ] {
            let (mut tx, rx) = duplex(HANDSHAKE_LIMIT + 4);
            tx.write_all(&(body.len() as u32).to_le_bytes())
                .await
                .unwrap();
            tx.write_all(body).await.unwrap();
            drop(tx);
            let mut reader = Reader::new(rx, budget.clone());
            assert!(matches!(reader.receive(Phase::Handshake).await, Err(e) if e == code));
            assert!(reader.is_poisoned());
            assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
        }
        // Declared length is never satisfied: the partially filled canary buffer is
        // released by the error path, not by a delivered frame.
        let body = canary_body();
        let (tx, rx) = feed_prefix_and_partial(body.as_bytes(), 12).await;
        drop(tx);
        let mut reader = Reader::new(rx, budget.clone());
        assert!(matches!(
            reader.receive(Phase::Handshake).await,
            Err(FrameError::ProtocolTruncatedFrame)
        ));
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
        // Truncation inside the four-byte length header (one payload byte read).
        let (mut tx, rx) = duplex(8);
        tx.write_all(&[3, 0, 0, 0, b'{']).await.unwrap();
        drop(tx);
        let mut reader = Reader::new(rx, budget.clone());
        assert!(matches!(
            reader.receive(Phase::Handshake).await,
            Err(FrameError::ProtocolTruncatedFrame)
        ));
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
        let (wipes, unwiped) = test_proof::wipe_snapshot();
        assert_eq!(wipes, before_wipes + 5);
        assert_eq!(unwiped, before_unwiped);
    }

    /// The absolute handshake deadline abandons a partially received canary frame;
    /// dropping that future must wipe the raw buffer and return its permit.
    #[tokio::test(start_paused = true)]
    async fn s01_ipc_handshake_canary_wiped_on_partial_receive_deadline() {
        let budget = BufferBudget::new();
        let (before_wipes, before_unwiped) = test_proof::wipe_snapshot();
        let body = canary_body();
        let (_tx, rx) = feed_prefix_and_partial(body.as_bytes(), 10).await;
        let task = tokio::spawn({
            let budget = budget.clone();
            async move { Reader::new(rx, budget).receive(Phase::Handshake).await }
        });
        tokio::task::yield_now().await;
        advance(Duration::from_secs(3)).await;
        assert!(matches!(
            task.await.unwrap(),
            Err(FrameError::DeadlineExceeded)
        ));
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
        let (wipes, unwiped) = test_proof::wipe_snapshot();
        assert_eq!(wipes, before_wipes + 1);
        assert_eq!(unwiped, before_unwiped);
    }

    /// Caller cancellation mid-receive drops the same future before any deadline and
    /// must wipe the canary buffer just as deterministically.
    #[tokio::test]
    async fn s01_ipc_handshake_canary_wiped_on_partial_receive_cancel() {
        let budget = BufferBudget::new();
        let (before_wipes, before_unwiped) = test_proof::wipe_snapshot();
        let body = canary_body();
        let (_tx, rx) = feed_prefix_and_partial(body.as_bytes(), 10).await;
        let mut reader = Reader::new(rx, budget.clone());
        tokio::select! {
            biased;
            result = reader.receive(Phase::Handshake) => {
                panic!("unexpected completion: {}", result.err().map_or("ok", FrameError::code))
            }
            () = async { tokio::task::yield_now().await; } => {}
        }
        assert!(reader.is_poisoned());
        assert!(matches!(
            reader.receive(Phase::Handshake).await,
            Err(FrameError::Poisoned)
        ));
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
        let (wipes, unwiped) = test_proof::wipe_snapshot();
        assert_eq!(wipes, before_wipes + 1);
        assert_eq!(unwiped, before_unwiped);
    }

    /// A delivered frame keeps the canary alive until it is dropped, and the ordinary
    /// drop then wipes the same owned buffer exactly once.
    #[tokio::test]
    async fn s01_ipc_delivered_canary_frame_wiped_only_on_drop() {
        let budget = BufferBudget::new();
        let (before_wipes, before_unwiped) = test_proof::wipe_snapshot();
        let body = canary_body();
        let (mut tx, rx) = duplex(HANDSHAKE_LIMIT + 4);
        tx.write_all(&(body.len() as u32).to_le_bytes())
            .await
            .unwrap();
        tx.write_all(body.as_bytes()).await.unwrap();
        drop(tx);
        let mut reader = Reader::new(rx, budget.clone());
        let frame: Frame = reader.receive(Phase::Handshake).await.unwrap().unwrap();
        assert_eq!(frame.as_bytes(), body.as_bytes());
        assert_eq!(test_proof::wipe_snapshot(), (before_wipes, before_unwiped));
        drop(frame);
        let (wipes, unwiped) = test_proof::wipe_snapshot();
        assert_eq!(wipes, before_wipes + 1);
        assert_eq!(unwiped, before_unwiped);
        assert_eq!(budget.available(), HOST_BUFFER_BUDGET);
    }
}
