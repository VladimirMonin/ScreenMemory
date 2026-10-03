//! Runnable S03 coordinator on the original storage file lane. No second writer,
//! database, canonical store, native decode in the host, or RPC-waiter cancellation.
use crate::{
    import_db::{AssetSnapshot, Command, Item, Reply},
    FileRequest, Request, StorageWorker,
};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sm_application::{images::*, storage::StorageError, RequestContext, Scope};
use sm_domain::{AssetId, ImportBatchId, OperationId, OriginId, PrincipalId, RecordId};
use sm_image::{transform::ResizeLimits, Operation, Options, OutputProfile};
use sm_image_worker::{Cancellation, TrustedWorker};
use sm_isolation::{CanonicalPlan, CanonicalSeal, HostLease};
use sm_protocol::{
    errors::{AppError, ErrorCode},
    values::SafeMessage,
};
use std::{
    collections::HashMap,
    io::Write,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::SyncSender,
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::oneshot;
use uuid::Uuid;

pub(crate) struct JobControl {
    pub cancel: Cancellation,
    pub stopped: AtomicBool,
}
type PreviewCache = HashMap<Uuid, (std::time::Instant, Arc<Vec<u8>>, String)>;
#[derive(Clone)]
pub struct ImageServices {
    storage: StorageWorker,
    worker: Arc<TrustedWorker>,
    active: Arc<Mutex<HashMap<Uuid, Arc<JobControl>>>>,
    previews: Arc<Mutex<PreviewCache>>,
}
impl ImageServices {
    /// Package authority is supplied by the trusted native composition, never by RPC.
    pub fn new(storage: StorageWorker, worker: Arc<TrustedWorker>) -> Self {
        let active = storage.image_active.clone();
        Self {
            storage,
            worker,
            active,
            previews: Arc::new(Mutex::new(HashMap::new())),
        }
    }
    pub async fn begin_export_read(
        &self,
        ctx: RequestContext,
        id: OperationId,
    ) -> Result<ExportReadLease, AppError> {
        Self::authorize(&ctx, Scope::ReadImages)?;
        let (tx, rx) = oneshot::channel();
        {
            let gate = self
                .storage
                .gate
                .lock()
                .map_err(|_| app_error(StorageError::Unavailable))?;
            if gate.closing {
                return Err(app_error(StorageError::Unavailable));
            }
            self.storage
                .file_sender
                .try_send(FileRequest::ExportRead(
                    tx,
                    ctx.principal_id(),
                    id,
                    self.storage.sender.clone(),
                ))
                .map_err(|_| app_error(StorageError::Unavailable))?;
        }
        rx.await
            .map_err(|_| app_error(StorageError::Unavailable))?
            .map_err(app_error)
    }
    pub async fn cleanup_expired_exports(&self, ctx: RequestContext) -> Result<u32, AppError> {
        Self::authorize(&ctx, Scope::ExportImages)?;
        let (tx, rx) = oneshot::channel();
        {
            let gate = self
                .storage
                .gate
                .lock()
                .map_err(|_| app_error(StorageError::Unavailable))?;
            if gate.closing {
                return Err(app_error(StorageError::Unavailable));
            }
            self.storage
                .file_sender
                .try_send(FileRequest::CleanupExports(
                    tx,
                    ctx.principal_id(),
                    self.storage.sender.clone(),
                ))
                .map_err(|_| app_error(StorageError::Unavailable))?;
        }
        rx.await
            .map_err(|_| app_error(StorageError::Unavailable))?
            .map_err(app_error)
    }
    async fn db(&self, command: Command) -> Result<Reply, StorageError> {
        self.storage.ask(|tx| Request::Images(tx, command)).await
    }
    fn queue(&self, p: PrincipalId, b: ImportBatchId) -> Result<(), StorageError> {
        let mut active = self.active.lock().map_err(|_| StorageError::Unavailable)?;
        if active.contains_key(&b.as_uuid()) {
            return Ok(());
        }
        if active.len() >= 2 {
            return Err(StorageError::Unavailable);
        }
        let cancel = Arc::new(JobControl {
            cancel: Cancellation::default(),
            stopped: AtomicBool::new(false),
        });
        let gate = self
            .storage
            .gate
            .lock()
            .map_err(|_| StorageError::Unavailable)?;
        if gate.closing {
            return Err(StorageError::Unavailable);
        }
        self.storage
            .file_sender
            .try_send(FileRequest::ImportRun(
                self.worker.clone(),
                p,
                b,
                cancel.clone(),
                self.active.clone(),
                self.storage.sender.clone(),
                0,
            ))
            .map_err(|_| StorageError::Unavailable)?;
        active.insert(b.as_uuid(), cancel);
        Ok(())
    }
    fn report(&self, mut report: ImportReport) -> ImportReport {
        report.active_work = self
            .active
            .lock()
            .map(|m| m.contains_key(&report.batch_id.as_uuid()))
            .unwrap_or(true);
        if report.active_work {
            report.state = match report.control.as_str() {
                "paused" => ImportBatchState::Pausing,
                "cancelled" => ImportBatchState::Cancelling,
                _ => ImportBatchState::Running,
            };
        }
        report
    }
    fn authorize(ctx: &RequestContext, scope: Scope) -> Result<(), AppError> {
        if !ctx.allows(scope) || ctx.principal_id().as_uuid().is_nil() {
            return Err(app_error(StorageError::PermissionDenied));
        }
        Ok(())
    }
}
fn app_error(e: StorageError) -> AppError {
    AppError {
        code: match e {
            StorageError::PermissionDenied => ErrorCode::PermissionDenied,
            StorageError::Unavailable | StorageError::LockConflict => ErrorCode::RuntimeUnavailable,
            StorageError::IntegrityFailed | StorageError::AssetIntegrityFailed => {
                ErrorCode::AssetIntegrityFailed
            }
            StorageError::AssetNotFound => ErrorCode::AssetNotFound,
            StorageError::SchemaIncompatible => ErrorCode::SchemaIncompatible,
            _ => ErrorCode::InvalidRequest,
        },
        safe_message: SafeMessage::parse("Image operation rejected.").expect("constant"),
        retryable: e == StorageError::Unavailable,
    }
}
#[async_trait]
impl ImageLibraryApi for ImageServices {
    async fn record_metadata(
        &self,
        ctx: RequestContext,
        request: RecordMetadataRequest,
    ) -> Result<RecordMetadata, AppError> {
        Self::authorize(&ctx, Scope::ReadSourceMetadata)?;
        match self
            .db(Command::Metadata(request.record_id.as_uuid()))
            .await
            .map_err(app_error)?
        {
            Reply::Metadata(value) => Ok(value),
            _ => Err(app_error(StorageError::IntegrityFailed)),
        }
    }
    async fn preview_asset(
        &self,
        ctx: RequestContext,
        request: sm_protocol::application::LibraryAssetRequest,
    ) -> Result<sm_protocol::application::LibraryAssetChunk, AppError> {
        Self::authorize(&ctx, Scope::ReadImages)?;
        let id = request.asset_id.as_uuid();
        let cached = {
            let mut cache = self
                .previews
                .lock()
                .map_err(|_| app_error(StorageError::Unavailable))?;
            cache.retain(|_, (time, _, _)| time.elapsed() < Duration::from_secs(120));
            cache
                .get(&id)
                .map(|(_, bytes, hash)| (bytes.clone(), hash.clone()))
        };
        let (bytes, hash) = match cached {
            Some(value) => value,
            None => {
                if request.offset != 0 {
                    return Err(app_error(StorageError::InvalidQuery));
                }
                let Reply::Asset(Some(asset)) =
                    self.db(Command::Asset(id)).await.map_err(app_error)?
                else {
                    return Err(app_error(StorageError::AssetNotFound));
                };
                let (tx, rx) = oneshot::channel();
                {
                    let gate = self
                        .storage
                        .gate
                        .lock()
                        .map_err(|_| app_error(StorageError::Unavailable))?;
                    if gate.closing {
                        return Err(app_error(StorageError::Unavailable));
                    }
                    self.storage
                        .file_sender
                        .try_send(FileRequest::Preview(tx, self.worker.clone(), asset))
                        .map_err(|_| app_error(StorageError::Unavailable))?;
                }
                let bytes = Arc::new(
                    rx.await
                        .map_err(|_| app_error(StorageError::Unavailable))?
                        .map_err(app_error)?,
                );
                let hash = format!("{:x}", Sha256::digest(bytes.as_slice()));
                let mut cache = self
                    .previews
                    .lock()
                    .map_err(|_| app_error(StorageError::Unavailable))?;
                if cache.len() >= 4 {
                    let oldest = cache.iter().min_by_key(|(_, v)| v.0).map(|(id, _)| *id);
                    if let Some(id) = oldest {
                        cache.remove(&id);
                    }
                }
                cache.insert(id, (std::time::Instant::now(), bytes.clone(), hash.clone()));
                (bytes, hash)
            }
        };
        let start = request.offset as usize;
        if start >= bytes.len() {
            return Err(app_error(StorageError::InvalidQuery));
        }
        let end = (start + 32 * 1024).min(bytes.len());
        let data_hex = bytes[start..end]
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        Ok(sm_protocol::application::LibraryAssetChunk {
            asset_id: request.asset_id,
            offset: request.offset,
            next_offset: end as u32,
            eof: end == bytes.len(),
            total_size: bytes.len().to_string(),
            sha256: hash,
            mime: "image/webp".into(),
            data_hex,
        })
    }
    async fn import_paths(
        &self,
        ctx: RequestContext,
        request: ImportRequest,
    ) -> Result<ImportReport, AppError> {
        Self::authorize(&ctx, Scope::ImportFiles)?;
        if request.batch_id.as_uuid().is_nil()
            || request.source_paths.is_empty()
            || request.source_paths.len() > 128
            || request
                .source_paths
                .iter()
                .any(|p| p.len() > 32760 || p.is_empty())
        {
            return Err(app_error(StorageError::InvalidPayload));
        }
        let batch = request.batch_id;
        let Reply::Report(report) = self
            .db(Command::Create(ctx.principal_id(), request))
            .await
            .map_err(app_error)?
        else {
            return Err(app_error(StorageError::IntegrityFailed));
        };
        if report.control == "running" && (!report.discovery_complete || report.in_flight > 0) {
            self.queue(ctx.principal_id(), batch).map_err(app_error)?;
        }
        if report.saved_new + report.linked_existing > 0
            && !report.active_work
            && report.discovery_complete
            && report.in_flight == 0
        {
            let (tx, rx) = oneshot::channel();
            {
                let gate = self
                    .storage
                    .gate
                    .lock()
                    .map_err(|_| app_error(StorageError::Unavailable))?;
                if gate.closing {
                    return Err(app_error(StorageError::Unavailable));
                }
                self.storage
                    .file_sender
                    .try_send(FileRequest::VerifyImportReport(
                        tx,
                        report,
                        self.storage.sender.clone(),
                    ))
                    .map_err(|_| app_error(StorageError::Unavailable))?;
            }
            return rx
                .await
                .map_err(|_| app_error(StorageError::Unavailable))?
                .map(|r| self.report(r))
                .map_err(app_error);
        }
        Ok(self.report(report))
    }
    async fn import_status(
        &self,
        ctx: RequestContext,
        request: ImportBatchRequest,
    ) -> Result<ImportReport, AppError> {
        Self::authorize(&ctx, Scope::ImportFiles)?;
        match self
            .db(Command::Report(ctx.principal_id(), request.batch_id))
            .await
            .map_err(app_error)?
        {
            Reply::Report(r) => Ok(self.report(r)),
            _ => Err(app_error(StorageError::IntegrityFailed)),
        }
    }
    async fn import_report_page(
        &self,
        ctx: RequestContext,
        request: ImportReportPageRequest,
    ) -> Result<ImportReport, AppError> {
        Self::authorize(&ctx, Scope::ImportFiles)?;
        match self
            .db(Command::ReportPage(ctx.principal_id(), request))
            .await
            .map_err(app_error)?
        {
            Reply::Report(r) => Ok(self.report(r)),
            _ => Err(app_error(StorageError::IntegrityFailed)),
        }
    }
    async fn import_control(
        &self,
        ctx: RequestContext,
        request: ImportControlRequest,
    ) -> Result<ImportReport, AppError> {
        Self::authorize(&ctx, Scope::ImportFiles)?;
        let batch = request.batch_id;
        let resume = matches!(
            request.control,
            ImportControl::Resume | ImportControl::Retry
        );
        let Reply::Report(report) = self
            .db(Command::Control(
                ctx.principal_id(),
                if resume {
                    ImportControlRequest {
                        batch_id: batch,
                        control: ImportControl::Pause,
                    }
                } else {
                    request.clone()
                },
            ))
            .await
            .map_err(app_error)?
        else {
            return Err(app_error(StorageError::IntegrityFailed));
        };
        let previous = self
            .active
            .lock()
            .map_err(|_| app_error(StorageError::Unavailable))?
            .get(&batch.as_uuid())
            .cloned();
        if let Some(c) = previous {
            c.stopped.store(true, Ordering::Release);
            c.cancel.cancel();
        }
        if resume {
            let end = tokio::time::Instant::now() + Duration::from_secs(10);
            loop {
                if !self
                    .active
                    .lock()
                    .map_err(|_| app_error(StorageError::Unavailable))?
                    .contains_key(&batch.as_uuid())
                {
                    break;
                }
                if tokio::time::Instant::now() >= end {
                    return Err(app_error(StorageError::Unavailable));
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let Reply::Report(report) = self
                .db(Command::Control(ctx.principal_id(), request))
                .await
                .map_err(app_error)?
            else {
                return Err(app_error(StorageError::IntegrityFailed));
            };
            self.queue(ctx.principal_id(), batch).map_err(app_error)?;
            return Ok(self.report(report));
        }
        Ok(self.report(report))
    }
    async fn export_image(
        &self,
        ctx: RequestContext,
        request: ExportRequest,
    ) -> Result<ExportReceipt, AppError> {
        Self::authorize(&ctx, Scope::ExportImages)?;
        if request.metadata_policy == ExportMetadataPolicy::IncludeSourceSidecar {
            Self::authorize(&ctx, Scope::ExportSourceSidecar)?;
        }
        if request.operation_id.as_uuid().is_nil()
            || request.record_id.as_uuid().is_nil()
            || request
                .destination_path
                .as_ref()
                .is_some_and(|p| p.len() > 32760)
            || request.max_side.is_some_and(|n| n == 0 || n > 16383)
        {
            return Err(app_error(StorageError::InvalidPayload));
        }
        let (tx, rx) = oneshot::channel();
        {
            let gate = self
                .storage
                .gate
                .lock()
                .map_err(|_| app_error(StorageError::Unavailable))?;
            if gate.closing {
                return Err(app_error(StorageError::Unavailable));
            }
            self.storage
                .file_sender
                .try_send(FileRequest::ImageExport(
                    tx,
                    self.worker.clone(),
                    ctx.principal_id(),
                    request,
                    self.storage.sender.clone(),
                ))
                .map_err(|_| app_error(StorageError::Unavailable))?;
        }
        rx.await
            .map_err(|_| app_error(StorageError::Unavailable))?
            .map_err(app_error)
    }
}
pub(crate) fn sql(sender: &SyncSender<Request>, command: Command) -> Result<Reply, StorageError> {
    let (tx, rx) = oneshot::channel();
    sender
        .send(Request::Images(tx, command))
        .map_err(|_| StorageError::Unavailable)?;
    rx.blocking_recv().map_err(|_| StorageError::Unavailable)?
}
fn plan(
    sender: &SyncSender<Request>,
    p: PrincipalId,
    b: ImportBatchId,
) -> Result<(ImportRequest, String, String, Uuid), StorageError> {
    match sql(sender, Command::Plan(p, b))? {
        Reply::Plan(r, c, t, l) => Ok((r, c, t, l)),
        _ => Err(StorageError::IntegrityFailed),
    }
}
fn profile(p: CanonicalProfile) -> OutputProfile {
    match p {
        CanonicalProfile::WebpExact => OutputProfile::WebpExact,
        CanonicalProfile::AvifSdr90 => OutputProfile::AvifQuality90,
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    version: u32,
    library: Uuid,
    principal: Uuid,
    batch: ImportBatchId,
    item: Uuid,
    snapshot: Uuid,
    operation: Uuid,
    record: Uuid,
    asset: Uuid,
    source_path: String,
    resolved_source_path: String,
    source_observation: (u32, u64, u64, u64, u32),
    profile: CanonicalProfile,
    duplicates: DuplicatePolicy,
    profile_fingerprint: String,
    request_fingerprint: String,
    source_sha256: String,
    source_length: u64,
}
struct HashWriter<W: Write> {
    inner: W,
    hash: Sha256,
}
impl<W: Write> Write for HashWriter<W> {
    fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
        let n = self.inner.write(b)?;
        self.hash.update(&b[..n]);
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}
fn request_fingerprint(
    request: &ImportRequest,
    library: Uuid,
    p: PrincipalId,
) -> Result<String, StorageError> {
    let bytes=serde_json::to_vec(&serde_json::json!({"domain":"sm-import-request-v1","library":library,"principal":p,"request":request})).map_err(|_|StorageError::InvalidPayload)?;
    Ok(format!("{:x}", Sha256::digest(&bytes)))
}
fn snapshot(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    p: PrincipalId,
    request: &ImportRequest,
    library: Uuid,
    item: &mut Item,
) -> Result<Vec<u8>, StorageError> {
    let directory = lease
        .image_snapshot_directory(item.snapshot)
        .map_err(|_| StorageError::RootRejected)?;
    let expected = directory.open("manifest.json");
    let manifest = match expected {
        Ok(mut f) => {
            let bytes = f
                .read_bounded(65536)
                .map_err(|_| StorageError::IntegrityFailed)?;
            serde_json::from_slice::<Manifest>(&bytes).map_err(|_| StorageError::IntegrityFailed)?
        }
        Err(sm_isolation::StagingError::Missing) if !item.durable => {
            let mut source = lease
                .image_source(Path::new(&item.path))
                .map_err(|_| StorageError::RootRejected)?;
            let mut manifest = Manifest {
                version: 1,
                library,
                principal: p.as_uuid(),
                batch: request.batch_id,
                item: item.id,
                snapshot: item.snapshot,
                operation: item.operation,
                record: item.record,
                asset: item.asset,
                source_path: item.path.clone(),
                resolved_source_path: source
                    .resolved_path()
                    .map_err(|_| StorageError::RootRejected)?
                    .to_string_lossy()
                    .into_owned(),
                source_observation: source.observation(),
                profile: request.profile,
                duplicates: request.duplicates,
                profile_fingerprint: profile(request.profile).fingerprint(),
                request_fingerprint: request_fingerprint(request, library, p)?,
                source_sha256: "0".repeat(64),
                source_length: source.len(),
            };
            if serde_json::to_vec(&manifest)
                .map_err(|_| StorageError::InvalidPayload)?
                .len()
                > 65536
            {
                return Err(StorageError::InvalidPayload);
            }
            let leaf = format!("{}.tmp", Uuid::new_v4());
            let file = directory
                .create(&leaf)
                .map_err(|_| StorageError::RootRejected)?;
            let mut output = HashWriter {
                inner: file,
                hash: Sha256::new(),
            };
            source
                .copy_to(&mut output, sm_image::MAX_INPUT_BYTES as u64, || {
                    plan(sender, p, request.batch_id).map_or(true, |(_, c, _, _)| c != "running")
                })
                .map_err(|_| StorageError::Unavailable)?;
            let hash = format!("{:x}", output.hash.finalize());
            directory
                .publish(&leaf, "source.bin", output.inner)
                .map_err(|_| StorageError::RootRejected)?;
            manifest.source_sha256 = hash;
            let bytes = serde_json::to_vec(&manifest).map_err(|_| StorageError::InvalidPayload)?;
            let leaf = format!("{}.tmp", Uuid::new_v4());
            let mut file = directory
                .create(&leaf)
                .map_err(|_| StorageError::RootRejected)?;
            file.write_all(&bytes)
                .map_err(|_| StorageError::Unavailable)?;
            directory
                .publish(&leaf, "manifest.json", file)
                .map_err(|_| StorageError::RootRejected)?;
            manifest
        }
        _ => return Err(StorageError::IntegrityFailed),
    };
    if manifest.version != 1
        || manifest.library != library
        || manifest.principal != p.as_uuid()
        || manifest.batch != request.batch_id
        || manifest.item != item.id
        || manifest.snapshot != item.snapshot
        || manifest.operation != item.operation
        || manifest.record != item.record
        || manifest.asset != item.asset
        || manifest.profile != request.profile
        || manifest.duplicates != request.duplicates
        || manifest.profile_fingerprint != profile(request.profile).fingerprint()
        || manifest.request_fingerprint != request_fingerprint(request, library, p)?
        || manifest.source_path != item.path
        || item
            .hash
            .as_ref()
            .is_some_and(|h| h != &manifest.source_sha256)
        || item.length.is_some_and(|n| n != manifest.source_length)
    {
        return Err(StorageError::IntegrityFailed);
    }
    let mut source = directory
        .open("source.bin")
        .map_err(|_| StorageError::IntegrityFailed)?;
    let bytes = source
        .read_bounded(sm_image::MAX_INPUT_BYTES as u64)
        .map_err(|_| StorageError::IntegrityFailed)?;
    if bytes.len() as u64 != manifest.source_length
        || format!("{:x}", Sha256::digest(&bytes)) != manifest.source_sha256
    {
        return Err(StorageError::IntegrityFailed);
    }
    sql(
        sender,
        Command::Accepted(
            item.id,
            manifest.source_sha256.clone(),
            manifest.source_length,
            manifest.resolved_source_path.clone(),
        ),
    )?;
    item.durable = true;
    item.hash = Some(manifest.source_sha256);
    item.length = Some(manifest.source_length);
    Ok(bytes)
}
fn canonical_plan(a: &AssetSnapshot) -> Result<CanonicalPlan, StorageError> {
    let r = RecordId::new(a.record);
    if a.format == "avif_sdr90" {
        CanonicalPlan::avif(r, &a.created)
    } else if a.format == "webp_lossless" {
        CanonicalPlan::new(r, &a.created)
    } else {
        return Err(StorageError::IntegrityFailed);
    }
    .map_err(|_| StorageError::IntegrityFailed)
}
fn sealed(lease: &HostLease, a: &AssetSnapshot) -> Result<(CanonicalSeal, Vec<u8>), StorageError> {
    let plan = canonical_plan(a)?;
    if plan.relative_asset_path() != a.path {
        return Err(StorageError::IntegrityFailed);
    }
    let root = lease
        .canonical_root()
        .map_err(|_| StorageError::RootRejected)?;
    let mut file = root
        .open_final(&plan)
        .and_then(|f| f.seal())
        .map_err(|_| StorageError::IntegrityFailed)?;
    if file.len() != a.length || a.length > 128 * 1024 * 1024 {
        return Err(StorageError::IntegrityFailed);
    }
    let bytes = file
        .read_all_borrowed()
        .map_err(|_| StorageError::IntegrityFailed)?;
    if format!("{:x}", Sha256::digest(&bytes)) != a.hash {
        return Err(StorageError::IntegrityFailed);
    }
    Ok((file, bytes))
}
/// Contained derivative, retained once per transfer; never rewrites the canonical.
pub(crate) fn make_preview(
    lease: &HostLease,
    worker: &TrustedWorker,
    asset: &AssetSnapshot,
) -> Result<Vec<u8>, StorageError> {
    let (_seal, bytes) = sealed(lease, asset)?;
    if asset.format == "webp_lossless" && bytes.len() <= 8 * 1024 * 1024 {
        return Ok(bytes);
    }
    let result = worker
        .execute(
            Uuid::new_v4(),
            bytes,
            Options {
                method: Operation::DeriveExport,
                profile: OutputProfile::WebpExact,
                resize: ResizeLimits {
                    max_side: Some(2048),
                    ..Default::default()
                },
                jpeg_background: None,
            },
            &Cancellation::default(),
            Duration::from_secs(30),
        )
        .map_err(|_| StorageError::Unavailable)?;
    if result.bytes().is_empty() || result.bytes().len() > 8 * 1024 * 1024 {
        return Err(StorageError::InvalidPayload);
    }
    Ok(result.into_parts().0)
}
struct ImportAttempt<'a> {
    principal: PrincipalId,
    request: &'a ImportRequest,
    library: Uuid,
    time: &'a str,
}
fn process_item(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    worker: &TrustedWorker,
    cancel: &Cancellation,
    attempt: ImportAttempt<'_>,
    item: &mut Item,
) -> Result<(), StorageError> {
    let ImportAttempt {
        principal: p,
        request,
        library,
        time,
    } = attempt;
    let input = snapshot(lease, sender, p, request, library, item)?;
    if plan(sender, p, request.batch_id)?.1 != "running" {
        return Err(StorageError::Unavailable);
    }
    let result = worker
        .execute(
            item.operation,
            input,
            Options::canonical(profile(request.profile))
                .map_err(|_| StorageError::InvalidPayload)?,
            cancel,
            Duration::from_secs(180),
        )
        .map_err(|error| {
            use sm_image::ImageError as E;
            use sm_image_worker::WorkerError as W;
            let code = match error {
                W::Codec(E::RawNotSupported) => "RAW_NOT_SUPPORTED",
                W::Codec(E::MultiframeNotSupported) => "ANIMATION_NOT_SUPPORTED",
                W::Codec(E::ColorConversionRequired) => "COLOR_CONVERSION_REQUIRED",
                W::Codec(E::UnsupportedFormat) => "UNSUPPORTED_FORMAT",
                W::Codec(E::LimitExceeded) => "IMAGE_LIMIT_EXCEEDED",
                W::Codec(E::Malformed) => "MALFORMED_IMAGE",
                W::Cancelled => "CANCELLED",
                W::Timeout => "WORKER_TIMEOUT",
                W::Crashed => "WORKER_CRASHED",
                _ => "IMAGE_WORKER_REFUSED",
            };
            let _ = sql(sender, Command::State(item.id, "recoverable", code));
            StorageError::Unavailable
        })?;
    let (bytes, report) = result.into_parts();
    // Independent decode/codec verification occurs in another contained worker, not SQL/host native decode.
    let verified = worker
        .execute(
            Uuid::new_v4(),
            bytes.clone(),
            Options::canonical(OutputProfile::WebpExact)
                .map_err(|_| StorageError::InvalidPayload)?,
            cancel,
            Duration::from_secs(30),
        )
        .map_err(|_| StorageError::IntegrityFailed)?;
    if verified.report().pixels_sha256 != report.decoded_pixels_sha256
        || verified.report().normalized_dimensions != report.dimensions
    {
        return Err(StorageError::IntegrityFailed);
    }
    let duplicate = if request.duplicates == DuplicatePolicy::ReuseRecord {
        match sql(
            sender,
            Command::Duplicate(
                item.hash.clone().ok_or(StorageError::IntegrityFailed)?,
                item.length.ok_or(StorageError::IntegrityFailed)?,
            ),
        )? {
            Reply::Asset(a) => a,
            _ => return Err(StorageError::IntegrityFailed),
        }
    } else {
        None
    };
    let linked = duplicate.is_some();
    let target = duplicate.unwrap_or_else(|| AssetSnapshot {
        asset: item.asset,
        record: item.record,
        created: time.to_owned(),
        format: if request.profile == CanonicalProfile::AvifSdr90 {
            "avif_sdr90"
        } else {
            "webp_lossless"
        }
        .into(),
        path: format!(
            "assets/{}/{}/original.{}",
            &time[..7],
            item.record,
            if request.profile == CanonicalProfile::AvifSdr90 {
                "avif"
            } else {
                "webp"
            }
        ),
        hash: format!("{:x}", Sha256::digest(&bytes)),
        length: bytes.len() as u64,
    });
    if !linked {
        let root = lease
            .canonical_root()
            .map_err(|_| StorageError::RootRejected)?;
        let plan = canonical_plan(&target)?;
        match root.begin(&plan) {
            Ok(mut temp) => {
                temp.write_all(&bytes)
                    .map_err(|_| StorageError::Unavailable)?;
                temp.sync_all().map_err(|_| StorageError::Unavailable)?;
                temp.publish().map_err(|_| StorageError::RootRejected)?;
            }
            Err(sm_isolation::CanonicalError::Existing) => {}
            Err(_) => return Err(StorageError::RootRejected),
        }
    }
    let (seal, existing) = sealed(lease, &target)?;
    if !linked && existing != bytes {
        return Err(StorageError::IntegrityFailed);
    }
    let current = if linked {
        let Reply::Asset(Some(current)) = sql(sender, Command::CurrentAsset(target.record))? else {
            return Err(StorageError::IntegrityFailed);
        };
        current.asset
    } else {
        target.asset
    };
    let receipt = ImportReceipt {
        operation_id: OperationId::new(item.operation),
        record_id: RecordId::new(target.record),
        original_asset_id: AssetId::new(target.asset),
        current_asset_id: AssetId::new(current),
        origin_id: OriginId::new(Uuid::new_v4()),
        canonical_mime: if target.format == "avif_sdr90" {
            "image/avif"
        } else {
            "image/webp"
        }
        .into(),
        canonical_sha256: target.hash.clone(),
        relative_path: target.path.clone(),
        width: report.dimensions.width,
        height: report.dimensions.height,
        requested_profile_used: !linked,
    };
    sql(
        sender,
        Command::Commit {
            batch: request.batch_id,
            item: item.clone(),
            target: Box::new(target),
            receipt: Box::new(receipt),
            report: serde_json::to_string(&report).map_err(|_| StorageError::InvalidPayload)?,
            linked,
            _seal: seal,
        },
    )?;
    Ok(())
}
/// Streaming DFS: at most 128 open scans, no tree-wide filename/pixel allocation.
fn discovery(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    p: PrincipalId,
    request: &ImportRequest,
) -> Result<(), StorageError> {
    let mut issues = 0;
    let mut visited = 0u32;
    let Reply::Report(report) = sql(sender, Command::Report(p, request.batch_id))? else {
        return Err(StorageError::IntegrityFailed);
    };
    let limit = report.discovery_limit;
    let mut complete = true;
    for (root_index, root) in request.source_paths.iter().enumerate() {
        let path = PathBuf::from(root);
        if let Ok(directory) = lease.image_external_directory(&path) {
            let Ok(entries) = directory.entries() else {
                issues += 1;
                continue;
            };
            let mut stack = vec![(directory, entries)];
            while !stack.is_empty() {
                if plan(sender, p, request.batch_id)?.1 != "running" {
                    complete = false;
                    break;
                }
                let entry = stack.last_mut().expect("nonempty").1.next();
                let Some(entry) = entry else {
                    stack.pop();
                    continue;
                };
                visited += 1;
                if visited > limit {
                    sql(sender, Command::DiscoveryLimit(request.batch_id))?;
                    complete = false;
                    issues += 1;
                    break;
                }
                let Ok(entry) = entry else {
                    issues += 1;
                    continue;
                };
                let path = entry.path();
                let Ok(metadata) = std::fs::symlink_metadata(&path) else {
                    issues += 1;
                    continue;
                };
                use std::os::windows::fs::MetadataExt;
                if metadata.file_attributes() & 0x400 != 0 {
                    issues += 1;
                    continue;
                }
                if metadata.is_dir() {
                    if stack.len() >= 128 {
                        issues += 1;
                        continue;
                    }
                    match lease
                        .image_external_directory(&path)
                        .and_then(|d| d.entries().map(|e| (d, e)))
                    {
                        Ok(scan) => stack.push(scan),
                        Err(_) => issues += 1,
                    }
                } else if metadata.is_file() {
                    sql(
                        sender,
                        Command::Register(
                            request.batch_id,
                            path.to_string_lossy().into_owned(),
                            root_index as u32,
                        ),
                    )?;
                }
            }
        } else if lease.image_path_is_file(&path).unwrap_or(false) {
            sql(
                sender,
                Command::Register(request.batch_id, root.clone(), root_index as u32),
            )?;
        } else {
            issues += 1;
        }
        if !complete {
            break;
        }
    }
    sql(
        sender,
        Command::DiscoveryDone(request.batch_id, complete, issues),
    )?;
    Ok(())
}
pub(crate) fn run_import(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    worker: &TrustedWorker,
    p: PrincipalId,
    b: ImportBatchId,
    job: &JobControl,
    after: u32,
) -> Result<Option<u32>, StorageError> {
    let (request, control, time, library) = plan(sender, p, b)?;
    if control != "running" || job.stopped.load(Ordering::Acquire) {
        return Ok(None);
    }
    // Rescan is idempotent. Saved/staged items never reread their historical source.
    let Reply::Report(report) = sql(sender, Command::Report(p, b))? else {
        return Err(StorageError::IntegrityFailed);
    };
    if !report.discovery_complete && !report.discovery_abandoned {
        discovery(lease, sender, p, &request)?;
    }
    {
        if job.stopped.load(Ordering::Acquire) || plan(sender, p, b)?.1 != "running" {
            return Ok(None);
        }
        let Reply::Item(Some(mut item)) = sql(sender, Command::Next(b, after))? else {
            return Ok(None);
        };
        let next = item.ordinal + 1;
        if let Err(error) = process_item(
            lease,
            sender,
            worker,
            &job.cancel,
            ImportAttempt {
                principal: p,
                request: &request,
                library,
                time: &time,
            },
            &mut item,
        ) {
            let control = plan(sender, p, b)?.1;
            sql(
                sender,
                Command::State(
                    item.id,
                    if item.durable && control != "running" {
                        "held"
                    } else if item.durable {
                        "recoverable"
                    } else if control == "cancelled" {
                        "cancelled_unaccepted"
                    } else {
                        "skipped"
                    },
                    match error {
                        StorageError::RootRejected => "SOURCE_REFUSED",
                        StorageError::IntegrityFailed => "INTEGRITY_FAILED",
                        _ => "WORK_FAILED",
                    },
                ),
            )?;
        }
        Ok(Some(next))
    }
}
pub(crate) fn verify_report(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    report: ImportReport,
) -> Result<ImportReport, StorageError> {
    for item in &report.items {
        if let Some(receipt) = &item.receipt {
            let Reply::Asset(Some(asset)) =
                sql(sender, Command::Asset(receipt.original_asset_id.as_uuid()))?
            else {
                return Err(StorageError::IntegrityFailed);
            };
            if asset.record != receipt.record_id.as_uuid()
                || asset.path != receipt.relative_path
                || asset.hash != receipt.canonical_sha256
            {
                return Err(StorageError::IntegrityFailed);
            }
            let _ = sealed(lease, &asset)?;
        }
    }
    Ok(report)
}
pub(crate) fn run_export(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    worker: &TrustedWorker,
    p: PrincipalId,
    request: ExportRequest,
) -> Result<ExportReceipt, StorageError> {
    let (output, ext) = match request.format {
        ExportFormat::Png => (OutputProfile::Png, "png"),
        ExportFormat::Jpeg => (OutputProfile::JpegQuality90, "jpg"),
        ExportFormat::Webp => (OutputProfile::WebpQuality90, "webp"),
        ExportFormat::Avif => (OutputProfile::AvifQuality90, "avif"),
    };
    let directory = match &request.destination_path {
        Some(path) => lease
            .image_external_directory(Path::new(path).parent().ok_or(StorageError::RootRejected)?),
        None => lease.image_export_directory(),
    }
    .map_err(|_| StorageError::RootRejected)?;
    let leaf = match &request.destination_path {
        Some(path) => Path::new(path)
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or(StorageError::InvalidPayload)?
            .to_owned(),
        None => format!("{}.{}", request.operation_id.as_uuid(), ext),
    };
    if !Path::new(&leaf)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| {
            s.eq_ignore_ascii_case(ext) || (ext == "jpg" && s.eq_ignore_ascii_case("jpeg"))
        })
    {
        return Err(StorageError::InvalidPayload);
    }
    let Reply::Export(source, old, metadata) = sql(
        sender,
        Command::ResolveExport(
            p,
            request.clone(),
            directory.path().join(&leaf).to_string_lossy().into_owned(),
        ),
    )?
    else {
        return Err(StorageError::IntegrityFailed);
    };
    let planned = old.as_ref().map(|(r, _)| r.clone());
    let recovered = match old {
        Some((mut receipt, true)) if !receipt.partial => {
            receipt.existence = ExportExistence::NotChecked;
            return Ok(receipt);
        }
        Some((receipt, saved)) => {
            if receipt.encoding_profile_fingerprint != output.fingerprint() {
                return Err(StorageError::IntegrityFailed);
            }
            match directory.open(&leaf) {
                Ok(mut file) => {
                    let bytes = file
                        .read_bounded(128 * 1024 * 1024)
                        .map_err(|_| StorageError::IntegrityFailed)?;
                    if directory.path().join(&leaf) != Path::new(&receipt.export_path)
                        || bytes.len().to_string() != receipt.byte_size
                        || format!("{:x}", Sha256::digest(&bytes)) != receipt.sha256
                    {
                        return Err(StorageError::IntegrityFailed);
                    }
                    if !saved {
                        sql(
                            sender,
                            Command::SaveExport(request.operation_id, receipt.clone()),
                        )?;
                    }
                    Some(receipt)
                }
                Err(sm_isolation::StagingError::Missing) if !saved => None,
                _ => return Err(StorageError::IntegrityFailed),
            }
        }
        None => None,
    };
    let mut receipt = if let Some(receipt) = recovered {
        receipt
    } else {
        match directory.open(&leaf) {
            Err(sm_isolation::StagingError::Missing) => {}
            _ => return Err(StorageError::RootRejected),
        }
        let mut source_file = lease
            .image_asset_read(&source.path)
            .map_err(|_| StorageError::IntegrityFailed)?;
        let bytes = source_file
            .read_bounded(128 * 1024 * 1024)
            .map_err(|_| StorageError::IntegrityFailed)?;
        if bytes.len() as u64 != source.length
            || format!("{:x}", Sha256::digest(&bytes)) != source.hash
        {
            return Err(StorageError::IntegrityFailed);
        }
        let result = worker
            .execute(
                request.operation_id.as_uuid(),
                bytes,
                Options {
                    method: Operation::DeriveExport,
                    profile: output,
                    resize: ResizeLimits {
                        max_side: request.max_side,
                        ..ResizeLimits::default()
                    },
                    jpeg_background: request.jpeg_background,
                },
                &Cancellation::default(),
                Duration::from_secs(180),
            )
            .map_err(|_| StorageError::InvalidPayload)?;
        let (bytes, report) = result.into_parts();
        let temporary = format!("{}.tmp", Uuid::new_v4());
        let mut file = directory
            .create(&temporary)
            .map_err(|_| StorageError::RootRejected)?;
        file.write_all(&bytes)
            .map_err(|_| StorageError::Unavailable)?;
        let path = directory.path().join(&leaf);
        let mut receipt = ExportReceipt {
            operation_id: request.operation_id,
            source_asset_id: AssetId::new(source.asset),
            export_path: path.to_string_lossy().into_owned(),
            mime: report.output_format.mime().into(),
            width: report.dimensions.width,
            height: report.dimensions.height,
            byte_size: bytes.len().to_string(),
            sha256: format!("{:x}", Sha256::digest(&bytes)),
            expires_at_epoch_seconds: request
                .destination_path
                .is_none()
                .then(|| (epoch() + 86400).to_string()),
            sidecar: None,
            partial: request.metadata_policy == ExportMetadataPolicy::IncludeSourceSidecar,
            existence: ExportExistence::Verified,
            encoding_profile_fingerprint: report.profile_fingerprint.clone(),
            applied_parameters: ExportAppliedParameters {
                format: request.format,
                max_side: request.max_side,
                jpeg_background: request.jpeg_background,
                quality: if request.format == ExportFormat::Png {
                    None
                } else {
                    Some(90)
                },
                resize_applied: report.resized,
                private_metadata_stripped: true,
            },
        };
        if let Some(planned) = planned {
            if planned.sha256 != receipt.sha256
                || planned.byte_size != receipt.byte_size
                || planned.source_asset_id != receipt.source_asset_id
                || planned.export_path != receipt.export_path
                || planned.encoding_profile_fingerprint != receipt.encoding_profile_fingerprint
            {
                return Err(StorageError::IntegrityFailed);
            }
            receipt.expires_at_epoch_seconds = planned.expires_at_epoch_seconds;
        }
        // Durable publication intent precedes the no-replace rename. A crash after
        // rename is recoverable by checksum; a foreign/changed file is never adopted.
        sql(
            sender,
            Command::ExportIntent(request.operation_id, receipt.clone()),
        )?;
        directory
            .publish(&temporary, &leaf, file)
            .map_err(|_| StorageError::RootRejected)?;
        let mut published = directory
            .open(&leaf)
            .map_err(|_| StorageError::IntegrityFailed)?;
        if published
            .read_bounded(128 * 1024 * 1024)
            .map_err(|_| StorageError::IntegrityFailed)?
            != bytes
        {
            return Err(StorageError::IntegrityFailed);
        }
        // Persist the image member before attempting the non-atomic sidecar publication.
        sql(
            sender,
            Command::SaveExport(request.operation_id, receipt.clone()),
        )?;
        receipt
    };
    if request.metadata_policy == ExportMetadataPolicy::IncludeSourceSidecar {
        let sidecar=serde_json::to_vec(&serde_json::json!({"schema_version":1,"record_id":request.record_id,"source_asset_id":receipt.source_asset_id,"image_sha256":receipt.sha256,"image":{"mime":receipt.mime,"width":receipt.width,"height":receipt.height},"selected_source":serde_json::from_str::<serde_json::Value>(&metadata).map_err(|_|StorageError::IntegrityFailed)?})).map_err(|_|StorageError::InvalidPayload)?;
        if sidecar.len() > 512 * 1024 {
            return Err(StorageError::InvalidPayload);
        }
        let side_leaf = format!("{leaf}.metadata.json");
        let result = (|| {
            sql(
                sender,
                Command::CheckDestination(
                    directory
                        .path()
                        .join(&side_leaf)
                        .to_string_lossy()
                        .into_owned(),
                ),
            )?;
            match directory.open(&side_leaf) {
                Ok(mut file) => {
                    if file
                        .read_bounded(512 * 1024)
                        .map_err(|_| StorageError::IntegrityFailed)?
                        != sidecar
                    {
                        return Err(StorageError::IntegrityFailed);
                    }
                }
                Err(sm_isolation::StagingError::Missing) => {
                    let temp = format!("{}.tmp", Uuid::new_v4());
                    let mut file = directory
                        .create(&temp)
                        .map_err(|_| StorageError::RootRejected)?;
                    file.write_all(&sidecar)
                        .map_err(|_| StorageError::Unavailable)?;
                    directory
                        .publish(&temp, &side_leaf, file)
                        .map_err(|_| StorageError::RootRejected)?;
                }
                Err(_) => return Err(StorageError::RootRejected),
            }
            Ok(ExportSidecar {
                export_path: directory
                    .path()
                    .join(&side_leaf)
                    .to_string_lossy()
                    .into_owned(),
                sha256: format!("{:x}", Sha256::digest(&sidecar)),
                byte_size: sidecar.len().to_string(),
            })
        })();
        if let Ok(sidecar) = result {
            receipt.sidecar = Some(sidecar);
            receipt.partial = false;
        }
        sql(
            sender,
            Command::SaveExport(request.operation_id, receipt.clone()),
        )?;
    }
    Ok(receipt)
}
fn epoch() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}
/// A native read-denying-delete lease. Holding this value prevents managed cleanup;
/// it is not a path string whose future external readers are unknowable.
pub struct ExportReadLease {
    file: sm_isolation::StableImageFile,
    hash: String,
    length: u64,
}
impl ExportReadLease {
    pub fn read_all(&mut self) -> Result<Vec<u8>, StorageError> {
        let bytes = self
            .file
            .read_bounded(128 * 1024 * 1024)
            .map_err(|_| StorageError::IntegrityFailed)?;
        if bytes.len() as u64 != self.length || format!("{:x}", Sha256::digest(&bytes)) != self.hash
        {
            return Err(StorageError::IntegrityFailed);
        }
        Ok(bytes)
    }
}
pub(crate) fn begin_export_read(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    p: PrincipalId,
    id: OperationId,
) -> Result<ExportReadLease, StorageError> {
    let Reply::ExportReceipt(receipt) = sql(sender, Command::ExportRead(p, id))? else {
        return Err(StorageError::IntegrityFailed);
    };
    let expiry = receipt
        .expires_at_epoch_seconds
        .as_ref()
        .and_then(|e| e.parse::<u64>().ok())
        .ok_or(StorageError::InvalidQuery)?;
    if receipt.partial {
        return Err(StorageError::Unavailable);
    }
    if expiry <= epoch() {
        // A durable unresolved processing hold outlives cache TTL, including host reopen.
        // This is a scoped operation/asset binding, not adoption of an arbitrary expired file.
        if !matches!(
            sql(sender, Command::ExportRetained(p, id))?,
            Reply::Retained(true)
        ) {
            return Err(StorageError::Unavailable);
        }
    }
    let directory = lease
        .image_export_directory()
        .map_err(|_| StorageError::RootRejected)?;
    let leaf = Path::new(&receipt.export_path)
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or(StorageError::IntegrityFailed)?;
    if directory.path().join(leaf) != Path::new(&receipt.export_path)
        || !leaf.starts_with(&format!("{}.", id.as_uuid()))
    {
        return Err(StorageError::IntegrityFailed);
    }
    let file = directory
        .open(leaf)
        .map_err(|_| StorageError::IntegrityFailed)?;
    let length = receipt
        .byte_size
        .parse::<u64>()
        .map_err(|_| StorageError::IntegrityFailed)?;
    if file.len() != length || length > 128 * 1024 * 1024 {
        return Err(StorageError::IntegrityFailed);
    }
    Ok(ExportReadLease {
        file,
        hash: receipt.sha256,
        length,
    })
}
pub(crate) fn cleanup_exports(
    lease: &HostLease,
    sender: &SyncSender<Request>,
    p: PrincipalId,
) -> Result<u32, StorageError> {
    let Reply::Exports(receipts) = sql(sender, Command::ExpiredExports(p))? else {
        return Err(StorageError::IntegrityFailed);
    };
    let directory = lease
        .image_export_directory()
        .map_err(|_| StorageError::RootRejected)?;
    let mut removed = 0;
    for receipt in receipts {
        let mut members = vec![(receipt.export_path, receipt.sha256, receipt.byte_size)];
        if let Some(sidecar) = receipt.sidecar {
            members.push((sidecar.export_path, sidecar.sha256, sidecar.byte_size));
        }
        for (path, hash, len) in members {
            let leaf = Path::new(&path)
                .file_name()
                .and_then(|s| s.to_str())
                .ok_or(StorageError::IntegrityFailed)?;
            if directory.path().join(leaf) != Path::new(&path)
                || !leaf.starts_with(&format!("{}.", receipt.operation_id.as_uuid()))
            {
                return Err(StorageError::IntegrityFailed);
            }
            let len = len
                .parse::<u64>()
                .map_err(|_| StorageError::IntegrityFailed)?;
            let result = directory.remove_verified(leaf, |file, actual| {
                if actual != len || len > 128 * 1024 * 1024 {
                    return false;
                }
                use std::io::Read;
                let mut bytes = Vec::with_capacity(len as usize);
                file.take(len + 1).read_to_end(&mut bytes).is_ok()
                    && bytes.len() as u64 == len
                    && format!("{:x}", Sha256::digest(&bytes)) == hash
            });
            if result.is_ok() {
                removed += 1;
            } else if !matches!(result, Err(sm_isolation::StagingError::Missing)) {
                break;
            }
        }
    }
    Ok(removed)
}
