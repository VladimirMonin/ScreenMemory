//! Application-facing storage snapshots only; no connection, SQL or path authority.
use async_trait::async_trait;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaSnapshot {
    pub library_id: Uuid,
    pub schema_version: u32,
    pub format_major: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    RootRejected,
    LockConflict,
    PermissionDenied,
    SchemaIncompatible,
    MigrationFailed,
    IntegrityFailed,
    Unavailable,
}

#[async_trait]
pub trait FoundationRepository: Send + Sync {
    async fn schema_snapshot(&self) -> Result<SchemaSnapshot, StorageError>;
    async fn read_ai_enabled(&self) -> Result<bool, StorageError>;
}
