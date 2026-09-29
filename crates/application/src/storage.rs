//! Application-facing storage snapshots only; no connection, SQL or path authority.
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use sm_domain::{AssetId, RecordId};
use uuid::Uuid;

/// Canonical format vocabulary of the images schema (S02: WebP lossless only).
pub const CANONICAL_FORMAT_WEBP_LOSSLESS: &str = "webp_lossless";
/// MIME of the one canonical stored format; recovered from the asset row, never guessed.
pub const CANONICAL_MIME_WEBP: &str = "image/webp";
/// Longest accepted capture-context title.
pub const MAX_TITLE_LENGTH: usize = 160;
/// Longest accepted provenance description.
pub const MAX_PROVENANCE_LENGTH: usize = 160;
/// Longest accepted surface-size of one canonical capture; the WebP lossless bound.
pub const MAX_CAPTURE_SIDE: u32 = 16_383;
/// Largest number of diagnostic warnings a saved capture row may carry.
pub const MAX_WARNINGS: usize = 16;
/// Longest accepted single diagnostic warning; a stable code, never free-form text.
pub const MAX_WARNING_LENGTH: usize = 64;

/// Images-schema state of an open library.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImagesSchema {
    /// Applied images migration version.
    pub migration_version: u32,
    /// Canonical format vocabulary the schema accepts; AVIF is deliberately absent.
    pub canonical_format: &'static str,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SchemaSnapshot {
    pub library_id: Uuid,
    pub schema_version: u32,
    pub format_major: u32,
    pub images: ImagesSchema,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageError {
    RootRejected,
    LockConflict,
    PermissionDenied,
    SchemaIncompatible,
    MigrationFailed,
    IntegrityFailed,
    /// A typed payload failed product-level validation before any SQL or file write.
    InvalidPayload,
    /// A page query carried an unusable limit, cursor or identity.
    InvalidQuery,
    /// The named asset row does not exist in this library.
    AssetNotFound,
    /// The canonical file does not match its committed row (size or digest).
    AssetIntegrityFailed,
    Unavailable,
}

/// Physical-pixel rectangle of the accepted capture (`[x, x+width) x [y, y+height)`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicalBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Capture target of one image; the same vocabulary names the provenance source.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureTarget {
    Region,
    ActiveWindow,
    Monitor,
    Desktop,
}

impl CaptureTarget {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Region => "region",
            Self::ActiveWindow => "active_window",
            Self::Monitor => "monitor",
            Self::Desktop => "desktop",
        }
    }

    /// Closed stored name of one capture target. A value outside the vocabulary is an
    /// integrity failure, never a silently repaired or newly invented target.
    pub fn parse(raw: &str) -> Result<Self, StorageError> {
        match raw {
            "region" => Ok(Self::Region),
            "active_window" => Ok(Self::ActiveWindow),
            "monitor" => Ok(Self::Monitor),
            "desktop" => Ok(Self::Desktop),
            _ => Err(StorageError::IntegrityFailed),
        }
    }
}

/// Stored capture context: device, confirmed application, observed title, geometry.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureContext {
    pub device: String,
    pub app: String,
    pub title: String,
    pub target: CaptureTarget,
    pub sources: Vec<CaptureTarget>,
    /// Captured geometry in physical pixels.
    pub bounds: PhysicalBounds,
}

/// Provenance of one observation: manual shortcuts and automatic sources differ.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Provenance {
    /// Created by the manual `PrintScreen` action; no automatic source applies.
    Manual { when: String },
    /// Created by an automatic capture action with the observed source.
    Automatic { when: String, source: CaptureTarget },
}

/// Manual provenance description, exactly as the S02 record vocabulary names it.
const MANUAL_PROVENANCE: &str = "PrintScreen";
/// Automatic provenance description; the observed `source` distinguishes the mode.
const AUTOMATIC_PROVENANCE: &str = "Automatic capture";

/// Stored provenance member shape: description, ISO instant and optional source.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProvenanceWire {
    description: String,
    when: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<CaptureTarget>,
}

