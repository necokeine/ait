//! Atomic project configuration persistence with optimistic file revisions.

use std::fs::File;
use std::io::Write;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::ports::provisioning::{
    ProjectConfigDocument, ProjectConfigRevision, ProjectConfigStore, ProjectConfigStoreError,
    ProjectConfigWrite,
};

const FILE_NAME: &str = "paseo.json";
const MAX_CONFIG_BYTES: u32 = 4 * 1024 * 1024;

/// Atomic local `paseo.json` adapter.
#[derive(Debug, Default)]
pub struct LocalProjectConfigStore;

impl ProjectConfigStore for LocalProjectConfigStore {
    fn read(&self, root: &str) -> Result<ProjectConfigDocument, ProjectConfigStoreError> {
        let path = Path::new(root).join(FILE_NAME);
        let Some(revision) = revision(&path).map_err(|_| ProjectConfigStoreError::Invalid)? else {
            return Ok(ProjectConfigDocument {
                config: None,
                revision: None,
            });
        };
        if revision.size > f64::from(MAX_CONFIG_BYTES) {
            return Err(ProjectConfigStoreError::Invalid);
        }
        let file = File::open(path).map_err(|_| ProjectConfigStoreError::Invalid)?;
        let config = serde_json::from_reader(file).map_err(|_| ProjectConfigStoreError::Invalid)?;
        Ok(ProjectConfigDocument {
            config: Some(config),
            revision: Some(revision),
        })
    }

    fn write(
        &self,
        root: &str,
        config: &serde_json::Value,
        expected_revision: Option<ProjectConfigRevision>,
    ) -> Result<ProjectConfigWrite, ProjectConfigStoreError> {
        let root = Path::new(root);
        let path = root.join(FILE_NAME);
        let mut staged =
            tempfile::NamedTempFile::new_in(root).map_err(|_| ProjectConfigStoreError::Write)?;
        serde_json::to_writer_pretty(staged.as_file_mut(), config)
            .map_err(|_| ProjectConfigStoreError::Write)?;
        staged
            .as_file_mut()
            .write_all(b"\n")
            .and_then(|()| staged.as_file_mut().sync_all())
            .map_err(|_| ProjectConfigStoreError::Write)?;
        let current_revision = revision(&path).map_err(|_| ProjectConfigStoreError::Write)?;
        if current_revision != expected_revision {
            return Ok(ProjectConfigWrite::Stale { current_revision });
        }
        staged
            .persist(&path)
            .map_err(|_| ProjectConfigStoreError::Write)?;
        #[cfg(unix)]
        File::open(root)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| ProjectConfigStoreError::Write)?;
        let revision = revision(&path)
            .map_err(|_| ProjectConfigStoreError::Write)?
            .ok_or(ProjectConfigStoreError::Write)?;
        Ok(ProjectConfigWrite::Written {
            config: config.clone(),
            revision,
        })
    }
}

fn revision(path: &Path) -> std::io::Result<Option<ProjectConfigRevision>> {
    let metadata = match path.metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(std::io::Error::other("project config is not a file"));
    }
    let modified = metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .map_err(std::io::Error::other)?;
    let size = u32::try_from(metadata.len())
        .map_err(|_| std::io::Error::other("project config is too large"))?;
    Ok(Some(ProjectConfigRevision {
        mtime_ms: modified.as_secs_f64() * 1000.0,
        size: f64::from(size),
    }))
}

#[cfg(test)]
mod tests;
