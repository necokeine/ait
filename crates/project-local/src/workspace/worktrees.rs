use super::{blocking::BlockingContext, error};
use ait_domain::{DomainError, ErrorCode};
use std::{
    fs::OpenOptions,
    io::Write as IoWrite,
    path::{Path, PathBuf},
};
impl BlockingContext {
    pub(super) fn ensure_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
    ) -> Result<bool, DomainError> {
        super::path_text(primary)?;
        super::path_text(worktree)?;
        let parent = worktree
            .parent()
            .ok_or_else(|| error(ErrorCode::InvalidSession, "missing Session parent", false))?;
        // Only direct children of the manager-owned .ait directory are valid.
        if parent != primary.join(".ait") || worktree.file_name().is_none() {
            return Err(error(
                ErrorCode::InvalidSession,
                "invalid Session worktree destination",
                false,
            ));
        }
        self.validate_session_worktree_parent(parent)?;
        if self.validate_existing_session_worktree(worktree)? {
            return Ok(false);
        }
        self.ensure_ait_excluded(primary)?;
        self.ensure_session_worktree_parent(parent)?;
        self.validate_session_baseline(primary, baseline)?;
        self.add_session_worktree(primary, worktree, baseline)?;
        if let Err(mut failure) = self.git_stdout(worktree, &["reset", "--hard", baseline]) {
            failure.message = format!(
                "{}; partial Session worktree retained at {}",
                failure.message,
                worktree.display()
            );
            return Err(failure);
        }
        Ok(true)
    }
    fn validate_session_worktree_parent(&self, parent: &Path) -> Result<bool, DomainError> {
        self.check()?;
        match std::fs::symlink_metadata(parent) {
            Ok(metadata) if !metadata.file_type().is_symlink() && metadata.is_dir() => Ok(true),
            Ok(_) => Err(error(
                ErrorCode::InvalidSession,
                "Project .ait path must be a real directory",
                false,
            )),
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(failure) => Err(error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot inspect Project .ait directory: {failure}"),
                false,
            )),
        }
    }

    fn validate_existing_session_worktree(&self, worktree: &Path) -> Result<bool, DomainError> {
        let metadata = match std::fs::symlink_metadata(worktree) {
            Ok(metadata) => metadata,
            Err(failure) if failure.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(failure) => {
                return Err(error(
                    ErrorCode::ProjectPathNotFound,
                    format!("cannot inspect Session worktree: {failure}"),
                    false,
                ));
            }
        };
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session worktree must be a real directory",
                false,
            ));
        }
        let top = self.git_stdout(worktree, &["rev-parse", "--show-toplevel"])?;
        let canonical = worktree.canonicalize().map_err(|failure| {
            error(
                ErrorCode::ProjectPathNotFound,
                format!("cannot resolve Session worktree: {failure}"),
                false,
            )
        })?;
        let top = PathBuf::from(top).canonicalize().map_err(|failure| {
            error(
                ErrorCode::InvalidSession,
                format!("cannot resolve Session Git root: {failure}"),
                false,
            )
        })?;
        if top != canonical {
            return Err(error(
                ErrorCode::InvalidSession,
                "Session workdir is not its own linked Git worktree",
                false,
            ));
        }
        Ok(true)
    }

    fn ensure_session_worktree_parent(&self, parent: &Path) -> Result<(), DomainError> {
        if self.validate_session_worktree_parent(parent)? {
            return Ok(());
        }
        match std::fs::create_dir(parent) {
            Ok(()) => Ok(()),
            Err(failure) if failure.kind() == std::io::ErrorKind::AlreadyExists => {
                self.validate_session_worktree_parent(parent).map(|_| ())
            }
            Err(failure) => Err(error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot create Project .ait directory: {failure}"),
                false,
            )),
        }
    }

    fn validate_session_baseline(&self, primary: &Path, baseline: &str) -> Result<(), DomainError> {
        let commit_expression = format!("{baseline}^{{commit}}");
        let verified = self.git_stdout(primary, &["rev-parse", "--verify", &commit_expression])?;
        if verified != baseline {
            return Err(error(
                ErrorCode::ProjectGitHeadUnavailable,
                "Session baseline does not resolve to the recorded commit",
                false,
            ));
        }
        Ok(())
    }

    fn add_session_worktree(
        &self,
        primary: &Path,
        worktree: &Path,
        baseline: &str,
    ) -> Result<(), DomainError> {
        let worktree_text = super::path_text(worktree)?.to_owned();
        self.git_stdout(
            primary,
            &[
                "worktree",
                "add",
                "--detach",
                "--no-checkout",
                &worktree_text,
                baseline,
            ],
        )?;
        Ok(())
    }

    fn ensure_ait_excluded(&self, primary: &Path) -> Result<(), DomainError> {
        let common = self.git_stdout(primary, &["rev-parse", "--git-common-dir"])?;
        let common = PathBuf::from(common);
        let common = if common.is_absolute() {
            common
        } else {
            primary.join(common)
        };
        let info = common.join("info");
        std::fs::create_dir_all(&info).map_err(|failure| {
            error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot create Git info directory: {failure}"),
                false,
            )
        })?;
        let exclude = info.join("exclude");
        let existing = std::fs::read_to_string(&exclude).unwrap_or_default();
        if existing.lines().any(|line| line.trim() == "/.ait/") {
            return Ok(());
        }
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&exclude)
            .map_err(|failure| {
                error(
                    ErrorCode::ProjectGitInitFailed,
                    format!("cannot update Git info/exclude: {failure}"),
                    false,
                )
            })?;
        if !existing.is_empty() && !existing.ends_with('\n') {
            IoWrite::write_all(&mut file, b"\n").map_err(|failure| {
                error(
                    ErrorCode::ProjectGitInitFailed,
                    format!("cannot update Git info/exclude: {failure}"),
                    false,
                )
            })?;
        }
        IoWrite::write_all(&mut file, b"/.ait/\n").map_err(|failure| {
            error(
                ErrorCode::ProjectGitInitFailed,
                format!("cannot update Git info/exclude: {failure}"),
                false,
            )
        })
    }
}
