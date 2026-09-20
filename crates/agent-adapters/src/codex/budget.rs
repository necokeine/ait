//! Bounds inherited from the supervising worker, independent of native tool schemas.
use std::{
    collections::{BTreeMap, HashSet},
    fmt, io,
};

use ait_domain::{DomainError, DomainMetadata, ErrorCode};
use serde::Serialize;
use serde_json::Value;

use crate::AgentEvent;

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

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
enum LimitMetric {
    NativeItems,
    ItemBytes,
    TextBytes,
    Tokens,
}

impl fmt::Display for LimitMetric {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NativeItems => "native item count",
            Self::ItemBytes => "individual item size (bytes)",
            Self::TextBytes => "streamed text output (bytes)",
            Self::Tokens => "model invocation usage (tokens)",
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, thiserror::Error)]
#[error("Codex {metric} limit exceeded: observed {actual}, limit {limit}")]
pub(super) struct LimitExceeded {
    metric: LimitMetric,
    actual: u64,
    limit: u64,
}

impl From<LimitExceeded> for DomainError {
    fn from(failure: LimitExceeded) -> Self {
        Self {
            code: ErrorCode::RunLimitExceeded,
            message: failure.to_string(),
            retryable: false,
            details: Some(DomainMetadata(BTreeMap::from([
                ("metric".into(), serde_json::json!(failure.metric)),
                ("actual".into(), failure.actual.into()),
                ("limit".into(), failure.limit.into()),
            ]))),
            cause_id: None,
        }
    }
}

fn enforce(metric: LimitMetric, actual: u64, limit: u64) -> Result<(), LimitExceeded> {
    if actual > limit {
        return Err(LimitExceeded {
            metric,
            actual,
            limit,
        });
    }
    Ok(())
}

#[derive(Default)]
struct ByteCounter(u64);

impl io::Write for ByteCounter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len() as u64);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn serialized_size(item: &Value) -> u64 {
    let mut counter = ByteCounter::default();
    serde_json::to_writer(&mut counter, item)
        .expect("JSON values serialize to an infallible byte counter");
    counter.0
}

#[derive(Default)]
pub(super) struct Meter {
    items: HashSet<String>,
    text_bytes: u64,
    failure: Option<LimitExceeded>,
}
impl Meter {
    pub(super) fn check(
        &mut self,
        event: &AgentEvent,
        limits: Option<CodexExecutionLimits>,
    ) -> Result<(), LimitExceeded> {
        // Preserve the first cause while buffered events and cancellation are drained.
        if let Some(failure) = self.failure {
            return Err(failure);
        }
        let Some(limits) = limits else {
            return Ok(());
        };
        let result = match event {
            AgentEvent::ItemStarted { item } | AgentEvent::ItemCompleted { item } => {
                if let Some(id) = item.get("id").and_then(serde_json::Value::as_str) {
                    self.items.insert(id.to_owned());
                }
                enforce(
                    LimitMetric::NativeItems,
                    self.items.len() as u64,
                    limits.max_steps,
                )
                .and_then(|()| {
                    enforce(
                        LimitMetric::ItemBytes,
                        serialized_size(item),
                        limits.max_output_bytes as u64,
                    )
                })
            }
            AgentEvent::MessageDelta { delta, .. } => {
                self.text_bytes = self.text_bytes.saturating_add(delta.len() as u64);
                enforce(
                    LimitMetric::TextBytes,
                    self.text_bytes,
                    limits.max_output_bytes as u64,
                )
            }
            AgentEvent::Usage { usage } => enforce(
                LimitMetric::Tokens,
                usage
                    .total_tokens
                    .max(usage.input_tokens.saturating_add(usage.output_tokens)),
                limits.max_tokens,
            ),
            _ => Ok(()),
        };
        self.failure = result.err();
        result
    }
}

#[cfg(test)]
mod tests;
