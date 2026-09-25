//! File payload projections and bounded filesystem jobs.

use std::io::Read;

use base64::{Engine, engine::general_purpose::STANDARD};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

use crate::service::files::{self as port, Files};
use crate::{protocol::files as wire, rpc::ErrorCode};

const INLINE_LIMIT: u64 = 512 * 1024;

/// Execute a filesystem request.
///
/// # Errors
/// Rejects invalid parameters, unknown methods, or failed result encoding.
pub fn dispatch(files: &mut Files, method: &str, params: Value) -> Result<Value, ErrorCode> {
    match method {
        "directory.suggestions.request" => suggestions(files, decode(params)?),
        "fs.explorer.request" => explorer(files, decode(params)?),
        "fs.file.write.request" => write(files, decode(params)?),
        "fs.entry.create.request" => create(files, decode(params)?),
        "fs.entry.rename.request" => rename(files, decode(params)?),
        "fs.entry.duplicate.request" => duplicate(files, decode(params)?),
        "fs.entry.delete.request" => delete(files, decode(params)?),
        "fs.file.download_token.request" => token(files, decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn suggestions(files: &Files, request: wire::SuggestionsRequest) -> Result<Value, ErrorCode> {
    let limit = request.limit.unwrap_or(30);
    if !(1..=100).contains(&limit) {
        return Err(ErrorCode::InvalidMessage);
    }
    let query = port::FileSearch {
        cwd: request.cwd,
        query: request.query,
        include_files: request.include_files.unwrap_or(false),
        include_directories: request.include_directories.unwrap_or(true),
        suffix: request.match_mode == Some(wire::MatchMode::Suffix),
        limit,
    };
    let (entries, error) = match files.filesystem.search(&query) {
        Ok(entries) => (
            entries
                .into_iter()
                .map(|(path, kind)| wire::Suggestion {
                    path,
                    kind: entry_kind(kind),
                })
                .collect::<Vec<_>>(),
            None,
        ),
        Err(error) => (Vec::new(), Some(error.0)),
    };
    encode(wire::SuggestionsResult {
        directories: entries
            .iter()
            .filter(|entry| entry.kind == wire::EntryKind::Directory)
            .map(|entry| entry.path.clone())
            .collect(),
        entries,
        error,
    })
}

fn explorer(files: &Files, request: wire::ExplorerRequest) -> Result<Value, ErrorCode> {
    if request.max_bytes == Some(0) {
        return Err(ErrorCode::InvalidMessage);
    }
    let cwd = request.cwd.trim().to_owned();
    let path = request.path.unwrap_or_else(|| ".".to_owned());
    let mut result = wire::ExplorerResult {
        cwd: cwd.clone(),
        path: path.clone(),
        mode: request.mode,
        directory: None,
        file: None,
        error: None,
    };
    let operation = match request.mode {
        wire::ExplorerMode::List => files.filesystem.list(&cwd, &path).map(|(path, entries)| {
            result.path.clone_from(&path);
            result.directory = Some(wire::Directory {
                path,
                entries: entries
                    .into_iter()
                    .map(|entry| wire::FileEntry {
                        name: entry.name,
                        path: entry.path,
                        kind: entry_kind(entry.kind),
                        size: entry.size,
                        modified_at: entry.modified_at,
                    })
                    .collect(),
            });
        }),
        wire::ExplorerMode::File => preview(files, &cwd, &path, request.max_bytes).map(|file| {
            result.path.clone_from(&file.path);
            result.file = Some(file);
        }),
    };
    result.error = operation.err().map(|error| error.0);
    encode(result)
}

fn preview(
    files: &Files,
    cwd: &str,
    path: &str,
    max_bytes: Option<u64>,
) -> Result<wire::FilePreview, port::FileError> {
    let mut reader = files.filesystem.open(cwd, path)?;
    let info = reader.info().clone();
    if info.size > max_bytes.unwrap_or(INLINE_LIMIT).min(INLINE_LIMIT) {
        return Err(port::FileError("File is too large to display".to_owned()));
    }
    let mut bytes = Vec::new();
    reader
        .by_ref()
        .take(INLINE_LIMIT + 1)
        .read_to_end(&mut bytes)?;
    reader.verify()?;
    if bytes.len() as u64 != info.size {
        return Err(port::FileError("File changed during transfer".to_owned()));
    }
    let (kind, encoding, content) = match info.kind {
        port::FileKind::Image => (
            wire::FileKind::Image,
            wire::Encoding::Base64,
            Some(STANDARD.encode(bytes)),
        ),
        port::FileKind::Text if !bytes.contains(&0) => match String::from_utf8(bytes) {
            Ok(text) => (wire::FileKind::Text, wire::Encoding::Utf8, Some(text)),
            Err(_) => (wire::FileKind::Binary, wire::Encoding::None, None),
        },
        port::FileKind::Text | port::FileKind::Binary => {
            (wire::FileKind::Binary, wire::Encoding::None, None)
        }
    };
    let mime = if kind == wire::FileKind::Binary {
        "application/octet-stream".to_owned()
    } else {
        info.mime_type
    };
    Ok(wire::FilePreview {
        path: info.path,
        kind,
        encoding,
        content,
        mime_type: Some(mime),
        size: info.size,
        modified_at: info.modified_at,
        revision: Some(info.revision),
    })
}

fn write(files: &Files, request: wire::WriteRequest) -> Result<Value, ErrorCode> {
    let request = port::FileWrite {
        cwd: request.cwd,
        path: request.path,
        content: request.content,
        expected_modified_at: request.expected_modified_at,
        expected_revision: request.expected_revision,
    };
    let result = match files.filesystem.write(&request) {
        Ok(port::FileWritten::Written(info)) => wire::WriteOutcome::Written {
            modified_at: info.modified_at,
            size: info.size,
            revision: Some(info.revision),
        },
        Ok(port::FileWritten::Conflict(version)) => wire::WriteOutcome::Conflict {
            version: project_version(&request.cwd, &request.path, version),
        },
        Err(error) => wire::WriteOutcome::Error { error: error.0 },
    };
    encode(wire::WriteResult { result })
}

fn create(files: &Files, request: wire::CreateRequest) -> Result<Value, ErrorCode> {
    let kind = match request.kind {
        wire::EntryKind::File => port::EntryKind::File,
        wire::EntryKind::Directory => port::EntryKind::Directory,
    };
    let (path, error) = split(files.filesystem.create(
        &request.cwd,
        &request.parent_path,
        &request.name,
        kind,
    ));
    encode(wire::CreateResult {
        cwd: request.cwd,
        parent_path: request.parent_path,
        success: error.is_none(),
        path,
        error,
    })
}

fn rename(files: &Files, request: wire::RenameRequest) -> Result<Value, ErrorCode> {
    let (renamed_path, error) = split(files.filesystem.rename(
        &request.cwd,
        &request.path,
        &request.name,
    ));
    encode(wire::RenameResult {
        cwd: request.cwd,
        path: request.path,
        success: error.is_none(),
        renamed_path,
        error,
    })
}

fn duplicate(files: &Files, request: wire::FilePathRequest) -> Result<Value, ErrorCode> {
    let (duplicated_path, error) = split(files.filesystem.duplicate(&request.cwd, &request.path));
    encode(wire::DuplicateResult {
        cwd: request.cwd,
        path: request.path,
        success: error.is_none(),
        duplicated_path,
        error,
    })
}

fn delete(files: &Files, request: wire::FilePathRequest) -> Result<Value, ErrorCode> {
    let error = files
        .filesystem
        .delete(&request.cwd, &request.path)
        .err()
        .map(|error| error.0);
    encode(wire::DeleteResult {
        cwd: request.cwd,
        path: request.path,
        success: error.is_none(),
        error,
    })
}

fn token(files: &mut Files, request: wire::FilePathRequest) -> Result<Value, ErrorCode> {
    let token = Uuid::new_v4().to_string();
    let mut result = wire::DownloadResult {
        cwd: request.cwd.trim().to_owned(),
        path: request.path,
        token: None,
        file_name: None,
        mime_type: None,
        size: None,
        error: None,
    };
    match files.issue_download(token.clone(), &result.cwd, &result.path) {
        Ok(info) => {
            result.path = info.path;
            result.token = Some(token);
            result.file_name = Some(info.file_name);
            result.mime_type = Some(info.mime_type);
            result.size = Some(info.size);
        }
        Err(error) => result.error = Some(error.0),
    }
    encode(result)
}

/// Project a file observation into the wire shape.
#[must_use]
pub fn project_version(cwd: &str, path: &str, version: port::FileVersion) -> wire::FileVersion {
    match version {
        port::FileVersion::Ready(info) => wire::FileVersion::Ready {
            cwd: cwd.to_owned(),
            path: info.path,
            size: info.size,
            modified_at: info.modified_at,
            revision: Some(info.revision),
        },
        port::FileVersion::Missing => wire::FileVersion::Missing {
            cwd: cwd.to_owned(),
            path: path.to_owned(),
        },
        port::FileVersion::Error(error) => wire::FileVersion::Error {
            cwd: cwd.to_owned(),
            path: path.to_owned(),
            error,
        },
    }
}

fn entry_kind(kind: port::EntryKind) -> wire::EntryKind {
    match kind {
        port::EntryKind::File => wire::EntryKind::File,
        port::EntryKind::Directory => wire::EntryKind::Directory,
    }
}

fn split<T>(result: Result<T, port::FileError>) -> (Option<T>, Option<String>) {
    match result {
        Ok(value) => (Some(value), None),
        Err(error) => (None, Some(error.0)),
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::ProjectIo)
}

#[cfg(test)]
mod tests;

/// File snapshot comparison owned by a subscription; the host controls its lifetime.
#[derive(Debug)]
pub struct FileObservation {
    cwd: String,
    path: String,
    previous: port::FileVersion,
}
impl FileObservation {
    /// Capture the initial file observation used in the subscription response.
    #[must_use]
    pub fn new(cwd: String, path: String, initial: port::FileVersion) -> Self {
        Self {
            cwd,
            path,
            previous: initial,
        }
    }
    /// Project a changed snapshot, suppressing duplicate file versions.
    pub fn update(&mut self, next: port::FileVersion) -> Option<wire::FileVersion> {
        if next == self.previous {
            return None;
        }
        self.previous = next.clone();
        Some(project_version(&self.cwd, &self.path, next))
    }
}

/// Open a binary preview and prepare its metadata before streaming begins.
///
/// # Errors
/// Rejects unreadable files and files larger than the requested preview limit.
pub fn binary_preview(
    files: &Files,
    cwd: &str,
    path: &str,
    max_bytes: Option<u64>,
) -> Result<
    (
        crate::protocol::file_transfer::FileBegin,
        crate::service::transfer::Cursor,
    ),
    port::FileError,
> {
    let reader = files.filesystem.open(cwd, path)?;
    let info = reader.info();
    if max_bytes.is_some_and(|limit| info.size > limit) {
        return Err(port::FileError("File is too large to display".to_owned()));
    }
    let metadata = crate::protocol::file_transfer::FileBegin {
        mime: info.mime_type.clone(),
        size: info.size,
        encoding: if info.kind == port::FileKind::Text {
            "utf-8"
        } else {
            "binary"
        }
        .to_owned(),
        modified_at: info.modified_at.clone(),
        revision: Some(info.revision.clone()),
        file_name: None,
    };
    Ok((metadata, crate::service::transfer::Cursor::new(reader)))
}
