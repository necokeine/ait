//! Forge search and pull request use cases.

pub use server_ports::forge::*;

/// Thin application boundary over the blocking forge adapter.
#[derive(Debug)]
pub struct Forge {
    runtime: Box<dyn ForgeRuntime>,
}

impl Forge {
    /// Compose forge use cases.
    #[must_use]
    pub fn new(runtime: Box<dyn ForgeRuntime>) -> Self {
        Self { runtime }
    }

    /// Search issues and change requests.
    ///
    /// # Errors
    /// Returns categorized CLI, authentication, remote, or parsing failures.
    pub fn search(
        &self,
        cwd: &str,
        query: &str,
        limit: usize,
        kinds: &[ForgeSearchKind],
    ) -> Result<ForgeSearch, ForgeRuntimeError> {
        self.runtime.search(cwd, query, limit, kinds)
    }

    /// Create a pull request for the current branch.
    ///
    /// # Errors
    /// Returns invalid metadata, Git push, CLI, authentication, or forge failures.
    pub fn create_pull_request(
        &self,
        cwd: &str,
        title: &str,
        body: &str,
        base_ref: Option<&str>,
    ) -> Result<PullRequestCreated, ForgeRuntimeError> {
        if title.trim().is_empty() || body.trim().is_empty() {
            return Err(ForgeRuntimeError {
                kind: ForgeFailureKind::Invalid,
                message: "Pull request title and body are required".to_owned(),
            });
        }
        self.runtime
            .create_pull_request(cwd, title.trim(), body.trim(), base_ref)
    }

    /// Read the current branch's pull request.
    ///
    /// # Errors
    /// Returns categorized local Git or forge failures.
    pub fn current_pull_request_status(
        &self,
        cwd: &str,
    ) -> Result<PullRequestStatusRead, ForgeRuntimeError> {
        self.runtime.current_pull_request_status(cwd)
    }

    /// Merge the current pull request.
    ///
    /// # Errors
    /// Returns current-request resolution, validation, or forge command failures.
    pub fn merge_current_pull_request(
        &self,
        cwd: &str,
        merge_method: PullRequestMergeMethod,
    ) -> Result<(), ForgeRuntimeError> {
        self.runtime.merge_current_pull_request(cwd, merge_method)
    }

    /// Enable or disable auto-merge.
    ///
    /// # Errors
    /// Rejects a missing enable method, an unexpected disable method, or forge failures.
    pub fn set_current_pull_request_auto_merge(
        &self,
        cwd: &str,
        enabled: bool,
        merge_method: Option<PullRequestMergeMethod>,
    ) -> Result<(), ForgeRuntimeError> {
        if enabled && merge_method.is_none() {
            return Err(ForgeRuntimeError {
                kind: ForgeFailureKind::Invalid,
                message: "mergeMethod is required when enabling auto-merge".to_owned(),
            });
        }
        if !enabled && merge_method.is_some() {
            return Err(ForgeRuntimeError {
                kind: ForgeFailureKind::Invalid,
                message: "mergeMethod is not allowed when disabling auto-merge".to_owned(),
            });
        }
        self.runtime
            .set_current_pull_request_auto_merge(cwd, enabled, merge_method)
    }

    /// Read a pull request timeline.
    ///
    /// # Errors
    /// Returns identity, CLI, authentication, or forge failures.
    pub fn pull_request_timeline(
        &self,
        cwd: &str,
        pr_number: u64,
        repo_owner: &str,
        repo_name: &str,
    ) -> Result<PullRequestTimeline, ForgeRuntimeError> {
        self.runtime
            .pull_request_timeline(cwd, pr_number, repo_owner, repo_name)
    }

    /// Read detailed check data.
    ///
    /// # Errors
    /// Returns invalid check identity, CLI, authentication, or forge failures.
    pub fn check_details(
        &self,
        cwd: &str,
        repo_owner: Option<&str>,
        repo_name: Option<&str>,
        check_run_id: Option<u64>,
        workflow_run_id: Option<u64>,
        change_request_number: Option<u64>,
    ) -> Result<CheckDetails, ForgeRuntimeError> {
        if check_run_id.is_none() && workflow_run_id.is_none() {
            return Err(ForgeRuntimeError {
                kind: ForgeFailureKind::Invalid,
                message:
                    "Check details request must address a check by checkRunId or workflowRunId"
                        .to_owned(),
            });
        }
        self.runtime.check_details(
            cwd,
            repo_owner,
            repo_name,
            check_run_id,
            workflow_run_id,
            change_request_number,
        )
    }
}

#[cfg(test)]
mod tests;
