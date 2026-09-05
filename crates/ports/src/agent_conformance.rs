//! Shared fixtures and assertions for every [`crate::AgentInvoker`] adapter.

use std::sync::atomic::{AtomicUsize, Ordering};

use ait_domain::{AgentId, DomainMetadata, DurationMs, RunUsage};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

use crate::{
    AgentCallId, AgentCallLimits, AgentCapabilities, AgentError, AgentErrorKind, AgentEvent,
    AgentExecutionProfile, AgentInput, AgentInvoker, AgentOutputContract, AgentPurpose,
    AgentRequest, AgentRevisionSnapshot, AgentStopReason, DirectTaskKind, OperationId,
};

/// Summary produced after a complete valid event stream is drained.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentConformanceReport {
    /// Total successful stream events including the terminal event.
    pub events: usize,
    /// Number of usage deltas observed.
    pub usage_events: usize,
    /// Saturating sum of normalized usage deltas.
    pub usage: RunUsage,
    /// Stop reason from the unique final event.
    pub stop_reason: AgentStopReason,
}

/// Contract violation or normalized invocation failure found by the harness.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentConformanceError {
    /// The adapter returned a normalized invocation or stream error.
    Invocation(AgentError),
    /// The successful stream ended without `Completed`.
    MissingCompleted,
    /// The successful stream emitted more than one `Completed` event.
    DuplicateCompleted,
    /// A non-terminal event appeared after `Completed`.
    EventAfterCompleted,
    /// A request requiring usage completed without a usage event.
    MissingUsage,
    /// A preflight check did not return the expected stable error kind.
    UnexpectedPreflightResult {
        /// Expected failure class.
        expected: AgentErrorKind,
        /// Actual class, or `None` when invocation was accepted.
        actual: Option<AgentErrorKind>,
    },
    /// A network, spawn, credential, or file probe changed before rejection.
    SideEffectBeforePreflight {
        /// Probe value immediately before invocation.
        before: usize,
        /// Probe value immediately after invocation.
        after: usize,
    },
}

impl std::fmt::Display for AgentConformanceError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invocation(error) => write!(formatter, "Agent invocation failed: {error}"),
            Self::MissingCompleted => formatter.write_str("successful stream omitted Completed"),
            Self::DuplicateCompleted => {
                formatter.write_str("successful stream emitted Completed more than once")
            }
            Self::EventAfterCompleted => {
                formatter.write_str("stream emitted an item after Completed")
            }
            Self::MissingUsage => formatter.write_str("usage was required but not emitted"),
            Self::UnexpectedPreflightResult { expected, actual } => write!(
                formatter,
                "expected preflight error {}, got {actual:?}",
                expected.code()
            ),
            Self::SideEffectBeforePreflight { before, after } => write!(
                formatter,
                "side-effect probe changed from {before} to {after} before preflight rejection"
            ),
        }
    }
}

impl std::error::Error for AgentConformanceError {}

/// Drains one invocation and checks the successful-stream invariants.
///
/// A stream error ends the current attempt and is returned as
/// [`AgentConformanceError::Invocation`]; a stream error is not required to be
/// followed by `Completed`.
///
/// # Errors
///
/// Returns an invocation failure or a terminal/usage contract violation.
pub async fn verify_stream_contract(
    invoker: &dyn AgentInvoker,
    request: AgentRequest,
) -> Result<AgentConformanceReport, AgentConformanceError> {
    let require_usage = request.profile.require_usage;
    let mut stream = invoker
        .invoke(request)
        .await
        .map_err(AgentConformanceError::Invocation)?;
    let mut events = 0;
    let mut usage_events = 0;
    let mut usage = RunUsage::default();
    let mut completed = None;

    while let Some(item) = stream.next().await {
        let event = match item {
            Ok(event) => event,
            Err(error) if completed.is_none() => {
                return Err(AgentConformanceError::Invocation(error));
            }
            Err(_) => return Err(AgentConformanceError::EventAfterCompleted),
        };
        events += 1;
        if completed.is_some() {
            return if matches!(event, AgentEvent::Completed { .. }) {
                Err(AgentConformanceError::DuplicateCompleted)
            } else {
                Err(AgentConformanceError::EventAfterCompleted)
            };
        }
        match event {
            AgentEvent::Usage(delta) => {
                usage_events += 1;
                add_usage(&mut usage, &delta);
            }
            AgentEvent::Completed { stop_reason } => completed = Some(stop_reason),
            AgentEvent::TextDelta { .. }
            | AgentEvent::ProposedMessage { .. }
            | AgentEvent::Checkpoint(_)
            | AgentEvent::Activity(_) => {}
        }
    }

    let stop_reason = completed.ok_or(AgentConformanceError::MissingCompleted)?;
    if require_usage && usage_events == 0 {
        return Err(AgentConformanceError::MissingUsage);
    }
    Ok(AgentConformanceReport {
        events,
        usage_events,
        usage,
        stop_reason,
    })
}