impl Serialize for Provenance {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ProvenanceWire {
            description: self.description().to_owned(),
            when: self.when().to_owned(),
            source: match self {
                Self::Manual { .. } => None,
                Self::Automatic { source, .. } => Some(*source),
            },
        }
        .serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Provenance {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = ProvenanceWire::deserialize(deserializer)?;
        match (wire.description.as_str(), wire.source) {
            (MANUAL_PROVENANCE, None) => Ok(Self::Manual { when: wire.when }),
            (AUTOMATIC_PROVENANCE, Some(source)) => Ok(Self::Automatic {
                when: wire.when,
                source,
            }),
            _ => Err(serde::de::Error::custom(
                "unknown provenance description or source",
            )),
        }
    }
}

impl Provenance {
    fn when(&self) -> &str {
        match self {
            Self::Manual { when } | Self::Automatic { when, .. } => when,
        }
    }
    fn description(&self) -> &'static str {
        match self {
            Self::Manual { .. } => MANUAL_PROVENANCE,
            Self::Automatic { .. } => AUTOMATIC_PROVENANCE,
        }
    }
    fn validate(&self) -> Result<(), StorageError> {
        if !is_canonical_utc_timestamp(self.when())
            || self.description().len() > MAX_PROVENANCE_LENGTH
        {
            return Err(StorageError::InvalidPayload);
        }
        Ok(())
    }
}

impl CaptureContext {
    fn validate(&self) -> Result<(), StorageError> {
        if self.device.is_empty()
            || self.device.len() > MAX_TITLE_LENGTH
            || self.app.is_empty()
            || self.app.len() > MAX_TITLE_LENGTH
            || self.title.is_empty()
            || self.title.len() > MAX_TITLE_LENGTH
            || self.bounds.width == 0
            || self.bounds.height == 0
            || self.sources.is_empty()
            || self.sources.windows(2).any(|pair| pair[0] == pair[1])
        {
            return Err(StorageError::InvalidPayload);
        }
        Ok(())
    }

    /// Stored JSON form, validated before the value can reach SQL.
    pub fn to_json(&self) -> Result<String, StorageError> {
        self.validate()?;
        serde_json::to_string(self).map_err(|_| StorageError::InvalidPayload)
    }

    /// Re-validated stored JSON form; a tampered row is refused, not repaired.
    pub fn from_json(raw: &str) -> Result<Self, StorageError> {
        let value: Self = serde_json::from_str(raw).map_err(|_| StorageError::InvalidPayload)?;
        value.validate()?;
        Ok(value)
    }
}

/// Stored provenance list, validated before the value can reach SQL.
pub fn provenances_to_json(provenances: &[Provenance]) -> Result<String, StorageError> {
    if provenances.is_empty() {
        return Err(StorageError::InvalidPayload);
    }
    for provenance in provenances {
        provenance.validate()?;
    }
    serde_json::to_string(provenances).map_err(|_| StorageError::InvalidPayload)
}

/// Re-validated stored provenance list.
pub fn provenances_from_json(raw: &str) -> Result<Vec<Provenance>, StorageError> {
    let values: Vec<Provenance> =
        serde_json::from_str(raw).map_err(|_| StorageError::InvalidPayload)?;
    if values.is_empty() {
        return Err(StorageError::InvalidPayload);
    }
    for value in &values {
        value.validate()?;
    }
    Ok(values)
}

/// Stored diagnostic-warning list, bounded and validated before the value can reach SQL.
///
/// A warning is a short stable code (`display_unknown`), never free-form text or a path,
/// so the stored receipt cannot smuggle content into the journal. The list is capped in
/// count and per-entry length so a tampered row is refused instead of read back.
pub fn warnings_to_json(warnings: &[String]) -> Result<String, StorageError> {
    validate_warnings(warnings)?;
    serde_json::to_string(warnings).map_err(|_| StorageError::InvalidPayload)
}

