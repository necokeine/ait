use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

use server_domain::OwnerEpoch;
use server_ports::{IdentityLease, Lease, ProjectError};

use crate::{read_optional, regular_file};

#[derive(Debug)]
pub(super) struct Identity {
    _lock: File,
    counter: PathBuf,
}

impl Identity {
    pub fn new(lock: File, counter: PathBuf) -> Self {
        Self {
            _lock: lock,
            counter,
        }
    }
}

impl Lease for Identity {}

impl IdentityLease for Identity {
    fn reserve_epoch(&mut self, database_epoch: OwnerEpoch) -> Result<OwnerEpoch, ProjectError> {
        let previous = if regular_file(&self.counter)? {
            read_optional(&self.counter, 32)?
                .trim()
                .parse::<u64>()
                .map_err(|_| ProjectError::Invalid)?
        } else {
            0
        };
        let value = previous
            .max(database_epoch.value())
            .checked_add(1)
            .ok_or(ProjectError::Invalid)?;
        let next = OwnerEpoch::new(value)?;
        let parent = self.counter.parent().ok_or(ProjectError::Invalid)?;
        let mut staged = tempfile::NamedTempFile::new_in(parent).map_err(|_| ProjectError::Io)?;
        writeln!(staged, "{value}").map_err(|_| ProjectError::Io)?;
        staged.as_file().sync_all().map_err(|_| ProjectError::Io)?;
        staged
            .persist(&self.counter)
            .map_err(|_| ProjectError::Io)?;
        #[cfg(unix)]
        File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ProjectError::Io)?;
        Ok(next)
    }
}

#[cfg(test)]
mod tests;
