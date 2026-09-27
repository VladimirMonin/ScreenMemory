//! Explicit-root, bounded operational diagnostics. No host startup side effects.
use sm_application::observability::{Level, SafeEvent, SafeEventSink};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Component as PathComponent, PathBuf};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Condvar, Mutex, TryLockError,
};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const DEFAULT_CAPACITY: usize = 8192;
pub const MAX_EVENT_BYTES: usize = 512;
const MAX_CAPACITY: usize = DEFAULT_CAPACITY;

pub trait Clock: Send + Sync + 'static {
    fn utc(&self) -> SystemTime;
    fn monotonic(&self) -> Duration;
}

pub struct SystemClock {
    origin: Instant,
}
impl Default for SystemClock {
    fn default() -> Self {
        Self {
            origin: Instant::now(),
        }
    }
}
impl Clock for SystemClock {
    fn utc(&self) -> SystemTime {
        SystemTime::now()
    }
    fn monotonic(&self) -> Duration {
        self.origin.elapsed()
    }
}

/// Deterministic calendar and elapsed time are controlled independently.
pub struct FakeClock {
    state: Mutex<(SystemTime, Duration)>,
}
impl FakeClock {
    pub fn new(utc: SystemTime) -> Self {
        Self {
            state: Mutex::new((utc, Duration::ZERO)),
        }
    }
    pub fn advance_utc(&self, delta: Duration) {
        let mut s = self.state.lock().unwrap();
        s.0 += delta;
    }
    pub fn advance_monotonic(&self, delta: Duration) {
        let mut s = self.state.lock().unwrap();
        s.1 += delta;
    }
}
impl Clock for FakeClock {
    fn utc(&self) -> SystemTime {
        self.state.lock().unwrap().0
    }
    fn monotonic(&self) -> Duration {
        self.state.lock().unwrap().1
    }
}

/// Only typed events cross the public sink boundary. Raw JSONL writers stay private.
///
/// A consumer cannot pass an arbitrary line or attach free-form details:
/// ```compile_fail
/// use sm_observability::{ConsoleSink, LogSink};
/// let mut sink = ConsoleSink::new(Vec::<u8>::new());
/// sink.write_line("{\"details\":\"secret\"}", std::time::SystemTime::now()); // no public raw writer
/// ```
/// ```compile_fail
/// use sm_application::observability::{Component, EventId, Level, Operation, SafeEvent};
/// let mut event = SafeEvent::new(Level::Info, Component::Probe, EventId::ProbeStarted, Operation::Probe);
/// event.details = "secret"; // no arbitrary details field
/// ```
pub trait LogSink: Send + 'static {
    fn write_event(&mut self, event: &SafeEvent) -> io::Result<()>;
    fn flush(&mut self) -> io::Result<()>;
}

/// Enforce the byte limit even for direct calls to public production sinks.
fn checked_line(event: &SafeEvent) -> io::Result<String> {
    let line = serde_json::to_string(event).expect("closed SafeEvent is serializable");
    if line.len() > MAX_EVENT_BYTES {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "event exceeds byte limit",
        ));
    }
    Ok(line)
}

pub struct ConsoleSink<W: Write + Send + 'static> {
    writer: W,
}
impl<W: Write + Send + 'static> ConsoleSink<W> {
    pub fn new(writer: W) -> Self {
        Self { writer }
    }
}
impl<W: Write + Send + 'static> ConsoleSink<W> {
    fn write_line(&mut self, line: &str) -> io::Result<()> {
        self.writer.write_all(line.as_bytes())?;
        self.writer.write_all(b"\n")
    }
}
impl<W: Write + Send + 'static> LogSink for ConsoleSink<W> {
    fn write_event(&mut self, event: &SafeEvent) -> io::Result<()> {
        self.write_line(&checked_line(event)?)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.writer.flush()
    }
}

#[derive(Clone, Copy, Debug)]
pub struct RotationLimits {
    pub files: usize,
    pub bytes: u64,
    pub age: Duration,
}
impl Default for RotationLimits {
    fn default() -> Self {
        Self {
            files: 5,
            bytes: 10 * 1024 * 1024,
            age: Duration::from_secs(14 * 86400),
        }
    }
}

