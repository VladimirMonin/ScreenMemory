//! Fixed original.webp publication capability. No codec, SQL, paths or cleanup API.
use crate::{windows, HostLease, StagingError};
use sm_application::storage::is_canonical_utc_timestamp;
use sm_domain::RecordId;
use std::io::{Read, Seek, SeekFrom, Write};
use uuid::{Uuid, Variant};

pub const MAX_CANONICAL_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CanonicalError {
    InvalidPlan,
    Existing,
    Missing,
    UnsafeEntry,
    TooLarge,
    NotSynced,
    Full,
    AccessDenied,
}
impl From<StagingError> for CanonicalError {
    fn from(error: StagingError) -> Self {
        match error {
            StagingError::AlreadyExists => Self::Existing,
            StagingError::Missing => Self::Missing,
            StagingError::TooLarge => Self::TooLarge,
            StagingError::NotSynced => Self::NotSynced,
            StagingError::Full => Self::Full,
            StagingError::AccessDenied => Self::AccessDenied,
            _ => Self::UnsafeEntry,
        }
    }
}

/// DB-facing planned creation time, never the retry wall clock. No caller path or leaf.
pub struct CanonicalPlan {
    month: String,
    record: String,
    leaf: &'static str,
}
impl CanonicalPlan {
    pub fn new(record: RecordId, planned_created_at_utc: &str) -> Result<Self, CanonicalError> {
        let uuid = record.as_uuid();
        if uuid.get_version_num() != 4
            || uuid.get_variant() != Variant::RFC4122
            || !is_canonical_utc_timestamp(planned_created_at_utc)
        {
            return Err(CanonicalError::InvalidPlan);
        }
        let month = &planned_created_at_utc[..7];
        if month.len() != 7
            || month.as_bytes()[4] != b'-'
            || !month[..4].bytes().all(|b| b.is_ascii_digit())
            || !("01"..="12").contains(&&month[5..])
        {
            return Err(CanonicalError::InvalidPlan);
        }
        // The shared timestamp guard validates shape/ranges only. This public
        // path-bearing plan must additionally reject impossible Gregorian dates.
        let year = planned_created_at_utc[..4]
            .parse::<u32>()
            .map_err(|_| CanonicalError::InvalidPlan)?;
        let month_number = planned_created_at_utc[5..7]
            .parse::<u32>()
            .map_err(|_| CanonicalError::InvalidPlan)?;
        let day = planned_created_at_utc[8..10]
            .parse::<u32>()
            .map_err(|_| CanonicalError::InvalidPlan)?;
        let leap = year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
        let days = match month_number {
            2 if leap => 29,
            2 => 28,
            4 | 6 | 9 | 11 => 30,
            _ => 31, // 01..=12 already validated by the shared guard
        };
        if year == 0 || !(1..=days).contains(&day) {
            return Err(CanonicalError::InvalidPlan);
        }
        Ok(Self {
            month: month.to_owned(),
            record: uuid.to_string(),
            leaf: "original.webp",
        })
    }
    /// Metadata spelling of the already validated fixed original plan; not a
    /// caller-controlled filesystem capability.
    pub fn relative_asset_path(&self) -> String {
        format!("assets/{}/{}/{}", self.month, self.record, self.leaf)
    }
    pub fn avif(record: RecordId, time: &str) -> Result<Self, CanonicalError> {
        let mut plan = Self::new(record, time)?;
        plan.leaf = "original.avif";
        Ok(plan)
    }
}

pub struct CanonicalRoot<'lease> {
    pub(crate) _lease: &'lease HostLease,
    pub(crate) native: windows::Directory,
}
impl<'lease> CanonicalRoot<'lease> {
    /// Creates only the validated month/record directories; existing directories
    /// are reopened no-follow and identity checked under the retained assets chain.
    pub fn begin(&self, plan: &CanonicalPlan) -> Result<CanonicalTemp<'_, 'lease>, CanonicalError> {
        let month = windows::ensure_child_directory(&self.native, &plan.month)?;
        let record = windows::ensure_child_directory(&month, &plan.record)?;
        match windows::open_fixed(&record, plan.leaf) {
            Ok(_) => return Err(CanonicalError::Existing),
            Err(StagingError::Missing) => {}
            Err(error) => return Err(error.into()),
        }
        let temp_name = format!("{}.tmp", Uuid::new_v4());
        let file = windows::create_temp(&record, &temp_name)?;
        Ok(CanonicalTemp {
            _root: self,
            record,
            temp_name,
            leaf: plan.leaf,
            file,
            written: 0,
            synced: false,
        })
    }

