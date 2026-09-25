//! Bounded, versioned atomic schedule storage.
use crate::{
    ports::{Error, Store},
    protocol::Schedule,
};
use serde::{Deserialize, Serialize};
use std::{fs, io::Read, path::PathBuf};

/// Local JSON adapter; the independent host owns the containing data directory lease.
#[derive(Debug)]
pub struct FileStore {
    path: PathBuf,
}
#[derive(Serialize, Deserialize)]
struct Document {
    version: u32,
    schedules: Vec<Schedule>,
}
impl FileStore {
    /// Configure an absolute private state path without writing it.
    #[must_use]
    pub fn new(path: PathBuf) -> Self {
        Self { path }
    }
    fn check(&self) -> Result<(), Error> {
        let mut current = PathBuf::new();
        for part in self.path.components() {
            if matches!(part, std::path::Component::ParentDir) {
                return Err(Error::Storage);
            }
            current.push(part);
            match fs::symlink_metadata(&current) {
                Ok(meta) if meta.file_type().is_symlink() => return Err(Error::Storage),
                Ok(_) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(Error::Storage),
            }
        }
        Ok(())
    }
}
impl Store for FileStore {
    fn load(&self) -> Result<Vec<Schedule>, Error> {
        self.check()?;
        let file = match fs::File::open(&self.path) {
            Ok(file) => file,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
            Err(_) => return Err(Error::Storage),
        };
        let mut bytes = Vec::new();
        file.take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut bytes)
            .map_err(|_| Error::Storage)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error::Storage);
        }
        let doc: Document = serde_json::from_slice(&bytes).map_err(|_| Error::Storage)?;
        if doc.version != 1 {
            return Err(Error::Storage);
        }
        Ok(doc.schedules)
    }
    fn save(&mut self, schedules: &[Schedule]) -> Result<(), Error> {
        use std::io::Write;
        self.check()?;
        let bytes = serde_json::to_vec(&Document {
            version: 1,
            schedules: schedules.to_vec(),
        })
        .map_err(|_| Error::Storage)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(Error::Conflict);
        }
        let parent = self.path.parent().ok_or(Error::Storage)?;
        fs::create_dir_all(parent).map_err(|_| Error::Storage)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|_| Error::Storage)?;
        temp.write_all(&bytes)
            .and_then(|()| temp.as_file().sync_all())
            .map_err(|_| Error::Storage)?;
        temp.persist(&self.path).map_err(|_| Error::Storage)?;
        Ok(())
    }
}
#[cfg(test)]
mod tests;
