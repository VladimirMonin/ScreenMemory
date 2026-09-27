use sm_application::observability::{
    Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
};
use sm_observability::{
    BoundedLogger, Clock, ConsoleSink, FailingLogSink, FakeClock, FileSink, FlushOutcome, LogSink,
    RotationLimits, MAX_EVENT_BYTES,
};
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

fn emit_eventual(sink: &impl SafeEventSink, e: SafeEvent) {
    for _ in 0..1000 {
        if sink.emit(e) {
            return;
        }
        std::thread::yield_now();
    }
    panic!("synthetic queue did not accept event");
}

fn event(level: Level, id: EventId) -> SafeEvent {
    let mut e = SafeEvent::new(level, Component::Probe, id, Operation::Probe);
    e.outcome = Some(Outcome::Failed);
    e.safe_code = Some(SafeCode::InternalFailure);
    e.duration_ms = Some(7);
    e
}
fn root() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../.pi");
    fs::create_dir_all(&root).unwrap();
    let root = fs::canonicalize(root).unwrap();
    let dir = root.join(format!(
        "s01-log-{}-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir(&dir).unwrap();
    dir
}
fn owned_files(root: &PathBuf) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| {
            p.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("operational-")
        })
        .collect()
}

#[derive(Clone)]
struct SharedWriter(Arc<Mutex<Vec<u8>>>);
impl Write for SharedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn progress_terminal_and_independent_clocks() {
    let clock = Arc::new(FakeClock::new(UNIX_EPOCH + Duration::from_secs(42)));
    let output = Arc::new(Mutex::new(Vec::new()));
    let mut log = BoundedLogger::start(
        ConsoleSink::new(SharedWriter(output.clone())),
        clock.clone(),
        4,
    )
    .unwrap();
    let p = log.producer();
    emit_eventual(&p, event(Level::Info, EventId::ProbeProgress));
    clock.advance_utc(Duration::from_secs(200));
    assert!(!p.emit(event(Level::Info, EventId::ProbeProgress)));
    emit_eventual(&p, event(Level::Info, EventId::ProbeCompleted));
    clock.advance_monotonic(Duration::from_secs(1));
    emit_eventual(&p, event(Level::Info, EventId::ProbeProgress));
    assert_eq!(log.shutdown(Duration::from_secs(3)), FlushOutcome::Flushed);
    assert!(log.counters().dropped >= 1);
    let data = String::from_utf8(output.lock().unwrap().clone()).unwrap();
    let lines: Vec<serde_json::Value> = data
        .lines()
        .map(|s| serde_json::from_str(s).unwrap())
        .collect();
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["timestamp_unix_ms"], 42000);
    assert_eq!(lines[1]["timestamp_unix_ms"], 242000);
    assert_eq!(lines[1]["event"], "probe_completed");
}

struct BlockedSink(Arc<(Mutex<bool>, Condvar)>, Arc<Mutex<Vec<String>>>);
impl LogSink for BlockedSink {
    fn write_event(&mut self, event: &SafeEvent) -> io::Result<()> {
        let (lock, wake) = &*self.0;
        let mut ready = lock.lock().unwrap();
        while !*ready {
            ready = wake.wait(ready).unwrap();
        }
        self.1
            .lock()
            .unwrap()
            .push(serde_json::to_string(event).unwrap());
        Ok(())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
#[test]
fn full_queue_prioritises_warning_and_timeout_does_not_claim_flush() {
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let clock = Arc::new(FakeClock::new(UNIX_EPOCH));
    let written = Arc::new(Mutex::new(Vec::new()));
    let mut log =
        BoundedLogger::start(BlockedSink(gate.clone(), written.clone()), clock, 2).unwrap();
    let p = log.producer();
    emit_eventual(&p, event(Level::Info, EventId::ProbeStarted));
    let initial_drops = log.counters().dropped;
    // Ensure worker is blocked in I/O before filling the queue.
    let start = std::time::Instant::now();
    while log.counters().queued != 0 && start.elapsed() < Duration::from_secs(2) {
        std::thread::yield_now();
    }
    assert_eq!(log.counters().queued, 0);
    assert!(p.emit(event(Level::Info, EventId::ProbeBlocked)));
    assert!(p.emit(event(Level::Info, EventId::ProbeCompleted)));
    assert!(!p.emit(event(Level::Info, EventId::ProbeStarted)));
    assert!(p.emit(event(Level::Warn, EventId::HostFailed)));
    assert!(p.emit(event(Level::Error, EventId::BridgeExited))); // evicts remaining INFO
    assert!(!p.emit(event(Level::Error, EventId::BridgeExited))); // only high-priority slots remain
    assert_eq!(log.counters().queued, 2);
    assert_eq!(log.counters().dropped, initial_drops + 4);
    assert_eq!(
        log.shutdown(Duration::from_millis(20)),
        FlushOutcome::TimedOut
    );
    assert!(!p.emit(event(Level::Info, EventId::ProbeStarted)));
    {
        let (lock, wake) = &*gate;
        *lock.lock().unwrap() = true;
        wake.notify_all();
    }
    assert_eq!(log.shutdown(Duration::from_secs(3)), FlushOutcome::Flushed);
    assert_eq!(log.counters().dropped, initial_drops + 5);
    assert_eq!(log.shutdown(Duration::ZERO), FlushOutcome::Flushed);
    let written = written.lock().unwrap();
    assert_eq!(written.len(), 3);
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&written[1]).unwrap()["level"],
        "WARN"
    );
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&written[2]).unwrap()["level"],
        "ERROR"
    );
}

