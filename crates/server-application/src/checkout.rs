//! Checkout status, diff, refresh, and history use cases.

pub use server_ports::checkout::*;

/// Thin application boundary over the independent blocking Git adapter.
#[derive(Debug)]
pub struct Checkout {
    runtime: Box<dyn CheckoutRuntime>,
}

impl Checkout {
    /// Compose checkout use cases.
    #[must_use]
    pub fn new(runtime: Box<dyn CheckoutRuntime>) -> Self {
        Self { runtime }
    }

    /// Inspect checkout status.
    ///
    /// # Errors
    /// Returns categorized local Git/filesystem failures.
    pub fn status(&self, cwd: &str) -> Result<CheckoutStatus, CheckoutRuntimeError> {
        self.runtime.status(cwd)
    }

    /// Force a fresh checkout read.
    ///
    /// # Errors
    /// Returns categorized local Git/filesystem failures.
    pub fn refresh(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        self.runtime.refresh(cwd)
    }

    /// Read a structured checkout diff.
    ///
    /// # Errors
    /// Returns categorized local Git/filesystem failures.
    pub fn diff(
        &self,
        cwd: &str,
        compare: &CheckoutDiffCompare,
    ) -> Result<CheckoutDiff, CheckoutRuntimeError> {
        self.runtime.diff(cwd, compare)
    }

    /// List checkout commits plus bounded base context.
    ///
    /// # Errors
    /// Returns categorized local Git/filesystem failures.
    pub fn commits(&self, cwd: &str) -> Result<CheckoutCommits, CheckoutRuntimeError> {
        self.runtime.commits(cwd)
    }

    /// Read one textual file diff for a commit.
    ///
    /// # Errors
    /// Returns categorized input, Git, or filesystem failures.
    pub fn commit_file_diff(
        &self,
        cwd: &str,
        sha: &str,
        path: &str,
    ) -> Result<Option<ParsedDiffFile>, CheckoutRuntimeError> {
        self.runtime.commit_file_diff(cwd, sha, path)
    }

    /// Resolve a local or origin branch.
    ///
    /// # Errors
    /// Returns categorized validation or Git failures.
    pub fn validate_branch(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<CheckoutBranchResolution, CheckoutRuntimeError> {
        self.runtime.validate_branch(cwd, branch)
    }

    /// List branch suggestions.
    ///
    /// # Errors
    /// Returns categorized validation or Git failures.
    pub fn branch_suggestions(
        &self,
        cwd: &str,
        query: Option<&str>,
        limit: usize,
    ) -> Result<Vec<CheckoutBranchSuggestion>, CheckoutRuntimeError> {
        self.runtime.branch_suggestions(cwd, query, limit)
    }

    /// Check out an existing branch.
    ///
    /// # Errors
    /// Returns categorized dirty-tree, validation, or Git failures.
    pub fn switch_branch(
        &self,
        cwd: &str,
        branch: &str,
    ) -> Result<CheckoutBranchSource, CheckoutRuntimeError> {
        self.runtime.switch_branch(cwd, branch)
    }

    /// Rename the current branch.
    ///
    /// # Errors
    /// Returns categorized detached-head, validation, or Git failures.
    pub fn rename_branch(&self, cwd: &str, branch: &str) -> Result<String, CheckoutRuntimeError> {
        self.runtime.rename_branch(cwd, branch)
    }

    /// Commit checkout changes.
    ///
    /// # Errors
    /// Returns categorized validation or Git failures.
    pub fn commit(
        &self,
        cwd: &str,
        message: &str,
        add_all: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        self.runtime.commit(cwd, message, add_all)
    }

    /// Merge the current branch into its base checkout.
    ///
    /// # Errors
    /// Returns categorized preflight, conflict, or Git failures.
    pub fn merge_to_base(
        &self,
        cwd: &str,
        base_ref: Option<&str>,
        strategy: CheckoutMergeStrategy,
        require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        self.runtime
            .merge_to_base(cwd, base_ref, strategy, require_clean_target)
    }

    /// Merge the selected base into the current branch.
    ///
    /// # Errors
    /// Returns categorized preflight, conflict, or Git failures.
    pub fn merge_from_base(
        &self,
        cwd: &str,
        base_ref: Option<&str>,
        require_clean_target: bool,
    ) -> Result<(), CheckoutRuntimeError> {
        self.runtime
            .merge_from_base(cwd, base_ref, require_clean_target)
    }

    /// Pull the current branch.
    ///
    /// # Errors
    /// Returns categorized remote, conflict, or Git failures.
    pub fn pull(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        self.runtime.pull(cwd)
    }

    /// Push the current branch.
    ///
    /// # Errors
    /// Returns categorized remote or Git failures.
    pub fn push(&self, cwd: &str) -> Result<(), CheckoutRuntimeError> {
        self.runtime.push(cwd)
    }

    /// Discard selected checkout paths.
    ///
    /// # Errors
    /// Returns categorized path or Git failures.
    pub fn discard_changes(&self, cwd: &str, paths: &[String]) -> Result<(), CheckoutRuntimeError> {
        self.runtime.discard_changes(cwd, paths)
    }

    /// Save a Paseo-tagged stash.
    ///
    /// # Errors
    /// Returns categorized Git failures.
    pub fn stash_save(&self, cwd: &str, branch: Option<&str>) -> Result<(), CheckoutRuntimeError> {
        self.runtime.stash_save(cwd, branch)
    }

    /// Pop a stash.
    ///
    /// # Errors
    /// Returns categorized conflict or Git failures.
    pub fn stash_pop(&self, cwd: &str, index: usize) -> Result<(), CheckoutRuntimeError> {
        self.runtime.stash_pop(cwd, index)
    }

    /// List stashes.
    ///
    /// # Errors
    /// Returns categorized Git failures.
    pub fn stashes(
        &self,
        cwd: &str,
        paseo_only: bool,
    ) -> Result<Vec<CheckoutStashEntry>, CheckoutRuntimeError> {
        self.runtime.stashes(cwd, paseo_only)
    }
}
