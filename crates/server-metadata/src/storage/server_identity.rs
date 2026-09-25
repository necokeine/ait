//! Stable server identity persistence, used while the host owns its data-directory lock.

use std::fs;
use std::io::{self, Write};
use std::path::Path;

use uuid::Uuid;

/// Stable identity loading or publication failed.
#[derive(Debug, thiserror::Error)]
pub enum IdentityError {
    /// Persisted state is not a non-nil UUID in a regular file.
    #[error("invalid persisted server identity")]
    Invalid,
    /// The identity file could not be read, synced or atomically installed.
    #[error("server identity I/O failed")]
    Io(#[from] io::Error),
}

/// Load or create `directory/server-id` without replacing invalid or existing state.
///
/// The host must hold the directory's exclusive instance lock until all consumers finish.
///
/// # Errors
/// Returns invalid-state or I/O errors without overwriting an existing identity.
pub fn load_or_create(directory: &Path) -> Result<Uuid, IdentityError> {
    let path = directory.join("server-id");
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            return Err(IdentityError::Invalid);
        }
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    match fs::read_to_string(&path) {
        Ok(text) => {
            let id = Uuid::parse_str(text.trim()).map_err(|_| IdentityError::Invalid)?;
            if id.is_nil() {
                return Err(IdentityError::Invalid);
            }
            Ok(id)
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let id = Uuid::new_v4();
            let mut staged = tempfile::NamedTempFile::new_in(directory)?;
            writeln!(staged, "{id}")?;
            staged.as_file().sync_all()?;
            staged
                .persist_noclobber(path)
                .map_err(|error| error.error)?;
            #[cfg(unix)]
            fs::File::open(directory)?.sync_all()?;
            Ok(id)
        }
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests;
