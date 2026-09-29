//! Pre-host Windows namespace and retained root ownership boundary.
//! Resolving a root is NOT a host lease, migration preflight or permission to write SQLite.
#[cfg(not(all(windows, target_pointer_width = "64")))]
compile_error!("sm-isolation requires Windows x64 for the audited native adapter");

use sm_application::observability::{
    Component, EventId, Level, Operation, Outcome, SafeCode, SafeEvent, SafeEventSink,
};
use std::io::Read;
use std::os::windows::ffi::OsStrExt;
use std::path::{Component as PathComponent, Path};
mod windows;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Environment {
    Product,
    Dev,
    Test,
}

/// One owner for identifiers; none are imported from the Inspector shell.
impl Environment {
    pub const fn installed_directory(self) -> &'static str {
        match self {
            Self::Product => "ScreenMemory",
            Self::Dev => "ScreenMemory.Dev",
            Self::Test => "ScreenMemory.Test",
        }
    }
    pub const fn app_id(self) -> &'static str {
        match self {
            Self::Product => "io.screenmemory.app",
            Self::Dev => "io.screenmemory.dev",
            Self::Test => "io.screenmemory.test",
        }
    }
    pub const fn secret_namespace(self) -> &'static str {
        match self {
            Self::Product => "ScreenMemory.V1",
            Self::Dev => "ScreenMemory.Dev.V1",
            Self::Test => "ScreenMemory.Test.V1",
        }
    }
    pub const fn window_class(self) -> &'static str {
        match self {
            Self::Product => "ScreenMemory.Window.V1",
            Self::Dev => "ScreenMemory.Dev.Window.V1",
            Self::Test => "ScreenMemory.Test.Window.V1",
        }
    }
    pub const fn endpoint_prefix(self) -> &'static str {
        match self {
            Self::Product => "screenmemory.v1",
            Self::Dev => "screenmemory.dev.v1",
            Self::Test => "screenmemory.test.v1",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootError {
    Invalid,
    Missing,
    Inaccessible,
    Unsupported,
    Reparse,
    Overlap,
}

/// Caller-supplied trusted installed configuration, NOT loaded from home/CWD/PATH.
/// All roots must exist; no directory creation/adoption or fallback occurs here.
pub struct InstalledRoots<'a> {
    pub environment: Environment,
    pub data: &'a Path,
    pub config: &'a Path,
    pub cache: &'a Path,
    pub webview: &'a Path,
    pub forbidden_donor_roots: &'a [&'a Path],
}

/// Explicit test root; four independent, pre-existing child directories are required.
pub enum RootSource<'a> {
    Installed(InstalledRoots<'a>),
    Test {
        root: &'a Path,
        forbidden_donor_roots: &'a [&'a Path],
    },
}

/// Private native handles intentionally held for the lifetime of this identity.
/// Never serialize/debug these values or place paths/IDs in SafeEvent/DTO/status.
pub struct RootIdentity {
    native: windows::Directory,
}
impl RootIdentity {
    pub fn canonical_path(&self) -> &Path {
        &self.native.path
    }
    pub fn volume_serial(&self) -> u32 {
        self.native.volume
    }
    pub fn file_identity(&self) -> u64 {
        self.native.file
    }
}

/// A lease consumes the complete layout: the owner cannot drop the rename-pinned
/// ancestors while the OS lock is held. Not Clone, not a status DTO.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    Busy,
    UnsafeEntry,
}

pub struct HostLease {
    // Native lock drops before layout/ancestor pins (Rust field drop order).
    _lock: windows::NativeLease,
    _layout: NamespaceLayout,
}
impl HostLease {
    pub fn acquire(layout: NamespaceLayout) -> Result<Self, LeaseError> {
        let lock = windows::NativeLease::acquire(&layout.data.native)?;
        Ok(Self {
            _lock: lock,
            _layout: layout,
        })
    }

    /// Only the fixed database and SQLite's fixed sidecars are permitted. Hold
    /// existing entries against replacement for the complete connection lifetime.
    pub fn database_entry(&self) -> Result<DatabaseEntry, LeaseError> {
        windows::DatabaseEntry::inspect(&self._layout.data.native)
            .map(|native| DatabaseEntry { native })
    }