/// Never creates/cleans a default home directory. Only exact owned names in an explicit root.
pub struct FileSink {
    root: PathBuf,
    limits: RotationLimits,
    current: Option<(File, PathBuf, u64)>,
    seq: u32,
}
impl FileSink {
    pub fn new(root: PathBuf, limits: RotationLimits) -> io::Result<Self> {
        if limits.files == 0
            || limits.files > 5
            || limits.bytes == 0
            || limits.bytes > 10 * 1024 * 1024
            || limits.age > Duration::from_secs(14 * 86400)
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid rotation limits",
            ));
        }
        // Fail closed on link/reparse points in all existing ancestors, not merely the leaf.
        if !root.is_absolute() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "absolute owned root required",
            ));
        }
        let mut cursor = PathBuf::new();
        for part in root.components() {
            if matches!(part, PathComponent::ParentDir | PathComponent::CurDir) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "noncanonical root",
                ));
            }
            cursor.push(part.as_os_str());
            if matches!(part, PathComponent::Prefix(_)) {
                continue;
            }
            let meta = fs::symlink_metadata(&cursor)?;
            if meta.file_type().is_symlink() || is_reparse(&meta) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidInput,
                    "linked log root",
                ));
            }
        }
        if !root.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "log root not directory",
            ));
        }
        Ok(Self {
            root,
            limits,
            current: None,
            seq: 0,
        })
    }

    fn owned(&self) -> io::Result<Vec<(u64, PathBuf)>> {
        let mut result = Vec::new();
        for entry in fs::read_dir(&self.root)? {
            let entry = entry?;
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(stamp) = name
                .strip_prefix("operational-")
                .and_then(|s| s.strip_suffix(".jsonl"))
            else {
                continue;
            };
            let Some((ms, seq)) = stamp.split_once('-') else {
                continue;
            };
            if ms.len() != 20
                || seq.len() != 8
                || !ms.bytes().all(|b| b.is_ascii_digit())
                || !seq.bytes().all(|b| b.is_ascii_digit())
            {
                continue;
            }
            let meta = fs::symlink_metadata(entry.path())?;
            if !meta.is_file() || meta.file_type().is_symlink() || is_reparse(&meta) {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "owned name is not regular file",
                ));
            }
            result.push((
                ms.parse().map_err(|_| {
                    io::Error::new(io::ErrorKind::InvalidData, "invalid owned timestamp")
                })?,
                entry.path(),
            ));
        }
        result.sort_by(|a, b| a.1.cmp(&b.1));
        Ok(result)
    }
    fn prune(&mut self, now_ms: u64) -> io::Result<()> {
        let mut files = self.owned()?;
        for (stamp, path) in &files {
            if now_ms.saturating_sub(*stamp) > self.limits.age.as_millis() as u64 {
                if self.current.as_ref().is_some_and(|(_, p, _)| p == path) {
                    self.current = None;
                }
                fs::remove_file(path)?;
            }
        }
        files = self.owned()?;
        while files.len() > self.limits.files {
            let (_, path) = files.remove(0);
            if self.current.as_ref().is_some_and(|(_, p, _)| p == &path) {
                self.current = None;
            }
            fs::remove_file(path)?;
        }
        Ok(())
    }
}
#[cfg(windows)]
fn is_reparse(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    meta.file_attributes() & 0x400 != 0
}
#[cfg(not(windows))]
fn is_reparse(_: &fs::Metadata) -> bool {
    false
}
fn epoch_ms(now: SystemTime) -> u64 {
    now.duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_millis()
        .min(u64::MAX as u128) as u64
}
impl FileSink {
    fn write_line(&mut self, line: &str, now: SystemTime) -> io::Result<()> {
        let len = line.len() as u64 + 1;
        if len > self.limits.bytes {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "event exceeds file limit",
            ));
        }
        let ms = epoch_ms(now);
        self.prune(ms)?;
        if self
            .current
            .as_ref()
            .is_some_and(|(_, _, size)| *size + len > self.limits.bytes)
        {
            self.current = None;
        }
        if self.current.is_none() {
            let (file, path) = loop {
                let path = self
                    .root
                    .join(format!("operational-{ms:020}-{:08}.jsonl", self.seq));
                self.seq = self
                    .seq
                    .checked_add(1)
                    .ok_or_else(|| io::Error::other("log sequence exhausted"))?;
                match OpenOptions::new().write(true).create_new(true).open(&path) {
                    Ok(file) => break (file, path),
                    Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
                    Err(err) => return Err(err),
                }
            };
            self.current = Some((file, path, 0));
            self.prune(ms)?;
        }
        let (file, _, size) = self.current.as_mut().expect("new file");
        file.write_all(line.as_bytes())?;
        file.write_all(b"\n")?;
        *size += len;
        Ok(())
    }
}
impl LogSink for FileSink {
    fn write_event(&mut self, event: &SafeEvent) -> io::Result<()> {
        let line = checked_line(event)?;
        self.write_line(
            &line,
            UNIX_EPOCH + Duration::from_millis(event.timestamp_unix_ms),
        )
    }
    fn flush(&mut self) -> io::Result<()> {
        if let Some((file, _, _)) = &mut self.current {
            file.sync_all()?;
        }
        Ok(())
    }
}

