use super::*;
use crate::PrepareCandidateError;
use crate::{OwnedReopenInputV2, OwnedStageInput, OwnedStageInputV2, StorageWorker};
use sm_application::{
    capture::{CaptureMode, CaptureRequest, CaptureTrigger},
    storage::{
        CaptureContext, CaptureTarget, CreateCaptureIntent, CreateIntentOutcome,
        FoundationRepository, LookupOutcome, PhysicalBounds, Provenance, ReadIntentOutcome,
    },
};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{HostLease, LibraryRootResolver, RootSource};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use uuid::Uuid;

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
    fn lease(&self) -> HostLease {
        HostLease::acquire(
            LibraryRootResolver::resolve(RootSource::Test {
                root: &self.0,
                forbidden_donor_roots: &[],
            })
            .unwrap(),
        )
        .unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

fn input(library_id: LibraryId, pixels: &[u8]) -> CreateCaptureIntent {
    let mode = CaptureTarget::Region;
    CreateCaptureIntent {
        library_id,
        principal_id: PrincipalId::new(Uuid::new_v4()),
        request: CaptureRequest::new(
            Uuid::new_v4(),
            CaptureMode::Region,
            CaptureTrigger::ClientCommand,
        ),
        record_id: RecordId::new(Uuid::new_v4()),
        asset_id: AssetId::new(Uuid::new_v4()),
        frame_id: Uuid::new_v4(),
        width: 3,
        height: 2,
        pixel_sha256: format!("{:x}", Sha256::digest(pixels)),
        context: CaptureContext {
            device: "synthetic-device".into(),
            app: "synthetic-app".into(),
            title: "synthetic-title".into(),
            target: mode,
            sources: vec![mode],
            bounds: PhysicalBounds {
                x: -5,
                y: 0,
                width: 3,
                height: 2,
            },
        },
        warnings: vec!["display_unknown".into(), "window_unknown".into()],
        encoder_fingerprint: "bundled-webp-v1".into(),
        captured_at_utc: "2026-09-28T09:15:00.000Z".into(),
    }
}

fn stage(input: &CreateCaptureIntent, pixels: Vec<u8>) -> OwnedStageInputV2 {
    OwnedStageInputV2 {
        capture: OwnedStageInput {
            library_id: input.library_id,
            principal_id: input.principal_id,
            operation_id: OperationId::new(input.request.operation_id()),
            record_id: input.record_id,
            asset_id: input.asset_id,
            frame_id: input.frame_id,
            mode: input.request.target(),
            context: input.context.clone(),
            captured_at: input.captured_at_utc.clone(),
            encoder_fingerprint: input.encoder_fingerprint.clone(),
            pixels,
        },
        warnings: input.warnings.clone(),
    }
}

#[test]
fn s02_t09m3_01_pending_before_stage_restart_exact_and_field_refusals() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let first = StorageWorker::start(f.lease()).unwrap();
        let library = LibraryId::new(first.schema_snapshot().await.unwrap().library_id);
        let pixels = vec![7u8; 24];
        let original = input(library, &pixels);
        let CreateIntentOutcome::Created(pending) =
            first.create_capture_intent(original.clone()).await.unwrap()
        else {
            panic!("created");
        };
        first
            .stage_capture_owned_v2(stage(&original, pixels.clone()))
            .await
            .unwrap();
        let mut empty = input(library, &pixels);
        empty.warnings.clear();
        first.create_capture_intent(empty.clone()).await.unwrap();
        first
            .stage_capture_owned_v2(stage(&empty, pixels.clone()))
            .await
            .unwrap();
        // A valid v2 stage can belong to this very operation yet disagree with
        // its SQLite warnings; equality must refuse it without touching either.
        let negative = input(library, &pixels);
        first.create_capture_intent(negative.clone()).await.unwrap();
        let mut mismatched_stage = stage(&negative, pixels.clone());
        mismatched_stage.warnings.reverse();
        first
            .stage_capture_owned_v2(mismatched_stage)
            .await
            .unwrap();
        first.shutdown(Duration::from_secs(5)).await.unwrap();

        let second = StorageWorker::start(f.lease()).unwrap();
        let ValidatedIntentRead::Pending(witness) = second
            .read_validated_capture_intent(library, original.principal_id, original.request.clone())
            .await
            .unwrap()
        else {
            panic!("validated pending");
        };
        let ReadIntentOutcome::Pending(read_back) = second
            .read_capture_intent(library, original.principal_id, original.request.clone())
            .await
            .unwrap()
        else {
            panic!("pending");
        };
        assert_eq!(pending, read_back);
        assert_eq!(*read_back, witness.intent);
        assert!(matches!(
            second
                .read_validated_capture_intent(
                    library,
                    PrincipalId::new(Uuid::new_v4()),
                    original.request.clone(),
                )
                .await
                .unwrap(),
            ValidatedIntentRead::Conflict
        ));
        assert!(matches!(
            second
                .read_validated_capture_intent(
                    library,
                    original.principal_id,
                    CaptureRequest::new(
                        Uuid::new_v4(),
                        CaptureMode::Region,
                        CaptureTrigger::ClientCommand,
                    ),
                )
                .await
                .unwrap(),
            ValidatedIntentRead::Unknown
        ));
        let reopened = second
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: library,
                principal_id: original.principal_id,
                operation_id: OperationId::new(original.request.operation_id()),
                mode: original.request.target(),
                encoder_fingerprint: original.encoder_fingerprint.clone(),
            })
            .await
            .unwrap();
        let matched = match_pending_reopened_v2(&witness, &reopened).unwrap();
        assert_eq!(matched.pixels(), pixels);
        assert_eq!(matched.context(), &original.context);
        assert_eq!(
            matched.planned_created_at_utc(),
            pending.planned_created_at_utc
        );
        assert_ne!(matched.planned_created_at_utc(), original.captured_at_utc);
        assert_eq!(matched.captured_at_utc(), original.captured_at_utc);
        assert_eq!(matched.warnings(), original.warnings);
        assert_eq!(
            reopened.envelope().automatic_provenance(),
            Some(Provenance::Automatic {
                when: matched.captured_at_utc().to_owned(),
                source: matched.context().target,
            })
        );

        macro_rules! mismatch {
            ($change:expr) => {{
                let mut wrong = (*read_back).clone();
                $change(&mut wrong);
                assert!(matches!(
                    match_envelope(&wrong, reopened.envelope(), reopened.pixels()),
                    Err(MatchRecoveryError::Mismatch)
                ));
            }};
        }
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.library_id =
                LibraryId::new(Uuid::new_v4())
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.principal_id =
                PrincipalId::new(Uuid::new_v4())
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.request =
                CaptureRequest::new(
                    Uuid::new_v4(),
                    CaptureMode::Region,
                    CaptureTrigger::ClientCommand
                )
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.request =
                CaptureRequest::new(
                    original.request.operation_id(),
                    CaptureMode::Desktop,
                    CaptureTrigger::ClientCommand
                )
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.record_id =
                RecordId::new(Uuid::new_v4())
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.asset_id =
                AssetId::new(Uuid::new_v4())
        );
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.frame_id =
                Uuid::new_v4()
        );
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v.input.width = 4);
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v.input.height = 3);
        mismatch!(
            |v: &mut sm_application::storage::PendingCaptureIntent| v.input.pixel_sha256 =
                "a".repeat(64)
        );
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v
            .input
            .context
            .title
            .push_str("-changed"));
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v
            .input
            .context
            .bounds
            .x += 1);
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v
            .input
            .captured_at_utc =
            "2026-09-28T09:15:01.000Z".into());
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v
            .input
            .encoder_fingerprint =
            "different".into());
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v
            .input
            .warnings
            .reverse());
        mismatch!(|v: &mut sm_application::storage::PendingCaptureIntent| v.input.warnings.clear());
        // The public DTO's month can be forged, but the sealed witness is unchanged.
        let mut forged = (*read_back).clone();
        forged.planned_created_at_utc = "2037-12-01T00:00:00.000Z".into();
        assert_eq!(
            matched.planned_created_at_utc(),
            pending.planned_created_at_utc
        );
        assert_ne!(
            matched.planned_created_at_utc(),
            forged.planned_created_at_utc
        );
        let ValidatedIntentRead::Pending(empty_pending) = second
            .read_validated_capture_intent(library, empty.principal_id, empty.request.clone())
            .await
            .unwrap()
        else {
            panic!("empty pending")
        };
        let empty_reopened = second
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: library,
                principal_id: empty.principal_id,
                operation_id: OperationId::new(empty.request.operation_id()),
                mode: empty.request.target(),
                encoder_fingerprint: empty.encoder_fingerprint.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            match_pending_reopened_v2(&empty_pending, &empty_reopened)
                .unwrap()
                .warnings(),
            &[] as &[String]
        );
        assert_eq!(
            match_pending_reopened_v2(&witness, &empty_reopened).unwrap_err(),
            MatchRecoveryError::Mismatch
        );
        let ValidatedIntentRead::Pending(negative_witness) = second
            .read_validated_capture_intent(library, negative.principal_id, negative.request.clone())
            .await
            .unwrap()
        else {
            panic!("own negative pending");
        };
        let negative_reopened = second
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: library,
                principal_id: negative.principal_id,
                operation_id: OperationId::new(negative.request.operation_id()),
                mode: negative.request.target(),
                encoder_fingerprint: negative.encoder_fingerprint.clone(),
            })
            .await
            .unwrap();
        assert_eq!(
            match_pending_reopened_v2(&negative_witness, &negative_reopened).unwrap_err(),
            MatchRecoveryError::Mismatch
        );
        let v1 = ValidatedRecoveryEnvelope::LegacyV1(reopened.envelope().staging().clone());
        assert_eq!(
            match_envelope(&read_back, &v1, reopened.pixels()),
            Err(MatchRecoveryError::Mismatch)
        );
        assert_eq!(
            second
                .lookup_operation(
                    OperationId::new(original.request.operation_id()),
                    original.principal_id,
                    original.request.target()
                )
                .await
                .unwrap(),
            LookupOutcome::Unknown
        );
        second.shutdown(Duration::from_secs(5)).await.unwrap();
        assert!(f
            .0
            .join("staging")
            .join(original.request.operation_id().to_string())
            .join("manifest.json")
            .exists());
        let db = rusqlite::Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        for table in ["records", "assets", "ingest_operations"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                    row.get(0)
                })
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
    });
}

