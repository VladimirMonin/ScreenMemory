//! Real SQLite and file-lane schedules on owned synthetic NTFS roots.
use super::*;
use sm_application::storage::{CaptureTarget, PhysicalBounds};
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::mpsc,
    time::Duration,
};

struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let root = base.join(Uuid::new_v4().to_string());
        fs::create_dir(&root).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(root.join(child)).unwrap();
        }
        Self(root)
    }
    fn try_lease(&self) -> Result<HostLease, LeaseError> {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &self.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
    }
    fn input(&self) -> OwnedStageInput {
        OwnedStageInput {
            library_id: LibraryId::new(Uuid::from_u128(1)),
            principal_id: PrincipalId::new(Uuid::from_u128(2)),
            operation_id: OperationId::new(Uuid::new_v4()),
            record_id: RecordId::new(Uuid::new_v4()),
            asset_id: AssetId::new(Uuid::new_v4()),
            frame_id: Uuid::new_v4(),
            mode: CaptureTarget::Region,
            context: CaptureContext {
                device: "synthetic".into(),
                app: "synthetic".into(),
                title: "synthetic".into(),
                target: CaptureTarget::Region,
                sources: vec![CaptureTarget::Region],
                bounds: PhysicalBounds {
                    x: 0,
                    y: 0,
                    width: 8,
                    height: 4,
                },
            },
            captured_at: "2026-09-28T09:15:00.000Z".into(),
            encoder_fingerprint: "synthetic-profile".into(),
            pixels: vec![7; 128],
        }
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn s02_t09c_01_file_pause_sqlite_responsive_lease_and_stop_ack() {
    let f = Fixture::new();
    let sentinel = f.0.join("sentinel");
    fs::write(&sentinel, b"untouched").unwrap();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let worker =
        StorageWorker::start_with_file_pause(f.try_lease().unwrap(), entered_tx, release_rx)
            .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let op = f.input();
        let operation = op.operation_id;
        let first = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(op).await }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let dir = f.0.join("staging").join(operation.as_uuid().to_string());
        assert!(dir.join("pixels.rgba.tmp").exists());
        assert!(!dir.join("manifest.json").exists());
        assert!(
            tokio::time::timeout(Duration::from_secs(2), worker.schema_snapshot())
                .await
                .unwrap()
                .is_ok()
        );
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        assert_eq!(
            worker.shutdown(Duration::from_millis(30)).await,
            Err(StorageError::Unavailable)
        );
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        assert!(matches!(
            worker.clone().stage_capture_owned(f.input()).await,
            Err(StageOwnedError::Unavailable)
        ));
        release_tx.send(()).unwrap();
        let staged = first.await.unwrap().unwrap();
        assert_eq!(staged.operation_id(), operation);
        assert!(dir.join("manifest.json").exists());
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(fs::read(&sentinel).unwrap(), b"untouched");
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09c_02_cancelled_waiter_drains_fifo_and_queue_is_bounded() {
    let f = Fixture::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(2);
    let (release_tx, release_rx) = mpsc::sync_channel(2);
    let worker =
        StorageWorker::start_with_file_pause(f.try_lease().unwrap(), entered_tx, release_rx)
            .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let first_input = f.input();
        let first_op = first_input.operation_id;
        let first = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(first_input).await }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let second_input = f.input();
        let second_op = second_input.operation_id;
        let second = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(second_input).await }
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            worker.stage_capture_owned(f.input()).await,
            Err(StageOwnedError::Unavailable)
        ));
        second.abort(); // Only the waiter is cancelled; admitted file write remains FIFO.
        let stopping = tokio::spawn({
            let worker = worker.clone();
            async move { worker.shutdown(Duration::from_secs(5)).await }
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            worker.stage_capture_owned(f.input()).await,
            Err(StageOwnedError::Unavailable)
        ));
        release_tx.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap().operation_id(), first_op);
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        release_tx.send(()).unwrap();
        stopping.await.unwrap().unwrap();
        assert!(f
            .0
            .join("staging")
            .join(second_op.as_uuid().to_string())
            .join("manifest.json")
            .exists());
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09c_05_full_sqlite_queue_shutdown_retry_keeps_lease() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let pause = worker.pause_for_test(Duration::from_millis(250));
        let pending: Vec<_> = (0..32)
            .map(|_| {
                let worker = worker.clone();
                tokio::spawn(async move { worker.schema_snapshot().await })
            })
            .collect();
        tokio::task::yield_now().await;
        assert_eq!(
            worker.shutdown(Duration::from_millis(10)).await,
            Err(StorageError::Unavailable)
        );
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        let _ = pause.await;
        for task in pending {
            assert!(task.await.unwrap().is_ok());
        }
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09c_04_root_error_and_duplicate_preserve_existing_bytes() {
    let blocked = Fixture::new();
    let marker = blocked.0.join("staging");
    fs::write(&marker, b"foreign-sentinel").unwrap();
    let worker = StorageWorker::start(blocked.try_lease().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        assert!(matches!(
            worker.stage_capture_owned(blocked.input()).await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Storage(_)
            ))
        ));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
    assert_eq!(fs::read(marker).unwrap(), b"foreign-sentinel");

    let f = Fixture::new();
    let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
    rt.block_on(async {
        let input = f.input();
        let id = input.operation_id;
        worker.stage_capture_owned(input).await.unwrap();
        let manifest =
            f.0.join("staging")
                .join(id.as_uuid().to_string())
                .join("manifest.json");
        let original = fs::read(&manifest).unwrap();
        let mut duplicate = f.input();
        duplicate.operation_id = id;
        assert!(matches!(
            worker.stage_capture_owned(duplicate).await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Storage(sm_isolation::StagingError::AlreadyExists)
            ))
        ));
        assert_eq!(fs::read(manifest).unwrap(), original);
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09d_01_probe_absent_incomplete_complete_and_unsafe_root() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let input = f.input();
        let operation = input.operation_id;
        assert_eq!(
            worker.probe_stage_presence(operation).await,
            Ok(StagePresence::Absent)
        );
        let dir = f.0.join("staging").join(operation.as_uuid().to_string());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("pixels.rgba.tmp"), b"incomplete").unwrap();
        assert_eq!(
            worker.probe_stage_presence(operation).await,
            Ok(StagePresence::Present)
        );
        fs::remove_dir_all(&dir).unwrap();
        worker.stage_capture_owned(input).await.unwrap();
        assert!(dir.join("manifest.json").exists());
        // Presence does not parse or trust manifest contents, even if corrupted.
        fs::write(dir.join("manifest.json"), b"synthetic-corrupt-manifest").unwrap();
        assert_eq!(
            worker.probe_stage_presence(operation).await,
            Ok(StagePresence::Present)
        );
        let unsafe_operation = OperationId::new(Uuid::new_v4());
        let sentinel =
            f.0.join("staging")
                .join(unsafe_operation.as_uuid().to_string());
        fs::write(&sentinel, b"not-a-directory").unwrap();
        assert!(matches!(
            worker.probe_stage_presence(unsafe_operation).await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Storage(_)
            ))
        ));
        assert_eq!(fs::read(sentinel).unwrap(), b"not-a-directory");
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
    let blocked = Fixture::new();
    let sentinel = blocked.0.join("staging");
    fs::write(&sentinel, b"not-a-root").unwrap();
    let worker = StorageWorker::start(blocked.try_lease().unwrap()).unwrap();
    rt.block_on(async {
        assert!(matches!(
            worker
                .probe_stage_presence(OperationId::new(Uuid::new_v4()))
                .await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Storage(_)
            ))
        ));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
    assert_eq!(fs::read(sentinel).unwrap(), b"not-a-root");
}

