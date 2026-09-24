use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::ports::registry::RegistryError;
use indexmap::IndexMap;
use serde::{Serialize, de::DeserializeOwned};

type Writer = Arc<dyn Fn(&Path, &[u8]) -> Result<(), RegistryError> + Send + Sync>;

/// Shared atomic JSON array engine; business registries validate and publish their own records.
#[doc(hidden)]
pub struct FileRegistry<R> {
    path: PathBuf,
    state: Mutex<State<R>>,
    pub(super) writer: Writer,
    id: fn(&R) -> &str,
}

struct State<R> {
    loaded: bool,
    frozen: bool,
    records: IndexMap<String, R>,
}

impl<R> fmt::Debug for FileRegistry<R> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("FileRegistry")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl<R: Clone + Serialize + DeserializeOwned> FileRegistry<R> {
    /// Create a lazy registry at `path`, keyed by the identity returned by `id`.
    #[must_use]
    pub fn new(path: PathBuf, id: fn(&R) -> &str) -> Self {
        Self {
            path,
            state: Mutex::new(State {
                loaded: false,
                frozen: false,
                records: IndexMap::new(),
            }),
            writer: Arc::new(write_atomic),
            id,
        }
    }

    fn loaded(&self) -> Result<MutexGuard<'_, State<R>>, RegistryError> {
        let mut state = self.state.lock().map_err(|_| RegistryError::Frozen)?;
        if !state.loaded {
            let records: Vec<R> = match fs::read(&self.path) {
                Ok(bytes) => {
                    serde_json::from_slice(&bytes).map_err(|_| RegistryError::InvalidFile)?
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
                Err(_) => return Err(RegistryError::Io),
            };
            for record in records {
                state.records.insert((self.id)(&record).to_owned(), record);
            }
            state.loaded = true;
        }
        Ok(state)
    }

    /// Load the JSON array without creating a missing file.
    ///
    /// # Errors
    /// Returns malformed-file, lock or filesystem errors.
    pub fn initialize(&self) -> Result<(), RegistryError> {
        drop(self.loaded()?);
        Ok(())
    }

    pub(super) fn exists(&self) -> bool {
        self.path.try_exists().unwrap_or(false)
    }

    /// Read committed records in insertion order.
    ///
    /// # Errors
    /// Returns malformed-file, lock or filesystem errors.
    pub fn list(&self) -> Result<Vec<R>, RegistryError> {
        Ok(self.loaded()?.records.values().cloned().collect())
    }

    /// Read the committed record for `id`, or none when it is absent.
    ///
    /// # Errors
    /// Returns malformed-file, lock or filesystem errors.
    pub fn get(&self, id: &str) -> Result<Option<R>, RegistryError> {
        Ok(self.loaded()?.records.get(id).cloned())
    }

    pub(super) fn freeze(&self) -> Result<(), RegistryError> {
        self.loaded()?.frozen = true;
        Ok(())
    }

    // The bool is intentional: Paseo upsert/update writes even for structurally equal
    // records, while absent removals and already-archived projects are true no-ops.
    /// Transform a staged record map and atomically publish it when `update` returns true.
    ///
    /// The callback returns its result and whether the file must be replaced.
    ///
    /// # Errors
    /// Returns callback, validation, frozen-registry, lock or file-write errors.
    pub fn mutate<T>(
        &self,
        update: impl FnOnce(&mut IndexMap<String, R>) -> Result<(T, bool), RegistryError>,
    ) -> Result<T, RegistryError> {
        self.mutate_with(update, |_| Ok(()), || Ok(()))
    }

    pub(super) fn mutate_with<T>(
        &self,
        update: impl FnOnce(&mut IndexMap<String, R>) -> Result<(T, bool), RegistryError>,
        before_write: impl FnOnce(&[R]) -> Result<(), RegistryError>,
        after_write: impl FnOnce() -> Result<(), RegistryError>,
    ) -> Result<T, RegistryError> {
        let mut state = self.loaded()?;
        if state.frozen {
            return Err(RegistryError::Frozen);
        }
        let mut staged = state.records.clone();
        let (result, changed) = update(&mut staged)?;
        if changed {
            let records: Vec<&R> = staged.values().collect();
            let bytes =
                serde_json::to_vec_pretty(&records).map_err(|_| RegistryError::InvalidRecord)?;
            // Validate programmatically constructed records too (e.g. positive request numbers).
            serde_json::from_slice::<Vec<R>>(&bytes).map_err(|_| RegistryError::InvalidRecord)?;
            let records = staged.values().cloned().collect::<Vec<_>>();
            before_write(&records)?;
            (self.writer)(&self.path, &bytes)?;
            after_write()?;
            state.records = staged;
        }
        Ok(result)
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), RegistryError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(|_| RegistryError::Io)?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent).map_err(|_| RegistryError::Io)?;
    temporary.write_all(bytes).map_err(|_| RegistryError::Io)?;
    temporary
        .as_file()
        .sync_all()
        .map_err(|_| RegistryError::Io)?;
    temporary.persist(path).map_err(|_| RegistryError::Io)?;
    Ok(())
}
