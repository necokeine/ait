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
}