/// Re-validated stored warning list; a tampered array is refused, not repaired.
pub fn warnings_from_json(raw: &str) -> Result<Vec<String>, StorageError> {
    let values: Vec<String> =
        serde_json::from_str(raw).map_err(|_| StorageError::InvalidPayload)?;
    validate_warnings(&values)?;
    Ok(values)
}

fn validate_warnings(warnings: &[String]) -> Result<(), StorageError> {
    if warnings.len() > MAX_WARNINGS
        || warnings.iter().any(|warning| {
            warning.is_empty()
                || warning.len() > MAX_WARNING_LENGTH
                || !warning
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte == b'_')
        })
    {
        return Err(StorageError::InvalidPayload);
    }
    Ok(())
}

/// Canonical UTC millisecond timestamp shape: `YYYY-MM-DDTHH:MM:SS.mmmZ`.
pub fn is_canonical_utc_timestamp(raw: &str) -> bool {
    let bytes = raw.as_bytes();
    if bytes.len() != 24 || bytes[10] != b'T' || bytes[19] != b'.' || bytes[23] != b'Z' {
        return false;
    }
    let digits = |range: std::ops::Range<usize>| bytes[range].iter().all(u8::is_ascii_digit);
    if !digits(0..4)
        || bytes[4] != b'-'
        || !digits(5..7)
        || bytes[7] != b'-'
        || !digits(8..10)
        || !digits(11..13)
        || bytes[13] != b':'
        || !digits(14..16)
        || bytes[16] != b':'
        || !digits(17..19)
        || !digits(20..23)
    {
        return false;
    }
    let number = |range: std::ops::Range<usize>| raw[range].parse::<u32>().unwrap_or(u32::MAX);
    let (month, day) = (number(5..7), number(8..10));
    let (hour, minute, second) = (number(11..13), number(14..16), number(17..19));
    (1..=12).contains(&month)
        && (1..=31).contains(&day)
        && hour <= 23
        && minute <= 59
        && second <= 59
}

/// Asset role of one immutable file of a record.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AssetType {
    Original,
    Preview,
    Edited,
}

impl AssetType {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Original => "original",
            Self::Preview => "preview",
            Self::Edited => "edited",
        }
    }
    pub fn parse(raw: &str) -> Result<Self, StorageError> {
        match raw {
            "original" => Ok(Self::Original),
            "preview" => Ok(Self::Preview),
            "edited" => Ok(Self::Edited),
            _ => Err(StorageError::IntegrityFailed),
        }
    }
}

/// One committed immutable asset row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub id: AssetId,
    pub record_id: RecordId,
    pub asset_type: AssetType,
    pub format: &'static str,
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: String,
    pub created_at: String,
}

/// One committed record with its assets, without any absolute path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Record {
    pub id: RecordId,
    pub created_at: String,
    pub title: String,
    pub target: CaptureTarget,
    pub bounds: PhysicalBounds,
    pub assets: Vec<Asset>,
}

/// One bounded page of a stable snapshot; `next_cursor` is opaque and validated.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
}

/// Bounded list request. The caller never supplies SQL, only a limit and a cursor.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordPageQuery {
    limit: u16,
    cursor: Option<String>,
}

impl RecordPageQuery {
    pub const DEFAULT_LIMIT: u16 = 30;
    pub const MAX_LIMIT: u16 = 200;

    /// First page of a new snapshot; `None` uses the documented default limit.
    pub fn first(limit: Option<u16>) -> Result<Self, StorageError> {
        Self::new(limit, None)
    }

    /// Follow-up page of an already started snapshot.
    pub fn new(limit: Option<u16>, cursor: Option<String>) -> Result<Self, StorageError> {
        let limit = limit.unwrap_or(Self::DEFAULT_LIMIT);
        if limit == 0 || limit > Self::MAX_LIMIT {
            return Err(StorageError::InvalidQuery);
        }
        Ok(Self { limit, cursor })
    }

    pub const fn limit(&self) -> u16 {
        self.limit
    }

    pub fn cursor(&self) -> Option<&str> {
        self.cursor.as_deref()
    }
}

