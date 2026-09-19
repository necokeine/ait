//! Safe optional auto-commit: isolated index, durable object receipt, atomic ref transaction.
use super::{blocking::BlockingContext, error, git_line};
use ait_domain::{DomainError, ErrorCode};
use ait_workspace::{RunCommitBaseline, RunCommitPlan};
use std::{
    fs,
    path::{Path, PathBuf},
};

fn failed(message: &str) -> DomainError {
    error(ErrorCode::ProjectGitHeadUnavailable, message, true)
}
fn changed() -> DomainError {
    error(
        ErrorCode::ProjectGitDirty,
        "HEAD, branch, index or prepared files changed; auto-commit skipped",
        false,
    )
}

struct IndexLock {
    path: PathBuf,
    receipt: PathBuf,
}
impl Drop for IndexLock {
    fn drop(&mut self) {
        if same_file::is_same_file(&self.path, &self.receipt).unwrap_or(false) {
            let _ = fs::remove_file(&self.path);
        }
    }
}

impl BlockingContext {
    fn commit_git(
        &self,
        cwd: &Path,
        args: &[&str],
        index: Option<&Path>,
    ) -> Result<String, DomainError> {
        let mut command = self.command();
        command.arg("-C").arg(cwd).args(args);
        if let Some(index) = index {
            command.env("GIT_INDEX_FILE", index);
        }
        let output = command
            .output()
            .map_err(|_| failed("Git auto-commit command failed"))?;
        if !output.status.success() {
            return Err(failed("Git auto-commit command failed"));
        }
        Ok(git_line(&output.stdout)?.to_owned())
    }

    /// Reads an index copy so Git never needs to acquire the real index lock.
    fn read_index_tree(&self, cwd: &Path, index: &Path) -> Result<String, DomainError> {
        let snapshot = tempfile::NamedTempFile::new_in(self.absolute_git_dir(cwd)?)
            .map_err(|_| failed("cannot allocate index snapshot"))?;
        fs::copy(index, snapshot.path()).map_err(|_| failed("cannot snapshot Git index"))?;
        self.commit_git(cwd, &["write-tree"], Some(snapshot.path()))
    }

    pub(super) fn verify_commit_baseline(
        &self,
        cwd: &Path,
        baseline: &RunCommitBaseline,
    ) -> Result<(), DomainError> {
        if self.git_head(cwd)?.as_deref() != Some(&baseline.head)
            || self.git_symbolic_head(cwd)? != baseline.branch
            || self.read_index_tree(cwd, &self.absolute_git_dir(cwd)?.join("index"))?
                != baseline.index_tree
        {
            return Err(changed());
        }
        Ok(())
    }

    fn candidate_index(
        &self,
        cwd: &Path,
        tree: &str,
        stage: bool,
    ) -> Result<tempfile::TempPath, DomainError> {
        let file = tempfile::NamedTempFile::new_in(self.absolute_git_dir(cwd)?)
            .map_err(|_| failed("cannot allocate temporary Git index"))?
            .into_temp_path();
        fs::remove_file(&file).map_err(|_| failed("cannot initialize temporary Git index"))?;
        self.commit_git(cwd, &["read-tree", tree], Some(&file))?;
        if stage {
            self.commit_git(cwd, &["add", "--all", "--", "."], Some(&file))?;
        }
        Ok(file)
    }

    pub(super) fn prepare_commit(
        &self,
        cwd: &Path,
        baseline: &RunCommitBaseline,
        run_id: &str,
    ) -> Result<Option<RunCommitPlan>, DomainError> {
        self.verify_commit_baseline(cwd, baseline)?;
        let index = self.candidate_index(cwd, &baseline.head, true)?;
        let tree = self.commit_git(cwd, &["write-tree"], Some(&index))?;
        self.verify_commit_baseline(cwd, baseline)?;
        if tree == baseline.index_tree {
            return Ok(None);
        }
        let message = format!("ait: complete Codex run\n\nAit-Run: {run_id}");
        let commit_id = self.commit_git(
            cwd,
            &[
                "-c",
                "user.name=Ait",
                "-c",
                "user.email=ait@localhost",
                "commit-tree",
                &tree,
                "-p",
                &baseline.head,
                "-m",
                &message,
            ],
            None,
        )?;
        Ok(Some(RunCommitPlan {
            baseline: baseline.clone(),
            tree,
            commit_id,
        }))
    }