    /// Borrowed, pinned authority only while this original host lease is held.
    /// The staging directory is the fixed sibling of the resolved data directory.
    pub fn staging_root(&self) -> Result<StagingRoot<'_>, StagingError> {
        let native = windows::staging_directory(&self._layout.data.native)?;
        Ok(StagingRoot {
            _lease: self,
            native,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingError {
    InvalidOperation,
    AlreadyExists,
    UnsafeEntry,
    Missing,
    NotSynced,
    TooLarge,
    Full,
    AccessDenied,
}

/// No path or arbitrary filename is exported from this write authority.
pub struct StagingRoot<'lease> {
    _lease: &'lease HostLease,
    native: windows::Directory,
}
impl<'lease> StagingRoot<'lease> {
    pub fn create_operation<'root>(
        &'root self,
        id: sm_domain::OperationId,
    ) -> Result<StagingOperation<'root, 'lease>, StagingError> {
        let uuid = id.as_uuid();
        if uuid.is_nil() {
            return Err(StagingError::InvalidOperation);
        }
        let native = windows::create_operation_directory(&self.native, &uuid.to_string())?;
        Ok(StagingOperation {
            _root: self,
            native,
        })
    }

    /// Reopen a known operation without adopting, creating or cleaning its contents.
    pub fn open_operation<'root>(
        &'root self,
        id: sm_domain::OperationId,
    ) -> Result<StagingOperation<'root, 'lease>, StagingError> {
        let uuid = id.as_uuid();
        if uuid.is_nil() {
            return Err(StagingError::InvalidOperation);
        }
        let native = windows::open_operation_directory(&self.native, &uuid.to_string())?;
        Ok(StagingOperation {
            _root: self,
            native,
        })
    }
}

pub struct StagingOperation<'root, 'lease> {
    _root: &'root StagingRoot<'lease>,
    native: windows::Directory,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StagingTempKind {
    PixelsRgba,
    ContextJson,
    ManifestJson,
}
impl StagingTempKind {
    const fn filename(self) -> &'static str {
        match self {
            Self::PixelsRgba => "pixels.rgba.tmp",
            Self::ContextJson => "context.json.tmp",
            Self::ManifestJson => "manifest.json.tmp",
        }
    }
    const fn stable_filename(self) -> &'static str {
        match self {
            Self::PixelsRgba => "pixels.rgba",
            Self::ContextJson => "context.json",
            Self::ManifestJson => "manifest.json",
        }
    }
    const fn max_bytes(self) -> u64 {
        match self {
            Self::PixelsRgba => 512 * 1024 * 1024,
            Self::ContextJson => 256 * 1024,
            Self::ManifestJson => 64 * 1024,
        }
    }
}
impl<'root, 'lease> StagingOperation<'root, 'lease> {
    /// Exclusive CREATE_NEW on a pinned directory; no overwrite or reparse following.
    pub fn create_temp<'operation>(
        &'operation self,
        kind: StagingTempKind,
    ) -> Result<StagingTemp<'operation, 'root, 'lease>, StagingError> {
        let file = windows::create_temp(&self.native, kind.filename())?;
        Ok(StagingTemp {
            operation: self,
            file,
            kind,
            synced: false,
        })
    }

    /// Returns only a fixed, bounded, no-follow stable file; never an arbitrary path.
    pub fn open_fixed(&self, kind: StagingTempKind) -> Result<StagingRead, StagingError> {
        let (file, len) = windows::open_fixed(&self.native, kind.stable_filename())?;
        if len > kind.max_bytes() {
            return Err(StagingError::TooLarge);
        }
        Ok(StagingRead { file, len })
    }
}

/// Immutable length from the verified handle. No path or unbounded File is exposed.
pub struct StagingRead {
    file: std::fs::File,
    len: u64,
}
impl StagingRead {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn read_all(mut self) -> std::io::Result<Vec<u8>> {
        let mut bytes = Vec::with_capacity(self.len as usize);
        std::io::Read::take(&mut self.file, self.len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.len {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "staging changed",
            ));
        }
        Ok(bytes)
    }
}

pub struct StagingTemp<'operation, 'root, 'lease> {
    operation: &'operation StagingOperation<'root, 'lease>,
    file: std::fs::File,
    kind: StagingTempKind,
    synced: bool,
}
impl StagingTemp<'_, '_, '_> {
    pub fn sync_all(&mut self) -> std::io::Result<()> {
        self.synced = false;
        self.file.sync_all()?;
        self.synced = true;
        Ok(())
    }

    /// Explicit publication (including manifest); this does not assert durable_input.
    pub fn publish(self) -> Result<(), StagingError> {
        if !self.synced {
            return Err(StagingError::NotSynced);
        }
        let StagingTemp {
            operation,
            file,
            kind,
            ..
        } = self;
        if file
            .metadata()
            .map_err(|_| StagingError::UnsafeEntry)?
            .len()
            > kind.max_bytes()
        {
            return Err(StagingError::TooLarge);
        }
        windows::publish_temp(
            &operation.native,
            kind.filename(),
            kind.stable_filename(),
            file,
        )
    }
}
impl std::io::Write for StagingTemp<'_, '_, '_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.synced = false;
        std::io::Write::write(&mut self.file, bytes)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        std::io::Write::flush(&mut self.file)
    }
}

pub struct DatabaseEntry {
    native: windows::DatabaseEntry,
}
impl DatabaseEntry {
    pub fn path(&self) -> &Path {
        &self.native.path
    }
    pub fn existed(&self) -> bool {
        self.native.existed
    }
}

