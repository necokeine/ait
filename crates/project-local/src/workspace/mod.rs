//! Control-plane Git/filesystem operations. Authorization remains in application.
pub(crate) mod blocking;
mod git;
mod worktrees;

use ait_domain::{DomainError, ErrorCode};
use ait_ports::{GitBaseline, ProjectWorkspace, WorkspaceLease, WorkspacePathFacts};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

/// Native Project adapter with bounded blocking I/O and canonical workspace leases.
#[derive(Clone, Default)]
pub struct LocalProjectWorkspace {
    leases: Arc<Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>>,
}

fn error(code: ErrorCode, message: impl Into<String>, retryable: bool) -> DomainError {
    let mut failure = DomainError::invariant(code, message);
    failure.retryable = retryable;
    failure
}

fn strict_text(bytes: &[u8]) -> Result<&str, DomainError> {
    std::str::from_utf8(bytes).map_err(|_| {
        error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned non-UTF-8 output",
            false,
        )
    })
}

fn git_line(bytes: &[u8]) -> Result<&str, DomainError> {
    let text = strict_text(bytes)?;
    let line = text.strip_suffix('\n').unwrap_or(text);
    if line.contains(['\n', '\r', '\0']) {
        return Err(error(
            ErrorCode::ProjectGitHeadUnavailable,
            "Git returned an ambiguous line",
            false,
        ));
    }
    Ok(line)
}

fn path_text(path: &Path) -> Result<&str, DomainError> {
    path.to_str()
        .filter(|value| !value.contains(['\n', '\r', '\0']))
        .ok_or_else(|| {
            error(
                ErrorCode::ProjectPathNotFound,
                "Project path must be unambiguous UTF-8",
                false,
            )
        })
}

fn canonical_path(path: &Path) -> Result<PathBuf, DomainError> {
    path_text(path)?;
    let canonical = fs::canonicalize(path)
        .map_err(|failure| error(ErrorCode::ProjectPathNotFound, failure.to_string(), false))?;
    path_text(&canonical)?;
    Ok(canonical)
}

struct LocalLease {
    canonical: PathBuf,
    _queue: tokio::sync::OwnedMutexGuard<()>,
    _file: File,
}
impl WorkspaceLease for LocalLease {
    fn canonical_root(&self) -> &Path {
        &self.canonical
    }
}

#[async_trait::async_trait]
impl ProjectWorkspace for LocalProjectWorkspace {
    async fn prepare_git_root(&self, path: &Path) -> Result<PathBuf, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.prepare_git_root(&path)).await
    }
    async fn ensure_git_head(&self, path: &Path) -> Result<String, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.ensure_git_head(&path)).await
    }
    async fn git_head(&self, path: &Path) -> Result<Option<String>, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.git_head(&path)).await
    }
    async fn clean_baseline(&self, path: &Path) -> Result<GitBaseline, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.clean_git_baseline(&path)).await
    }
    async fn symbolic_head(&self, path: &Path) -> Result<Option<String>, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.git_symbolic_head(&path)).await
    }
    async fn git_dir(&self, path: &Path) -> Result<PathBuf, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        blocking::run(move |ctx| ctx.absolute_git_dir(&path)).await
    }
    async fn acquire_lease(&self, path: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        let canonical = blocking::run(move |_| canonical_path(&path)).await?;
        let queue = {
            let mut leases = self.leases.lock().map_err(|_| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "workspace lease registry unavailable",
                    true,
                )
            })?;
            leases.retain(|_, lease| lease.strong_count() > 0);
            if let Some(existing) = leases.get(&canonical).and_then(Weak::upgrade) {
                existing
            } else {
                let queue = Arc::new(tokio::sync::Mutex::new(()));
                leases.insert(canonical.clone(), Arc::downgrade(&queue));
                queue
            }
        };
        let guard = tokio::time::timeout(std::time::Duration::from_secs(30), queue.lock_owned())
            .await
            .map_err(|_| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "workspace lease admission timed out",
                    true,
                )
            })?;
        blocking::run(move |ctx| {
            let git_dir = ctx.absolute_git_dir(&canonical)?;
            let lock_dir = git_dir.join("ait").join("locks");
            fs::create_dir_all(&lock_dir).map_err(|failure| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    format!("cannot create workspace lease directory: {failure}"),
                    true,
                )
            })?;
            let file = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(lock_dir.join("workspace-write.lock"))
                .map_err(|failure| {
                    error(
                        ErrorCode::ProjectWorkspaceBusy,
                        format!("cannot open workspace write lease: {failure}"),
                        true,
                    )
                })?;
            file.try_lock().map_err(|failure| {
                error(
                    ErrorCode::ProjectWorkspaceBusy,
                    format!("cannot acquire Project workspace write lease: {failure}"),
                    true,
                )
            })?;
            Ok(Arc::new(LocalLease {
                canonical,
                _queue: guard,
                _file: file,
            }) as Arc<dyn WorkspaceLease>)
        })
        .await
    }
    async fn ensure_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
        lease: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError> {
        let lease = match lease {
            Some(lease) => lease,
            None => self.acquire_lease(primary).await?,
        };
        let (primary, worktree, baseline) =
            (primary.to_owned(), worktree.to_owned(), baseline.to_owned());
        blocking::run(move |ctx| {
            let held_lease = lease;
            if canonical_path(&primary)? != held_lease.canonical_root() {
                return Err(error(
                    ErrorCode::ProjectWorkspaceBusy,
                    "workspace lease belongs to another Project",
                    false,
                ));
            }
            ctx.ensure_session_worktree(&primary, &worktree, &baseline)
        })
        .await
    }
    async fn path_facts(
        &self,
        root: &Path,
        destination: &Path,
    ) -> Result<WorkspacePathFacts, DomainError> {
        let (root, destination) = (root.to_owned(), destination.to_owned());
        blocking::run(move |_| {
            path_text(&destination)?;
            let canonical_root = canonical_path(&root)?;
            if !canonical_root.is_dir() {
                return Err(error(
                    ErrorCode::ProjectPathNotDirectory,
                    "Project root must be a directory",
                    false,
                ));
            }
            let mut existing = destination.as_path();
            loop {
                match fs::symlink_metadata(existing) {
                    Ok(_) => break,
                    Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => {
                        existing = existing.parent().ok_or_else(|| {
                            error(
                                ErrorCode::ProjectPathNotFound,
                                "cannot resolve destination ancestor",
                                false,
                            )
                        })?;
                    }
                    Err(failure) => {
                        return Err(error(
                            ErrorCode::ProjectPathNotFound,
                            failure.to_string(),
                            false,
                        ));
                    }
                }
            }
            Ok(WorkspacePathFacts {
                canonical_root,
                canonical_existing: canonical_path(existing)?,
            })
        })
        .await
    }
}
