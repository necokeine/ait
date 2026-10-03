//! Coalesced wakeups for directory projections backed by committed state.

use tokio::sync::watch;

/// Shared signal for changes that may affect a directory snapshot.
#[derive(Debug, Clone)]
pub struct Changes(watch::Sender<()>);

impl Default for Changes {
    fn default() -> Self {
        let (sender, _) = watch::channel(());
        Self(sender)
    }
}

impl Changes {
    /// Wake current observers after the owning service commits a change.
    pub fn notify(&self) {
        self.0.send_replace(());
    }

    /// Observe future changes. A caller should compare its initial snapshot once after subscribing.
    #[must_use]
    pub fn subscribe(&self) -> watch::Receiver<()> {
        self.0.subscribe()
    }
}

#[cfg(test)]
mod tests;
