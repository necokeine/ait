//! Bounded upload state owned by one physical connection.
use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use uuid::Uuid;

use crate::ports::files::{FileError, FileUpload, UploadedFile};
use crate::protocol::{file_transfer::FileFrame, files as wire};
use crate::rpc::ErrorCode;
use crate::service::files::Files;

/// Connection-local pending uploads. Dropping it discards partial writers.
#[derive(Default)]
pub struct Uploads {
    uploads: BTreeMap<String, Upload>,
}
/// One in-flight upload removed from the connection while a blocking job runs.
pub struct Upload {
    metadata: UploadedFile,
    writer: Option<Box<dyn FileUpload>>,
    touched: Instant,
}

impl Uploads {
    /// Expire idle uploads and release their partial files.
    pub fn prune(&mut self) {
        self.uploads
            .retain(|_, upload| upload.touched.elapsed() < Duration::from_secs(600));
    }
    /// Take a connection-owned upload for processing; unknown IDs return none.
    pub fn take(&mut self, id: &str) -> Option<Upload> {
        self.prune();
        let mut upload = self.uploads.remove(id)?;
        upload.touched = Instant::now();
        Some(upload)
    }
    /// Retain an unfinished upload after its blocking job completes.
    pub fn resume(&mut self, id: String, upload: Upload) {
        self.uploads.insert(id, upload);
    }
    /// Validate and reserve one upload; repeated IDs replace their prior partial upload.
    ///
    /// # Errors
    /// Rejects malformed metadata or exhausted per-connection limits.
    pub fn begin(&mut self, id: &str, params: Value) -> Result<(), ErrorCode> {
        let request: wire::UploadRequest =
            serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
        self.prune();
        if request.file_name.is_empty()
            || request.mime_type.is_empty()
            || request.file_name.len() > 255
        {
            return Err(ErrorCode::InvalidMessage);
        }
        if request.size > 64 * 1024 * 1024
            || (self.uploads.len() >= 8 && !self.uploads.contains_key(id))
        {
            return Err(ErrorCode::ResourceExhausted);
        }
        self.uploads.insert(
            id.to_owned(),
            Upload {
                metadata: UploadedFile {
                    id: format!("upload_{}", Uuid::new_v4()),
                    file_name: request.file_name,
                    mime_type: request.mime_type,
                    size: request.size,
                    path: String::new(),
                },
                writer: None,
                touched: Instant::now(),
            },
        );
        Ok(())
    }
}
/// Next upload state after applying a binary frame.
pub enum UploadStep {
    /// More frames are required.
    Pending(Upload),
    /// File has been finalized.
    Complete(UploadedFile),
}

impl Upload {
    /// Apply one ordered frame and finalize only after a matching end frame.
    ///
    /// # Errors
    /// Rejects invalid frame order, size, and filesystem failures.
    pub fn apply(mut self, frame: FileFrame, files: &Files) -> Result<UploadStep, FileError> {
        use std::io::Write;
        match frame {
            FileFrame::Begin(_) => {
                if self.writer.is_some() {
                    return Err(FileError("Upload already started".to_owned()));
                }
                self.writer = Some(files.filesystem.upload(self.metadata.clone())?);
            }
            FileFrame::Chunk(bytes) => {
                let writer = self.writer.as_mut().ok_or_else(|| {
                    FileError("Upload chunks arrived before file begin.".to_owned())
                })?;
                writer.write_all(&bytes)?;
            }
            FileFrame::End => {
                let writer = self
                    .writer
                    .take()
                    .ok_or_else(|| FileError("Upload ended before file begin.".to_owned()))?;
                return Ok(UploadStep::Complete(writer.finish()?));
            }
        }
        Ok(UploadStep::Pending(self))
    }
}
#[cfg(test)]
mod tests;
