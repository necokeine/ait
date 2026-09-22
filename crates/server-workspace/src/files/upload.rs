use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::path::Path;

use server_ports::files::{FileError, FileUpload, UploadedFile};

#[derive(Debug)]
struct Upload {
    file: File,
    directory: Option<tempfile::TempDir>,
    metadata: UploadedFile,
    received: u64,
}

pub(super) fn begin(
    root: &Path,
    mut metadata: UploadedFile,
) -> Result<Box<dyn FileUpload>, FileError> {
    if !metadata.id.starts_with("upload_")
        || metadata.id.len() > 64
        || !metadata
            .id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
    {
        return super::fail("Invalid upload identity");
    }
    metadata.file_name = sanitize(&metadata.file_name);
    fs::create_dir_all(root)?;
    if fs::symlink_metadata(root)?.file_type().is_symlink() {
        return super::fail("Upload directory cannot be a symbolic link");
    }
    // TempDir provides cleanup on disconnect, errors, or an unfinished upload.
    let directory = tempfile::Builder::new()
        .prefix(&metadata.id)
        .tempdir_in(root)?;
    let path = directory.path().join(&metadata.file_name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)?;
    metadata.path = path.to_string_lossy().into_owned();
    Ok(Box::new(Upload {
        file,
        directory: Some(directory),
        metadata,
        received: 0,
    }))
}

impl Write for Upload {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if self.received.saturating_add(bytes.len() as u64) > self.metadata.size {
            return Err(std::io::Error::other(format!(
                "Upload exceeded declared size: expected {}, received {}.",
                self.metadata.size,
                self.received.saturating_add(bytes.len() as u64)
            )));
        }
        let written = self.file.write(bytes)?;
        self.received += written as u64;
        Ok(written)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.file.flush()
    }
}

impl FileUpload for Upload {
    fn finish(mut self: Box<Self>) -> Result<UploadedFile, FileError> {
        if self.received != self.metadata.size {
            return super::fail(format!(
                "Upload size mismatch: expected {}, received {}.",
                self.metadata.size, self.received
            ));
        }
        self.file.sync_all()?;
        if let Some(directory) = self.directory.take() {
            let _ = directory.keep();
        }
        Ok(self.metadata.clone())
    }
}

fn sanitize(value: &str) -> String {
    let value = value.rsplit('/').next().unwrap_or(value);
    let name: String = value
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | ' ' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let name = name.trim();
    if name.is_empty() || matches!(name, "." | "..") {
        "upload".to_owned()
    } else {
        name.to_owned()
    }
}
