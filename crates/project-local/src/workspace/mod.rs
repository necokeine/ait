//! Control-plane Git/filesystem operations. Authorization remains in application.
pub(crate) mod blocking;
mod git;
mod worktrees;

use ait_domain::{DomainError, ErrorCode};
use ait_ports::{GitBaseline, ProjectWorkspace, WorkspaceLease, WorkspacePathFacts};
use blocking::{Operation, OperationOptions};
use std::{
    collections::HashMap,
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};

/// Native Project adapter with bounded blocking I/O and canonical workspace leases.
#[derive(Clone, Default)]
pub struct LocalProjectWorkspace {
    options: OperationOptions,
    #[cfg(test)]
    lease_duplicate: Option<Arc<Mutex<Option<File>>>>,
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
    file: File,
}
impl Drop for LocalLease {
    fn drop(&mut self) {
        // Closing this descriptor alone need not release a lock while a forked
        // child still has the same open file description. Unlock before the
        // in-process queue guard admits another writer; close remains a fallback.
        let _ = self.file.unlock();
    }
}
impl WorkspaceLease for LocalLease {
    fn canonical_root(&self) -> &Path {
        &self.canonical
    }
}

#[async_trait::async_trait]
impl ProjectWorkspace for LocalProjectWorkspace {
    async fn prepare_git_root(
        &self,
        path: &Path,
        expected_root: Option<&Path>,
    ) -> Result<PathBuf, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitInitFailed);
        path_text(path)?;
        let path = path.to_owned();
        let expected_root = expected_root.map(Path::to_owned);
        operation
            .run(move |ctx| ctx.prepare_git_root(&path, expected_root.as_deref()))
            .await
    }
    async fn verify_git_root(&self, expected_root: &Path) -> Result<(), DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitInitFailed);
        path_text(expected_root)?;
        let expected_root = expected_root.to_owned();
        operation
            .run(move |ctx| ctx.verify_git_root(&expected_root))
            .await
    }
    async fn ensure_git_head(&self, path: &Path) -> Result<String, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitHeadUnavailable);
        path_text(path)?;
        let path = path.to_owned();
        operation.run(move |ctx| ctx.ensure_git_head(&path)).await
    }
    async fn git_head(&self, path: &Path) -> Result<Option<String>, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitHeadUnavailable);
        path_text(path)?;
        let path = path.to_owned();
        operation.run(move |ctx| ctx.git_head(&path)).await
    }
    async fn clean_baseline(&self, path: &Path) -> Result<GitBaseline, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitHeadUnavailable);
        path_text(path)?;
        let path = path.to_owned();
        operation
            .run(move |ctx| ctx.clean_git_baseline(&path))
            .await
    }
    async fn symbolic_head(&self, path: &Path) -> Result<Option<String>, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitHeadUnavailable);
        path_text(path)?;
        let path = path.to_owned();
        operation.run(move |ctx| ctx.git_symbolic_head(&path)).await
    }
    async fn git_dir(&self, path: &Path) -> Result<PathBuf, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitHeadUnavailable);
        path_text(path)?;
        let path = path.to_owned();
        operation.run(move |ctx| ctx.absolute_git_dir(&path)).await
    }
    async fn acquire_lease(&self, path: &Path) -> Result<Arc<dyn WorkspaceLease>, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectWorkspaceBusy);
        self.acquire_lease_with_operation(path, &operation).await
    }
    async fn ensure_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
        lease: Option<Arc<dyn WorkspaceLease>>,
    ) -> Result<bool, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectGitInitFailed);
        let lease = match lease {
            Some(lease) => lease,
            None => {
                self.acquire_lease_with_operation(primary, &operation)
                    .await?
            }
        };
        let (primary, worktree, baseline) =
            (primary.to_owned(), worktree.to_owned(), baseline.to_owned());
        operation
            .run(move |ctx| {
                let held_lease = lease;
                if canonical_path(&primary)? != held_lease.canonical_root() {
                    return Err(error(
                        ErrorCode::ProjectWorkspaceBusy,
                        "workspace lease belongs to another Project",
                        false,
                    ));
                }
                ctx.point("before_worktree");
                ctx.check()?;
                ctx.ensure_session_worktree(&primary, &worktree, &baseline)
            })
            .await
    }
    async fn path_facts(
        &self,
        root: &Path,
        destination: &Path,
    ) -> Result<WorkspacePathFacts, DomainError> {
        let operation = Operation::new(&self.options, ErrorCode::ProjectPathNotFound);
        let (root, destination) = (root.to_owned(), destination.to_owned());
        operation
            .run(move |ctx| {
                path_text(&destination)?;
                let canonical_root = canonical_path(&root)?;
                ctx.check()?;
                if !canonical_root.is_dir() {
                    return Err(error(
                        ErrorCode::ProjectPathNotDirectory,
                        "Project root must be a directory",
                        false,
                    ));
                }
                let mut existing = destination.as_path();
                loop {
                    ctx.check()?;
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

impl LocalProjectWorkspace {
    async fn acquire_lease_with_operation(
        &self,
        path: &Path,
        operation: &Operation,
    ) -> Result<Arc<dyn WorkspaceLease>, DomainError> {
        path_text(path)?;
        let path = path.to_owned();
        let canonical = operation.run(move |_| canonical_path(&path)).await?;
        operation.context().point("after_canonicalize");
        operation.context().check()?;
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
        let guard = operation.wait(queue.lock_owned()).await?;
        operation.context().point("after_lease_queue");
        #[cfg(test)]
        let lease_duplicate = self.lease_duplicate.clone();
        operation
            .run(move |ctx| {
                let git_dir = ctx.absolute_git_dir(&canonical)?;
                ctx.check()?;
                let lock_dir = git_dir.join("ait").join("locks");
                fs::create_dir_all(&lock_dir).map_err(|failure| {
                    error(
                        ErrorCode::ProjectWorkspaceBusy,
                        format!("cannot create workspace lease directory: {failure}"),
                        true,
                    )
                })?;
                ctx.check()?;
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
                ctx.check()?;
                file.try_lock().map_err(|failure| {
                    error(
                        ErrorCode::ProjectWorkspaceBusy,
                        format!("cannot acquire Project workspace write lease: {failure}"),
                        true,
                    )
                })?;
                #[cfg(test)]
                if let Some(duplicate) = lease_duplicate {
                    // A duplicated descriptor shares the open file description,
                    // just like one inherited by a concurrent child at fork.
                    *duplicate
                        .lock()
                        .expect("test lease duplicate lock must remain available") = Some(
                        file.try_clone()
                            .expect("test lease descriptor must be cloneable"),
                    );
                }
                ctx.point("lease_acquired");
                Ok(Arc::new(LocalLease {
                    canonical,
                    _queue: guard,
                    file,
                }) as Arc<dyn WorkspaceLease>)
            })
            .await
    }
}

#[cfg(test)]
mod deadline_tests;
