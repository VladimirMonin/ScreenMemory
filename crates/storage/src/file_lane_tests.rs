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

fn reopen_input(input: &OwnedStageInput) -> OwnedReopenInputV2 {
    OwnedReopenInputV2 {
        library_id: input.library_id,
        principal_id: input.principal_id,
        operation_id: input.operation_id,
        mode: input.mode,
        encoder_fingerprint: input.encoder_fingerprint.clone(),
    }
}

#[test]
fn s02_t09k_empty_v2_profile_rejected_before_file_lane_write() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        let mut input = f.input();
        input.encoder_fingerprint.clear();
        let directory =
            f.0.join("staging")
                .join(input.operation_id.as_uuid().to_string());
        assert_eq!(
            worker
                .stage_capture_owned_v2(OwnedStageInputV2 {
                    capture: input,
                    warnings: vec![],
                })
                .await
                .unwrap_err(),
            StageOwnedError::Stage(staging_files::StageFailure::Rejected(
                staging::StagingError::ProfileMismatch
            ))
        );
        assert!(!directory.exists());
        let mut oversized = f.input();
        oversized.encoder_fingerprint = "x".repeat(513);
        assert_eq!(
            worker
                .stage_capture_owned_v2(OwnedStageInputV2 {
                    capture: oversized,
                    warnings: vec![],
                })
                .await
                .unwrap_err(),
            StageOwnedError::Stage(staging_files::StageFailure::Rejected(
                staging::StagingError::ManifestBeyondLimit
            ))
        );
        assert!(!directory.exists());
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(!directory.exists());
    });
}

#[test]
fn s02_t09k_01_reacquired_worker_reopens_exact_v2_without_db_receipt() {
    use sha2::{Digest, Sha256};
    let f = Fixture::new();
    let sentinel = f.0.join("sentinel");
    fs::write(&sentinel, b"preserve").unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let input = f.input();
        let expected = reopen_input(&input);
        let ids = (input.record_id, input.asset_id, input.frame_id);
        let context = input.context.clone();
        let pixel_hash = format!("{:x}", Sha256::digest(&input.pixels));
        let pixels = input.pixels.clone();
        let warnings = vec!["topology_changed".into(), "display_unknown".into()];
        let first = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        first.stage_capture_owned_v2(OwnedStageInputV2 {
            capture: input, warnings: warnings.clone(),
        }).await.unwrap();
        assert!(matches!(first.lookup_operation(expected.operation_id, expected.principal_id, expected.mode).await, Ok(LookupOutcome::Unknown)));
        first.shutdown(Duration::from_secs(5)).await.unwrap();
        let second = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        let reopened = second.reopen_stage_owned_v2(expected).await.unwrap();
        let envelope = reopened.envelope();
        let staged = envelope.staging();
        assert_eq!(envelope.schema_version(), 2);
        assert_eq!(envelope.warnings(), Some(warnings.as_slice()));
        assert_eq!((staged.record_id(), staged.asset_id(), staged.frame_id()), ids);
        assert_eq!(staged.context(), &context);
        assert_eq!(staged.pixels_sha256(), pixel_hash);
        assert_eq!(reopened.pixels(), pixels);
        assert_eq!(format!("{:x}", Sha256::digest(reopened.pixels())), pixel_hash);
        assert_eq!(envelope.path_policy(), Some(staging::RECOVERY_PATH_POLICY));
        assert!(matches!(envelope.automatic_provenance(), Some(sm_application::storage::Provenance::Automatic { when, source }) if when == staged.captured_at() && source == staged.mode()));
        assert!(matches!(second.lookup_operation(staged.operation_id(), staged.principal_id(), staged.mode()).await, Ok(LookupOutcome::Unknown)));
        second.shutdown(Duration::from_secs(5)).await.unwrap();
    });
    assert_eq!(fs::read(sentinel).unwrap(), b"preserve");
}

