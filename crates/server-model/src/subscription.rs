//! Connection-owned subscription lifecycle payloads.

use serde::{Deserialize, Serialize};

/// Release a server-assigned connection subscription.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionReleaseRequest {
    /// Connection-local subscription identity.
    pub subscription_id: String,
}

/// Idempotent subscription release response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscriptionReleaseResult {
    /// Requested connection-local subscription identity.
    pub subscription_id: String,
}
