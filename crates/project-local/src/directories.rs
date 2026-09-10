use std::{fs, io::ErrorKind, path::PathBuf};

use ait_domain::{DomainError, ErrorCode};
use ait_ports::ProjectDirectoryCreator;

/// Creates named Project directories beneath the current host user's Documents.
pub struct DocumentsProjectDirectory {
    resolve: Box<dyn Fn() -> Option<PathBuf> + Send + Sync>,
}

impl Default for DocumentsProjectDirectory {
    fn default() -> Self {
        Self::with_resolver(dirs::document_dir)
    }
}

impl DocumentsProjectDirectory {
    /// Supplies the host-directory resolver; tests can use isolated temporary paths
    /// without changing process environment variables or accessing real Documents.
    #[must_use]
    pub fn with_resolver(resolve: impl Fn() -> Option<PathBuf> + Send + Sync + 'static) -> Self {
        Self {
            resolve: Box::new(resolve),
        }
    }
}

impl ProjectDirectoryCreator for DocumentsProjectDirectory {
    fn create_workdir(&self, name: &str) -> Result<PathBuf, DomainError> {
        validate_directory_name(name)?;
        let unavailable = |message| {
            DomainError::invariant(ErrorCode::ProjectDefaultDirectoryUnavailable, message)
        };
        let documents = (self.resolve)().filter(|path| path.is_absolute()).ok_or_else(|| {
            unavailable("The current user's Documents directory is unavailable; choose an existing directory explicitly.".to_owned())
        })?;
        let documents = documents.canonicalize().map_err(|failure| unavailable(format!(
            "Cannot access Documents directory {}: {failure}; choose an existing directory explicitly.", documents.display(),
        )))?;
        if !documents.is_dir() || documents.to_str().is_none() {
            return Err(unavailable(format!(
                "Documents path {} must be a directory with a UTF-8 path.",
                documents.display(),
            )));
        }
        let target = documents.join(name);
        // mkdir is the exclusive allocation boundary, including for dangling
        // symlinks. Never precheck then create_dir_all, reuse, or clean up a target.
        fs::create_dir(&target).map_err(|failure| {
            if failure.kind() == ErrorKind::AlreadyExists {
                DomainError::invariant(ErrorCode::ProjectPathAlreadyExists, format!(
                    "Project directory already exists: {}. Choose another name or explicitly select the existing directory.", target.display(),
                ))
            } else {
                DomainError::invariant(ErrorCode::ProjectDirectoryCreationFailed, format!(
                    "Cannot create Project directory {}: {failure}", target.display(),
                ))
            }
        })?;
        Ok(target)
    }
}

fn validate_directory_name(name: &str) -> Result<(), DomainError> {
    // Apply one portable component policy on every OS, including Windows device
    // names and alternate streams. Display names with explicit workdirs are unaffected.
    let stem = name.split('.').next().unwrap_or_default().to_uppercase();
    let reserved = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || ["COM", "LPT"].iter().any(|prefix| {
            stem.strip_prefix(prefix).is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            })
        });
    if name.is_empty()
        || name.trim() != name
        || name.len() > 255
        || name.ends_with('.')
        || reserved
        || name
            .chars()
            .any(|c| c.is_control() || "/\\:<>\"|?*".contains(c))
    {
        return Err(DomainError::invariant(
            ErrorCode::InvalidProject,
            "Project name must be a nonempty portable folder name (at most 255 UTF-8 bytes), without path separators, reserved names, surrounding whitespace, or a trailing dot.",
        ));
    }
    Ok(())
}