    fn validate_commit_plan(&self, cwd: &Path, plan: &RunCommitPlan) -> Result<(), DomainError> {
        // Validate durable receipt before following it across a process restart.
        if ![&plan.commit_id, &plan.tree, &plan.baseline.head]
            .iter()
            .all(|value| {
                matches!(value.len(), 40 | 64) && value.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        {
            return Err(failed("invalid auto-commit object identity"));
        }
        if self.commit_git(
            cwd,
            &["rev-parse", &format!("{}^{{tree}}", plan.commit_id)],
            None,
        )? != plan.tree
            || self.commit_git(cwd, &["rev-parse", &format!("{}^", plan.commit_id)], None)?
                != plan.baseline.head
        {
            return Err(failed("invalid auto-commit receipt"));
        }
        Ok(())
    }

    pub(super) fn publish_commit(
        &self,
        cwd: &Path,
        plan: &RunCommitPlan,
    ) -> Result<(), DomainError> {
        self.validate_commit_plan(cwd, plan)?;
        let head = self.git_head(cwd)?;
        let already_published = head.as_deref() == Some(&plan.commit_id);
        if !already_published {
            self.verify_commit_baseline(cwd, &plan.baseline)?;
        } else if self.git_symbolic_head(cwd)? != plan.baseline.branch {
            return Err(changed());
        }
        let index_path = self.absolute_git_dir(cwd)?.join("index");
        let current_tree = self.read_index_tree(cwd, &index_path)?;
        let original_index =
            fs::read(&index_path).map_err(|_| failed("cannot inspect Git index"))?;
        if current_tree != plan.baseline.index_tree
            && !(already_published && current_tree == plan.tree)
        {
            return Err(changed());
        }
        if already_published && current_tree == plan.tree {
            return Ok(());
        }
        let prepared = self.candidate_index(cwd, &plan.tree, false)?;
        if !already_published {
            let observed = self.candidate_index(cwd, &plan.baseline.head, true)?;
            if self.commit_git(cwd, &["write-tree"], Some(&observed))? != plan.tree {
                return Err(changed());
            }
        }
        let lock_path = index_path.with_file_name("index.lock");
        let receipt = index_path.with_file_name(format!("ait-commit-{}.index", plan.commit_id));
        fs::File::open(&prepared)
            .and_then(|file| file.sync_all())
            .map_err(|_| failed("cannot persist prepared index"))?;
        match fs::hard_link(&prepared, &receipt) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if self.read_index_tree(cwd, &receipt)? != plan.tree {
                    return Err(changed());
                }
            }
            Err(_) => return Err(failed("cannot create index receipt")),
        }
        match fs::hard_link(&receipt, &lock_path) {
            Ok(()) => {}
            Err(error)
                if error.kind() == std::io::ErrorKind::AlreadyExists
                    && same_file::is_same_file(&receipt, &lock_path).unwrap_or(false) => {}
            Err(_) => return Err(failed("Git index is locked; auto-commit can be retried")),
        }
        let owned = IndexLock {
            path: lock_path,
            receipt,
        };
        fs::File::open(self.absolute_git_dir(cwd)?)
            .and_then(|file| file.sync_all())
            .map_err(|_| failed("cannot persist index lock receipt"))?;
        if fs::read(&index_path).map_err(|_| failed("cannot recheck Git index"))? != original_index
        {
            return Err(changed());
        }
        if !already_published {
            let commands = format!(
                "start\nupdate HEAD {} {}\nprepare\n",
                plan.commit_id, plan.baseline.head
            );
            self.check()?;
            self.retain(cwd, "auto_commit_publication_started");
            // Git locks both HEAD and its referent during prepare. Validate the
            // branch while those locks are held, then authorize the ref update.
            self.command()
                .arg("-C")
                .arg(cwd)
                .args(["update-ref", "--stdin"])
                .prepared_transaction(&commands, || {
                    if self.git_symbolic_head(cwd)? != plan.baseline.branch {
                        return Err(changed());
                    }
                    Ok(())
                })?;
            self.point("auto_commit_ref_published");
        }
        fs::rename(&owned.path, &index_path)
            .map_err(|_| failed("commit published; index reconciliation required"))?;
        fs::File::open(self.absolute_git_dir(cwd)?)
            .and_then(|file| file.sync_all())
            .map_err(|_| failed("commit published; Git directory durability unconfirmed"))?;
        let _ = fs::remove_file(&owned.receipt);
        Ok(())
    }
}

#[cfg(test)]
mod tests;
