//! Scoped filesystem adapter for the independent server.

use std::fs::{self, File, Metadata, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use server_ports::files::{
    EntryKind, FileEntry, FileError, FileInfo, FileKind, FileReader, FileSearch, FileSystem,
    FileUpload, FileVersion, FileWrite, FileWritten, UploadedFile,
};

mod search;
mod upload;

const MAX_EDITABLE: u64 = 1024 * 1024;
const OUTSIDE: &str = "Access outside of workspace is not allowed";

/// Local filesystem access with explicit home and upload roots.
#[derive(Debug)]
pub struct LocalFiles {
    home: PathBuf,
    uploads: PathBuf,
}

impl LocalFiles {
    /// Use the supplied home for tilde expansion and data directory for uploads.
    #[must_use]
    pub fn new(home: PathBuf, data_directory: &Path) -> Self {
        Self {
            home,
            uploads: data_directory.join("uploads"),
        }
    }

    fn expand(&self, path: &str) -> PathBuf {
        if path == "~" {
            self.home.clone()
        } else if let Some(rest) = path.strip_prefix("~/") {
            self.home.join(rest)
        } else {
            PathBuf::from(path)
        }
    }

    fn scoped(&self, cwd: &str, path: &str) -> Result<Scoped, FileError> {
        if cwd.trim().is_empty() {
            return fail("cwd is required");
        }
        let root = absolute(self.expand(cwd))?;
        let requested = self.expand(path);
        let requested = normalize(&if requested.is_absolute() {
            requested
        } else {
            root.join(requested)
        });
        if !requested.starts_with(&root) {
            return fail(OUTSIDE);
        }
        let real_root = fs::canonicalize(&root)?;
        let resolved = resolve_missing(&requested)?;
        if !resolved.starts_with(&real_root) {
            return fail(OUTSIDE);
        }
        Ok(Scoped {
            root: real_root,
            relative: relative(&root, &requested)?,
            requested,
            resolved,
        })
    }

    fn open_local(&self, cwd: &str, path: &str) -> Result<LocalReader, FileError> {
        let scoped = self.scoped(cwd, path)?;
        let mut file = open_regular(&scoped.resolved)?;
        let stats = file.metadata()?;
        let mut info = metadata_info(&scoped, &stats);
        let extension = scoped
            .resolved
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_lowercase();
        if let Some(mime) = image_mime(&extension) {
            info.kind = FileKind::Image;
            mime.clone_into(&mut info.mime_type);
        } else if file_is_binary(&mut file, stats.len())? {
            info.kind = FileKind::Binary;
            "application/octet-stream".clone_into(&mut info.mime_type);
        } else if extension == "json" {
            "application/json".clone_into(&mut info.mime_type);
        }
        file.seek(SeekFrom::Start(0))?;
        let reader = LocalReader { file, info };
        reader.verify()?;
        Ok(reader)
    }
}

impl FileSystem for LocalFiles {
    fn list(&self, cwd: &str, path: &str) -> Result<(String, Vec<FileEntry>), FileError> {
        let scoped = self.scoped(cwd, path)?;
        if !scoped.resolved.is_dir() {
            return fail("Requested path is not a directory");
        }
        let mut entries = Vec::new();
        for entry in fs::read_dir(&scoped.requested)? {
            let entry = entry?;
            if entries.len() >= 20_000 {
                return fail("Directory contains too many entries");
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            let child = self.scoped(cwd, &format!("{}/{}", scoped.relative, name));
            let Ok(child) = child else {
                continue;
            };
            let stats = match fs::metadata(&child.resolved) {
                Ok(stats) => stats,
                Err(error) if missing(&error) => continue,
                Err(error) => return Err(error.into()),
            };
            entries.push(FileEntry {
                name,
                path: child.relative,
                kind: if entry.file_type()?.is_dir() {
                    EntryKind::Directory
                } else {
                    EntryKind::File
                },
                size: stats.len(),
                modified_at: modified(&stats),
            });
        }
        entries.sort_by(|a, b| {
            b.modified_at
                .cmp(&a.modified_at)
                .then_with(|| a.name.cmp(&b.name))
        });
        Ok((scoped.relative, entries))
    }

    fn open(&self, cwd: &str, path: &str) -> Result<Box<dyn FileReader>, FileError> {
        Ok(Box::new(self.open_local(cwd, path)?))
    }

    fn version(&self, cwd: &str, path: &str) -> FileVersion {
        let scoped = match self.scoped(cwd, path) {
            Ok(scoped) => scoped,
            Err(error) => return FileVersion::Error(error.0),
        };
        match fs::metadata(&scoped.resolved) {
            Ok(stats) if stats.is_file() => FileVersion::Ready(metadata_info(&scoped, &stats)),
            Ok(_) => FileVersion::Error("Requested path is not a file".to_owned()),
            Err(error) if missing(&error) => FileVersion::Missing,
            Err(error) => FileVersion::Error(error.to_string()),
        }
    }

    fn write(&self, request: &FileWrite) -> Result<FileWritten, FileError> {
        if request.content.len() as u64 > MAX_EDITABLE {
            return fail("File is too large to edit");
        }
        let scoped = self.scoped(&request.cwd, &request.path)?;
        let mut reader = match self.open_local(&request.cwd, &request.path) {
            Ok(reader) => reader,
            Err(error) => {
                return match self.version(&request.cwd, &request.path) {
                    FileVersion::Missing => Ok(FileWritten::Conflict(FileVersion::Missing)),
                    _ => Err(error),
                };
            }
        };
        if reader.info.size > MAX_EDITABLE {
            return fail("File is too large to edit");
        }
        let mut bytes = Vec::new();
        reader
            .by_ref()
            .take(MAX_EDITABLE + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_EDITABLE {
            return fail("File is too large to edit");
        }
        if binary(&bytes) || std::str::from_utf8(&bytes).is_err() {
            return fail("Binary files cannot be edited");
        }
        if !matches_expected(&reader.info, request) {
            return Ok(FileWritten::Conflict(FileVersion::Ready(reader.info)));
        }
        let parent = scoped
            .resolved
            .parent()
            .ok_or_else(|| FileError(OUTSIDE.to_owned()))?;
        let mut temporary = tempfile::NamedTempFile::new_in(parent)?;
        temporary
            .as_file()
            .set_permissions(reader.file.metadata()?.permissions())?;
        temporary.write_all(request.content.as_bytes())?;
        temporary.as_file().sync_all()?;
        let latest = self.scoped(&request.cwd, &request.path)?;
        if latest.resolved != scoped.resolved {
            return fail("File target changed during write");
        }
        let version = self.version(&request.cwd, &request.path);
        if !matches!(&version, FileVersion::Ready(info) if matches_expected(info, request)) {
            return Ok(FileWritten::Conflict(version));
        }
        temporary
            .persist(&scoped.resolved)
            .map_err(|error| FileError(error.error.to_string()))?;
        let stats = fs::metadata(&scoped.resolved)?;
        Ok(FileWritten::Written(metadata_info(&scoped, &stats)))
    }

    fn create(
        &self,
        cwd: &str,
        parent: &str,
        name: &str,
        kind: EntryKind,
    ) -> Result<String, FileError> {
        let name = valid_name(name)?;
        let parent = self.scoped(cwd, parent)?;
        if !parent.resolved.is_dir() {
            return fail("Parent path is not a directory");
        }
        let target = parent.resolved.join(name);
        let result = match kind {
            EntryKind::Directory => fs::create_dir(&target),
            EntryKind::File => OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&target)
                .map(drop),
        };
        result.map_err(|error| collision(error, name))?;
        Ok(join_relative(&parent.relative, name))
    }

    fn rename(&self, cwd: &str, path: &str, name: &str) -> Result<String, FileError> {
        let name = valid_name(name)?;
        let source = self.scoped(cwd, path)?;
        protect_root(&source, "rename")?;
        let stats = fs::symlink_metadata(&source.requested).map_err(entry_error)?;
        let target = source.requested.with_file_name(name);
        if let Ok(target_stats) = fs::symlink_metadata(&target)
            && !same_case_entry(&source.requested, &target, &stats, &target_stats)
        {
            return fail(format!("\"{name}\" already exists"));
        }
        let target_relative = source
            .relative
            .rsplit_once('/')
            .map_or_else(|| name.to_owned(), |(parent, _)| format!("{parent}/{name}"));
        if source.requested == target {
            return Ok(target_relative);
        }
        // Git stages the rename of tracked entries, matching Paseo's context action.
        let tracked = crate::git::run(
            &source.root,
            &["ls-files", "--error-unmatch", "--", &source.relative],
        )
        .is_ok();
        if tracked {
            crate::git::run(
                &source.root,
                &["mv", "--", &source.relative, &target_relative],
            )
            .map_err(|_| FileError("Git rename failed".to_owned()))?;
        } else {
            fs::rename(&source.requested, target).map_err(entry_error)?;
        }
        Ok(target_relative)
    }

    fn duplicate(&self, cwd: &str, path: &str) -> Result<String, FileError> {
        let source = self.scoped(cwd, path)?;
        protect_root(&source, "duplicate")?;
        let stats = fs::symlink_metadata(&source.requested).map_err(entry_error)?;
        let name = source
            .requested
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| FileError("Invalid name".to_owned()))?;
        let extension = if stats.is_dir() {
            None
        } else {
            Path::new(name).extension().and_then(|s| s.to_str())
        };
        let stem = extension.map_or(name, |ext| &name[..name.len() - ext.len() - 1]);
        for number in 1..=20_000 {
            let suffix = if number == 1 {
                " copy".to_owned()
            } else {
                format!(" copy {number}")
            };
            let candidate = format!(
                "{stem}{suffix}{}",
                extension.map_or_else(String::new, |ext| format!(".{ext}"))
            );
            let target = source.requested.with_file_name(&candidate);
            if fs::symlink_metadata(&target).is_ok() {
                continue;
            }
            copy_entry(&source.requested, &target, &mut CopyBudget::default(), 0)?;
            return Ok(source
                .relative
                .rsplit_once('/')
                .map_or(candidate.clone(), |(parent, _)| {
                    format!("{parent}/{candidate}")
                }));
        }
        fail("No available copy name")
    }

    fn delete(&self, cwd: &str, path: &str) -> Result<(), FileError> {
        let scoped = self.scoped(cwd, path)?;
        protect_root(&scoped, "delete")?;
        let stats = fs::symlink_metadata(&scoped.requested).map_err(entry_error)?;
        if stats.is_dir() {
            fs::remove_dir_all(&scoped.requested)
        } else {
            fs::remove_file(&scoped.requested)
        }
        .map_err(entry_error)
    }

    fn search(&self, request: &FileSearch) -> Result<Vec<(String, EntryKind)>, FileError> {
        search::search(self, request)
    }

    fn upload(&self, metadata: UploadedFile) -> Result<Box<dyn FileUpload>, FileError> {
        upload::begin(&self.uploads, metadata)
    }
}

#[derive(Debug)]
struct Scoped {
    root: PathBuf,
    requested: PathBuf,
    resolved: PathBuf,
    relative: String,
}

#[derive(Debug)]
struct LocalReader {
    file: File,
    info: FileInfo,
}

impl Read for LocalReader {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        self.file.read(buffer)
    }
}