pub struct FailingLogSink;
impl LogSink for FailingLogSink {
    fn write_event(&mut self, _: &SafeEvent) -> io::Result<()> {
        Err(io::Error::other("synthetic failure"))
    }
    fn flush(&mut self) -> io::Result<()> {
        Err(io::Error::other("synthetic failure"))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Counters {
    pub dropped: u64,
    pub sink_failed: u64,
    pub queued: usize,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FlushOutcome {
    Flushed,
    SinkFailed,
    TimedOut,
}
struct State {
    events: VecDeque<SafeEvent>,
    closing: bool,
    dropped: u64,
    sink_failed: u64,
    last_progress: Option<Duration>,
}
struct Shared {
    state: Mutex<State>,
    wake: Condvar,
    capacity: usize,
    clock: Arc<dyn Clock>,
    contention_drops: AtomicU64,
}

#[derive(Clone)]
pub struct Logger {
    shared: Arc<Shared>,
}
impl Logger {
    pub fn counters(&self) -> Counters {
        let state = self.shared.state.lock().unwrap();
        Counters {
            dropped: state
                .dropped
                .saturating_add(self.shared.contention_drops.load(Ordering::Relaxed)),
            sink_failed: state.sink_failed,
            queued: state.events.len(),
        }
    }
}
impl SafeEventSink for Logger {
    fn emit(&self, mut event: SafeEvent) -> bool {
        let mut state = match self.shared.state.try_lock() {
            Ok(s) => s,
            Err(TryLockError::Poisoned(p)) => p.into_inner(),
            Err(TryLockError::WouldBlock) => {
                self.shared.contention_drops.fetch_add(1, Ordering::Relaxed);
                return false;
            }
        };
        if state.closing {
            state.dropped += 1;
            return false;
        }
        let now = self.shared.clock.monotonic();
        if event.event.is_progress()
            && state
                .last_progress
                .is_some_and(|last| now.saturating_sub(last) < Duration::from_secs(1))
        {
            state.dropped += 1;
            return false;
        }
        event.timestamp_unix_ms = epoch_ms(self.shared.clock.utc());
        if checked_line(&event).is_err() {
            state.dropped += 1;
            return false;
        }
        if state.events.len() == self.shared.capacity {
            if matches!(event.level, Level::Warn | Level::Error) {
                if let Some(index) = state
                    .events
                    .iter()
                    .position(|e| !matches!(e.level, Level::Warn | Level::Error))
                {
                    state.events.remove(index);
                    state.dropped += 1;
                } else {
                    state.dropped += 1;
                    return false;
                }
            } else {
                state.dropped += 1;
                return false;
            }
        }
        if event.event.is_progress() {
            state.last_progress = Some(now);
        }
        state.events.push_back(event);
        self.shared.wake.notify_one();
        true
    }
}

pub struct BoundedLogger {
    logger: Logger,
    finished: mpsc::Receiver<()>,
    worker: Option<JoinHandle<()>>,
    completed: Option<FlushOutcome>,
}

impl Drop for BoundedLogger {
    fn drop(&mut self) {
        // Signal exit even when the owner omitted shutdown; never join an untrusted stuck sink.
        let mut state = self.logger.shared.state.lock().unwrap();
        state.closing = true;
        self.logger.shared.wake.notify_one();
    }
}
impl BoundedLogger {
    pub fn start(
        mut sink: impl LogSink,
        clock: Arc<dyn Clock>,
        capacity: usize,
    ) -> io::Result<Self> {
        if capacity == 0 || capacity > MAX_CAPACITY {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid queue capacity",
            ));
        }
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                events: VecDeque::with_capacity(capacity),
                closing: false,
                dropped: 0,
                sink_failed: 0,
                last_progress: None,
            }),
            wake: Condvar::new(),
            capacity,
            clock,
            contention_drops: AtomicU64::new(0),
        });
        let logger = Logger {
            shared: shared.clone(),
        };
        let (tx, finished) = mpsc::channel();
        let worker = thread::Builder::new()
            .name("sm-safe-log".into())
            .spawn(move || {
                let mut fallback_reported = false;
                loop {
                    let event = {
                        let mut state = shared.state.lock().unwrap();
                        while state.events.is_empty() && !state.closing {
                            state = shared.wake.wait(state).unwrap();
                        }
                        if state.events.is_empty() {
                            break;
                        }
                        // Prioritise errors already queued, without increasing queue capacity.
                        let index = state
                            .events
                            .iter()
                            .position(|e| matches!(e.level, Level::Warn | Level::Error))
                            .unwrap_or(0);
                        state.events.remove(index).expect("queued")
                    };
                    if sink.write_event(&event).is_err() {
                        shared.state.lock().unwrap().sink_failed += 1;
                        // One fixed fallback marker per logger lifetime, no recursive event/error text.
                        if !fallback_reported {
                            fallback_reported = true;
                            let _ = io::stderr().write_all(b"safe log sink unavailable\n");
                        }
                    }
                }
                if sink.flush().is_err() {
                    shared.state.lock().unwrap().sink_failed += 1;
                }
                let _ = tx.send(());
            })?;
        Ok(Self {
            logger,
            finished,
            worker: Some(worker),
            completed: None,
        })
    }
    pub fn producer(&self) -> Logger {
        self.logger.clone()
    }
    pub fn counters(&self) -> Counters {
        self.logger.counters()
    }
    pub fn shutdown(&mut self, deadline: Duration) -> FlushOutcome {
        if let Some(outcome) = self.completed {
            return outcome;
        }
        {
            let mut state = self.logger.shared.state.lock().unwrap();
            state.closing = true;
            self.logger.shared.wake.notify_one();
        }
        if self.finished.recv_timeout(deadline).is_err() {
            return FlushOutcome::TimedOut;
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
        let outcome = if self.counters().sink_failed > 0 {
            FlushOutcome::SinkFailed
        } else {
            FlushOutcome::Flushed
        };
        self.completed = Some(outcome);
        outcome
    }
}

