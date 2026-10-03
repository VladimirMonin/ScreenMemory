//! S03 file authority: retained native directory chains, no-follow stable sources,
//! fixed internal names, create-new/no-replace outputs. No SQL or decoding.
use crate::{windows, HostLease, RootError, StagingError};
use std::{
    fs::File,
    io::{Read, Write},
    path::{Component, Path, PathBuf},
};
use uuid::Uuid;

fn checked(path: &Path) -> Result<(), StagingError> {
    let text = path.to_str().ok_or(StagingError::UnsafeEntry)?;
    if text.len() < 3
        || !text.as_bytes()[0].is_ascii_alphabetic()
        || &text.as_bytes()[1..3] != b":\\"
        || text[2..].contains(':')
        || text.contains('\0')
        || text.contains('/')
        || path
            .components()
            .any(|c| matches!(c, Component::ParentDir | Component::CurDir))
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(())
}
fn pin(path: &Path) -> Result<windows::Directory, StagingError> {
    checked(path)?;
    windows::Directory::open(path).map_err(|e| match e {
        RootError::Missing => StagingError::Missing,
        RootError::Inaccessible => StagingError::AccessDenied,
        _ => StagingError::UnsafeEntry,
    })
}

pub struct ImageDirectory {
    native: windows::Directory,
}
pub struct StableImageFile {
    file: File,
    directory: ImageDirectory,
    len: u64,
    observation: (u32, u64, u64, u64, u32),
}
impl StableImageFile {
    pub fn len(&self) -> u64 {
        self.len
    }
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
    pub fn resolved_path(&self) -> Result<PathBuf, StagingError> {
        windows::image_resolved_path(&self.file)
    }
    pub fn observation(&self) -> (u32, u64, u64, u64, u32) {
        self.observation
    }
    pub fn read_bounded(&mut self, limit: u64) -> Result<Vec<u8>, StagingError> {
        if self.len == 0 || self.len > limit {
            return Err(StagingError::TooLarge);
        }
        let mut bytes = Vec::with_capacity(self.len as usize);
        (&mut self.file)
            .take(limit + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| StagingError::UnsafeEntry)?;
        if bytes.len() as u64 != self.len
            || self
                .file
                .metadata()
                .map_err(|_| StagingError::UnsafeEntry)?
                .len()
                != self.len
            || windows::image_observation(&self.file, &self.directory.native)? != self.observation
        {
            return Err(StagingError::UnsafeEntry);
        }
        Ok(bytes)
    }
    pub fn copy_to(
        &mut self,
        output: &mut impl Write,
        limit: u64,
        mut stopped: impl FnMut() -> bool,
    ) -> Result<(), StagingError> {
        if self.len == 0 || self.len > limit {
            return Err(StagingError::TooLarge);
        }
        let mut copied = 0u64;
        let mut buffer = [0u8; 256 * 1024];
        loop {
            if stopped() {
                return Err(StagingError::AccessDenied);
            }
            let n = self
                .file
                .read(&mut buffer)
                .map_err(|_| StagingError::UnsafeEntry)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            if copied > limit {
                return Err(StagingError::TooLarge);
            }
            output
                .write_all(&buffer[..n])
                .map_err(|_| StagingError::Full)?;
        }
        if copied != self.len
            || self
                .file
                .metadata()
                .map_err(|_| StagingError::UnsafeEntry)?
                .len()
                != self.len
            || windows::image_observation(&self.file, &self.directory.native)? != self.observation
        {
            return Err(StagingError::UnsafeEntry);
        }
        Ok(())
    }
}
impl HostLease {
    /// Denies the complete application namespace, including canonical and staging aliases.
    pub fn image_external_directory(&self, path: &Path) -> Result<ImageDirectory, StagingError> {
        let native = pin(path)?;
        let protected = self
            ._layout
            .data
            .native
            .ancestors
            .get(self._layout.data.native.ancestors.len().saturating_sub(2))
            .ok_or(StagingError::UnsafeEntry)?;
        if native.ancestors.contains(protected) {
            return Err(StagingError::UnsafeEntry);
        }
        Ok(ImageDirectory { native })
    }
    /// Metadata-only classification of an explicitly selected LOCAL path. It grants
    /// no pixel read authority, including when the file is inside a denied namespace.
    pub fn image_path_is_file(&self, path: &Path) -> Result<bool, StagingError> {
        checked(path)?;
        let parent = pin(path.parent().ok_or(StagingError::UnsafeEntry)?)?;
        let leaf = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(StagingError::UnsafeEntry)?;
        leaf_checked(leaf)?;
        match windows::image_entry_identity(&parent, leaf) {
            Ok(_) => Ok(true),
            Err(StagingError::UnsafeEntry) => Ok(false),
            Err(e) => Err(e),
        }
    }
    pub fn image_source(&self, path: &Path) -> Result<StableImageFile, StagingError> {
        checked(path)?;
        let directory =
            self.image_external_directory(path.parent().ok_or(StagingError::UnsafeEntry)?)?;
        let leaf = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or(StagingError::UnsafeEntry)?;
        leaf_checked(leaf)?;
        let (file, len) = windows::open_image_source(&directory.native, leaf)?;
        let observation = windows::image_observation(&file, &directory.native)?;
        // Hardlink occurrences are legitimate inputs, but aliases of ANY protected
        // internal file are not. Only this uncommon case needs a bounded metadata
        // scan of the host's explicit owned roots; never read their pixels here.
        if observation.4 > 1 && self.image_internal_identity((observation.0, observation.1))? {
            return Err(StagingError::UnsafeEntry);
        }
        Ok(StableImageFile {
            file,
            directory,
            len,
            observation,
        })
    }
    fn image_internal_identity(&self, id: (u32, u64)) -> Result<bool, StagingError> {
        let mut roots = vec![
            pin(&self._layout.data.native.path)?,
            pin(&self._layout.config.native.path)?,
            pin(&self._layout.cache.native.path)?,
            pin(&self._layout.webview.native.path)?,
            windows::assets_directory(&self._layout.data.native)?,
            windows::staging_directory(&self._layout.data.native)?,
        ];
        for name in ["backup", "backups"] {
            let path = self
                ._layout
                .data
                .native
                .path
                .parent()
                .ok_or(StagingError::UnsafeEntry)?
                .join(name);
            match pin(&path) {
                Ok(root) => roots.push(root),
                Err(StagingError::Missing) => {}
                Err(e) => return Err(e),
            }
        }
        let mut seen = 0u32;
        for root in roots {
            let entries =
                std::fs::read_dir(&root.path).map_err(|_| StagingError::EnumerationIncomplete)?;
            let mut stack = vec![(root, entries)];
            while !stack.is_empty() {
                let Some(entry) = stack
                    .last_mut()
                    .ok_or(StagingError::EnumerationIncomplete)?
                    .1
                    .next()
                else {
                    stack.pop();
                    continue;
                };
                seen += 1;
                if seen > 100000 {
                    return Err(StagingError::EnumerationIncomplete);
                }
                let entry = entry.map_err(|_| StagingError::EnumerationIncomplete)?;
                use std::os::windows::fs::MetadataExt;
                let metadata = std::fs::symlink_metadata(entry.path())
                    .map_err(|_| StagingError::EnumerationIncomplete)?;
                if metadata.file_attributes() & 0x400 != 0 {
                    return Err(StagingError::UnsafeEntry);
                }
                if metadata.is_dir() {
                    if stack.len() >= 128 {
                        return Err(StagingError::EnumerationIncomplete);
                    }
                    let root = pin(&entry.path())?;
                    let entries = std::fs::read_dir(&root.path)
                        .map_err(|_| StagingError::EnumerationIncomplete)?;
                    stack.push((root, entries));
                } else if metadata.is_file() {
                    let leaf = entry.file_name();
                    let leaf = leaf.to_str().ok_or(StagingError::UnsafeEntry)?;
                    let root = &stack.last().ok_or(StagingError::EnumerationIncomplete)?.0;
                    if windows::image_entry_identity(root, leaf)? == id {
                        return Ok(true);
                    }
                } else {
                    return Err(StagingError::UnsafeEntry);
                }
            }
        }
        Ok(false)
    }
    pub fn image_export_directory(&self) -> Result<ImageDirectory, StagingError> {
        let native = windows::ensure_child_directory(&self._layout.cache.native, "export")?;
        Ok(ImageDirectory { native })
    }
    pub fn image_asset_read(&self, relative: &str) -> Result<StableImageFile, StagingError> {
        let parts: Vec<&str> = relative.split('/').collect();
        if parts.len() != 4
            || parts[0] != "assets"
            || parts[1].len() != 7
            || parts[1].as_bytes()[4] != b'-'
            || !parts[1]
                .bytes()
                .enumerate()
                .all(|(i, b)| i == 4 || b.is_ascii_digit())
            || !matches!(
                parts[3],
                "original.webp"
                    | "edited.webp"
                    | "preview.webp"
                    | "original.avif"
                    | "edited.avif"
                    | "preview.avif"
            )
            || Uuid::parse_str(parts[2])
                .map_or(true, |id| id.is_nil() || id.to_string() != parts[2])
        {
            return Err(StagingError::UnsafeEntry);
        }
        let assets = windows::assets_directory(&self._layout.data.native)?;
        let month = windows::open_operation_directory(&assets, parts[1])?;
        let native = windows::open_operation_directory(&month, parts[2])?;
        ImageDirectory { native }.open(parts[3])
    }
    pub fn image_snapshot_directory(&self, id: Uuid) -> Result<ImageDirectory, StagingError> {
        if id.is_nil() {
            return Err(StagingError::InvalidOperation);
        }
        let staging = windows::staging_directory(&self._layout.data.native)?;
        let imports = windows::ensure_child_directory(&staging, "imports")?;
        let native = windows::ensure_child_directory(&imports, &id.to_string())?;
        Ok(ImageDirectory { native })
    }
}
impl ImageDirectory {
    pub fn path(&self) -> &Path {
        &self.native.path
    }
    pub fn entries(&self) -> Result<std::fs::ReadDir, StagingError> {
        std::fs::read_dir(&self.native.path).map_err(|_| StagingError::AccessDenied)
    }
    pub fn open(&self, leaf: &str) -> Result<StableImageFile, StagingError> {
        leaf_checked(leaf)?;
        let (file, len) = windows::open_image_stable(&self.native, leaf)?;
        let native = pin(&self.native.path)?;
        if native.volume != self.native.volume || native.file != self.native.file {
            return Err(StagingError::UnsafeEntry);
        }
        let observation = windows::image_observation(&file, &native)?;
        Ok(StableImageFile {
            file,
            len,
            observation,
            directory: ImageDirectory { native },
        })
    }
    pub fn create(&self, leaf: &str) -> Result<File, StagingError> {
        leaf_checked(leaf)?;
        windows::create_temp(&self.native, leaf)
    }
    pub fn remove_verified(
        &self,
        leaf: &str,
        verify: impl FnOnce(&mut File, u64) -> bool,
    ) -> Result<(), StagingError> {
        leaf_checked(leaf)?;
        windows::delete_image_verified(&self.native, leaf, verify)
    }
    pub fn publish(&self, source: &str, target: &str, file: File) -> Result<PathBuf, StagingError> {
        leaf_checked(source)?;
        leaf_checked(target)?;
        file.sync_all().map_err(|_| StagingError::Full)?;
        windows::publish_temp(&self.native, source, target, file)?;
        Ok(self.native.path.join(target))
    }
}
fn leaf_checked(leaf: &str) -> Result<(), StagingError> {
    if leaf.is_empty()
        || leaf.contains(['/', '\\', ':', '\0'])
        || leaf == "."
        || leaf == ".."
        || leaf.ends_with([' ', '.'])
    {
        return Err(StagingError::UnsafeEntry);
    }
    Ok(())
}
