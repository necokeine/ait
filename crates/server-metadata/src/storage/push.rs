//! Private atomic JSON persistence for push tokens.

use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;

use serde_json::Value;

use crate::service::push::{PushError, TokenStore};

/// File adapter owned by the server's exclusively leased data directory.
#[derive(Debug)]
pub struct FileTokenStore {
    path: PathBuf,
}

impl FileTokenStore {
    /// Select the private token file; no I/O is performed until load/save.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }

    fn check_file(&self) -> Result<(), PushError> {
        match fs::symlink_metadata(&self.path) {
            Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                Err(PushError::Invalid)
            }
            Ok(_) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(PushError::Io),
        }
    }
}

impl TokenStore for FileTokenStore {
    fn load(&self) -> Result<Value, PushError> {
        self.check_file()?;
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(serde_json::json!({}));
            }
            Err(_) => return Err(PushError::Io),
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(fs::Permissions::from_mode(0o600))
                .map_err(|_| PushError::Io)?;
        }
        let mut bytes = Vec::new();
        file.take(32 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| PushError::Io)?;
        if bytes.len() > 32 * 1024 * 1024 {
            return Err(PushError::Capacity);
        }
        serde_json::from_slice(&bytes).map_err(|_| PushError::Invalid)
    }

    fn save(&self, document: &Value) -> Result<(), PushError> {
        self.check_file()?;
        let bytes = serde_json::to_vec_pretty(document).map_err(|_| PushError::Invalid)?;
        if bytes.len() >= 32 * 1024 * 1024 {
            return Err(PushError::Capacity);
        }
        let parent = self
            .path
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
            .ok_or(PushError::Invalid)?;
        fs::create_dir_all(parent).map_err(|_| PushError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))
                .map_err(|_| PushError::Io)?;
        }
        let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| PushError::Io)?;
        temporary.write_all(&bytes).map_err(|_| PushError::Io)?;
        temporary.write_all(b"\n").map_err(|_| PushError::Io)?;
        temporary.as_file().sync_all().map_err(|_| PushError::Io)?;
        temporary.persist(&self.path).map_err(|_| PushError::Io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
