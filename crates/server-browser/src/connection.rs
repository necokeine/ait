//! Physical connection ownership for browser-host subscriptions.
use crate::broker::{Broker, Registration};
use serde_json::{Value, json};
use server_model::{ErrorCode, outbound::Outbound};
use std::collections::{BTreeMap, BTreeSet};

/// Host leases released on explicit subscription release or physical disconnect.
#[derive(Debug, Default)]
pub struct Connection {
    hosts: BTreeMap<String, Registration>,
}
impl Connection {
    /// Number of occupied connection subscription slots.
    #[must_use]
    pub fn len(&self) -> usize {
        self.hosts.len()
    }
    /// Whether this connection owns no browser registrations.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.hosts.is_empty()
    }
    /// Release only registrations owned by this connection.
    pub fn release(&mut self, id: &str) {
        self.hosts.remove(id);
    }
    /// Register a host and return its subscription identity.
    /// # Errors
    /// Returns validation or broker capacity errors before publishing a lease.
    pub fn register(
        &mut self,
        broker: &Broker,
        params: Value,
        outbound: Outbound,
    ) -> Result<Value, ErrorCode> {
        let request = serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
        let lease = broker.register(request, outbound)?;
        let id = lease.id().to_owned();
        self.hosts.insert(id.clone(), lease);
        Ok(json!({"subscriptionId":id}))
    }
    /// Route a callback using only this connection's registration identities.
    pub fn receive(&self, broker: &Broker, id: &str, params: Value) {
        let owners: BTreeSet<_> = self.hosts.keys().map(String::as_str).collect();
        let _ = broker.receive(id, params, &owners);
    }
}