#[test]
fn s02_t09d_02_probe_is_fifo_with_stage_and_stop() {
    let f = Fixture::new();
    let (entered_tx, entered_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let worker =
        StorageWorker::start_with_file_pause(f.try_lease().unwrap(), entered_tx, release_rx)
            .unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let input = f.input();
        let operation = input.operation_id;
        let stage = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(input).await }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let probe = tokio::spawn({
            let worker = worker.clone();
            async move { worker.probe_stage_presence(operation).await }
        });
        tokio::task::yield_now().await;
        assert!(!probe.is_finished(), "probe cannot skip paused stage");
        assert_eq!(
            worker.shutdown(Duration::from_millis(30)).await,
            Err(StorageError::Unavailable)
        );
        assert!(matches!(
            worker.probe_stage_presence(operation).await,
            Err(StageOwnedError::Unavailable)
        ));
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        release_tx.send(()).unwrap();
        stage.await.unwrap().unwrap();
        assert_eq!(probe.await.unwrap(), Ok(StagePresence::Present));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09c_03_pre_admission_rejects_wrong_length_without_staging_write() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let mut bad = f.input();
        bad.pixels.pop();
        assert!(matches!(
            worker.stage_capture_owned(bad).await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(staging::StagingError::PixelLengthMismatch)
            ))
        ));
        assert!(!f.0.join("staging").exists());
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}
