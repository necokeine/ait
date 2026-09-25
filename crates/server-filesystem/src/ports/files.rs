//! Workspace file access and streaming ports.

use std::fmt::Debug;
use std::io::{Read, Write};

/// File access failure with a client-facing diagnostic.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct FileError(pub String);

impl From<std::io::Error> for FileError {
    fn from(error: std::io::Error) -> Self {
        Self(error.to_string())
    }
}

/// Directory entry category.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryKind {
    /// Regular file or an in-scope symbolic link.
    File,
    /// Directory.
    Directory,
}

/// One directory entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    /// Display name.
    pub name: String,
    /// Workspace-relative path.
    pub path: String,
    /// Entry category.
    pub kind: EntryKind,
    /// Size in bytes.
    pub size: u64,
    /// ISO timestamp with millisecond precision.
    pub modified_at: String,
}

/// File metadata and opaque disk revision.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileInfo {
    /// Canonical scope captured while the file was opened.
    pub root: String,
    /// Canonical target captured while the file was opened.
    pub absolute_path: String,
    /// Workspace-relative path.
    pub path: String,
    /// Download name.
    pub file_name: String,
    /// Content type.
    pub mime_type: String,
    /// Classification.
    pub kind: FileKind,
    /// Size in bytes.
    pub size: u64,
    /// ISO modification timestamp.
    pub modified_at: String,
    /// High precision identity and modification token.
    pub revision: String,
}

/// Preview classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// UTF-8 text.
    Text,
    /// Recognized image extension.
    Image,
    /// Arbitrary binary data.
    Binary,
}

/// Observable file version, preserving missing-file subscriptions.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileVersion {
    /// Current file metadata.
    Ready(FileInfo),
    /// The file no longer exists.
    Missing,
    /// The path cannot be inspected.
    Error(String),
}

/// Optimistic text write.
#[derive(Debug, Clone)]
pub struct FileWrite {
    /// Root directory.
    pub cwd: String,
    /// Scoped path.
    pub path: String,
    /// Replacement UTF-8 text.
    pub content: String,
    /// Legacy timestamp guard.
    pub expected_modified_at: String,
    /// Preferred high precision guard.
    pub expected_revision: Option<String>,
}

/// Result of an optimistic write.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileWritten {
    /// Atomically replaced and synced file.
    Written(FileInfo),
    /// Existing content was preserved.
    Conflict(FileVersion),
}

/// Directory suggestion options.
#[derive(Debug, Clone)]
pub struct FileSearch {
    /// Optional workspace root; absent selects the home directory.
    pub cwd: Option<String>,
    /// User query.
    pub query: String,
    /// Include files.
    pub include_files: bool,
    /// Include directories.
    pub include_directories: bool,
    /// Match an exact path suffix rather than a fuzzy subsequence.
    pub suffix: bool,
    /// Maximum number of results.
    pub limit: usize,
}

/// Reader bound to one open regular file and its advertised revision.
pub trait FileReader: Debug + Read + Send {
    /// Metadata captured before the transfer starts.
    fn info(&self) -> &FileInfo;
    /// Verify that the open file still has the advertised revision.
    ///
    /// # Errors
    /// Returns an error when a file changes during transfer.
    fn verify(&self) -> Result<(), FileError>;
}

/// Completed upload metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UploadedFile {
    /// Server-generated upload identity.
    pub id: String,
    /// Sanitized file name.
    pub file_name: String,
    /// Client-declared MIME type.
    pub mime_type: String,
    /// Byte count.
    pub size: u64,
    /// Absolute path of the retained upload.
    pub path: String,
}

/// Upload writer whose incomplete directory is removed on drop.
pub trait FileUpload: Debug + Write + Send {
    /// Sync and retain a completed upload of the declared size.
    ///
    /// # Errors
    /// Returns an error on mismatched length or persistence failure.
    fn finish(self: Box<Self>) -> Result<UploadedFile, FileError>;
}

/// Blocking local filesystem operations, independent of WebSocket and HTTP.
pub trait FileSystem: Debug + Send {
    /// List scoped children, newest first.
    ///
    /// # Errors
    /// Rejects outside paths and unavailable directories.
    fn list(&self, cwd: &str, path: &str) -> Result<(String, Vec<FileEntry>), FileError>;
    /// Open a scoped regular file with bounded classification reads.
    ///
    /// # Errors
    /// Rejects unavailable, special, and outside files.
    fn open(&self, cwd: &str, path: &str) -> Result<Box<dyn FileReader>, FileError>;
    /// Read the observable file version without loading content.
    fn version(&self, cwd: &str, path: &str) -> FileVersion;
    /// Optimistically replace a text file.
    ///
    /// # Errors
    /// Rejects binary, oversized, outside, or unwritable files.
    fn write(&self, request: &FileWrite) -> Result<FileWritten, FileError>;
    /// Exclusively create a single child.
    ///
    /// # Errors
    /// Rejects invalid names, collisions, and outside paths.
    fn create(
        &self,
        cwd: &str,
        parent: &str,
        name: &str,
        kind: EntryKind,
    ) -> Result<String, FileError>;
    /// Rename one entry, using Git for tracked paths.
    ///
    /// # Errors
    /// Rejects collisions, root mutation, and invalid paths.
    fn rename(&self, cwd: &str, path: &str, name: &str) -> Result<String, FileError>;
    /// Duplicate an entry under a collision-free sibling name.
    ///
    /// # Errors
    /// Rejects root mutation, invalid paths, and copy failures.
    fn duplicate(&self, cwd: &str, path: &str) -> Result<String, FileError>;
    /// Remove an entry without following its final symbolic link.
    ///
    /// # Errors
    /// Rejects root mutation, invalid paths, and deletion failures.
    fn delete(&self, cwd: &str, path: &str) -> Result<(), FileError>;
    /// Search a bounded directory tree.
    ///
    /// # Errors
    /// Returns directory or query failures.
    fn search(&self, request: &FileSearch) -> Result<Vec<(String, EntryKind)>, FileError>;
    /// Begin a connection-owned upload in the server data directory.
    ///
    /// # Errors
    /// Rejects invalid identity or storage failures.
    fn upload(&self, metadata: UploadedFile) -> Result<Box<dyn FileUpload>, FileError>;
}
