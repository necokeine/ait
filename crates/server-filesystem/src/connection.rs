//! Filesystem-owned connection observers and upload state.

use std::collections::BTreeMap;

pub(crate) mod checkout;
pub mod files;

/// Filesystem resources belonging to one physical connection.
#[derive(Default)]
pub struct Connection {
    /// File subscriptions and unfinished uploads.
    pub files: files::FileConnection,
    pub(crate) diffs: BTreeMap<String, checkout::CheckoutDiffSubscription>,
}

impl Connection {
    /// Count active filesystem subscriptions.
    #[must_use]
    pub fn len(&self) -> usize {
        self.diffs.len().saturating_add(self.files.len())
    }

    /// Whether the connection has no filesystem subscriptions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Release a matching file or diff observer.
    pub fn release(&mut self, id: &str) {
        self.diffs.remove(id);
        self.files.release(id);
    }
}