#[cfg(test)]
mod boundary_tests {
    use super::*;
    use sm_application::observability::{Component, EventId, Operation};
    use std::sync::{Arc, Mutex};

    const CANARY: &str = "UNICODE-Ж-非標準-Canary_527!";

    #[derive(Clone)]
    struct Capture(Arc<Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    // Test-only private copy of the serialization boundary. The same assertion inspects
    // actual console and JSONL bytes for both typed production output and the mutation.
    fn check_sink_bytes(mutate: bool) {
        let event = SafeEvent::new(
            Level::Info,
            Component::Probe,
            EventId::ProbeStarted,
            Operation::Probe,
        );
        let captured = Arc::new(Mutex::new(Vec::new()));
        let mut console = ConsoleSink::new(Capture(captured.clone()));
        let dir = std::env::current_dir().unwrap().join(".pi");
        fs::create_dir_all(&dir).unwrap();
        let dir = dir.join(format!(
            "log-boundary-{}-{:?}",
            std::process::id(),
            thread::current().id()
        ));
        fs::create_dir(&dir).unwrap();
        let mut file = FileSink::new(dir.clone(), RotationLimits::default()).unwrap();
        if mutate {
            // Not exposed by any production API; inject details inside the private
            // serializer -> raw-writer edge, not into an unrelated format/panic test.
            let mut line = checked_line(&event).unwrap();
            line.insert_str(line.len() - 1, &format!(",\"details\":\"{CANARY}\""));
            console.write_line(&line).unwrap();
            file.write_line(&line, UNIX_EPOCH).unwrap();
        } else {
            console.write_event(&event).unwrap();
            file.write_event(&event).unwrap();
        }
        file.flush().unwrap();
        drop(file);
        let console_bytes = captured.lock().unwrap().clone();
        let paths = fs::read_dir(&dir)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .collect::<Vec<_>>();
        assert_eq!(paths.len(), 1);
        let file_bytes = fs::read(&paths[0]).unwrap();
        fs::remove_dir_all(dir).unwrap();
        for bytes in [console_bytes, file_bytes] {
            assert!(bytes.ends_with(b"\n"));
            let line = &bytes[..bytes.len() - 1];
            assert!(line.len() <= MAX_EVENT_BYTES);
            let json: serde_json::Value = serde_json::from_slice(line).unwrap();
            assert_eq!(json["event"], "probe_started");
            assert_eq!(json["operation"], "probe");
            assert!(
                json.get("details").is_none()
                    && !bytes.windows(CANARY.len()).any(|s| s == CANARY.as_bytes()),
                "canary leaked through sink bytes"
            );
        }
    }

    #[test]
    fn typed_boundary_outputs_are_allowlisted() {
        check_sink_bytes(false);
    }

    // Run explicitly, separately: expected nonzero exit with concrete canary leakage.
    #[test]
    #[ignore]
    fn private_boundary_mutation_must_fail() {
        check_sink_bytes(true);
    }
}