#[test]
fn s02_t09k_02_v1_foreign_and_tamper_fail_without_deletion() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        let v1 = f.input();
        let legacy = reopen_input(&v1);
        worker.stage_capture_owned(v1).await.unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(legacy).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(
                    staging::StagingError::UnsupportedSchemaVersion
                )
            ))
        ));
        let v2 = f.input();
        let trusted = reopen_input(&v2);
        let dir =
            f.0.join("staging")
                .join(v2.operation_id.as_uuid().to_string());
        worker
            .stage_capture_owned_v2(OwnedStageInputV2 {
                capture: v2,
                warnings: vec!["topology_changed".into()],
            })
            .await
            .unwrap();
        let sentinel = dir.join("sentinel");
        fs::write(&sentinel, b"keep").unwrap();
        let mut foreign = OwnedReopenInputV2 {
            library_id: trusted.library_id,
            principal_id: PrincipalId::new(Uuid::new_v4()),
            operation_id: trusted.operation_id,
            mode: trusted.mode,
            encoder_fingerprint: trusted.encoder_fingerprint.clone(),
        };
        assert!(matches!(
            worker.reopen_stage_owned_v2(foreign).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::ForeignIdentity)
            ))
        ));
        foreign = OwnedReopenInputV2 {
            library_id: trusted.library_id,
            principal_id: trusted.principal_id,
            operation_id: trusted.operation_id,
            mode: trusted.mode,
            encoder_fingerprint: "wrong-profile".into(),
        };
        assert!(matches!(
            worker.reopen_stage_owned_v2(foreign).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::ProfileMismatch)
            ))
        ));
        let pixels_path = dir.join("pixels.rgba");
        let mut pixels = fs::read(&pixels_path).unwrap();
        pixels[0] ^= 1;
        fs::write(&pixels_path, &pixels).unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(trusted).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::PixelDigestMismatch)
            ))
        ));
        assert_eq!(fs::read(&pixels_path).unwrap(), pixels);
        assert_eq!(fs::read(&sentinel).unwrap(), b"keep");
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09k_03_incomplete_oversize_and_stop_keep_sentinel() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        let input = f.input();
        let trusted = reopen_input(&input);
        assert!(matches!(
            worker.reopen_stage_owned_v2(reopen_input(&input)).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Incomplete
            ))
        ));
        let dir =
            f.0.join("staging")
                .join(input.operation_id.as_uuid().to_string());
        fs::create_dir(&dir).unwrap();
        fs::write(dir.join("sentinel"), b"keep").unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(reopen_input(&input)).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Incomplete
            ))
        ));
        let manifest = dir.join("manifest.json");
        let file = fs::File::create(&manifest).unwrap();
        file.set_len((staging::MAX_MANIFEST_BYTES + 1) as u64)
            .unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(trusted).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::ManifestBeyondLimit)
            ))
        ));
        assert_eq!(
            worker.probe_stage_presence(input.operation_id).await,
            Ok(StagePresence::Present)
        );
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(reopen_input(&input)).await,
            Err(ReopenOwnedError::Unavailable)
        ));
        assert_eq!(fs::read(dir.join("sentinel")).unwrap(), b"keep");
        assert_eq!(
            fs::metadata(manifest).unwrap().len(),
            (staging::MAX_MANIFEST_BYTES + 1) as u64
        );
    });
}

#[test]
fn s02_t09k_06_unsafe_root_and_invalid_trusted_input_fail_closed() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
        let input = f.input();
        let mut invalid = reopen_input(&input);
        invalid.encoder_fingerprint = "x".repeat(513);
        assert!(matches!(
            worker.reopen_stage_owned_v2(invalid).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Rejected(staging::StagingError::ProfileMismatch)
            ))
        ));
        let sentinel = f.0.join("staging");
        fs::write(&sentinel, b"not-a-directory").unwrap();
        assert!(matches!(
            worker.reopen_stage_owned_v2(reopen_input(&input)).await,
            Err(ReopenOwnedError::Reopen(
                staging_files::ReopenFailure::Storage(_)
            ))
        ));
        assert_eq!(fs::read(&sentinel).unwrap(), b"not-a-directory");
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
    });
}