impl FileReader for LocalReader {
    fn info(&self) -> &FileInfo {
        &self.info
    }
    fn verify(&self) -> Result<(), FileError> {
        if revision(&self.file.metadata()?) != self.info.revision {
            return fail("File changed during transfer");
        }
        Ok(())
    }
}

fn open_regular(path: &Path) -> Result<File, FileError> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return fail("Requested path is not a file");
    }
    Ok(file)
}

fn metadata_info(scoped: &Scoped, stats: &Metadata) -> FileInfo {
    FileInfo {
        root: scoped.root.to_string_lossy().into_owned(),
        absolute_path: scoped.resolved.to_string_lossy().into_owned(),
        path: scoped.relative.clone(),
        file_name: scoped
            .requested
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        mime_type: "text/plain".to_owned(),
        kind: FileKind::Text,
        size: stats.len(),
        modified_at: modified(stats),
        revision: revision(stats),
    }
}

fn modified(stats: &Metadata) -> String {
    DateTime::<Utc>::from(stats.modified().unwrap_or(std::time::UNIX_EPOCH))
        .to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn revision(stats: &Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!(
            "{}:{}:{}:{}",
            stats.dev(),
            stats.ino(),
            stats.len(),
            i128::from(stats.mtime()) * 1_000_000_000 + i128::from(stats.mtime_nsec())
        )
    }
    #[cfg(not(unix))]
    {
        format!(
            "{}:{:?}:{:?}",
            stats.len(),
            stats.modified(),
            stats.created()
        )
    }
}