#[test]
fn failing_sink_counters_are_not_success() {
    let mut log =
        BoundedLogger::start(FailingLogSink, Arc::new(FakeClock::new(UNIX_EPOCH)), 1).unwrap();
    emit_eventual(&log.producer(), event(Level::Error, EventId::HostFailed));
    assert_eq!(
        log.shutdown(Duration::from_secs(3)),
        FlushOutcome::SinkFailed
    );
    assert_eq!(log.counters().sink_failed, 2); // write and flush
    assert!(
        BoundedLogger::start(FailingLogSink, Arc::new(FakeClock::new(UNIX_EPOCH)), 8193).is_err()
    );
}

#[test]
fn file_rotation_age_size_and_foreign_files() {
    let dir = root();
    let foreign = dir.join("foreign.txt");
    fs::write(&foreign, "keep").unwrap();
    let limits = RotationLimits {
        files: 2,
        bytes: 300,
        age: Duration::from_secs(14),
    };
    let mut sink = FileSink::new(dir.clone(), limits).unwrap();
    let mut e = event(Level::Info, EventId::ProbeStarted);
    assert!(serde_json::to_string(&e).unwrap().len() < 300);
    let clock = FakeClock::new(UNIX_EPOCH);
    for _ in 0..8 {
        e.timestamp_unix_ms = clock.utc().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
        sink.write_event(&e).unwrap();
        clock.advance_utc(Duration::from_secs(1));
    }
    sink.flush().unwrap();
    assert_eq!(owned_files(&dir).len(), 2);
    for p in owned_files(&dir) {
        let data = fs::read(p).unwrap();
        assert!(data.len() <= 300);
        for line in data.split(|b| *b == b'\n').filter(|line| !line.is_empty()) {
            assert!(line.len() <= MAX_EVENT_BYTES);
            let item: serde_json::Value = serde_json::from_slice(line).unwrap();
            assert_eq!(item["event"], "probe_started");
            assert!(item.get("details").is_none());
        }
    }
    clock.advance_utc(Duration::from_secs(92));
    e.timestamp_unix_ms = clock.utc().duration_since(UNIX_EPOCH).unwrap().as_millis() as u64;
    sink.write_event(&e).unwrap();
    sink.flush().unwrap();
    assert_eq!(owned_files(&dir).len(), 1);
    assert_eq!(fs::read_to_string(foreign).unwrap(), "keep");
    drop(sink);
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unexpected_linked_owned_entry_is_never_removed() {
    let dir = root();
    let target = dir.join("foreign.txt");
    fs::write(&target, "keep").unwrap();
    let link = dir.join("operational-00000000000000000000-00000000.jsonl");
    #[cfg(windows)]
    let linked = std::os::windows::fs::symlink_file(&target, &link).is_ok();
    #[cfg(unix)]
    let linked = std::os::unix::fs::symlink(&target, &link).is_ok();
    if linked {
        let mut sink = FileSink::new(dir.clone(), RotationLimits::default()).unwrap();
        assert!(sink
            .write_event(&event(Level::Info, EventId::ProbeStarted))
            .is_err());
        assert_eq!(fs::read_to_string(&target).unwrap(), "keep");
        assert!(fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink());
    }
    assert!(FileSink::new(PathBuf::from("."), RotationLimits::default()).is_err());
    fs::remove_dir_all(dir).unwrap();
}

fn assert_canary_absent(data: &str, canary: &str) {
    assert!(!data.contains(canary), "unsafe diagnostic payload");
}
#[test]
fn canary_child_entry() {
    if std::env::var_os("SM_LOG_CHILD").is_none() {
        return;
    }
    let dir = PathBuf::from(std::env::var_os("SM_LOG_ROOT").unwrap());
    let clock = Arc::new(FakeClock::new(UNIX_EPOCH));
    let mut file = BoundedLogger::start(
        FileSink::new(dir, RotationLimits::default()).unwrap(),
        clock.clone(),
        8,
    )
    .unwrap();
    emit_eventual(
        &file.producer(),
        event(Level::Info, EventId::ProbeCompleted),
    );
    emit_eventual(&file.producer(), event(Level::Error, EventId::BridgeExited));
    assert_eq!(file.shutdown(Duration::from_secs(2)), FlushOutcome::Flushed);
    let mut console = BoundedLogger::start(ConsoleSink::new(io::stderr()), clock, 8).unwrap();
    emit_eventual(
        &console.producer(),
        event(Level::Error, EventId::BridgeExited),
    );
    assert_eq!(
        console.shutdown(Duration::from_secs(2)),
        FlushOutcome::Flushed
    );
    let mut failed =
        BoundedLogger::start(FailingLogSink, Arc::new(FakeClock::new(UNIX_EPOCH)), 8).unwrap();
    emit_eventual(&failed.producer(), event(Level::Error, EventId::HostFailed));
    emit_eventual(
        &failed.producer(),
        event(Level::Error, EventId::BridgeExited),
    );
    assert_eq!(
        failed.shutdown(Duration::from_secs(2)),
        FlushOutcome::SinkFailed
    );
    assert_eq!(failed.counters().sink_failed, 3);
    println!("diagnostic result: failed (safe code)");
}

#[test]
fn process_file_console_stdout_stderr_are_canary_free() {
    let dir = root();
    let canary = "UNICODE-Ж-非標準-Canary_527!";
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "canary_child_entry", "--nocapture"])
        .env("SM_LOG_CHILD", "1")
        .env("SM_LOG_ROOT", &dir)
        .output()
        .unwrap();
    assert!(output.status.success(), "synthetic subprocess failed");
    assert_canary_absent(&String::from_utf8_lossy(&output.stdout), canary);
    assert_canary_absent(&String::from_utf8_lossy(&output.stderr), canary);
    let files = owned_files(&dir);
    assert_eq!(files.len(), 1);
    let file = fs::read_to_string(&files[0]).unwrap();
    assert_canary_absent(&file, canary);
    let file_events: Vec<serde_json::Value> = file
        .lines()
        .map(|line| {
            assert!(line.len() <= MAX_EVENT_BYTES);
            let item: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(item.get("details").is_none());
            assert!(item.as_object().unwrap().keys().all(|key| matches!(
                key.as_str(),
                "schema_version"
                    | "timestamp_unix_ms"
                    | "level"
                    | "component"
                    | "event"
                    | "operation"
                    | "correlation_id"
                    | "duration_ms"
                    | "outcome"
                    | "safe_code"
                    | "count"
            )));
            item
        })
        .collect();
    assert_eq!(file_events.len(), 2);
    let file_ids: Vec<_> = file_events
        .iter()
        .map(|item| item["event"].as_str().unwrap())
        .collect();
    assert!(file_ids.contains(&"probe_completed"));
    assert!(file_ids.contains(&"bridge_exited"));
    let console = String::from_utf8_lossy(&output.stderr);
    assert_eq!(console.matches("safe log sink unavailable").count(), 1);
    let console_events: Vec<serde_json::Value> = console
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if !line.starts_with('{') {
                return None;
            }
            assert!(line.len() <= MAX_EVENT_BYTES);
            let item: serde_json::Value = serde_json::from_str(line).unwrap();
            assert!(item.get("details").is_none());
            Some(item)
        })
        .collect();
    assert_eq!(console_events.len(), 1);
    assert_eq!(console_events[0]["event"], "bridge_exited");
    assert!(file_events.contains(&console_events[0]));
    fs::remove_dir_all(dir).unwrap();
}
