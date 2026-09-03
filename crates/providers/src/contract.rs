//! Reusable conformance checks for every new adapter.

use std::collections::BTreeSet;

use futures_util::StreamExt;

use crate::{ProviderAdapter, ProviderError, ProviderEvent, ProviderInvocation, StopReason, Usage};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContractReport {
    pub events: usize,
    pub usage: Option<Usage>,
    pub stop_reason: StopReason,
}

/// Consume a real provider stream and enforce provider-neutral ordering rules.
/// Adapter authors can call this from their own integration test suite.
///
/// # Errors
///
/// Returns a description when stream creation fails, an event errors, or the
/// provider violates ordering, tool lifecycle, usage, or stop invariants.
pub async fn verify_stream_contract(
    adapter: &dyn ProviderAdapter,
    invocation: ProviderInvocation,
) -> Result<ContractReport, String> {
    let mut stream = adapter
        .stream(invocation)
        .await
        .map_err(|error| format!("stream creation failed: {error}"))?;
    let mut event_count = 0;
    let mut usage = None;
    let mut stop = None;
    let mut open_tools = BTreeSet::new();

    while let Some(item) = stream.next().await {
        let event = item.map_err(|error: ProviderError| error.to_string())?;
        if stop.is_some() {
            return Err("adapter emitted an event after Stop".to_owned());
        }
        event_count += 1;
        match event {
            ProviderEvent::ToolUseStart { index, .. } => {
                if !open_tools.insert(index) {
                    return Err(format!("duplicate ToolUseStart for index {index}"));
                }
            }
            ProviderEvent::ToolUseArgumentsDelta { index, .. } => {
                if !open_tools.contains(&index) {
                    return Err(format!(
                        "tool argument delta before start for index {index}"
                    ));
                }
            }
            ProviderEvent::ToolUseEnd { index } => {
                if !open_tools.remove(&index) {
                    return Err(format!("ToolUseEnd without start for index {index}"));
                }
            }
            ProviderEvent::Usage { usage: observed } => {
                if observed.total_tokens < observed.input_tokens + observed.output_tokens {
                    return Err("usage total is smaller than input + output".to_owned());
                }
                usage = Some(observed);
            }
            ProviderEvent::Stop { reason, .. } => stop = Some(reason),
            ProviderEvent::TextDelta { .. } => {}
        }
    }
    if !open_tools.is_empty() {
        return Err(format!("unclosed tool indices: {open_tools:?}"));
    }
    let stop_reason = stop.ok_or_else(|| "adapter stream ended without Stop".to_owned())?;
    Ok(ContractReport {
        events: event_count,
        usage,
        stop_reason,
    })
}