#[test]
fn s02_t09k_04_fifo_stage_probe_reopen_stop_drains_before_lease_release() {
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
        let trusted = reopen_input(&input);
        let operation = input.operation_id;
        let stage = tokio::spawn({
            let worker = worker.clone();
            async move {
                worker
                    .stage_capture_owned_v2(OwnedStageInputV2 {
                        capture: input,
                        warnings: vec!["topology_changed".into()],
                    })
                    .await
            }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let probe = tokio::spawn({
            let worker = worker.clone();
            async move { worker.probe_stage_presence(operation).await }
        });
        tokio::task::yield_now().await;
        // The one queued slot belongs to Probe; this Reopen is rejected as overload,
        // not misreported as an incomplete file or as a stopped worker.
        let rejected = worker
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: trusted.library_id,
                principal_id: trusted.principal_id,
                operation_id: trusted.operation_id,
                mode: trusted.mode,
                encoder_fingerprint: trusted.encoder_fingerprint.clone(),
            })
            .await;
        assert!(matches!(rejected, Err(ReopenOwnedError::Unavailable)));
        assert!(!probe.is_finished());
        assert_eq!(
            worker.shutdown(Duration::from_millis(30)).await,
            Err(StorageError::Unavailable)
        );
        assert!(matches!(
            worker.reopen_stage_owned_v2(trusted).await,
            Err(ReopenOwnedError::Unavailable)
        ));
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        release_tx.send(()).unwrap();
        assert!(stage.await.unwrap().unwrap().is_recovery_ready());
        assert_eq!(probe.await.unwrap(), Ok(StagePresence::Present));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09k_05_admitted_reopen_completes_before_stop_ack() {
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
        let trusted = reopen_input(&input);
        let stage = tokio::spawn({
            let worker = worker.clone();
            async move {
                worker
                    .stage_capture_owned_v2(OwnedStageInputV2 {
                        capture: input,
                        warnings: vec!["topology_changed".into()],
                    })
                    .await
            }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let reopen = tokio::spawn({
            let worker = worker.clone();
            async move { worker.reopen_stage_owned_v2(trusted).await }
        });
        tokio::task::yield_now().await;
        assert!(!reopen.is_finished());
        assert_eq!(
            worker.shutdown(Duration::from_millis(30)).await,
            Err(StorageError::Unavailable)
        );
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        release_tx.send(()).unwrap();
        stage.await.unwrap().unwrap();
        let reopened = reopen.await.unwrap().unwrap();
        assert_eq!(
            reopened.envelope().warnings(),
            Some(["topology_changed".to_owned()].as_slice())
        );
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(f.try_lease().unwrap());
    });
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
fn s02_t09j_v2_file_pause_keeps_sqlite_responsive_and_stop_waits_for_lease() {
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
        let warnings = vec!["topology_changed".into(), "display_unknown".into()];
        let stage = tokio::spawn({
            let worker = worker.clone();
            let warnings = warnings.clone();
            async move {
                worker
                    .stage_capture_owned_v2(OwnedStageInputV2 {
                        capture: input,
                        warnings,
                    })
                    .await
            }
        });
        tokio::task::yield_now().await;
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
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
        let denied = worker
            .stage_capture_owned_v2(OwnedStageInputV2 {
                capture: f.input(),
                warnings: vec![],
            })
            .await;
        assert!(matches!(denied, Err(StageOwnedError::Unavailable)));
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));
        release_tx.send(()).unwrap();
        let envelope = stage.await.unwrap().unwrap();
        assert_eq!(envelope.schema_version(), 2);
        assert_eq!(envelope.warnings(), Some(warnings.as_slice()));
        let manifest = fs::read(
            f.0.join("staging")
                .join(operation.as_uuid().to_string())
                .join("manifest.json"),
        )
        .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&manifest).unwrap();
        assert_eq!(json["schema_version"], 2);
        assert_eq!(json["warnings"], serde_json::json!(warnings));
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        drop(f.try_lease().unwrap());
    });
}

#[test]
fn s02_t09j_v2_invalid_warnings_refused_before_admission_or_write() {
    let f = Fixture::new();
    let worker = StorageWorker::start(f.try_lease().unwrap()).unwrap();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let input = f.input();
        let op = input.operation_id;
        assert!(matches!(
            worker
                .stage_capture_owned_v2(OwnedStageInputV2 {
                    capture: input,
                    warnings: vec!["invalid/path".into()],
                })
                .await,
            Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(staging::StagingError::InvalidWarnings)
            ))
        ));
        assert!(!f.0.join("staging").join(op.as_uuid().to_string()).exists());
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
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
        let pending_input = f.input();
        let pending_op = pending_input.operation_id;
        let pending = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(pending_input).await }
        });
        tokio::task::yield_now().await;
        assert!(!pending.is_finished());
        second.abort(); // Only the waiter is cancelled; admitted file write remains FIFO.
        let stopping = tokio::spawn({
            let worker = worker.clone();
            async move { worker.shutdown(Duration::from_secs(5)).await }
        });
        tokio::task::yield_now().await;
        assert!(matches!(
            pending.await.unwrap(),
            Err(StageOwnedError::Unavailable)
        ));
        assert!(
            !stopping.is_finished(),
            "stop cannot acknowledge a paused file stage"
        );
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
        assert!(!f
            .0
            .join("staging")
            .join(pending_op.as_uuid().to_string())
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
fn s02_t09g_stage_waits_behind_queued_probe_without_losing_owned_pixels() {
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

        // A is executing; B takes the only queued slot. C must retain its original
        // owned allocation in its caller task until the file lane can accept it.
        let probe = tokio::spawn({
            let worker = worker.clone();
            async move { worker.probe_stage_presence(first_op).await }
        });
        tokio::task::yield_now().await;
        assert!(!probe.is_finished());
        let third_input = f.input();
        let third_op = third_input.operation_id;
        let third = tokio::spawn({
            let worker = worker.clone();
            async move { worker.stage_capture_owned(third_input).await }
        });
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(
            !third.is_finished(),
            "accepted C cannot be dropped on a full file queue"
        );
        assert!(
            tokio::time::timeout(Duration::from_secs(2), worker.schema_snapshot())
                .await
                .unwrap()
                .is_ok()
        );
        assert!(matches!(f.try_lease(), Err(LeaseError::Busy)));

        release_tx.send(()).unwrap();
        assert_eq!(first.await.unwrap().unwrap().operation_id(), first_op);
        assert_eq!(probe.await.unwrap(), Ok(StagePresence::Present));
        entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(!third.is_finished());
        release_tx.send(()).unwrap();
        assert_eq!(third.await.unwrap().unwrap().operation_id(), third_op);
        assert!(f
            .0
            .join("staging")
            .join(third_op.as_uuid().to_string())
            .join("manifest.json")
            .exists());
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