fn matches_expected(info: &FileInfo, request: &FileWrite) -> bool {
    match request
        .expected_revision
        .as_deref()
        .filter(|revision| !revision.is_empty())
    {
        Some(revision) => info.revision == revision,
        None => info.modified_at == request.expected_modified_at,
    }
}

fn binary(bytes: &[u8]) -> bool {
    bytes.contains(&0)
        || (!bytes.is_empty()
            && bytes
                .iter()
                .filter(|&&b| (b < 32 && !matches!(b, 9 | 10 | 13)) || b == 127)
                .count()
                * 10
                > bytes.len() * 3)
}

fn file_is_binary(file: &mut File, size: u64) -> Result<bool, FileError> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let mut remaining = size;
    let mut suspicious = 0_u64;
    let mut carry = Vec::with_capacity(3);
    let mut block = vec![0; 256 * 1024 + 3];
    while remaining > 0 {
        if std::time::Instant::now() >= deadline {
            return fail("File classification exceeded its time limit");
        }
        let length = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(256 * 1024);
        let prefix = carry.len();
        block[..prefix].copy_from_slice(&carry);
        file.read_exact(&mut block[prefix..prefix + length])?;
        let bytes = &block[prefix..prefix + length];
        if bytes.contains(&0) {
            return Ok(true);
        }
        suspicious += bytes
            .iter()
            .filter(|&&b| (b < 32 && !matches!(b, 9 | 10 | 13)) || b == 127)
            .count() as u64;
        let complete = &block[..prefix + length];
        carry.clear();
        if let Err(error) = std::str::from_utf8(complete) {
            if error.error_len().is_some() {
                return Ok(true);
            }
            carry.extend_from_slice(&complete[error.valid_up_to()..]);
        }
        remaining -= length as u64;
    }
    Ok(!carry.is_empty() || u128::from(suspicious) * 10 > u128::from(size) * 3)
}