    /// Explicit bounded inspection for a future codec/hash comparison. Never
    /// adopts an existing final as a matching canonical image by itself.
    pub fn open_final(&self, plan: &CanonicalPlan) -> Result<CanonicalRead, CanonicalError> {
        let month = windows::open_operation_directory(&self.native, &plan.month)?;
        let record = windows::open_operation_directory(&month, &plan.record)?;
        let (file, len) = windows::open_fixed(&record, plan.leaf)?;
        if len > MAX_CANONICAL_BYTES {
            return Err(CanonicalError::TooLarge);
        }
        Ok(CanonicalRead {
            file,
            len,
            _record: record,
            leaf: plan.leaf,
        })
    }
}

pub struct CanonicalTemp<'root, 'lease> {
    _root: &'root CanonicalRoot<'lease>,
    record: windows::Directory,
    temp_name: String,
    leaf: &'static str,
    file: std::fs::File,
    written: u64,
    synced: bool,
}
impl CanonicalTemp<'_, '_> {
    pub fn sync_all(&mut self) -> std::io::Result<()> {
        self.synced = false;
        self.file.sync_all()?;
        self.synced = true;
        Ok(())
    }
    /// No replacement, no copy, no unlink. Any failed publication retains the
    /// attempt temp, including an incomplete or unsynced attempt. Publication
    /// does not prove codec validity or protect against same-length mutation.
    pub fn publish(self) -> Result<(), CanonicalError> {
        if !self.synced {
            return Err(CanonicalError::NotSynced);
        }
        let len = self
            .file
            .metadata()
            .map_err(|_| CanonicalError::UnsafeEntry)?
            .len();
        // SHARE_WRITE permits another same-user writer to alter content, even
        // after our sync. A size mismatch or empty temp cannot be published.
        if self.written == 0 || len == 0 || len != self.written {
            return Err(CanonicalError::UnsafeEntry);
        }
        if len > MAX_CANONICAL_BYTES {
            return Err(CanonicalError::TooLarge);
        }
        windows::publish_temp(&self.record, &self.temp_name, self.leaf, self.file)
            .map_err(Into::into)
    }
}
impl Write for CanonicalTemp<'_, '_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.synced = false;
        if (bytes.len() as u64) > MAX_CANONICAL_BYTES - self.written {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "canonical size limit",
            ));
        }
        let n = self.file.write(bytes)?;
        self.written += n as u64;
        Ok(n)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

pub struct CanonicalRead {
    leaf: &'static str,
    file: std::fs::File,
    len: u64,
    // Directory::open owns no-delete pins for the entire ancestor chain,
    // including assets/month/record, even after CanonicalRoot is dropped.
    _record: windows::Directory,
}
impl CanonicalRead {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    /// Read from the pinned, no-follow handle without consuming it. A writer may
    /// change content through SHARE_WRITE, so callers must recheck immediately
    /// before relying on this proof; the handle alone is not an immutable snapshot.
    pub fn read_all_borrowed(&mut self) -> std::io::Result<Vec<u8>> {
        if self.file.metadata()?.len() != self.len || self.len > MAX_CANONICAL_BYTES {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical length changed",
            ));
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(self.len as usize);
        (&mut self.file)
            .take(self.len + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.len || self.file.metadata()?.len() != self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical changed",
            ));
        }
        Ok(bytes)
    }
    pub fn read_all(mut self) -> std::io::Result<Vec<u8>> {
        self.read_all_borrowed()
    }

    /// Consume the pinned old read while opening fixed original.webp with a
    /// write-denying share. Failure releases both; no stale proof can be reused.
    pub fn seal(self) -> Result<CanonicalSeal, CanonicalError> {
        let (file, len) = windows::seal_fixed(&self._record, &self.file, self.len, self.leaf)?;
        if len > MAX_CANONICAL_BYTES {
            return Err(CanonicalError::TooLarge);
        }
        Ok(CanonicalSeal {
            file,
            len,
            _record: self._record,
        })
    }
}

/// Non-Clone fixed canonical HANDLE and complete no-delete ancestor pin chain.
/// Share denial is not a sandbox and does not revoke preexisting writable views.
pub struct CanonicalSeal {
    file: std::fs::File,
    len: u64,
    _record: windows::Directory,
}
impl CanonicalSeal {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn read_all_borrowed(&mut self) -> std::io::Result<Vec<u8>> {
        if self.len > MAX_CANONICAL_BYTES || self.file.metadata()?.len() != self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical length changed",
            ));
        }
        self.file.seek(SeekFrom::Start(0))?;
        let mut bytes = Vec::with_capacity(self.len as usize);
        (&mut self.file)
            .take(self.len + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.len || self.file.metadata()?.len() != self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "canonical changed",
            ));
        }
        Ok(bytes)
    }
}
