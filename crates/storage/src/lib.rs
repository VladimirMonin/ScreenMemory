//! SQLite worker holds one OS lease and a single connection; no SQL escape hatch.
#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("sm-storage requires the audited Windows x64 root adapter");
#[cfg(test)]
mod file_lane_tests;
#[cfg(test)]
mod image_tests;
mod images;
mod migration;
pub mod staging;
pub mod staging_files;
#[cfg(test)]
mod tests;

use async_trait::async_trait;
use rusqlite::{Connection, OpenFlags};
use sm_application::storage::{
    AssetChunk, CommitImageCapture, CommittedImage, FoundationRepository, LookupOutcome, Page,
    Record, RecordPageQuery, SchemaSnapshot, StorageError,
};
use sm_application::{
    capture::{MAX_CAPTURE_PIXELS, MAX_CAPTURE_SIDE},
    storage::{CaptureContext, CaptureTarget},
};
use sm_domain::{AssetId, LibraryId, OperationId, PrincipalId, RecordId};
use sm_isolation::{HostLease, LeaseError};
use std::{
    path::PathBuf,
    sync::{
        mpsc::{self, Receiver, SyncSender, TrySendError},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{oneshot, watch};
use uuid::Uuid;

type Response<T> = oneshot::Sender<Result<T, StorageError>>;

/// An owned, path-free staging command. The caller transfers the pixel allocation, never
/// clones it; cancellation of its waiter does not revoke an admitted file write.
pub struct OwnedStageInput {
    pub library_id: LibraryId,
    pub principal_id: PrincipalId,
    pub operation_id: OperationId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub frame_id: Uuid,
    pub mode: CaptureTarget,
    pub context: CaptureContext,
    pub captured_at: String,
    pub encoder_fingerprint: String,
    pub pixels: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StageOwnedError {
    Unavailable,
    Stage(staging_files::StageFailure),
}

impl OwnedStageInput {
    fn checked(&self) -> Result<(), StageOwnedError> {
        use staging::StagingError;
        let width = self.context.bounds.width;
        let height = self.context.bounds.height;
        if width == 0 || height == 0 || width > MAX_CAPTURE_SIDE || height > MAX_CAPTURE_SIDE {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidGeometry),
            ));
        }
        let bytes = u64::from(width)
            .checked_mul(u64::from(height))
            .and_then(|n| n.checked_mul(4))
            .filter(|n| *n <= MAX_CAPTURE_PIXELS * 4)
            .ok_or(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidGeometry),
            ))?;
        if self.pixels.len() as u64 != bytes {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::PixelLengthMismatch),
            ));
        }
        if self.context.sources.len() > 4 {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidContext),
            ));
        }
        if self
            .context
            .to_json()
            .map_or(true, |json| json.len() > staging::MAX_CONTEXT_BYTES)
        {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::InvalidContext),
            ));
        }
        if self.encoder_fingerprint.len() > 512 || self.captured_at.len() > 32 {
            return Err(StageOwnedError::Stage(
                staging_files::StageFailure::Rejected(StagingError::ManifestBeyondLimit),
            ));
        }
        Ok(())
    }
    fn borrowed(&self) -> staging_files::StageInput<'_> {
        staging_files::StageInput {
            library_id: self.library_id,
            principal_id: self.principal_id,
            operation_id: self.operation_id,
            record_id: self.record_id,
            asset_id: self.asset_id,
            frame_id: self.frame_id,
            mode: self.mode,
            context: &self.context,
            captured_at: &self.captured_at,
            encoder_fingerprint: &self.encoder_fingerprint,
            pixels: &self.pixels,
        }
    }
}

type StageResponse = oneshot::Sender<Result<staging_files::Staged, StageOwnedError>>;
enum FileRequest {
    Stage(StageResponse, Box<OwnedStageInput>),
    Stop,
}
struct Admission {
    closing: bool,
    stop_sent: bool,
    stopped: watch::Sender<bool>,
}

/// Test-only fault channel between the file write and its commit. The production
/// path always uses `Fault::None`.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Fault {
    None,
    MigrationCommit,
    ImageCommit,
}

enum Request {
    Snapshot(Response<SchemaSnapshot>),
    AiEnabled(Response<bool>),
    SqliteVersion(Response<String>),
    CommitImage(Response<CommittedImage>, Box<CommitImageCapture>),
    LookupOperation(Response<LookupOutcome>, String, String, String),
    PageRecords(Response<Page<Record>>, RecordPageQuery),
    ReadAssetChunk(Response<AssetChunk>, String, u64, u32),
    #[cfg(test)]
    Pragmas(Response<(String, i64, i64, i64)>),
    #[cfg(test)]
    Pause(Response<()>, Duration),
    Stop,
}

