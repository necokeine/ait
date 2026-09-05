//! Contract coverage shared by future Codex and provider invokers.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use ait_domain::{CostMicros, RunUsage};
use ait_ports::{
    AgentCapabilities, AgentCapability, AgentError, AgentErrorKind, AgentEvent, AgentEventStream,
    AgentInvoker, AgentOutputContract, ResolvedAgent, RetryDirective,
    agent_conformance::{
        AgentConformanceError, revision_fixture, text_request_fixture,
        verify_cancellation_preflight, verify_capability_preflight, verify_stream_contract,
    },
};
use async_trait::async_trait;

struct ScriptedInvoker {
    capabilities: AgentCapabilities,
    events: Mutex<Option<Vec<Result<AgentEvent, AgentError>>>>,
    side_effects: Arc<AtomicUsize>,
}

impl ScriptedInvoker {
    fn new(
        capabilities: AgentCapabilities,
        events: Vec<Result<AgentEvent, AgentError>>,
        side_effects: Arc<AtomicUsize>,
    ) -> Self {
        Self {
            capabilities,
            events: Mutex::new(Some(events)),
            side_effects,
        }
    }
}

#[async_trait]
impl AgentInvoker for ScriptedInvoker {
    fn capabilities(&self) -> AgentCapabilities {
        self.capabilities.clone()
    }

    async fn invoke(
        &self,
        _request: ait_ports::AgentRequest,
    ) -> Result<AgentEventStream, AgentError> {
        self.side_effects.fetch_add(1, Ordering::SeqCst);
        let events = self.events.lock().unwrap().take().unwrap_or_default();
        Ok(Box::pin(futures_util::stream::iter(events)))
    }
}

fn text_capabilities(require_usage: bool) -> AgentCapabilities {
    let mut values = vec![AgentCapability::Text, AgentCapability::Streaming];
    if require_usage {
        values.push(AgentCapability::Usage);
    }
    AgentCapabilities::new(values)
}

fn resolved(
    events: Vec<Result<AgentEvent, AgentError>>,
    capabilities: AgentCapabilities,
) -> (ResolvedAgent, Arc<AtomicUsize>) {
    let side_effects = Arc::new(AtomicUsize::new(0));
    let raw = Arc::new(ScriptedInvoker::new(
        capabilities.clone(),
        events,
        Arc::clone(&side_effects),
    ));
    let invoker = ResolvedAgent::new(revision_fixture(capabilities), raw).unwrap();
    (invoker, side_effects)
}