#[test]
fn s02_t09n2_01_sealed_candidate_real_exact_alpha_and_refusals() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let first = StorageWorker::start(f.lease()).unwrap();
        let library = LibraryId::new(first.schema_snapshot().await.unwrap().library_id);
        // Transparent pixels retain nonzero RGB in the real libwebp exact roundtrip.
        let pixels = vec![
            11, 22, 33, 0, 70, 80, 90, 255, 8, 9, 10, 0, 1, 2, 3, 255, 40, 50, 60, 0, 7, 8, 9, 255,
        ];
        let mut good = input(library, &pixels);
        good.encoder_fingerprint = sm_codec::encoder_fingerprint();
        first.create_capture_intent(good.clone()).await.unwrap();
        first
            .stage_capture_owned_v2(stage(&good, pixels.clone()))
            .await
            .unwrap();
        let old_profile = input(library, &pixels);
        first
            .create_capture_intent(old_profile.clone())
            .await
            .unwrap();
        first
            .stage_capture_owned_v2(stage(&old_profile, pixels.clone()))
            .await
            .unwrap();
        let mut bad_warnings = input(library, &pixels);
        bad_warnings.encoder_fingerprint = sm_codec::encoder_fingerprint();
        first
            .create_capture_intent(bad_warnings.clone())
            .await
            .unwrap();
        let mut wrong_stage = stage(&bad_warnings, pixels.clone());
        wrong_stage.warnings.reverse();
        first.stage_capture_owned_v2(wrong_stage).await.unwrap();
        first.shutdown(Duration::from_secs(5)).await.unwrap();

        let second = StorageWorker::start(f.lease()).unwrap();
        let reopen = |i: &CreateCaptureIntent| OwnedReopenInputV2 {
            library_id: i.library_id,
            principal_id: i.principal_id,
            operation_id: OperationId::new(i.request.operation_id()),
            mode: i.request.target(),
            encoder_fingerprint: i.encoder_fingerprint.clone(),
        };
        assert!(matches!(
            second
                .read_validated_capture_intent(
                    library,
                    PrincipalId::new(Uuid::new_v4()),
                    good.request.clone()
                )
                .await
                .unwrap(),
            ValidatedIntentRead::Conflict
        ));
        assert!(second
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                principal_id: PrincipalId::new(Uuid::new_v4()),
                ..reopen(&good)
            })
            .await
            .is_err());
        let ValidatedIntentRead::Pending(witness) = second
            .read_validated_capture_intent(library, good.principal_id, good.request.clone())
            .await
            .unwrap()
        else {
            panic!("pending")
        };
        let mismatched = second
            .reopen_stage_owned_v2(reopen(&bad_warnings))
            .await
            .unwrap();
        assert!(matches!(
            second.prepare_recovery_candidate(witness, mismatched).await,
            Err(PrepareCandidateError::Mismatch(_))
        ));
        let ValidatedIntentRead::Pending(witness) = second
            .read_validated_capture_intent(
                library,
                old_profile.principal_id,
                old_profile.request.clone(),
            )
            .await
            .unwrap()
        else {
            panic!("pending")
        };
        let reopened = second
            .reopen_stage_owned_v2(reopen(&old_profile))
            .await
            .unwrap();
        assert!(matches!(
            second.prepare_recovery_candidate(witness, reopened).await,
            Err(PrepareCandidateError::EncoderProfileMismatch)
        ));
        let ValidatedIntentRead::Pending(witness) = second
            .read_validated_capture_intent(library, good.principal_id, good.request.clone())
            .await
            .unwrap()
        else {
            panic!("pending")
        };
        let reopened = second.reopen_stage_owned_v2(reopen(&good)).await.unwrap();
        let candidate = second
            .prepare_recovery_candidate(witness, reopened)
            .await
            .unwrap();
        assert_eq!(
            candidate.trusted_ids(),
            (
                good.library_id,
                good.principal_id,
                OperationId::new(good.request.operation_id()),
                good.record_id,
                good.asset_id,
            )
        );
        assert_eq!(candidate.geometry(), (3, 2));
        assert_eq!(candidate.length() as usize, candidate.encoded_bytes().len());
        assert_eq!(candidate.encoder_fingerprint(), good.encoder_fingerprint);
        assert_eq!(
            candidate.encoded_sha256().as_slice(),
            Sha256::digest(candidate.encoded_bytes()).as_slice()
        );
        assert_ne!(
            candidate.encoded_sha256().as_slice(),
            Sha256::digest(&pixels).as_slice()
        );
        let debug = format!("{candidate:?}");
        assert!(!debug.contains(&good.encoder_fingerprint));
        assert!(!debug.contains(&good.request.operation_id().to_string()));
        assert!(!debug.contains("encoded:"));
        let decoded = sm_codec::decode_rgba(candidate.encoded_bytes()).unwrap();
        assert_eq!(decoded.pixels(), pixels);
        assert_eq!(
            second
                .lookup_operation(
                    OperationId::new(good.request.operation_id()),
                    good.principal_id,
                    good.request.target()
                )
                .await
                .unwrap(),
            LookupOutcome::Unknown
        );
        second.shutdown(Duration::from_secs(5)).await.unwrap();
        let db = rusqlite::Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        for table in ["records", "assets", "ingest_operations"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert!(
            !f.0.join("assets").exists()
                || fs::read_dir(f.0.join("assets")).unwrap().next().is_none()
        );
        let pixel_path =
            f.0.join("staging")
                .join(good.request.operation_id().to_string())
                .join("pixels.rgba");
        fs::write(&pixel_path, [9u8; 24]).unwrap();
        let third = StorageWorker::start(f.lease()).unwrap();
        assert!(third.reopen_stage_owned_v2(reopen(&good)).await.is_err());
        third.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(fs::read(pixel_path).unwrap(), [9u8; 24]);
    });
}