/// Verifies an unsupported request is rejected before a supplied side-effect probe changes.
///
/// The probe should be incremented by a fake network client, process spawner,
/// credential resolver, or filesystem boundary used by the adapter test.
///
/// # Errors
///
/// Returns a contract violation when rejection is not immediate and stable.
pub async fn verify_capability_preflight(
    invoker: &dyn AgentInvoker,
    request: AgentRequest,
    side_effect_probe: &AtomicUsize,
) -> Result<(), AgentConformanceError> {
    verify_preflight_rejection(
        invoker,
        request,
        side_effect_probe,
        AgentErrorKind::CapabilityUnsupported,
    )
    .await
}

/// Verifies a pre-cancelled request is rejected before a supplied side-effect probe changes.
///
/// # Errors
///
/// Returns a contract violation when rejection is not immediate and stable.
pub async fn verify_cancellation_preflight(
    invoker: &dyn AgentInvoker,
    request: AgentRequest,
    side_effect_probe: &AtomicUsize,
) -> Result<(), AgentConformanceError> {
    request.cancellation.cancel();
    verify_preflight_rejection(
        invoker,
        request,
        side_effect_probe,
        AgentErrorKind::Cancelled,
    )
    .await
}

/// Creates a provider-neutral direct text fixture with deterministic limits.
#[must_use]
pub fn text_request_fixture(require_usage: bool) -> AgentRequest {
    AgentRequest {
        call_id: AgentCallId::new("call-fixture"),
        purpose: AgentPurpose::Direct {
            operation_id: OperationId::new("operation-fixture"),
            kind: DirectTaskKind::SessionMetadata,
        },
        input: AgentInput::Prompt("Summarize this session".into()),
        context_digest: "b".repeat(64),
        profile: AgentExecutionProfile {
            require_usage,
            ..AgentExecutionProfile::default()
        },
        tools: Vec::new(),
        output: AgentOutputContract::Text,
        limits: AgentCallLimits {
            timeout: DurationMs(5_000),
            ..AgentCallLimits::default()
        },
        resume_from: None,
        cancellation: CancellationToken::new(),
    }
}

/// Creates a non-secret fixed-revision fixture for adapter contract tests.
#[must_use]
pub fn revision_fixture(capabilities: AgentCapabilities) -> AgentRevisionSnapshot {
    AgentRevisionSnapshot {
        agent_id: AgentId::new("agent-fixture"),
        revision: 7,
        adapter_key: "fixture".into(),
        model: "fixture-model".into(),
        endpoint: None,
        credential_ref: None,
        capabilities,
        parameters: DomainMetadata::default(),
        config_digest: "a".repeat(64),
    }
}

fn add_usage(total: &mut RunUsage, delta: &RunUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.cached_input_tokens = total
        .cached_input_tokens
        .saturating_add(delta.cached_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.tool_executions = total.tool_executions.saturating_add(delta.tool_executions);
    total.cost = match (total.cost, delta.cost) {
        (None, None) => None,
        (Some(cost), None) | (None, Some(cost)) => Some(cost),
        (Some(left), Some(right)) => Some(ait_domain::CostMicros(left.0.saturating_add(right.0))),
    };
}

async fn verify_preflight_rejection(
    invoker: &dyn AgentInvoker,
    request: AgentRequest,
    side_effect_probe: &AtomicUsize,
    expected: AgentErrorKind,
) -> Result<(), AgentConformanceError> {
    let before = side_effect_probe.load(Ordering::SeqCst);
    let result = invoker.invoke(request).await;
    let after = side_effect_probe.load(Ordering::SeqCst);
    if before != after {
        return Err(AgentConformanceError::SideEffectBeforePreflight { before, after });
    }
    match result {
        Err(error) if error.kind() == expected && error.retry() == crate::RetryDirective::Never => {
            Ok(())
        }
        Err(error) => Err(AgentConformanceError::UnexpectedPreflightResult {
            expected,
            actual: Some(error.kind()),
        }),
        Ok(_) => Err(AgentConformanceError::UnexpectedPreflightResult {
            expected,
            actual: None,
        }),
    }
}
