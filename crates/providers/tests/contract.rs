//! Reusable provider contract and credential-boundary tests.

use std::sync::Arc;

use ait_providers::{
    AgentCatalog, AgentDefinition, ContentPart, CredentialRef, ProviderAdapter,
    ProviderCapabilities, ProviderEvent, ProviderInvocation, ProviderMessage, ProviderParameters,
    ProviderRequest, Role, SecretValue, StopReason, Usage, contract::verify_stream_contract,
    mock::ScriptedProvider, provider::invocation_from_revision, secret::InMemoryCredentialResolver,
};
use futures_util::StreamExt;
use tokio_util::sync::CancellationToken;

fn capabilities() -> ProviderCapabilities {
    ProviderCapabilities {
        streaming: true,
        tool_calling: true,
        parallel_tool_calls: true,
        usage: true,
        system_messages: true,
    }
}

fn request() -> ProviderRequest {
    ProviderRequest {
        messages: vec![ProviderMessage {
            role: Role::User,
            content: vec![ContentPart::Text {
                text: "hello".into(),
            }],
        }],
        tools: vec![],
        parameters: ProviderParameters::default(),
        required_capabilities: ProviderCapabilities {
            streaming: true,
            ..ProviderCapabilities::default()
        },
    }
}

fn invocation(request: ProviderRequest) -> ProviderInvocation {
    ProviderInvocation {
        request_id: "test-request".into(),
        model: "mock-model".into(),
        endpoint: None,
        credential: None,
        request,
        cancellation: CancellationToken::new(),
    }
}

#[tokio::test]
async fn reusable_contract_accepts_a_valid_mock_adapter() {
    let provider = ScriptedProvider::new(
        capabilities(),
        vec![
            Ok(ProviderEvent::TextDelta { text: "hi".into() }),
            Ok(ProviderEvent::Usage {
                usage: Usage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    cached_input_tokens: None,
                },
            }),
            Ok(ProviderEvent::Stop {
                reason: StopReason::Completed,
                provider_reason: Some("stop".into()),
            }),
        ],
    );
    let report = verify_stream_contract(&provider, invocation(request()))
        .await
        .unwrap();
    assert_eq!(report.stop_reason, StopReason::Completed);
    assert_eq!(report.events, 3);
}

#[tokio::test]
async fn reusable_contract_accepts_ordered_tool_events() {
    let provider = ScriptedProvider::new(
        capabilities(),
        vec![
            Ok(ProviderEvent::ToolUseStart {
                index: 0,
                call_id: "call-1".into(),
                name: "lookup".into(),
            }),
            Ok(ProviderEvent::ToolUseArgumentsDelta {
                index: 0,
                delta: "{\"query\":\"rust\"}".into(),
            }),
            Ok(ProviderEvent::ToolUseEnd { index: 0 }),
            Ok(ProviderEvent::Stop {
                reason: StopReason::ToolUse,
                provider_reason: Some("tool_calls".into()),
            }),
        ],
    );
    let report = verify_stream_contract(&provider, invocation(request()))
        .await
        .unwrap();
    assert_eq!(report.stop_reason, StopReason::ToolUse);
}

#[tokio::test]
async fn capabilities_reject_before_invocation() {
    let provider = ScriptedProvider::new(ProviderCapabilities::default(), vec![]);
    let Err(error) = provider.stream(invocation(request())).await else {
        panic!("unsupported request unexpectedly succeeded");
    };
    assert_eq!(
        error.kind,
        ait_providers::ProviderErrorKind::CapabilityUnsupported
    );
}

#[test]
fn agent_revisions_are_append_only_and_pinnable() {
    let catalog = AgentCatalog::default();
    let base = AgentDefinition {
        id: "agent-a".into(),
        name: "Agent A".into(),
        driver: "mock".into(),
        model: "v1".into(),
        endpoint: None,
        credential_ref: None,
        capabilities: capabilities(),
        default_parameters: ProviderParameters::default(),
        enabled: true,
    };
    let first = catalog.publish(base.clone()).unwrap();
    let mut changed = base;
    changed.model = "v2".into();
    let second = catalog.publish(changed).unwrap();
    assert_eq!(first.revision, 1);
    assert_eq!(second.revision, 2);
    assert_eq!(
        catalog.pin("agent-a", Some(1)).unwrap().definition.model,
        "v1"
    );
    assert_eq!(catalog.pin("agent-a", None).unwrap().definition.model, "v2");
}

#[tokio::test]
async fn only_credential_reference_is_serialized() {
    let catalog = AgentCatalog::default();
    let revision = catalog
        .publish(AgentDefinition {
            id: "remote".into(),
            name: "Remote".into(),
            driver: "openai_compatible".into(),
            model: "model".into(),
            endpoint: Some("https://example.invalid/v1/chat/completions".into()),
            credential_ref: Some(CredentialRef("keychain://remote".into())),
            capabilities: capabilities(),
            default_parameters: ProviderParameters::default(),
            enabled: true,
        })
        .unwrap();
    let resolver = Arc::new(InMemoryCredentialResolver::default());
    resolver.insert(
        CredentialRef("keychain://remote".into()),
        SecretValue::new("super-secret"),
    );
    let serialized = serde_json::to_string(&revision).unwrap();
    assert!(serialized.contains("keychain://remote"));
    assert!(!serialized.contains("super-secret"));
    let invocation = invocation_from_revision(
        &revision,
        "request",
        request(),
        resolver,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert!(!format!("{invocation:?}").contains("super-secret"));
}

#[tokio::test]
async fn cancellation_is_normalized() {
    let token = CancellationToken::new();
    token.cancel();
    let provider = ScriptedProvider::new(
        capabilities(),
        vec![Ok(ProviderEvent::Stop {
            reason: StopReason::Completed,
            provider_reason: None,
        })],
    );
    let mut invocation = invocation(request());
    invocation.cancellation = token;
    let mut stream = provider.stream(invocation).await.unwrap();
    let error = stream.next().await.unwrap().unwrap_err();
    assert_eq!(error.kind, ait_providers::ProviderErrorKind::Cancelled);
}
