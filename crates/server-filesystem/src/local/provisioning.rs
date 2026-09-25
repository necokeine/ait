//! Local directory inspection and creation adapter.

use std::io;
use std::path::{Component, Path, PathBuf};

use server_metadata::ports::provisioning::{Checkout, DirectorySource, DirectorySourceError};

use crate::local::git::GitError;

/// Local directory and Git inspector for Paseo-compatible provisioning.
#[derive(Debug, Default)]
pub struct LocalDirectorySource;

impl DirectorySource for LocalDirectorySource {
    fn inspect(&self, path: &str) -> Result<Checkout, DirectorySourceError> {
        let cwd = resolve_path(path)?;
        let metadata = std::fs::metadata(&cwd).map_err(|error| map_io(&error))?;
        if !metadata.is_dir() {
            return Err(DirectorySourceError::NotFound);
        }
        let cwd_text = path_text(&cwd)?;
        let is_git = match crate::local::git::run(&cwd, &["rev-parse", "--is-inside-work-tree"]) {
            Ok(value) => value.trim() == "true",
            Err(GitError::Rejected) => false,
            Err(_) => return Err(DirectorySourceError::Io),
        };
        if !is_git {
            return Ok(Checkout {
                cwd: cwd_text,
                is_git: false,
                current_branch: None,
                remote_url: None,
                worktree_root: None,
                is_paseo_owned_worktree: false,
                main_repo_root: None,
            });
        }

        let worktree_root = required_git_path(&cwd, &["rev-parse", "--show-toplevel"])?;
        let git_dir =
            required_git_path(&cwd, &["rev-parse", "--path-format=absolute", "--git-dir"])?;
        let common_dir = required_git_path(
            &cwd,
            &["rev-parse", "--path-format=absolute", "--git-common-dir"],
        )?;
        let main_repo_root = if git_dir == common_dir {
            None
        } else {
            common_dir.parent().map(path_text).transpose()?
        };
        let current_branch = optional_git(&cwd, &["symbolic-ref", "--quiet", "--short", "HEAD"])?;
        let remote_url = optional_git(&cwd, &["remote", "get-url", "origin"])?;

        Ok(Checkout {
            cwd: cwd_text,
            is_git: true,
            current_branch,
            remote_url,
            worktree_root: Some(path_text(&worktree_root)?),
            is_paseo_owned_worktree: false,
            main_repo_root,
        })
    }

    fn create_child(&self, parent: &str, name: &str) -> Result<String, DirectorySourceError> {
        let parent = resolve_path(parent)?;
        if !std::fs::metadata(&parent)
            .map_err(|error| map_io(&error))?
            .is_dir()
        {
            return Err(DirectorySourceError::NotFound);
        }
        let child = normalize_absolute(&parent.join(name))?;
        std::fs::create_dir(&child).map_err(|error| map_io(&error))?;
        path_text(&child)
    }

    fn remove_empty(&self, path: &str) -> Result<(), DirectorySourceError> {
        std::fs::remove_dir(path).map_err(|error| map_io(&error))
    }

    fn equivalent(&self, left: &str, right: &str) -> bool {
        match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
            (Ok(left), Ok(right)) => path_eq(&left, &right),
            _ => match (resolve_path(left), resolve_path(right)) {
                (Ok(left), Ok(right)) => path_eq(&left, &right),
                _ => false,
            },
        }
    }

    fn canonical(&self, path: &str) -> Result<String, DirectorySourceError> {
        let path = resolve_path(path)?;
        let canonical = std::fs::canonicalize(path).map_err(|error| map_io(&error))?;
        if !canonical.is_dir() {
            return Err(DirectorySourceError::NotFound);
        }
        path_text(&canonical)
    }
}

fn resolve_path(path: &str) -> Result<PathBuf, DirectorySourceError> {
    let expanded = if path == "~" {
        home()?
    } else if let Some(remainder) = path.strip_prefix("~/") {
        home()?.join(remainder)
    } else {
        PathBuf::from(path)
    };
    let absolute = if expanded.is_absolute() {
        expanded
    } else {
        std::env::current_dir()
            .map_err(|error| map_io(&error))?
            .join(expanded)
    };
    normalize_absolute(&absolute)
}

fn normalize_absolute(path: &Path) -> Result<PathBuf, DirectorySourceError> {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            Component::Prefix(_) | Component::RootDir | Component::Normal(_) => {
                normalized.push(component.as_os_str());
            }
        }
    }
    if normalized.is_absolute() {
        Ok(normalized)
    } else {
        Err(DirectorySourceError::Io)
    }
}

fn home() -> Result<PathBuf, DirectorySourceError> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or(DirectorySourceError::Io)
}

fn required_git_path(root: &Path, arguments: &[&str]) -> Result<PathBuf, DirectorySourceError> {
    crate::local::git::run(root, arguments)
        .map(PathBuf::from)
        .map_err(|_| DirectorySourceError::Io)
}

fn optional_git(root: &Path, arguments: &[&str]) -> Result<Option<String>, DirectorySourceError> {
    match crate::local::git::run(root, arguments) {
        Ok(value) => Ok((!value.trim().is_empty()).then(|| value.trim().to_owned())),
        Err(GitError::Rejected) => Ok(None),
        Err(_) => Err(DirectorySourceError::Io),
    }
}

fn path_text(path: &Path) -> Result<String, DirectorySourceError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or(DirectorySourceError::Io)
}

fn map_io(error: &io::Error) -> DirectorySourceError {
    match error.kind() {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => DirectorySourceError::NotFound,
        io::ErrorKind::AlreadyExists => DirectorySourceError::AlreadyExists,
        io::ErrorKind::PermissionDenied | io::ErrorKind::ReadOnlyFilesystem => {
            DirectorySourceError::PermissionDenied
        }
        _ => DirectorySourceError::Io,
    }
}

#[cfg(windows)]
fn path_eq(left: &Path, right: &Path) -> bool {
    left.to_string_lossy()
        .eq_ignore_ascii_case(&right.to_string_lossy())
}

#[cfg(not(windows))]
fn path_eq(left: &Path, right: &Path) -> bool {
    left == right
}

#[cfg(test)]
mod tests;