/// One accepted image waiting for its durable record, asset row and file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommitImageCapture {
    pub operation_id: sm_domain::OperationId,
    pub principal_id: sm_domain::PrincipalId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub asset_type: AssetType,
    /// Closed capture target of the accepted frame; part of both the semantic fingerprint
    /// and the replayable saved receipt metadata.
    pub mode: CaptureTarget,
    /// Positive canonical pixel geometry of the accepted crop.
    pub width: u32,
    pub height: u32,
    pub capture_context: CaptureContext,
    pub provenances: Vec<Provenance>,
    /// Bounded, validated diagnostic codes; stored atomically with the saved row.
    pub warnings: Vec<String>,
    /// Verified canonical container bytes; the worker re-checks the container shape.
    pub canonical: Vec<u8>,
}

/// Replayable receipt metadata of one committed image; paths are library-relative only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CommittedImage {
    pub operation_id: sm_domain::OperationId,
    pub principal_id: sm_domain::PrincipalId,
    pub record_id: RecordId,
    pub asset_id: AssetId,
    pub mode: CaptureTarget,
    pub width: u32,
    pub height: u32,
    /// MIME of the stored canonical asset format, recovered from the asset row.
    pub canonical_mime: &'static str,
    pub warnings: Vec<String>,
    pub created_at: String,
    pub relative_path: String,
    pub size_bytes: u64,
    pub sha256: String,
}

/// Typed, ownership-scoped outcome of one operation lookup (S02 §8.7).
///
/// A caller can only ever observe the receipt of its **own** saved operation. A foreign
/// principal, or a replay whose capture mode differs from the stored one, yields the
/// same safe [`LookupOutcome::Conflict`] carrying **no** receipt metadata. Only a truly
/// absent operation id is [`LookupOutcome::Unknown`]. A saved row whose stored metadata
/// is malformed is an error, never a fabricated receipt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LookupOutcome {
    /// The caller's own saved operation, with its committed receipt metadata.
    Saved(CommittedImage),
    /// The operation id is truly absent from the journal.
    Unknown,
    /// The id is owned by another principal or was committed with a different mode; no
    /// stored metadata is disclosed.
    Conflict,
}

/// One bounded chunk of a committed canonical asset; the digest covers the whole file.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetChunk {
    pub asset_id: AssetId,
    pub offset: u64,
    pub next_offset: u64,
    pub eof: bool,
    pub total_size: u64,
    pub sha256: String,
    pub format: &'static str,
    pub bytes: Vec<u8>,
}

/// One storage owner: snapshots, image commits and bounded reads.
#[async_trait]
pub trait FoundationRepository: Send + Sync {
    async fn schema_snapshot(&self) -> Result<SchemaSnapshot, StorageError>;
    async fn read_ai_enabled(&self) -> Result<bool, StorageError>;
    /// Insert the record, write the canonical asset file and commit, or leave neither.
    async fn commit_image_capture(
        &self,
        command: CommitImageCapture,
    ) -> Result<CommittedImage, StorageError>;
    /// Idempotent, ownership-scoped replay lookup of one operation.
    async fn lookup_operation(
        &self,
        operation_id: sm_domain::OperationId,
        principal_id: sm_domain::PrincipalId,
        mode: CaptureTarget,
    ) -> Result<LookupOutcome, StorageError>;
    /// Bounded, snapshot-stable page of committed records with their assets.
    async fn page_records(&self, query: RecordPageQuery) -> Result<Page<Record>, StorageError>;
    /// Read one bounded chunk of a committed canonical asset. The path is never supplied
    /// by the caller: it is taken from the committed row under the leased assets root.
    async fn read_asset_chunk(
        &self,
        asset_id: AssetId,
        offset: u64,
        length: u32,
    ) -> Result<AssetChunk, StorageError>;
}

/// Largest canonical file a bounded read may touch; a bigger file is refused instead of
/// being read into memory.
pub const MAX_CANONICAL_FILE_BYTES: u64 = 128 * 1024 * 1024;
/// Chunk size of one bounded asset read, chosen to keep one application frame small.
pub const ASSET_CHUNK_BYTES: u32 = 512 * 1024;
