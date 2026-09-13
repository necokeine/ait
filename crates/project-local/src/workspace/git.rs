use super::{blocking::BlockingContext, canonical_path, error, git_line};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::GitBaseline;
use std::path::{Path, PathBuf};

impl BlockingContext {
    pub(super) fn absolute_git_dir(&self, workdir: &Path) -> Result<PathBuf, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(workdir)
            .args(["rev-parse", "--absolute-git-dir"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    format!("cannot locate Project Git directory: {failure}"),
                    false,
                )
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                String::from_utf8_lossy(&output.stderr).trim(),
                false,
            ));
        }
        let path = PathBuf::from(git_line(&output.stdout)?);
        if !path.is_absolute() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "Git returned a non-absolute metadata directory",
                false,
            ));
        }
        canonical_path(&path)
    }

    pub(super) fn git_symbolic_head(&self, workdir: &Path) -> Result<Option<String>, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(workdir)
            .args(["symbolic-ref", "--quiet", "HEAD"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    format!("cannot inspect Project branch identity: {failure}"),
                    false,
                )
            })?;
        if output.status.success() {
            return Ok(Some(git_line(&output.stdout)?.to_owned()));
        }
        if output.status.code() == Some(1) {
            Ok(None)
        } else {
            Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                String::from_utf8_lossy(&output.stderr).trim(),
                false,
            ))
        }
    }

    pub(super) fn prepare_git_root(
        &self,
        path: &Path,
        expected_root: Option<&Path>,
    ) -> Result<std::path::PathBuf, DomainError> {
        if !path.exists() {
            return Err(error(
                ErrorCode::ProjectPathNotFound,
                "project path does not exist",
                false,
            ));
        }
        self.check()?;
        if !path.is_dir() {
            return Err(error(
                ErrorCode::ProjectPathNotDirectory,
                "project path is not a directory",
                false,
            ));
        }
        self.check()?;
        let canonical = canonical_path(path)?;
        if expected_root.is_some_and(|expected| expected != canonical) {
            return Err(error(
                ErrorCode::RunQueueConflict,
                "canonical Project target changed before preparation; retry the request",
                true,
            ));
        }
        let top = self.git_top_level(&canonical)?;
        if top.as_deref() != Some(canonical.as_path()) {
            self.check()?;
            self.retain(&canonical, "git_initialization_started");
            let output = self
                .command()
                .arg("-C")
                .arg(&canonical)
                .arg("init")
                .output()
                .map_err(|failure| {
                    error(ErrorCode::ProjectGitInitFailed, failure.to_string(), false)
                })?;
            if !output.status.success() {
                return Err(error(
                    ErrorCode::ProjectGitInitFailed,
                    String::from_utf8_lossy(&output.stderr).into_owned(),
                    false,
                ));
            }
            self.retain(&canonical, "git_initialized");
            self.point("git_initialized");
        }
        if self.git_top_level(&canonical)?.as_deref() != Some(canonical.as_path()) {
            return Err(error(
                ErrorCode::ProjectGitInitFailed,
                "git root verification failed",
                false,
            ));
        }
        Ok(canonical)
    }

    pub(super) fn verify_git_root(&self, expected_root: &Path) -> Result<(), DomainError> {
        if canonical_path(expected_root)? != expected_root
            || self.git_top_level(expected_root)?.as_deref() != Some(expected_root)
        {
            return Err(error(
                ErrorCode::ProjectGitInitFailed,
                "Project Git root changed",
                false,
            ));
        }
        Ok(())
    }

    pub(super) fn ensure_git_head(&self, path: &Path) -> Result<String, DomainError> {
        if let Some(head) = self.git_head(path)? {
            return Ok(head);
        }
        let staged = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["diff", "--cached", "--quiet", "--exit-code"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    failure.to_string(),
                    false,
                )
            })?;
        if !staged.status.success() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "cannot create an empty initial commit while the index contains staged changes",
                false,
            ));
        }
        self.check()?;
        self.retain(path, "initial_commit_started");
        let output = self
            .command()
            .arg("-C")
            .arg(path)
            .args([
                "-c",
                "user.name=AIT",
                "-c",
                "user.email=ait@localhost",
                "commit",
                "--allow-empty",
                "--no-gpg-sign",
                "--no-verify",
                "--quiet",
                "-m",
                "Initialize AIT project",
            ])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    failure.to_string(),
                    false,
                )
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                String::from_utf8_lossy(&output.stderr).trim(),
                false,
            ));
        }
        self.retain(path, "initial_commit_created");
        self.point("initial_commit_created");
        self.git_head(path)?.ok_or_else(|| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                "initial commit succeeded but Git HEAD is unavailable",
                false,
            )
        })
    }

    pub(super) fn git_stdout(&self, cwd: &Path, arguments: &[&str]) -> Result<String, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(cwd)
            .args(arguments)
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    format!("cannot run Git for Session worktree: {failure}"),
                    false,
                )
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitInitFailed,
                String::from_utf8_lossy(&output.stderr).trim(),
                false,
            ));
        }
        Ok(git_line(&output.stdout)?.to_owned())
    }

    pub(super) fn clean_git_baseline(&self, path: &Path) -> Result<GitBaseline, DomainError> {
        let before = self.git_head(path)?.ok_or_else(|| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                "project repository has no HEAD commit",
                false,
            )
        })?;
        let index_before = self.git_index_tree(path, false)?;
        let status = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["status", "--porcelain=v1", "--untracked-files=normal"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    failure.to_string(),
                    false,
                )
            })?;
        if !status.status.success() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                String::from_utf8_lossy(&status.stderr).trim(),
                false,
            ));
        }
        if !status.stdout.is_empty() {
            return Err(error(
                ErrorCode::ProjectGitDirty,
                "project Git worktree and index must be clean before adding a user message",
                false,
            ));
        }
        let after = self.git_head(path)?.ok_or_else(|| {
            error(
                ErrorCode::ProjectGitHeadUnavailable,
                "project repository HEAD disappeared while adding a user message",
                true,
            )
        })?;
        if before != after {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "project repository HEAD changed while adding a user message; retry",
                true,
            ));
        }
        let index_after = self.git_index_tree(path, true)?;
        if index_before != index_after {
            return Err(error(
                ErrorCode::ProjectGitDirty,
                "project Git index changed while adding a user message; retry",
                true,
            ));
        }
        let head_tree = self.git_commit_tree(path, &after)?;
        if index_after != head_tree {
            return Err(error(
                ErrorCode::ProjectGitDirty,
                "project Git index does not match HEAD at write admission",
                false,
            ));
        }
        Ok(GitBaseline {
            commit: after,
            index_tree: index_after,
        })
    }

    fn git_index_tree(&self, path: &Path, rechecking: bool) -> Result<String, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["diff-index", "--cached", "--quiet", "HEAD", "--"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    format!("cannot snapshot Project Git index: {failure}"),
                    false,
                )
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitDirty,
                "Project Git index must match HEAD before adding a user message",
                rechecking,
            ));
        }
        self.git_commit_tree(path, "HEAD")
    }

    fn git_commit_tree(&self, path: &Path, commit: &str) -> Result<String, DomainError> {
        let expression = format!("{commit}^{{tree}}");
        let output = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--verify", &expression])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    format!("cannot resolve Project Git commit tree: {failure}"),
                    false,
                )
            })?;
        if !output.status.success() {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                String::from_utf8_lossy(&output.stderr).trim(),
                false,
            ));
        }
        Ok(git_line(&output.stdout)?.to_owned())
    }

    pub(super) fn git_head(&self, path: &Path) -> Result<Option<String>, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--verify", "HEAD"])
            .output()
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitHeadUnavailable,
                    failure.to_string(),
                    false,
                )
            })?;
        if !output.status.success() {
            return Ok(None);
        }
        let head = git_line(&output.stdout)?.to_owned();
        if Self::is_git_commit(&head) {
            Ok(Some(head))
        } else {
            Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "Git returned an invalid full HEAD object id",
                false,
            ))
        }
    }

    pub(super) fn is_git_commit(value: &str) -> bool {
        matches!(value.len(), 40 | 64)
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    }

    fn git_top_level(&self, path: &Path) -> Result<Option<PathBuf>, DomainError> {
        let output = self
            .command()
            .arg("-C")
            .arg(path)
            .args(["rev-parse", "--show-toplevel"])
            .output()
            .map_err(|failure| {
                error(ErrorCode::ProjectGitInitFailed, failure.to_string(), false)
            })?;
        if !output.status.success() {
            return Ok(None);
        }
        canonical_path(Path::new(git_line(&output.stdout)?)).map(Some)
    }
}
