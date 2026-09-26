//! Provider-reported usage snapshots; missing facts are never inferred from prices or quotas.

use serde::{Deserialize, Serialize};

/// Latest provider token, cost and context facts, compatible with Paseo `AgentUsage`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct AgentUsage {
    /// Reported input tokens, with the provider's own accounting scope.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_tokens: Option<u64>,
    /// Reported cached input tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cached_input_tokens: Option<u64>,
    /// Reported output tokens.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_tokens: Option<u64>,
    /// Native reported cost in USD; absent for subscription providers without cost reporting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_cost_usd: Option<f64>,
    /// Native reported maximum context size.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_max_tokens: Option<u64>,
    /// Tokens occupying the active model request, rather than cumulative session usage.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context_window_used_tokens: Option<u64>,
}

impl AgentUsage {
    /// Reject negative/nonfinite costs or token counters outside exact JSON integer range.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        [
            self.input_tokens,
            self.cached_input_tokens,
            self.output_tokens,
            self.context_window_max_tokens,
            self.context_window_used_tokens,
        ]
        .into_iter()
        .flatten()
        .all(|tokens| tokens <= 9_007_199_254_740_991)
            && self
                .total_cost_usd
                .is_none_or(|cost| cost.is_finite() && cost >= 0.0)
    }
}

#[cfg(test)]
mod tests;
