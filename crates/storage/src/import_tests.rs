//! Real synthetic service tests: owned NTFS roots, real contained image worker.
use super::*;
use sha2::Digest;
use sm_application::{images::*, RequestContext, Scope};
use sm_domain::{CorrelationId, ImportBatchId, PrincipalId, RequestId};
use sm_isolation::{LibraryRootResolver, RootSource};
use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    sync::Arc,
};

// All contained workers in this test executable share the production one-active
// permit. Separate library fixtures must not race that permit accidentally; each
// test's own cancellation/file-lane concurrency remains unchanged.
static IMAGE_TEST: std::sync::Mutex<()> = std::sync::Mutex::new(());
struct Fixture {
    _worker_permit: std::sync::MutexGuard<'static, ()>,
    library: PathBuf,
    external: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let worker_permit = IMAGE_TEST.lock().unwrap_or_else(|p| p.into_inner());
        let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.pi/temporary");
        fs::create_dir_all(&base).unwrap();
        let base = fs::canonicalize(base).unwrap();
        let base = PathBuf::from(base.to_string_lossy().trim_start_matches(r"\\?\"));
        let library = base.join(Uuid::new_v4().to_string());
        let external = base.join(Uuid::new_v4().to_string());
        fs::create_dir(&library).unwrap();
        fs::create_dir(&external).unwrap();
        for child in ["data", "config", "cache", "webview"] {
            fs::create_dir(library.join(child)).unwrap();
        }
        Self {
            _worker_permit: worker_permit,
            library,
            external,
        }
    }
    fn storage(&self) -> StorageWorker {
        StorageWorker::start(
            HostLease::acquire(
                LibraryRootResolver::resolve(RootSource::Test {
                    root: &self.library,
                    forbidden_donor_roots: &[],
                })
                .unwrap(),
            )
            .unwrap(),
        )
        .unwrap()
    }
    fn run<T>(&self, f: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(f)
    }
    fn close(&self, s: &StorageWorker) {
        self.run(s.shutdown(Duration::from_secs(20))).unwrap();
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        if std::thread::panicking() {
            return;
        } // preserve accepted fixtures on failed checks
        let _ = fs::remove_dir_all(&self.library);
        let _ = fs::remove_dir_all(&self.external);
    }
}
fn worker() -> Arc<sm_image_worker::TrustedWorker> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../.pi/staging-target-t09b/debug/shot-image-worker.exe");
    let bytes = fs::read(&path).expect("build production image worker first");
    Arc::new(
        sm_image_worker::TrustedWorker::from_manifest(&path, sm_image::sha256(&bytes)).unwrap(),
    )
}
fn context(p: PrincipalId, scopes: Vec<Scope>) -> RequestContext {
    RequestContext::from_authenticated_peer(
        RequestId::new(Uuid::new_v4()),
        CorrelationId::new(Uuid::new_v4()),
        p,
        scopes,
    )
}
fn ctx(p: PrincipalId) -> RequestContext {
    context(
        p,
        vec![
            Scope::ImportFiles,
            Scope::ExportImages,
            Scope::ReadImages,
            Scope::ExportSourceSidecar,
            Scope::ReadSourceMetadata,
        ],
    )
}
fn request(path: &Path, duplicates: DuplicatePolicy, profile: CanonicalProfile) -> ImportRequest {
    ImportRequest {
        batch_id: ImportBatchId::new(Uuid::new_v4()),
        source_paths: vec![path.to_string_lossy().into_owned()],
        profile,
        duplicates,
    }
}
fn wait(
    f: &Fixture,
    service: &image_services::ImageServices,
    p: PrincipalId,
    b: ImportBatchId,
) -> ImportReport {
    let end = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let r = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if r.discovery_complete
            && r.in_flight == 0
            && (r.saved_new
                + r.linked_existing
                + r.skipped
                + r.failed
                + r.held
                + r.cancelled_unaccepted
                == r.discovered)
            && r.discovered > 0
        {
            // held is a durable phase, not completion while an owned job remains active.
            if r.saved_new + r.linked_existing + r.skipped + r.failed + r.cancelled_unaccepted
                == r.discovered
            {
                return r;
            }
        }
        assert!(std::time::Instant::now() < end, "batch did not finish");
        std::thread::sleep(Duration::from_millis(10));
    }
}
#[test]
fn s03_source_aware_pages_avif_preview_metadata_and_source_delete() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let folder = f.external.join("nested");
    fs::create_dir(&folder).unwrap();
    for n in 0..26 {
        fs::write(
            folder.join(format!("{n}.png")),
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
        )
        .unwrap();
    }
    let req = request(
        &folder,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let report = wait(&f, &service, p, b);
    assert_eq!(report.saved_new, 26);
    let page = f
        .run(s.library_page(sm_application::storage::RecordPageQuery::new(Some(24), None).unwrap()))
        .unwrap();
    assert_eq!(page.items.len(), 24);
    assert!(page.items.iter().all(|i| i.target == "imported_image"));
    let next = f
        .run(s.library_page(
            sm_application::storage::RecordPageQuery::new(Some(24), page.next_cursor).unwrap(),
        ))
        .unwrap();
    assert_eq!(next.items.len(), 2);
    assert!(next.next_cursor.is_none());
    let source = folder.join("0.png");
    let req = request(
        &source,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::AvifSdr90,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let report = wait(&f, &service, p, b);
    let receipt = report.items[0].receipt.as_ref().unwrap();
    let canonical = f.library.join(&receipt.relative_path);
    let original = fs::read(&canonical).unwrap();
    fs::remove_dir_all(folder).unwrap();
    let preview_request = sm_protocol::application::LibraryAssetRequest {
        asset_id: receipt.original_asset_id,
        offset: 0,
    };
    let preview = f
        .run(service.preview_asset(ctx(p), preview_request))
        .unwrap();
    assert_eq!(preview.mime, "image/webp");
    assert!(preview.eof);
    assert!(preview.data_hex.len() / 2 <= 8 * 1024 * 1024);
    assert_ne!(preview.sha256, receipt.canonical_sha256);
    let bytes = preview
        .data_hex
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|b| u8::from_str_radix(std::str::from_utf8(b).unwrap(), 16).unwrap())
        .collect::<Vec<_>>();
    assert_eq!(
        format!("{:x}", sha2::Sha256::digest(&bytes)),
        preview.sha256
    );
    assert!(bytes.starts_with(b"RIFF"));
    assert!(f
        .run(service.record_metadata(
            context(p, vec![Scope::ReadImages]),
            RecordMetadataRequest {
                record_id: receipt.record_id
            }
        ))
        .is_err());
    let metadata = f
        .run(service.record_metadata(
            ctx(p),
            RecordMetadataRequest {
                record_id: receipt.record_id,
            },
        ))
        .unwrap();
    assert_eq!(metadata.source_kind, "imported_image");
    assert_eq!(metadata.source_reports.len(), 1);
    assert_eq!(fs::read(canonical).unwrap(), original);
    f.close(&s);
    let reopened = f.storage();
    let page = f
        .run(
            reopened.library_page(
                sm_application::storage::RecordPageQuery::new(Some(24), None).unwrap(),
            ),
        )
        .unwrap();
    assert_eq!(page.items[0].id, receipt.record_id);
    f.close(&reopened);
}
#[test]
fn s03_import_duplicate_keepboth_reopen_export_and_scope() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let source = f.external.join("source.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let req = request(
        &source,
        DuplicatePolicy::ReuseRecord,
        CanonicalProfile::WebpExact,
    );
    let batch = req.batch_id;
    let denied = context(p, vec![Scope::ReadStatus, Scope::ListRecords]);
    assert!(f
        .run(service.import_paths(denied.clone(), req.clone()))
        .is_err());
    f.run(service.import_paths(ctx(p), req.clone())).unwrap();
    let first = wait(&f, &service, p, batch);
    assert_eq!(first.saved_new, 1);
    let receipt = first.items[0].receipt.clone().unwrap();
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let replay = wait(&f, &service, p, batch);
    assert_eq!(
        replay.items[0].receipt.as_ref().unwrap().origin_id,
        receipt.origin_id
    );
    let copied = f.external.join("other.png");
    fs::copy(&source, &copied).unwrap();
    let req = request(
        &copied,
        DuplicatePolicy::ReuseRecord,
        CanonicalProfile::AvifSdr90,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let duplicate = wait(&f, &service, p, b);
    assert_eq!(duplicate.linked_existing, 1);
    assert_eq!(
        duplicate.items[0].receipt.as_ref().unwrap().record_id,
        receipt.record_id
    );
    assert_eq!(
        duplicate.items[0].receipt.as_ref().unwrap().canonical_mime,
        "image/webp"
    );
    let req = request(
        &copied,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::AvifSdr90,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let both = wait(&f, &service, p, b);
    assert_eq!(both.saved_new, 1);
    assert_ne!(
        both.items[0].receipt.as_ref().unwrap().record_id,
        receipt.record_id
    );
    let canonical = f.library.join(&receipt.relative_path);
    let before = fs::read(&canonical).unwrap();
    let export = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: receipt.record_id,
        asset_id: None,
        format: ExportFormat::Png,
        max_side: Some(2),
        jpeg_background: None,
        destination_path: Some(f.external.join("export.png").to_string_lossy().into_owned()),
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    assert!(f.run(service.export_image(denied, export.clone())).is_err());
    let output = f.run(service.export_image(ctx(p), export.clone())).unwrap();
    assert_eq!(output.mime, "image/png");
    assert!(output.width <= 2 && output.height <= 2);
    assert_eq!(fs::read(&canonical).unwrap(), before);
    assert!(fs::read(&output.export_path)
        .unwrap()
        .starts_with(b"\x89PNG\r\n\x1a\n"));
    let replay = f.run(service.export_image(ctx(p), export)).unwrap();
    assert_eq!(replay.sha256, output.sha256);
    let mut protected = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: receipt.record_id,
        asset_id: None,
        format: ExportFormat::Webp,
        max_side: None,
        jpeg_background: None,
        destination_path: Some(canonical.to_string_lossy().into_owned()),
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    assert!(f
        .run(service.export_image(ctx(p), protected.clone()))
        .is_err());
    protected.destination_path = Some(source.to_string_lossy().into_owned());
    protected.format = ExportFormat::Png;
    assert!(f.run(service.export_image(ctx(p), protected)).is_err());
    f.close(&s);
    drop(service);
    drop(s);
    fs::remove_file(source).unwrap();
    fs::remove_file(copied).unwrap();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let reopened = f
        .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: batch }))
        .unwrap();
    assert_eq!(reopened.saved_new, 1);
    assert_eq!(fs::read(canonical).unwrap(), before);
    assert!(f
        .run(service.import_status(
            ctx(PrincipalId::new(Uuid::new_v4())),
            ImportBatchRequest { batch_id: batch }
        ))
        .is_err());
    f.close(&s);
}
#[test]
fn s03_import_recursive_overlapping_roots_failures_and_closed_waiter() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let tree = f.external.join("tree");
    let nested = tree.join("nested");
    fs::create_dir_all(&nested).unwrap();
    fs::write(
        tree.join("one.png"),
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    fs::write(
        nested.join("two.png"),
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    fs::write(tree.join("bad.png"), b"not an image").unwrap();
    let junction = tree.join("library-alias");
    let link = std::process::Command::new("cmd.exe")
        .args(["/d", "/c", "mklink", "/J"])
        .arg(&junction)
        .arg(&f.library)
        .output()
        .unwrap();
    assert_eq!(link.status.code(), Some(0));
    let mut req = request(
        &tree,
        DuplicatePolicy::ReuseRecord,
        CanonicalProfile::WebpExact,
    );
    req.source_paths.push(nested.to_string_lossy().into_owned());
    req.source_paths
        .push(f.library.to_string_lossy().into_owned());
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let report = wait(&f, &service, p, b);
    assert_eq!(report.discovered, 3);
    assert_eq!(report.saved_new, 1);
    assert_eq!(report.linked_existing, 1);
    assert_eq!(report.failed, 1);
    assert_eq!(report.retained_input, 1);
    assert_eq!(report.directory_issues, 2);
    assert_eq!(
        report.discovered,
        report.saved_new
            + report.linked_existing
            + report.skipped
            + report.failed
            + report.held
            + report.cancelled_unaccepted
            + report.in_flight
    );
    assert!(report
        .items
        .iter()
        .any(|i| i.safe_code.as_deref() == Some("UNSUPPORTED_FORMAT")));
    let read = rusqlite::Connection::open_with_flags(
        f.library.join("data/library.sqlite3"),
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
    )
    .unwrap();
    assert_eq!(
        read.query_row("SELECT count(*) FROM import_item_roots", [], |r| r
            .get::<_, u32>(0))
            .unwrap(),
        4
    );
    drop(read);
    let saved = report
        .items
        .iter()
        .find_map(|i| i.receipt.as_ref())
        .unwrap();
    let export = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: saved.record_id,
        asset_id: None,
        format: ExportFormat::Png,
        max_side: None,
        jpeg_background: None,
        destination_path: None,
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    f.run(async {
        let mut waiter = Box::pin(service.export_image(ctx(p), export.clone()));
        std::future::poll_fn(|cx| {
            assert!(matches!(waiter.as_mut().poll(cx), std::task::Poll::Pending));
            std::task::Poll::Ready(())
        })
        .await;
        drop(waiter); // admitted file work must survive cancellation of the RPC waiter
    });
    let receipt = f.run(service.export_image(ctx(p), export)).unwrap();
    assert!(Path::new(&receipt.export_path).exists());
    assert!(!receipt.partial);
    let page = f
        .run(service.import_report_page(
            ctx(p),
            ImportReportPageRequest {
                batch_id: b,
                after_ordinal: 1,
            },
        ))
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert_eq!(page.items[0].ordinal, 1);
    let forbidden = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: saved.record_id,
        asset_id: None,
        format: ExportFormat::Png,
        max_side: None,
        jpeg_background: None,
        destination_path: Some(
            junction
                .join("forbidden.png")
                .to_string_lossy()
                .into_owned(),
        ),
        metadata_policy: ExportMetadataPolicy::StripPrivate,
    };
    assert!(f.run(service.export_image(ctx(p), forbidden)).is_err());
    assert!(!f.library.join("forbidden.png").exists());
    let empty = f.external.join("empty.png");
    fs::write(&empty, b"").unwrap();
    let empty_request = request(
        &empty,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let empty_batch = empty_request.batch_id;
    f.run(service.import_paths(ctx(p), empty_request)).unwrap();
    let empty_report = wait(&f, &service, p, empty_batch);
    assert_eq!(empty_report.skipped, 1);
    assert_eq!(empty_report.saved_new, 0);
    assert_eq!(empty_report.state, ImportBatchState::CompletedWithErrors);
    assert_eq!(
        empty_report.items[0].safe_code.as_deref(),
        Some("WORK_FAILED")
    );
    f.close(&s);
    fs::remove_dir(junction).unwrap();
}

#[test]
fn s03_import_cancel_during_discovery_resume_does_not_adopt_unaccepted_files() {
    let f = Fixture::new();
    let storage = f.storage();
    let service = image_services::ImageServices::new(storage.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    for n in 0..2000 {
        fs::write(
            f.external.join(format!("{n}.png")),
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
        )
        .unwrap();
    }
    let req = request(
        &f.external,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if r.discovered > 0 && !r.discovery_complete {
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    f.run(service.import_control(
        ctx(p),
        ImportControlRequest {
            batch_id: b,
            control: ImportControl::CancelProcessing,
        },
    ))
    .unwrap();
    f.run(service.import_control(
        ctx(p),
        ImportControlRequest {
            batch_id: b,
            control: ImportControl::Resume,
        },
    ))
    .unwrap();
    let r = loop {
        let r = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if !r.active_work {
            break r;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    };
    assert!(r.discovery_abandoned && r.discovery_complete);
    assert_eq!(r.saved_new, 0);
    assert_eq!(r.held, 0);
    assert_eq!(r.in_flight, 0);
    assert_eq!(r.cancelled_unaccepted, r.discovered);
    assert!(r.discovered < 2000);
    f.close(&storage);
}

#[test]
fn s03_import_yields_existing_file_lane_between_items() {
    let f = Fixture::new();
    let storage = f.storage();
    let service = image_services::ImageServices::new(storage.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let source = f.external.join("initial.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let initial = request(
        &source,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let initial_id = initial.batch_id;
    f.run(service.import_paths(ctx(p), initial)).unwrap();
    let initial = wait(&f, &service, p, initial_id);
    let receipt = initial.items[0].receipt.as_ref().unwrap();
    let root = f.external.join("bulk");
    fs::create_dir(&root).unwrap();
    for n in 0..60 {
        fs::write(
            root.join(format!("{n}.png")),
            include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
        )
        .unwrap();
    }
    let req = ImportRequest {
        batch_id: ImportBatchId::new(Uuid::new_v4()),
        source_paths: vec![root.to_string_lossy().into_owned()],
        profile: CanonicalProfile::WebpExact,
        duplicates: DuplicatePolicy::KeepBoth,
    };
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let r = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if r.discovered > 1
            && r.active_work
            && r.items
                .iter()
                .any(|item| item.state == ImportState::Processing)
        {
            assert_eq!(
                r.discovered,
                r.saved_new
                    + r.linked_existing
                    + r.skipped
                    + r.failed
                    + r.held
                    + r.cancelled_unaccepted
                    + r.in_flight,
                "public status must count every unfinished file"
            );
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    let chunk = f
        .run(storage.read_asset_chunk(receipt.original_asset_id, 0, 1024))
        .unwrap();
    assert!(!chunk.bytes.is_empty());
    let while_read = f
        .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
        .unwrap();
    assert!(
        while_read.saved_new < 60,
        "file-lane read waited for the whole batch"
    );
    assert_eq!(wait(&f, &service, p, b).saved_new, 60);
    f.close(&storage);
}

#[test]
fn s03_import_hardlink_occurrences_and_protected_alias_refusal() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let source = f.external.join("a.png");
    let alias = f.external.join("b.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    fs::hard_link(&source, &alias).unwrap();
    let mut req = request(
        &source,
        DuplicatePolicy::ReuseRecord,
        CanonicalProfile::WebpExact,
    );
    req.source_paths.push(alias.to_string_lossy().into_owned());
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let report = wait(&f, &service, p, b);
    assert_eq!(report.saved_new, 1);
    assert_eq!(report.linked_existing, 1);
    let canonical = f
        .library
        .join(&report.items[0].receipt.as_ref().unwrap().relative_path);
    let before = fs::read(&canonical).unwrap();
    let unsafe_alias = f.external.join("canonical-alias.webp");
    fs::hard_link(&canonical, &unsafe_alias).unwrap();
    let req = request(
        &unsafe_alias,
        DuplicatePolicy::ReuseRecord,
        CanonicalProfile::WebpExact,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(20);
    loop {
        let report = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if report.discovery_complete && !report.active_work {
            assert_eq!(report.saved_new, 0);
            assert_eq!(report.linked_existing, 0);
            assert_eq!(report.skipped, 1);
            assert!(!report.items[0].durable_input);
            assert_eq!(report.items[0].safe_code.as_deref(), Some("SOURCE_REFUSED"));
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(fs::read(canonical).unwrap(), before);
    fs::remove_file(unsafe_alias).unwrap();
    f.close(&s);
}

#[test]
fn s03_import_managed_export_sidecar_ttl_read_lease_cleanup_and_no_replace() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let source = f.external.join("source.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let req = request(
        &source,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let b = req.batch_id;
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let imported = wait(&f, &service, p, b);
    let saved = imported.items[0].receipt.as_ref().unwrap();
    let export = ExportRequest {
        operation_id: OperationId::new(Uuid::new_v4()),
        record_id: saved.record_id,
        asset_id: None,
        format: ExportFormat::Png,
        max_side: None,
        jpeg_background: None,
        destination_path: None,
        metadata_policy: ExportMetadataPolicy::IncludeSourceSidecar,
    };
    assert!(f
        .run(service.export_image(context(p, vec![Scope::ExportImages]), export.clone()))
        .is_err());
    let receipt = f.run(service.export_image(ctx(p), export.clone())).unwrap();
    assert!(!receipt.partial);
    assert!(receipt.expires_at_epoch_seconds.is_some());
    let mut jpeg = export.clone();
    jpeg.operation_id = OperationId::new(Uuid::new_v4());
    jpeg.format = ExportFormat::Jpeg;
    jpeg.metadata_policy = ExportMetadataPolicy::StripPrivate;
    assert!(
        f.run(service.export_image(ctx(p), jpeg.clone())).is_err(),
        "transparent JPEG requires an explicit background"
    );
    jpeg.operation_id = OperationId::new(Uuid::new_v4());
    jpeg.jpeg_background = Some([255, 255, 255]);
    let jpeg = f.run(service.export_image(ctx(p), jpeg)).unwrap();
    assert_eq!(jpeg.mime, "image/jpeg");
    assert!(fs::read(jpeg.export_path)
        .unwrap()
        .starts_with(b"\xff\xd8\xff"));
    let sidecar = receipt.sidecar.as_ref().unwrap();
    assert!(fs::read(&sidecar.export_path)
        .unwrap()
        .windows(b"source_path".len())
        .any(|b| b == b"source_path"));
    let mut read = f
        .run(service.begin_export_read(ctx(p), export.operation_id))
        .unwrap();
    assert_eq!(
        sm_image::sha256(&read.read_all().unwrap()),
        sm_image::sha256(&fs::read(&receipt.export_path).unwrap())
    );
    let sentinel = f.library.join("cache/export/sentinel.bin");
    fs::write(&sentinel, b"do-not-delete").unwrap();
    f.close(&s);
    drop(service);
    drop(s);
    // Own fixture mutation of the clock-bound receipt while no SQLite owner is running.
    let db = rusqlite::Connection::open(f.library.join("data/library.sqlite3")).unwrap();
    db.execute("UPDATE export_operations SET receipt_json=json_set(receipt_json,'$.expires_at_epoch_seconds','1') WHERE id=?1",[export.operation_id.as_uuid().to_string()]).unwrap();
    drop(db);
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    assert!(f
        .run(service.begin_export_read(ctx(p), export.operation_id))
        .is_err());
    assert_eq!(
        f.run(service.cleanup_expired_exports(ctx(p))).unwrap(),
        0,
        "active read pins the entire registered group"
    );
    assert!(Path::new(&sidecar.export_path).exists());
    drop(read);
    assert_eq!(f.run(service.cleanup_expired_exports(ctx(p))).unwrap(), 2);
    assert_eq!(fs::read(sentinel).unwrap(), b"do-not-delete");
    assert!(f.library.join(&saved.relative_path).exists());
    let target = f.external.join("exists.png");
    fs::write(&target, b"external-sentinel").unwrap();
    let mut conflict = export;
    conflict.operation_id = OperationId::new(Uuid::new_v4());
    conflict.destination_path = Some(target.to_string_lossy().into_owned());
    conflict.metadata_policy = ExportMetadataPolicy::StripPrivate;
    assert!(f.run(service.export_image(ctx(p), conflict)).is_err());
    assert_eq!(fs::read(target).unwrap(), b"external-sentinel");
    f.close(&s);
}

#[test]
fn s03_import_accepted_snapshot_survives_cancel_source_move_delete_and_retry() {
    let f = Fixture::new();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    let p = PrincipalId::new(Uuid::new_v4());
    let source = f.external.join("retry.png");
    fs::write(
        &source,
        include_bytes!("../../../tests/fixtures/images/odd-alpha.png"),
    )
    .unwrap();
    let req = request(
        &source,
        DuplicatePolicy::KeepBoth,
        CanonicalProfile::WebpExact,
    );
    let b = req.batch_id;
    // Public cancellation after durable acceptance; the external source is deleted before retry.
    f.run(service.import_paths(ctx(p), req)).unwrap();
    let end = std::time::Instant::now() + Duration::from_secs(60);
    loop {
        let r = f
            .run(service.import_status(ctx(p), ImportBatchRequest { batch_id: b }))
            .unwrap();
        if r.items.first().is_some_and(|i| i.durable_input) {
            break;
        }
        assert!(std::time::Instant::now() < end);
        std::thread::sleep(Duration::from_millis(1));
    }
    f.run(service.import_control(
        ctx(p),
        ImportControlRequest {
            batch_id: b,
            control: ImportControl::CancelProcessing,
        },
    ))
    .unwrap();
    f.close(&s);
    drop(service);
    drop(s);
    fs::rename(&source, f.external.join("moved.png")).unwrap();
    fs::remove_file(f.external.join("moved.png")).unwrap();
    let s = f.storage();
    let service = image_services::ImageServices::new(s.clone(), worker());
    f.run(service.import_control(
        ctx(p),
        ImportControlRequest {
            batch_id: b,
            control: ImportControl::Retry,
        },
    ))
    .unwrap();
    let r = wait(&f, &service, p, b);
    assert_eq!(r.saved_new, 1);
    assert_eq!(r.retained_input, 0);
    f.close(&s);
}