fn image_mime(extension: &str) -> Option<&'static str> {
    match extension {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "gif" => Some("image/gif"),
        "webp" => Some("image/webp"),
        "svg" => Some("image/svg+xml"),
        _ => None,
    }
}

fn normalize(path: &Path) -> PathBuf {
    let mut result = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    result
}

fn absolute(path: PathBuf) -> Result<PathBuf, FileError> {
    Ok(normalize(&if path.is_absolute() {
        path
    } else {
        std::env::current_dir()?.join(path)
    }))
}

fn resolve_missing(path: &Path) -> Result<PathBuf, FileError> {
    match fs::canonicalize(path) {
        Ok(path) => Ok(path),
        Err(error) if missing(&error) => {
            // Validate existing ancestors even when the final file does not exist.
            if fs::symlink_metadata(path).is_ok_and(|meta| meta.file_type().is_symlink()) {
                return Err(error.into());
            }
            let parent = path.parent().ok_or_else(|| FileError(OUTSIDE.to_owned()))?;
            let name = path
                .file_name()
                .ok_or_else(|| FileError(OUTSIDE.to_owned()))?;
            Ok(resolve_missing(parent)?.join(name))
        }
        Err(error) => Err(error.into()),
    }
}

fn relative(root: &Path, path: &Path) -> Result<String, FileError> {
    let path = path
        .strip_prefix(root)
        .map_err(|_| FileError(OUTSIDE.to_owned()))?;
    Ok(if path.as_os_str().is_empty() {
        ".".to_owned()
    } else {
        path.to_string_lossy().replace('\\', "/")
    })
}