/// Cloneable bounded command endpoint, never a cloneable connection or lease.
#[derive(Clone)]
pub struct StorageWorker {
    sender: SyncSender<Request>,
    file_sender: SyncSender<FileRequest>,
    gate: Arc<Mutex<Admission>>,
}
impl StorageWorker {
    /// Consumes a real acquired OS lease. No database filename/path input.
    pub fn start(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(
            lease,
            Fault::None,
            #[cfg(test)]
            None,
        )
    }
    fn start_inner(
        lease: HostLease,
        fault: Fault,
        #[cfg(test)] pause: Option<FilePause>,
    ) -> Result<Self, StorageError> {
        let (sender, receiver) = mpsc::sync_channel(32);
        let (file_sender, file_receiver) = mpsc::sync_channel(1);
        let (stopped, _) = watch::channel(false);
        let gate = Arc::new(Mutex::new(Admission {
            closing: false,
            stop_sent: false,
            stopped,
        }));
        let status = Arc::clone(&gate);
        let file_stop_sender = file_sender.clone();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        std::thread::Builder::new()
            .name("sm-sqlite-worker".into())
            .spawn(move || {
                let entry = match lease.database_entry() {
                    Ok(entry) => entry,
                    Err(e) => {
                        drop(lease);
                        let _ = ready_tx.send(Err(map_entry(e)));
                        return;
                    }
                };
                // The canonical assets directory is derived from the leased entry only.
                let assets = match images::assets_root(entry.path()) {
                    Ok(assets) => assets,
                    Err(error) => {
                        drop(entry);
                        drop(lease);
                        let _ = ready_tx.send(Err(error));
                        return;
                    }
                };
                let result = open(&entry, fault == Fault::MigrationCommit);
                match result {
                    Ok(db) => {
                        let completed = std::thread::scope(|scope| {
                            let (started_tx, started_rx) = mpsc::sync_channel(1);
                            let lease_ref = &lease;
                            let file_thread = std::thread::Builder::new()
                                .name("sm-staging-files".into())
                                .spawn_scoped(scope, move || {
                                    let _ = started_tx.send(());
                                    file_loop(
                                        lease_ref,
                                        file_receiver,
                                        #[cfg(test)]
                                        pause,
                                    );
                                });
                            let Ok(file_thread) = file_thread else {
                                let _ = ready_tx.send(Err(StorageError::Unavailable));
                                drop(db);
                                return false;
                            };
                            if started_rx.recv().is_err() || ready_tx.send(Ok(())).is_err() {
                                drop(db);
                                let _ = file_stop_sender.send(FileRequest::Stop);
                                let _ = file_thread.join();
                                return false;
                            }
                            // A panicking DB loop still sends the file stop and joins before
                            // the scoped borrow and its original lease can be released.
                            let sqlite_result =
                                std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                                    run(db, receiver, assets, fault)
                                }));
                            let file_stop = file_stop_sender.send(FileRequest::Stop).is_ok();
                            let file_joined = file_thread.join().is_ok();
                            matches!(sqlite_result, Ok(true)) && file_stop && file_joined
                        });
                        drop(entry);
                        drop(lease);
                        if completed {
                            status
                                .lock()
                                .unwrap_or_else(|e| e.into_inner())
                                .stopped
                                .send_replace(true);
                        }
                    }
                    Err(error) => {
                        drop(entry);
                        drop(lease);
                        let _ = ready_tx.send(Err(error));
                    }
                }
            })
            .map_err(|_| StorageError::Unavailable)?;
        ready_rx.recv().map_err(|_| StorageError::Unavailable)??;
        Ok(Self {
            sender,
            file_sender,
            gate,
        })
    }
    /// Separate, single-slot file queue. Only accepted requests own file-lane memory;
    /// caller-owned pending buffers are outside this queue's budget.
    pub async fn stage_capture_owned(
        &self,
        input: OwnedStageInput,
    ) -> Result<staging_files::Staged, StageOwnedError> {
        input.checked()?;
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(StageOwnedError::Unavailable);
            }
            self.file_sender
                .try_send(FileRequest::Stage(tx, Box::new(input)))
        };
        match queued {
            Ok(()) => rx.await.unwrap_or(Err(StageOwnedError::Unavailable)),
            Err(_) => Err(StageOwnedError::Unavailable),
        }
    }
    async fn ask<T>(&self, build: impl FnOnce(Response<T>) -> Request) -> Result<T, StorageError> {
        let (tx, rx) = oneshot::channel();
        let queued = {
            let gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
            if gate.closing {
                return Err(StorageError::Unavailable);
            }
            self.sender.try_send(build(tx))
        };
        match queued {
            Ok(()) => rx.await.map_err(|_| StorageError::Unavailable)?,
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => {
                Err(StorageError::Unavailable)
            }
        }
    }
    /// Timeout is unknown/still-owned, never a declaration of Stopped.
    pub async fn shutdown(&self, deadline: Duration) -> Result<(), StorageError> {
        tokio::time::timeout(deadline, async {
            let mut done = {
                let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                gate.closing = true;
                gate.stopped.subscribe()
            };
            loop {
                if *done.borrow() {
                    return Ok(());
                }
                {
                    let mut gate = self.gate.lock().unwrap_or_else(|e| e.into_inner());
                    if !gate.stop_sent {
                        match self.sender.try_send(Request::Stop) {
                            Ok(()) => gate.stop_sent = true,
                            Err(TrySendError::Disconnected(_)) => {
                                return Err(StorageError::Unavailable)
                            }
                            Err(TrySendError::Full(_)) => {}
                        }
                    }
                }
                if *done.borrow() {
                    return Ok(());
                }
                // If Stop has been queued, wait only for completed close + file join.
                let stop_sent = self
                    .gate
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .stop_sent;
                if stop_sent {
                    done.changed()
                        .await
                        .map_err(|_| StorageError::Unavailable)?;
                } else {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            }
        })
        .await
        .map_err(|_| StorageError::Unavailable)?
    }
    /// Actual bundled SQLite runtime version; NOT a sqlite-vec query proof.
    pub async fn sqlite_version(&self) -> Result<String, StorageError> {
        self.ask(Request::SqliteVersion).await
    }
}
fn map_entry(err: LeaseError) -> StorageError {
    match err {
        LeaseError::Busy => StorageError::LockConflict,
        LeaseError::UnsafeEntry => StorageError::RootRejected,
    }
}
fn open(entry: &sm_isolation::DatabaseEntry, fail: bool) -> Result<Connection, StorageError> {
    let fresh = migration::preflight(entry.path(), entry.existed())?;
    let mut db = Connection::open_with_flags(
        entry.path(),
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_CREATE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| StorageError::PermissionDenied)?;
    if !entry.existed() {
        // Durable provenance precedes WAL and the transactional migration. A
        // failure before this point leaves an unclaimed file: fail closed later.
        migration::mark_new_file(&db)?;
    }
    migration::pragmas(&db)?;
    migration::initialize(&mut db, fresh, fail)?;
    Ok(db)
}
fn run(db: Connection, receiver: Receiver<Request>, assets: PathBuf, fault: Fault) -> bool {
    let mut db = db;
    for request in receiver {
        match request {
            Request::Snapshot(tx) => {
                let _ = tx.send(migration::validate(&db));
            }
            Request::AiEnabled(tx) => {
                let _ = tx.send(migration::validate(&db).map(|_| false));
            }
            Request::SqliteVersion(tx) => {
                let _ = tx.send(migration::sqlite_version(&db));
            }
            Request::CommitImage(tx, command) => {
                let result = images::commit_image_capture(
                    &mut db,
                    &assets,
                    *command,
                    fault == Fault::ImageCommit,
                );
                let _ = tx.send(result);
            }
            Request::LookupOperation(tx, op_id, principal_id, mode) => {
                let result =
                    sm_application::storage::CaptureTarget::parse(&mode).and_then(|mode| {
                        images::lookup_operation(&db, &assets, &op_id, &principal_id, mode)
                    });
                let _ = tx.send(result);
            }
            Request::PageRecords(tx, query) => {
                let _ = tx.send(images::page_records(&db, query));
            }
            Request::ReadAssetChunk(tx, asset_id, offset, length) => {
                let _ = tx.send(images::read_asset_chunk(
                    &db, &assets, &asset_id, offset, length,
                ));
            }
            #[cfg(test)]
            Request::Pragmas(tx) => {
                let result = (|| -> Result<_, StorageError> {
                    Ok((
                        db.query_row("PRAGMA journal_mode", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA foreign_keys", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA synchronous", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                        db.query_row("PRAGMA busy_timeout", [], |r| r.get(0))
                            .map_err(|_| StorageError::Unavailable)?,
                    ))
                })();
                let _ = tx.send(result);
            }
            #[cfg(test)]
            Request::Pause(tx, duration) => {
                std::thread::sleep(duration);
                let _ = tx.send(Ok(()));
            }
            Request::Stop => {
                close_confirmed(db);
                return true;
            }
        }
    }
    close_confirmed(db);
    false
}

fn file_loop(
    lease: &HostLease,
    receiver: Receiver<FileRequest>,
    #[cfg(test)] pause: Option<FilePause>,
) {
    for request in receiver {
        match request {
            FileRequest::Stop => break,
            FileRequest::Stage(tx, input) => {
                let result = lease
                    .staging_root()
                    .map_err(|e| StageOwnedError::Stage(staging_files::StageFailure::Storage(e)))
                    .and_then(|root| {
                        #[cfg(test)]
                        if let Some(ref pause) = pause {
                            return staging_files::stage_capture_pausing(
                                &root,
                                input.borrowed(),
                                &|| {
                                    let _ = pause.entered.send(());
                                    let _ = pause
                                        .release
                                        .lock()
                                        .unwrap_or_else(|e| e.into_inner())
                                        .recv();
                                },
                            )
                            .map_err(StageOwnedError::Stage);
                        }
                        staging_files::stage_capture(&root, input.borrowed())
                            .map_err(StageOwnedError::Stage)
                    });
                let _ = tx.send(result);
            }
        }
    }
}

#[cfg(test)]
struct FilePause {
    entered: SyncSender<()>,
    release: Mutex<Receiver<()>>,
}
/// sqlite3_close may return BUSY together with the *still-open* connection.
/// Retain both the connection and the worker-owned HostLease until closure succeeds;
/// never translate a failed close into a released writer lock or Stopped.
fn close_confirmed(mut db: Connection) {
    loop {
        match db.close() {
            Ok(()) => return,
            Err((still_open, _)) => {
                db = still_open;
                std::thread::sleep(Duration::from_millis(10));
            }
        }
    }
}
#[async_trait]
impl FoundationRepository for StorageWorker {
    async fn schema_snapshot(&self) -> Result<SchemaSnapshot, StorageError> {
        self.ask(Request::Snapshot).await
    }
    async fn read_ai_enabled(&self) -> Result<bool, StorageError> {
        self.ask(Request::AiEnabled).await
    }
    async fn commit_image_capture(
        &self,
        command: CommitImageCapture,
    ) -> Result<CommittedImage, StorageError> {
        self.ask(|tx| Request::CommitImage(tx, Box::new(command)))
            .await
    }
    async fn lookup_operation(
        &self,
        operation_id: sm_domain::OperationId,
        principal_id: sm_domain::PrincipalId,
        mode: sm_application::storage::CaptureTarget,
    ) -> Result<LookupOutcome, StorageError> {
        self.ask(|tx| {
            Request::LookupOperation(
                tx,
                operation_id.as_uuid().to_string(),
                principal_id.as_uuid().to_string(),
                mode.as_str().to_owned(),
            )
        })
        .await
    }
    async fn page_records(&self, query: RecordPageQuery) -> Result<Page<Record>, StorageError> {
        self.ask(|tx| Request::PageRecords(tx, query)).await
    }
    async fn read_asset_chunk(
        &self,
        asset_id: sm_domain::AssetId,
        offset: u64,
        length: u32,
    ) -> Result<AssetChunk, StorageError> {
        // Only the canonical UUID text of the named asset reaches the worker.
        let raw = asset_id.as_uuid().to_string();
        self.ask(|tx| Request::ReadAssetChunk(tx, raw, offset, length))
            .await
    }
}
#[cfg(test)]
impl StorageWorker {
    pub(crate) fn start_with_fault(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(lease, Fault::MigrationCommit, None)
    }
    pub(crate) fn start_with_image_fault(lease: HostLease) -> Result<Self, StorageError> {
        Self::start_inner(lease, Fault::ImageCommit, None)
    }
    pub(crate) fn start_with_file_pause(
        lease: HostLease,
        entered: SyncSender<()>,
        release: Receiver<()>,
    ) -> Result<Self, StorageError> {
        Self::start_inner(
            lease,
            Fault::None,
            Some(FilePause {
                entered,
                release: Mutex::new(release),
            }),
        )
    }
    pub(crate) async fn read_pragmas(&self) -> Result<(String, i64, i64, i64), StorageError> {
        self.ask(Request::Pragmas).await
    }
    pub(crate) fn pause_for_test(
        &self,
        duration: Duration,
    ) -> oneshot::Receiver<Result<(), StorageError>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .try_send(Request::Pause(tx, duration))
            .unwrap_or_else(|_| panic!("test queue empty"));
        rx
    }
}
