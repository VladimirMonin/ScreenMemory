#![cfg(windows)]
use sm_image::{sha256, Options, OutputProfile};
use sm_image_worker::{Cancellation, TrustedWorker, WorkerError};
#[cfg(feature = "worker-test-seams")]
use std::time::Instant;
use std::{path::Path, sync::Mutex, time::Duration};
static SERIAL: Mutex<()> = Mutex::new(());
const INPUT: &[u8] = include_bytes!("../../../tests/fixtures/images/odd-alpha.png");
fn worker() -> TrustedWorker {
    let path = Path::new(env!("CARGO_BIN_EXE_shot-image-worker"));
    let bytes = std::fs::read(path).unwrap();
    TrustedWorker::from_manifest(path, sha256(&bytes)).unwrap()
}
fn opts() -> Options {
    Options::canonical(OutputProfile::WebpExact).unwrap()
}
#[test]
fn s03_worker_real_bundled_webp_avif_roundtrip_and_package_refusal() {
    let _g = SERIAL.lock().unwrap();
    let w = worker();
    for profile in [OutputProfile::WebpExact, OutputProfile::AvifQuality90] {
        let id = uuid::Uuid::new_v4();
        let result = w
            .execute(
                id,
                INPUT.to_vec(),
                Options::canonical(profile).unwrap(),
                &Cancellation::default(),
                Duration::from_secs(30),
            )
            .unwrap();
        assert_eq!(result.operation_id, id);
        assert_eq!(result.report().source_sha256, sha256(INPUT));
        assert_eq!(result.report().output_sha256, sha256(result.bytes()));
        assert!(result.report().alpha_verified_exact);
    }
    let path = Path::new(env!("CARGO_BIN_EXE_shot-image-worker"));
    assert!(matches!(
        TrustedWorker::from_manifest(path, [0; 32]),
        Err(WorkerError::UntrustedPackage)
    ));
    assert!(matches!(
        TrustedWorker::from_manifest(Path::new("shot-image-worker.exe"), [0; 32]),
        Err(WorkerError::UntrustedPackage)
    ));
    let c = Cancellation::default();
    c.cancel();
    assert!(matches!(
        w.execute(
            uuid::Uuid::new_v4(),
            INPUT.to_vec(),
            opts(),
            &c,
            Duration::from_secs(1)
        ),
        Err(WorkerError::Cancelled)
    ));
}
#[test]
fn s03_worker_malformed_bounded_refusal_and_twenty_no_handle_growth() {
    let _g = SERIAL.lock().unwrap();
    let w = worker();
    // Establish the real stdio/thread/Windows runtime before measuring steady-state
    // ownership. Cold lazy handles are not a per-child leak; do not mask growth.
    w.execute(
        uuid::Uuid::new_v4(),
        INPUT.to_vec(),
        opts(),
        &Cancellation::default(),
        Duration::from_secs(30),
    )
    .unwrap();
    let mut before = 0;
    unsafe {
        assert_ne!(
            windows_sys::Win32::System::Threading::GetProcessHandleCount(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &mut before
            ),
            0
        );
    }
    assert!(matches!(
        w.execute(
            uuid::Uuid::new_v4(),
            vec![0; 32],
            opts(),
            &Cancellation::default(),
            Duration::from_secs(30)
        ),
        Err(WorkerError::Codec(sm_image::ImageError::UnsupportedFormat))
    ));
    for _ in 0..20 {
        w.execute(
            uuid::Uuid::new_v4(),
            INPUT.to_vec(),
            opts(),
            &Cancellation::default(),
            Duration::from_secs(30),
        )
        .unwrap();
    }
    let mut after = 0;
    unsafe {
        assert_ne!(
            windows_sys::Win32::System::Threading::GetProcessHandleCount(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                &mut after
            ),
            0
        );
    }
    assert!(
        after <= before,
        "own parent handles before={before}, after={after}"
    );
}
#[cfg(feature = "worker-test-seams")]
#[test]
fn s03_worker_hard_timeout_crash_cancel_overload_and_job_memory_no_leaked_child() {
    let _g = SERIAL.lock().unwrap();
    let w = worker();
    let start = Instant::now();
    assert!(matches!(
        w.execute(
            uuid::Uuid::new_v4(),
            b"SM-S03-TEST-HANG".to_vec(),
            opts(),
            &Cancellation::default(),
            Duration::from_millis(300)
        ),
        Err(WorkerError::Timeout)
    ));
    assert!(start.elapsed() < Duration::from_secs(6));
    assert!(matches!(
        w.execute(
            uuid::Uuid::new_v4(),
            b"SM-S03-TEST-CRASH".to_vec(),
            opts(),
            &Cancellation::default(),
            Duration::from_secs(30)
        ),
        Err(WorkerError::Protocol) | Err(WorkerError::Crashed)
    ));
    // The private Job Object's 1280MiB committed-memory limit must actually stop
    // allocation. Not an RSS assertion and not a fake wall-clock timeout proof.
    let result = w.execute(
        uuid::Uuid::new_v4(),
        b"SM-S03-TEST-OOM".to_vec(),
        opts(),
        &Cancellation::default(),
        Duration::from_secs(20),
    );
    assert!(matches!(
        result,
        Err(WorkerError::Protocol) | Err(WorkerError::Crashed)
    ));
    let cancel = Cancellation::default();
    let watcher = cancel.clone();
    std::thread::scope(|s| {
        let task = s.spawn(|| {
            w.execute(
                uuid::Uuid::new_v4(),
                b"SM-S03-TEST-HANG".to_vec(),
                opts(),
                &cancel,
                Duration::from_secs(30),
            )
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        while !watcher.is_executing() {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(matches!(
            w.execute(
                uuid::Uuid::new_v4(),
                INPUT.to_vec(),
                opts(),
                &Cancellation::default(),
                Duration::from_secs(1)
            ),
            Err(WorkerError::Overloaded)
        ));
        watcher.cancel();
        assert!(matches!(task.join().unwrap(), Err(WorkerError::Cancelled)));
        assert!(!watcher.is_executing());
    });
    // Every failure returns only after WaitForSingleObject and job ActiveProcesses=0.
    // Next work gets the same single permit only after that proof, not after timeout.
    w.execute(
        uuid::Uuid::new_v4(),
        INPUT.to_vec(),
        opts(),
        &Cancellation::default(),
        Duration::from_secs(30),
    )
    .unwrap();
}
