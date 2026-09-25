//! Metadata-owned connection subscriptions.

use std::collections::{BTreeMap, BTreeSet};

use crate::service::session::{SessionConnection, SessionSubscription};
use crate::service::workspace_labels::WorkspaceLabelSubscription;

pub(crate) mod daemon;
/// Session presence and event subscription handling.
pub mod session;
pub(crate) mod workspace_labels;

/// Metadata subscriptions belonging to one physical connection.
#[derive(Default)]
pub struct Connection {
    pub(crate) status: BTreeSet<String>,
    pub(crate) labels: BTreeMap<String, WorkspaceLabelSubscription>,
    pub(crate) events: BTreeMap<String, SessionSubscription>,
    /// Presence connection created during the API handshake.
    pub session: Option<SessionConnection>,
}

impl Connection {
    /// Attach a presence session created by the transport handshake.
    #[must_use]
    pub fn new(session: SessionConnection) -> Self {
        Self {
            session: Some(session),
            ..Self::default()
        }
    }

    /// Count active metadata subscriptions toward the shared connection budget.
    #[must_use]
    pub fn len(&self) -> usize {
        self.status
            .len()
            .saturating_add(self.labels.len())
            .saturating_add(self.events.len())
    }

    /// Whether this connection has no metadata subscriptions.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Release a matching metadata subscription, if present.
    pub fn release(&mut self, id: &str) {
        self.status.remove(id);
        self.labels.remove(id);
        self.events.remove(id);
    }

    /// Notify status observers that admission is draining.
    /// # Errors
    /// Returns an encoding or queue failure.
    pub fn draining(
        &self,
        outbound: &server_model::outbound::Outbound,
    ) -> Result<(), server_model::outbound::QueueError> {
        for subscription_id in &self.status {
            outbound.send(&server_model::ServerMessage::Status {
                subscription_id: subscription_id.clone(),
                lifecycle: server_model::Lifecycle::Draining,
            })?;
        }
        Ok(())
    }
}
