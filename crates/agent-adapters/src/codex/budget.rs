//! Bounds inherited from the supervising worker, independent of native tool schemas.
use crate::AgentEvent;
use std::collections::HashSet;

/// Native execution limits enforced as app-server usage and output arrive.
#[derive(Clone, Copy, Debug)]
pub struct CodexExecutionLimits {
    /// Maximum distinct native items in a turn.
    pub max_steps: u64,
    /// Maximum reported tokens in one native model invocation.
    pub max_tokens: u64,
    /// Maximum streamed text bytes and maximum individual item size.
    pub max_output_bytes: usize,
}

#[derive(Default)]
pub(super) struct Meter {
    items: HashSet<String>,
    text_bytes: usize,
}
impl Meter {
    pub(super) fn exceeded(
        &mut self,
        event: &AgentEvent,
        limits: Option<CodexExecutionLimits>,
    ) -> bool {
        let Some(limits) = limits else {
            return false;
        };
        match event {
            AgentEvent::ItemStarted { item } | AgentEvent::ItemCompleted { item } => {
                if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                    self.items.insert(id.to_owned());
                }
                self.items.len() as u64 > limits.max_steps
                    || serde_json::to_vec(item)
                        .map_or(true, |bytes| bytes.len() > limits.max_output_bytes)
            }
            AgentEvent::MessageDelta { delta, .. } => {
                self.text_bytes = self.text_bytes.saturating_add(delta.len());
                self.text_bytes > limits.max_output_bytes
            }
            AgentEvent::Usage { usage } => {
                usage
                    .total_tokens
                    .max(usage.input_tokens.saturating_add(usage.output_tokens))
                    > limits.max_tokens
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests;
