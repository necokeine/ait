//! Paseo workspace filesystem request and response payloads.

use serde::{Deserialize, Serialize};

/// Implemented canonical filesystem methods.
pub const CAPABILITIES: &[&str] = &[
    "directory.suggestions.request",
    "fs.explorer.request",
    "fs.file.subscribe.request",
    "fs.file.unsubscribe.request",
    "fs.file.write.request",
    "fs.entry.create.request",
    "fs.entry.rename.request",
    "fs.entry.duplicate.request",
    "fs.entry.delete.request",
    "fs.file.download_token.request",
    "file.upload.request",
];

/// Entry Kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EntryKind {
    /// File value.
    File,
    /// Directory value.
    Directory,
}

/// Explorer Mode discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExplorerMode {
    /// List value.
    List,
    /// File value.
    File,
}

/// Match Mode discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchMode {
    /// Fuzzy value.
    Fuzzy,
    /// Suffix value.
    Suffix,
}

/// File Kind discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    /// Text value.
    Text,
    /// Image value.
    Image,
    /// Binary value.
    Binary,
}

/// Encoding discriminator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Encoding {
    /// Utf8 value.
    #[serde(rename = "utf-8")]
    Utf8,
    /// Base64 value.
    Base64,
    /// None value.
    None,
}

/// Suggestions Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionsRequest {
    /// query.
    pub query: String,
    /// cwd.
    pub cwd: Option<String>,
    /// include files.
    pub include_files: Option<bool>,
    /// include directories.
    pub include_directories: Option<bool>,
    /// match mode.
    pub match_mode: Option<MatchMode>,
    /// limit.
    pub limit: Option<usize>,
}

/// Explorer Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerRequest {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: Option<String>,
    /// mode.
    pub mode: ExplorerMode,
    /// accept binary.
    pub accept_binary: Option<bool>,
    /// max bytes.
    pub max_bytes: Option<u64>,
}

/// File Path Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePathRequest {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
}

/// Subscribe Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// subscription id.
    pub subscription_id: Option<String>,
}

/// Unsubscribe Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UnsubscribeRequest {
    /// subscription id.
    pub subscription_id: String,
}

/// Write Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteRequest {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// content.
    pub content: String,
    /// expected modified at.
    pub expected_modified_at: String,
    /// expected revision.
    pub expected_revision: Option<String>,
}

/// Create Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    /// cwd.
    pub cwd: String,
    /// parent path.
    pub parent_path: String,
    /// name.
    pub name: String,
    /// kind.
    pub kind: EntryKind,
}

/// Rename Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameRequest {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// name.
    pub name: String,
}

/// Upload Request payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadRequest {
    /// file name.
    pub file_name: String,
    /// mime type.
    pub mime_type: String,
    /// size.
    pub size: u64,
    /// modified at.
    pub modified_at: String,
}

/// File Entry payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    /// name.
    pub name: String,
    /// path.
    pub path: String,
    /// kind.
    pub kind: EntryKind,
    /// size.
    pub size: u64,
    /// modified at.
    pub modified_at: String,
}

/// Directory payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Directory {
    /// path.
    pub path: String,
    /// entries.
    pub entries: Vec<FileEntry>,
}

/// File Preview payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FilePreview {
    /// path.
    pub path: String,
    /// kind.
    pub kind: FileKind,
    /// encoding.
    pub encoding: Encoding,
    /// content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    /// mime type.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mime_type: Option<String>,
    /// size.
    pub size: u64,
    /// modified at.
    pub modified_at: String,
    /// revision.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<String>,
}

/// Explorer Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExplorerResult {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// mode.
    pub mode: ExplorerMode,
    /// directory.
    pub directory: Option<Directory>,
    /// file.
    pub file: Option<FilePreview>,
    /// error.
    pub error: Option<String>,
}

/// Suggestion payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Suggestion {
    /// path.
    pub path: String,
    /// kind.
    pub kind: EntryKind,
}

/// Suggestions Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SuggestionsResult {
    /// directories.
    pub directories: Vec<String>,
    /// entries.
    pub entries: Vec<Suggestion>,
    /// error.
    pub error: Option<String>,
}

/// Subscribe Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeResult {
    /// subscription id.
    pub subscription_id: String,
    /// initial.
    pub initial: FileVersion,
}

/// File Update payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileUpdate {
    /// subscription id.
    pub subscription_id: String,
    /// version.
    pub version: FileVersion,
}

/// Write Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WriteResult {
    /// result.
    pub result: WriteOutcome,
}

/// Create Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateResult {
    /// cwd.
    pub cwd: String,
    /// parent path.
    pub parent_path: String,
    /// path.
    pub path: Option<String>,
    /// success.
    pub success: bool,
    /// error.
    pub error: Option<String>,
}

/// Rename Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameResult {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// renamed path.
    pub renamed_path: Option<String>,
    /// success.
    pub success: bool,
    /// error.
    pub error: Option<String>,
}

/// Duplicate Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DuplicateResult {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// duplicated path.
    pub duplicated_path: Option<String>,
    /// success.
    pub success: bool,
    /// error.
    pub error: Option<String>,
}

/// Delete Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DeleteResult {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// success.
    pub success: bool,
    /// error.
    pub error: Option<String>,
}

/// Download Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadResult {
    /// cwd.
    pub cwd: String,
    /// path.
    pub path: String,
    /// token.
    pub token: Option<String>,
    /// file name.
    pub file_name: Option<String>,
    /// mime type.
    pub mime_type: Option<String>,
    /// size.
    pub size: Option<u64>,
    /// error.
    pub error: Option<String>,
}

/// Uploaded File payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadedFile {
    /// id.
    pub id: String,
    /// file name.
    pub file_name: String,
    /// mime type.
    pub mime_type: String,
    /// size.
    pub size: u64,
    /// path.
    pub path: String,
}

/// Upload Result payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UploadResult {
    /// file.
    pub file: Option<UploadedAttachment>,
    /// error.
    pub error: Option<String>,
}

/// Uploaded attachment discriminator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UploadedAttachment {
    /// File retained in the server upload directory.
    UploadedFile(UploadedFile),
}

/// File version copied from Paseo's discriminated union.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum FileVersion {
    /// Current metadata.
    Ready {
        /// Workspace root.
        cwd: String,
        /// Relative path.
        path: String,
        /// Byte count.
        size: u64,
        /// ISO modification time.
        modified_at: String,
        /// High precision disk identity.
        #[serde(skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    /// The file has disappeared.
    Missing {
        /// Workspace root.
        cwd: String,
        /// Relative path.
        path: String,
    },
    /// Inspection failure.
    Error {
        /// Workspace root.
        cwd: String,
        /// Relative path.
        path: String,
        /// Diagnostic.
        error: String,
    },
}

/// Atomic edit outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "status",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub enum WriteOutcome {
    /// Successfully persisted edit.
    Written {
        /// Modification timestamp.
        modified_at: String,
        /// Byte count.
        size: u64,
        /// Disk identity.
        #[serde(skip_serializing_if = "Option::is_none")]
        revision: Option<String>,
    },
    /// The expected revision is no longer current.
    Conflict {
        /// Current disk state.
        version: FileVersion,
    },
    /// The edit was rejected.
    Error {
        /// Diagnostic.
        error: String,
    },
}

#[cfg(test)]
mod tests;