pub struct NamespaceLayout {
    pub environment: Environment,
    pub data: RootIdentity,
    pub config: RootIdentity,
    pub cache: RootIdentity,
    pub webview: RootIdentity,
}
impl NamespaceLayout {
    pub fn app_id(&self) -> &'static str {
        self.environment.app_id()
    }
    pub fn secret_namespace(&self) -> &'static str {
        self.environment.secret_namespace()
    }
}

fn invalid_syntax(path: &Path) -> bool {
    if !path.is_absolute() || path.as_os_str().encode_wide().any(|unit| unit == 0) {
        return true;
    }
    let mut parts = path.components();
    if !matches!(parts.next(), Some(PathComponent::Prefix(_)))
        || !matches!(parts.next(), Some(PathComponent::RootDir))
    {
        return true;
    }
    // Reject extended/device, UNC, ADS, traversal and trailing path tricks BEFORE any I/O.
    let text = path.as_os_str().to_string_lossy();
    if !text.as_bytes().get(1).is_some_and(|c| *c == b':')
        || text.starts_with("\\\\")
        || text.starts_with("//")
    {
        return true;
    }
    if text.chars().skip(2).any(|c| c == ':') {
        return true;
    }
    parts.any(|p| !matches!(p, PathComponent::Normal(_)))
}

fn open(path: &Path) -> Result<RootIdentity, RootError> {
    if invalid_syntax(path) {
        return Err(RootError::Invalid);
    }
    windows::Directory::open(path).map(|native| RootIdentity { native })
}
fn overlaps(a: &RootIdentity, b: &RootIdentity) -> bool {
    if a.native.volume != b.native.volume {
        return false;
    }
    if a.native.file == b.native.file {
        return true;
    }
    // Compare opened ancestor directory identities, never path string casing.
    a.native
        .ancestors
        .contains(&(b.native.volume, b.native.file))
        || b.native
            .ancestors
            .contains(&(a.native.volume, a.native.file))
}

pub struct LibraryRootResolver;
impl LibraryRootResolver {
    pub fn resolve(source: RootSource<'_>) -> Result<NamespaceLayout, RootError> {
        let (env, data, config, cache, webview, donor, installed) = match source {
            RootSource::Installed(c) => (
                c.environment,
                c.data.to_path_buf(),
                c.config.to_path_buf(),
                c.cache.to_path_buf(),
                c.webview.to_path_buf(),
                c.forbidden_donor_roots,
                true,
            ),
            RootSource::Test {
                root,
                forbidden_donor_roots,
            } => {
                if invalid_syntax(root) {
                    return Err(RootError::Invalid);
                }
                (
                    Environment::Test,
                    root.join("data"),
                    root.join("config"),
                    root.join("cache"),
                    root.join("webview"),
                    forbidden_donor_roots,
                    false,
                )
            }
        };
        if installed {
            // The trusted installed configuration is explicit, but cannot silently
            // point product/dev/test at a shared directory tree.
            for (path, leaf) in [
                (&data, "data"),
                (&config, "config"),
                (&cache, "cache"),
                (&webview, "webview"),
            ] {
                if !path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case(leaf))
                    || !path
                        .parent()
                        .and_then(Path::file_name)
                        .and_then(|name| name.to_str())
                        .is_some_and(|name| name.eq_ignore_ascii_case(env.installed_directory()))
                {
                    return Err(RootError::Invalid);
                }
            }
        }
        let roots = [open(&data)?, open(&config)?, open(&cache)?, open(&webview)?];
        for i in 0..roots.len() {
            for j in 0..i {
                if overlaps(&roots[i], &roots[j]) {
                    return Err(RootError::Overlap);
                }
            }
        }
        for path in donor {
            let forbidden = open(path)?; // absent/unsafe declared donor roots fail closed
            if roots.iter().any(|root| overlaps(root, &forbidden)) {
                return Err(RootError::Overlap);
            }
        }
        let [data, config, cache, webview] = roots;
        Ok(NamespaceLayout {
            environment: env,
            data,
            config,
            cache,
            webview,
        })
    }

    /// Emit only typed diagnostics; no path, OS exception or canary enters the event.
    pub fn resolve_logged(
        source: RootSource<'_>,
        sink: &dyn SafeEventSink,
    ) -> Result<NamespaceLayout, RootError> {
        let result = Self::resolve(source);
        if let Err(error) = result {
            let mut event = SafeEvent::new(
                Level::Error,
                Component::Storage,
                EventId::ProbeBlocked,
                Operation::Probe,
            );
            event.outcome = Some(Outcome::Failed);
            event.safe_code = Some(match error {
                RootError::Inaccessible => SafeCode::PermissionDenied,
                _ => SafeCode::ProbeBlocked,
            });
            let _ = sink.emit(event);
        }
        result
    }
}
