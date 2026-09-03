//! Local HTTP fixtures for the OpenAI-compatible streaming adapter.

use std::collections::BTreeMap;

use ait_providers::{
    ContentPart, ProviderAdapter, ProviderCapabilities, ProviderErrorKind, ProviderInvocation,
    ProviderMessage, ProviderParameters, ProviderRequest, RetryDirective, Role, SecretValue,
    StopReason, contract::verify_stream_contract, openai::OpenAiCompatibleProvider,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn openai_compatible_adapter_normalizes_remote_sse() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 16 * 1024];
        let bytes = socket.read(&mut request).await.unwrap();
        let request = String::from_utf8_lossy(&request[..bytes]);
        assert!(request.contains("authorization: Bearer test-secret"));
        assert!(request.contains("\"stream\":true"));

        let body = concat!(
            "data: {\"choices\":[{\"delta\":{\"content\":\"hello\"},\"finish_reason\":null}]}\n\n",
            "data: {\"choices\":[{\"delta\":{},\"finish_reason\":\"stop\"}]}\n\n",
            "data: {\"choices\":[],\"usage\":{\"prompt_tokens\":3,\"completion_tokens\":2,\"total_tokens\":5}}\n\n",
            "data: [DONE]\n\n"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
        socket.shutdown().await.unwrap();
    });

    let request = ProviderRequest {
        messages: vec![ProviderMessage {
            role: Role::User,
            content: vec![ContentPart::Text {
                text: "hello".into(),
            }],
        }],
        tools: vec![],
        parameters: ProviderParameters {
            max_output_tokens: Some(20),
            temperature: Some(0.0),
            extra: BTreeMap::new(),
        },
        required_capabilities: ProviderCapabilities {
            streaming: true,
            usage: true,
            ..ProviderCapabilities::default()
        },
    };
    let invocation = ProviderInvocation {
        request_id: "remote-test".into(),
        model: "remote-model".into(),
        endpoint: Some(format!("http://{address}/v1/chat/completions")),
        credential: Some(SecretValue::new("test-secret")),
        request,
        cancellation: CancellationToken::new(),
    };
    let report = verify_stream_contract(&OpenAiCompatibleProvider::default(), invocation)
        .await
        .unwrap();
    assert_eq!(report.stop_reason, StopReason::Completed);
    assert_eq!(report.usage.unwrap().total_tokens, 5);
    server.await.unwrap();
}

#[tokio::test]
async fn openai_compatible_adapter_classifies_rate_limits() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        let mut request = vec![0; 4096];
        let _ = socket.read(&mut request).await.unwrap();
        let body = r#"{"error":{"message":"slow down","code":"rate_limit"}}"#;
        let response = format!(
            "HTTP/1.1 429 Too Many Requests\r\ncontent-type: application/json\r\nretry-after: 2\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        socket.write_all(response.as_bytes()).await.unwrap();
    });
    let invocation = ProviderInvocation {
        request_id: "rate-limit-test".into(),
        model: "remote-model".into(),
        endpoint: Some(format!("http://{address}/v1/chat/completions")),
        credential: Some(SecretValue::new("test-secret")),
        request: ProviderRequest {
            messages: vec![ProviderMessage {
                role: Role::User,
                content: vec![ContentPart::Text { text: "hi".into() }],
            }],
            tools: vec![],
            parameters: ProviderParameters::default(),
            required_capabilities: ProviderCapabilities::default(),
        },
        cancellation: CancellationToken::new(),
    };
    let Err(error) = OpenAiCompatibleProvider::default().stream(invocation).await else {
        panic!("rate-limited request unexpectedly succeeded");
    };
    assert_eq!(error.kind, ProviderErrorKind::RateLimited);
    assert_eq!(error.retry, RetryDirective::AfterMillis(2_000));
    assert_eq!(error.provider_code.as_deref(), Some("rate_limit"));
    server.await.unwrap();
}