fn join_relative(parent: &str, name: &str) -> String {
    if parent == "." {
        name.to_owned()
    } else {
        format!("{parent}/{name}")
    }
}

fn valid_name(name: &str) -> Result<&str, FileError> {
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") || name.contains('\0') {
        return fail("Invalid name");
    }
    if name.contains(['/', '\\']) {
        return fail("Name cannot contain path separators");
    }
    Ok(name)
}

fn protect_root(scoped: &Scoped, verb: &str) -> Result<(), FileError> {
    if scoped.resolved == scoped.root {
        return fail(format!("Cannot {verb} the workspace root"));
    }
    Ok(())
}

fn missing(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::NotFound | std::io::ErrorKind::NotADirectory
    )
}

fn collision(error: std::io::Error, name: &str) -> FileError {
    if error.kind() == std::io::ErrorKind::AlreadyExists {
        FileError(format!("\"{name}\" already exists"))
    } else {
        error.into()
    }
}

fn entry_error(error: std::io::Error) -> FileError {
    if missing(&error) {
        FileError("File or folder no longer exists".to_owned())
    } else {
        error.into()
    }
}

fn same_case_entry(source: &Path, target: &Path, left: &Metadata, right: &Metadata) -> bool {
    if source.to_string_lossy().to_lowercase() != target.to_string_lossy().to_lowercase() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        left.dev() == right.dev() && left.ino() == right.ino()
    }
    #[cfg(not(unix))]
    {
        let _ = (left, right);
        fs::canonicalize(source).ok() == fs::canonicalize(target).ok()
    }
}

#[derive(Default)]
struct CopyBudget {
    entries: usize,
    bytes: u64,
}

fn copy_entry(
    source: &Path,
    target: &Path,
    budget: &mut CopyBudget,
    depth: usize,
) -> Result<(), FileError> {
    budget.entries += 1;
    let metadata = fs::symlink_metadata(source)?;
    budget.bytes = budget.bytes.saturating_add(metadata.len());
    if depth > 64 || budget.entries > 20_000 || budget.bytes > 64 * 1024 * 1024 {
        return fail("Copy exceeds filesystem operation limit");
    }
    if metadata.file_type().is_symlink() {
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(fs::read_link(source)?, target)?;
            return Ok(());
        }
        #[cfg(not(unix))]
        {
            return fail("Copying symbolic links is unavailable on this platform");
        }
    }
    if metadata.is_dir() {
        fs::create_dir(target)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_entry(
                &entry.path(),
                &target.join(entry.file_name()),
                budget,
                depth + 1,
            )?;
        }
        fs::set_permissions(target, metadata.permissions())?;
    } else if metadata.is_file() {
        let mut input = open_regular(source)?;
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(target)?;
        let copied = std::io::copy(
            &mut Read::by_ref(&mut input).take(metadata.len() + 1),
            &mut output,
        )?;
        if copied != metadata.len() || revision(&input.metadata()?) != revision(&metadata) {
            return fail("File changed during copy");
        }
        output.set_permissions(metadata.permissions())?;
        if let Ok(modified) = metadata.modified() {
            output.set_times(fs::FileTimes::new().set_modified(modified))?;
        }
    } else {
        return fail("Special files cannot be copied");
    }
    Ok(())
}

fn fail<T>(message: impl Into<String>) -> Result<T, FileError> {
    Err(FileError(message.into()))
}

#[cfg(test)]
mod tests;
