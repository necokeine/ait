//! Native filesystem and Git adapter for local Projects.

use std::{
    ffi::OsString,
    fs,
    path::{Component, Path, PathBuf},
    process::Command,
};

use ait_ports::{EnvironmentError, ProjectEnvironment};

/// Canonical project-root guard shared by instruction loading and future file tools.
///
/// Project-relative access is the default. External access is deliberately a
/// separate API that requires a caller-provided authorization root.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProjectPathGuard {
    root: PathBuf,
}

impl ProjectPathGuard {
    /// Creates a guard from an existing directory and stores its canonical path.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when `root` is missing, is not a
    /// directory, or cannot be canonicalized.
    pub fn new(root: &Path) -> Result<Self, EnvironmentError> {
        let root = canonicalize_directory(root)?;
        Ok(Self { root })
    }

    /// Returns the canonical Project root.
    #[must_use]
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolves an existing project-relative path, rejecting traversal and
    /// symlinks whose canonical target leaves the Project root.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when the relative path is invalid,
    /// missing, unreadable, or resolves outside the Project root.
    pub fn resolve_existing(&self, relative: &Path) -> Result<PathBuf, EnvironmentError> {
        validate_relative(relative)?;
        let candidate = self.root.join(relative);
        let resolved = fs::canonicalize(&candidate).map_err(|error| map_io(&candidate, &error))?;
        self.require_contained(&resolved, &candidate)?;
        Ok(resolved)
    }

    /// Resolves a project-relative destination that may not exist yet.
    ///
    /// The nearest existing ancestor is canonicalized so an existing symlink
    /// cannot redirect creation outside the Project. The eventual open/create
    /// operation must still use an OS-level no-follow/dir-handle strategy when
    /// defending against a concurrently mutating hostile process.
    ///
    /// # Errors
    ///
    /// Returns an [`EnvironmentError`] when the relative path is invalid, its
    /// existing ancestor is unreadable, or a symlink redirects it out of scope.
    pub fn resolve_for_creation(&self, relative: &Path) -> Result<PathBuf, EnvironmentError> {
        validate_relative(relative)?;
        let candidate = self.root.join(relative);
        let mut ancestor = candidate.clone();
        let mut suffix = Vec::<OsString>::new();

        loop {
            match ancestor.try_exists() {
                Ok(true) => break,
                Ok(false) => {
                    let name = ancestor.file_name().ok_or_else(|| {
                        EnvironmentError::InvalidRelativePath(relative.to_path_buf())
                    })?;
                    suffix.push(name.to_owned());
                    if !ancestor.pop() {
                        return Err(EnvironmentError::InvalidRelativePath(
                            relative.to_path_buf(),
                        ));
                    }
                }
                Err(error) => return Err(map_io(&ancestor, &error)),
            }
        }

        let mut resolved =
            fs::canonicalize(&ancestor).map_err(|error| map_io(&ancestor, &error))?;
        self.require_contained(&resolved, &candidate)?;
        for component in suffix.into_iter().rev() {
            resolved.push(component);
        }
        Ok(resolved)
    }

    fn require_contained(&self, resolved: &Path, requested: &Path) -> Result<(), EnvironmentError> {
        if resolved.starts_with(&self.root) {
            Ok(())
        } else {
            Err(EnvironmentError::OutOfScope(requested.to_path_buf()))
        }
    }
}

/// Native implementation of Project filesystem and Git capabilities.
#[derive(Clone, Copy, Debug, Default)]
pub struct LocalProjectEnvironment;

impl ProjectEnvironment for LocalProjectEnvironment {
    fn canonicalize_directory(&self, path: &Path) -> Result<PathBuf, EnvironmentError> {
        canonicalize_directory(path)
    }

    fn git_top_level(&self, directory: &Path) -> Result<Option<PathBuf>, EnvironmentError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|error| EnvironmentError::Git(error.to_string()))?;
        if !output.status.success() {
            return Ok(None);
        }

        let raw = String::from_utf8(output.stdout)
            .map_err(|error| EnvironmentError::Git(error.to_string()))?;
        let path = PathBuf::from(raw.trim());
        let canonical = fs::canonicalize(&path).map_err(|error| map_io(&path, &error))?;
        Ok(Some(canonical))
    }

    fn git_init(&self, directory: &Path) -> Result<(), EnvironmentError> {
        let output = Command::new("git")
            .arg("-C")
            .arg(directory)
            .args(["init", "--quiet"])
            .output()
            .map_err(|error| EnvironmentError::Git(error.to_string()))?;
        if output.status.success() {
            Ok(())
        } else {
            Err(EnvironmentError::Git(
                String::from_utf8_lossy(&output.stderr).trim().to_owned(),
            ))
        }
    }

    fn read_project_file(
        &self,
        project_root: &Path,
        relative_path: &Path,
    ) -> Result<Option<Vec<u8>>, EnvironmentError> {
        validate_relative(relative_path)?;
        let candidate = project_root.join(relative_path);
        if !candidate
            .try_exists()
            .map_err(|error| map_io(&candidate, &error))?
        {
            return Ok(None);
        }
        let resolved = ProjectPathGuard::new(project_root)?.resolve_existing(relative_path)?;
        read_regular_file(&resolved).map(Some)
    }

    fn read_authorized_file(
        &self,
        authorized_root: &Path,
        absolute_path: &Path,
    ) -> Result<Option<Vec<u8>>, EnvironmentError> {
        if !authorized_root.is_absolute() || !absolute_path.is_absolute() {
            return Err(EnvironmentError::InvalidRelativePath(
                absolute_path.to_path_buf(),
            ));
        }
        let guard = ProjectPathGuard::new(authorized_root)?;
        let relative = absolute_path
            .strip_prefix(authorized_root)
            .map_err(|_| EnvironmentError::OutOfScope(absolute_path.to_path_buf()))?;
        let resolved = guard.resolve_for_creation(relative)?;
        if !resolved
            .try_exists()
            .map_err(|error| map_io(&resolved, &error))?
        {
            return Ok(None);
        }
        read_regular_file(&resolved).map(Some)
    }
}

fn canonicalize_directory(path: &Path) -> Result<PathBuf, EnvironmentError> {
    let metadata = fs::metadata(path).map_err(|error| map_io(path, &error))?;
    if !metadata.is_dir() {
        return Err(EnvironmentError::NotDirectory(path.to_path_buf()));
    }
    fs::canonicalize(path).map_err(|error| map_io(path, &error))
}

fn validate_relative(path: &Path) -> Result<(), EnvironmentError> {
    if path.as_os_str().is_empty()
        || path
            .components()
            .any(|component| !matches!(component, Component::Normal(_) | Component::CurDir))
    {
        return Err(EnvironmentError::InvalidRelativePath(path.to_path_buf()));
    }
    Ok(())
}

fn read_regular_file(path: &Path) -> Result<Vec<u8>, EnvironmentError> {
    let metadata = fs::metadata(path).map_err(|error| map_io(path, &error))?;
    if !metadata.is_file() {
        return Err(EnvironmentError::Io(format!(
            "instruction source is not a regular file: {}",
            path.display()
        )));
    }
    fs::read(path).map_err(|error| map_io(path, &error))
}

fn map_io(path: &Path, error: &std::io::Error) -> EnvironmentError {
    if error.kind() == std::io::ErrorKind::NotFound {
        EnvironmentError::NotFound(path.to_path_buf())
    } else {
        EnvironmentError::Io(format!("{}: {error}", path.display()))
    }
}

#[cfg(test)]
mod tests;
