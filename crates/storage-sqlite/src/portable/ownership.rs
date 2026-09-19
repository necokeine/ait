//! Lifetime-held local locks; persisted owner metadata never authorizes access.
use std::{
    fs::File,
    path::{Path, PathBuf},
};

use sha2::{Digest, Sha256};

use super::{ControlStoreError, io_error, other};

pub(super) struct ProjectLocks {
    _directory: File,
    _identity: File,
}

pub(super) fn reject_link(path: &Path) -> Result<(), ControlStoreError> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(other(format!(
            "Project storage must not be a symlink: {}",
            path.display()
        ))),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}

pub(super) fn secure_directory(path: &Path) -> Result<(), ControlStoreError> {
    reject_link(path)?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(path).map_err(io_error)
}

fn lock(path: &Path) -> Result<File, ControlStoreError> {
    reject_link(path)?;
    let file = File::options()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .map_err(io_error)?;
    file.try_lock().map_err(|error| {
        other(format!(
            "PROJECT_BUSY: Project is managed by another Ait backend ({}): {error}",
            path.display()
        ))
    })?;
    Ok(file)
}

pub(super) fn common_directory() -> Result<PathBuf, ControlStoreError> {
    // Resolve the OS account, not HOME/XDG overrides: separate Ait configurations
    // running under the same user must share identity locks.
    #[cfg(unix)]
    let home = {
        let user = nix::unistd::User::from_uid(nix::unistd::Uid::effective())
            .map_err(|failure| other(failure.to_string()))?
            .ok_or_else(|| other("local OS account unavailable"))?;
        if cfg!(target_os = "macos") {
            user.dir.join("Library/Application Support")
        } else {
            user.dir.join(".local/share")
        }
    };
    #[cfg(not(unix))]
    let home =
        dirs::data_local_dir().ok_or_else(|| other("local user data directory unavailable"))?;
    let path = home.join("ait-shared").join("project-locks");
    secure_directory(&path)?;
    Ok(path)
}

pub(super) fn acquire(root: &Path, id: &str) -> Result<ProjectLocks, ControlStoreError> {
    let directory = root.join(".ait");
    secure_directory(&directory)?;
    let directory_lock = lock(&directory.join("project.lock"))?;
    let identity_path =
        common_directory()?.join(format!("{:x}.lock", Sha256::digest(id.as_bytes())));
    Ok(ProjectLocks {
        _directory: directory_lock,
        _identity: lock(&identity_path)?,
    })
}

/// Physical identities are checked in addition to canonical path strings.
pub(super) struct FileIdentity {
    #[cfg(unix)]
    entries: Vec<(u64, u64)>,
}

impl FileIdentity {
    pub(super) fn capture(root: &Path) -> Result<Self, ControlStoreError> {
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            let entries = [
                root.to_owned(),
                root.join(".ait"),
                root.join(".ait/project.sqlite3"),
                root.join(".ait/project.lock"),
            ]
            .iter()
            .map(|path| {
                std::fs::metadata(path)
                    .map(|m| (m.dev(), m.ino()))
                    .map_err(io_error)
            })
            .collect::<Result<Vec<_>, _>>()?;
            Ok(Self { entries })
        }
        #[cfg(not(unix))]
        {
            let _ = root;
            Ok(Self {})
        }
    }

    pub(super) fn verify(&self, root: &Path) -> Result<(), ControlStoreError> {
        let current = Self::capture(root)?;
        #[cfg(unix)]
        if self.entries != current.entries {
            return Err(other(
                "WORKSPACE_INVALID: open Project storage was replaced; close it before changing its location",
            ));
        }
        #[cfg(not(unix))]
        let _ = current;
        Ok(())
    }
}