#[test]
fn s02_t09n2_02_pixel_cap_refuses_valid_stage_before_encoding() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let worker = StorageWorker::start(f.lease()).unwrap();
        let library = LibraryId::new(worker.schema_snapshot().await.unwrap().library_id);
        let pixels = vec![7; 1025 * 1025 * 4];
        let mut intent = input(library, &pixels);
        intent.width = 1025;
        intent.height = 1025;
        intent.context.bounds.width = 1025;
        intent.context.bounds.height = 1025;
        intent.encoder_fingerprint = sm_codec::encoder_fingerprint();
        worker.create_capture_intent(intent.clone()).await.unwrap();
        worker
            .stage_capture_owned_v2(stage(&intent, pixels))
            .await
            .unwrap();
        let ValidatedIntentRead::Pending(witness) = worker
            .read_validated_capture_intent(library, intent.principal_id, intent.request.clone())
            .await
            .unwrap()
        else {
            panic!("pending")
        };
        let stage = worker
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: library,
                principal_id: intent.principal_id,
                operation_id: OperationId::new(intent.request.operation_id()),
                mode: intent.request.target(),
                encoder_fingerprint: intent.encoder_fingerprint.clone(),
            })
            .await
            .unwrap();
        assert!(matches!(
            worker.prepare_recovery_candidate(witness, stage).await,
            Err(PrepareCandidateError::PixelCapExceeded)
        ));
        assert_eq!(
            worker
                .lookup_operation(
                    OperationId::new(intent.request.operation_id()),
                    intent.principal_id,
                    intent.request.target()
                )
                .await
                .unwrap(),
            LookupOutcome::Unknown
        );
        worker.shutdown(Duration::from_secs(5)).await.unwrap();
        let db = rusqlite::Connection::open(f.0.join("data/library.sqlite3")).unwrap();
        for table in ["records", "assets", "ingest_operations"] {
            let count: i64 = db
                .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
                .unwrap();
            assert_eq!(count, 0, "{table}");
        }
        assert!(!f.0.join("assets").exists());
    });
}

