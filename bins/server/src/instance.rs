use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::Path;

use anyhow::{Context, bail};
use uuid::Uuid;

#[derive(Debug)]
pub(super) struct InstanceLease {
    // Never unlink the lock file: other processes may already hold the same inode open.
    _lock: File,
    pub server_id: Uuid,
    pub instance_id: Uuid,
}

impl InstanceLease {
    pub fn acquire(directory: &Path) -> anyhow::Result<Self> {
        create_directory(directory)?;
        let directory = directory
            .canonicalize()
            .context("resolve server data directory")?;
        let lock_path = directory.join("instance.lock");
        reject_symlink(&lock_path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(lock_path)
            .context("open server instance lock")?;
        lock.try_lock()
            .context("server data directory is already in use or cannot be locked")?;
        let identity_path = directory.join("server-id");
        reject_symlink(&identity_path)?;
        let server_id = match std::fs::read_to_string(&identity_path) {
            Ok(text) => {
                let id =
                    Uuid::parse_str(text.trim()).context("invalid persisted server identity")?;
                if id.is_nil() {
                    bail!("persisted server identity must not be nil");
                }
                id
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let id = Uuid::new_v4();
                let mut temporary =
                    tempfile::NamedTempFile::new_in(&directory).context("stage server identity")?;
                writeln!(temporary, "{id}").context("write server identity")?;
                temporary
                    .as_file()
                    .sync_all()
                    .context("sync server identity")?;
                temporary
                    .persist_noclobber(identity_path)
                    .context("publish server identity")?;
                #[cfg(unix)]
                File::open(&directory)?
                    .sync_all()
                    .context("sync server directory")?;
                id
            }
            Err(error) => return Err(error).context("read server identity"),
        };
        Ok(Self {
            _lock: lock,
            server_id,
            instance_id: Uuid::new_v4(),
        })
    }
}

fn create_directory(directory: &Path) -> anyhow::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder
        .create(directory)
        .context("create server data directory")
}

fn reject_symlink(path: &Path) -> anyhow::Result<()> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
            bail!("server state must be a regular file: {}", path.display());
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).context("inspect server state file"),
    }
}

#[cfg(test)]
mod tests;