#[tokio::test]
async fn accepts_one_final_completed_event_and_accumulates_usage() {
    let capabilities = text_capabilities(true);
    let (invoker, side_effects) = resolved(
        vec![
            Ok(AgentEvent::TextDelta {
                text: "hello".into(),
            }),
            Ok(AgentEvent::Usage(RunUsage {
                input_tokens: 2,
                cached_input_tokens: 1,
                output_tokens: 3,
                tool_executions: 0,
                cost: Some(CostMicros(4)),
            })),
            Ok(AgentEvent::Usage(RunUsage {
                input_tokens: 5,
                cached_input_tokens: 0,
                output_tokens: 7,
                tool_executions: 1,
                cost: Some(CostMicros(6)),
            })),
            Ok(AgentEvent::Completed {
                stop_reason: ait_ports::AgentStopReason::EndTurn,
            }),
        ],
        capabilities,
    );

    let report = verify_stream_contract(&invoker, text_request_fixture(true))
        .await
        .unwrap();
    assert_eq!(report.events, 4);
    assert_eq!(report.usage_events, 2);
    assert_eq!(report.usage.input_tokens, 7);
    assert_eq!(report.usage.cached_input_tokens, 1);
    assert_eq!(report.usage.output_tokens, 10);
    assert_eq!(report.usage.tool_executions, 1);
    assert_eq!(report.usage.cost, Some(CostMicros(10)));
    assert_eq!(side_effects.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn rejects_missing_duplicate_or_non_final_completed_events() {
    let cases = [
        (
            vec![Ok(AgentEvent::TextDelta {
                text: "hello".into(),
            })],
            AgentConformanceError::MissingCompleted,
        ),
        (
            vec![
                Ok(AgentEvent::Completed {
                    stop_reason: ait_ports::AgentStopReason::EndTurn,
                }),
                Ok(AgentEvent::Completed {
                    stop_reason: ait_ports::AgentStopReason::EndTurn,
                }),
            ],
            AgentConformanceError::DuplicateCompleted,
        ),
        (
            vec![
                Ok(AgentEvent::Completed {
                    stop_reason: ait_ports::AgentStopReason::EndTurn,
                }),
                Ok(AgentEvent::TextDelta {
                    text: "late".into(),
                }),
            ],
            AgentConformanceError::EventAfterCompleted,
        ),
    ];

    for (events, expected) in cases {
        let (invoker, _) = resolved(events, text_capabilities(false));
        assert_eq!(
            verify_stream_contract(&invoker, text_request_fixture(false))
                .await
                .unwrap_err(),
            expected
        );
    }
}

#[tokio::test]
async fn capability_mismatch_fails_before_the_raw_adapter_side_effect() {
    let capabilities = text_capabilities(false);
    let (invoker, side_effects) = resolved(Vec::new(), capabilities);
    let mut request = text_request_fixture(false);
    request.output = AgentOutputContract::JsonSchema(serde_json::json!({
        "type": "object"
    }));

    verify_capability_preflight(&invoker, request, &side_effects)
        .await
        .unwrap();
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn cancellation_fails_before_the_raw_adapter_side_effect() {
    let capabilities = text_capabilities(false);
    let (invoker, side_effects) = resolved(Vec::new(), capabilities);

    verify_cancellation_preflight(&invoker, text_request_fixture(false), &side_effects)
        .await
        .unwrap();
    assert_eq!(side_effects.load(Ordering::SeqCst), 0);
}

#[test]
fn stable_error_kinds_only_accept_safe_retry_directives() {
    for kind in [
        AgentErrorKind::InvalidRequest,
        AgentErrorKind::InvalidConfiguration,
        AgentErrorKind::RevisionNotFound,
        AgentErrorKind::CapabilityUnsupported,
        AgentErrorKind::Authentication,
        AgentErrorKind::PermissionDenied,
        AgentErrorKind::Protocol,
        AgentErrorKind::Cancelled,
        AgentErrorKind::Internal,
    ] {
        let error = AgentError::classified(kind, "safe", RetryDirective::Never).unwrap();
        assert_eq!(error.kind(), kind);
        assert!(AgentError::classified(kind, "unsafe", RetryDirective::Backoff).is_err());
    }
    for kind in [
        AgentErrorKind::Connection,
        AgentErrorKind::Timeout,
        AgentErrorKind::Unavailable,
    ] {
        assert!(AgentError::classified(kind, "retry", RetryDirective::Backoff).is_ok());
        assert!(AgentError::classified(kind, "unsafe", RetryDirective::Never).is_err());
    }
    assert!(
        AgentError::classified(
            AgentErrorKind::RateLimited,
            "retry later",
            RetryDirective::After(ait_domain::DurationMs(250))
        )
        .is_ok()
    );
    assert!(
        AgentError::classified(AgentErrorKind::RateLimited, "unsafe", RetryDirective::Never)
            .is_err()
    );
}

#[test]
fn serialized_revision_contains_only_a_credential_reference() {
    let mut revision = revision_fixture(text_capabilities(false));
    revision.credential_ref = Some(ait_ports::CredentialRef::new("keychain://agent-fixture"));
    let serialized = serde_json::to_string(&revision).unwrap();
    assert!(serialized.contains("keychain://agent-fixture"));
    assert!(!serialized.contains("super-secret"));
}