#[test]
fn s02_t09m3_02_foreign_and_tampered_stage_rejected_without_cleanup() {
    let f = Fixture::new();
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
        let first = StorageWorker::start(f.lease()).unwrap();
        let input = input(
            LibraryId::new(first.schema_snapshot().await.unwrap().library_id),
            &[7; 24],
        );
        first.create_capture_intent(input.clone()).await.unwrap();
        first
            .stage_capture_owned_v2(stage(&input, vec![7; 24]))
            .await
            .unwrap();
        first.shutdown(Duration::from_secs(5)).await.unwrap();
        let foreign = StorageWorker::start(f.lease()).unwrap();
        assert!(foreign
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: input.library_id,
                principal_id: PrincipalId::new(Uuid::new_v4()),
                operation_id: OperationId::new(input.request.operation_id()),
                mode: input.request.target(),
                encoder_fingerprint: input.encoder_fingerprint.clone(),
            })
            .await
            .is_err());
        foreign.shutdown(Duration::from_secs(5)).await.unwrap();
        let pixels =
            f.0.join("staging")
                .join(input.request.operation_id().to_string())
                .join("pixels.rgba");
        fs::write(&pixels, [9u8; 24]).unwrap();
        let again = StorageWorker::start(f.lease()).unwrap();
        assert!(again
            .reopen_stage_owned_v2(OwnedReopenInputV2 {
                library_id: input.library_id,
                principal_id: input.principal_id,
                operation_id: OperationId::new(input.request.operation_id()),
                mode: input.request.target(),
                encoder_fingerprint: input.encoder_fingerprint.clone(),
            })
            .await
            .is_err());
        again.shutdown(Duration::from_secs(5)).await.unwrap();
        assert_eq!(fs::read(&pixels).unwrap(), [9u8; 24]);
    });
}
