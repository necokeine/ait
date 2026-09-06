//! Real Rig clients against local HTTP fixtures; no credentials or API spend.

use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

use ait_agent_adapters::{
    AdapterErrorKind, LLMClient, LLMClientConfig, LLMProvider,
    llm::{AssistantContent, Message},
};
use axum::{
    Router, body::to_bytes, extract::Request, http::StatusCode, response::IntoResponse,
    routing::any,
};
use serde_json::{Value, json};
use tokio::{net::TcpListener, sync::mpsc, task::JoinHandle};

const TEST_KEY: &str = "local-fixture-key";
const PROVIDERS: [LLMProvider; 2] = [LLMProvider::OpenAI, LLMProvider::DeepSeek];

struct RecordedRequest {
    method: String,
    path: String,
    authorization: String,
    body: Value,
}

struct Fixture {
    base_url: String,
    requests: mpsc::UnboundedReceiver<RecordedRequest>,
    server: JoinHandle<()>,
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}

impl Fixture {
    async fn new(responses: Vec<(StatusCode, Value)>) -> Self {
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let (sender, requests) = mpsc::unbounded_channel();
        let app = Router::new().fallback(any(move |request: Request| {
            let responses = responses.clone();
            let sender = sender.clone();
            async move {
                let (parts, body) = request.into_parts();
                let bytes = to_bytes(body, 1024 * 1024).await.unwrap();
                sender
                    .send(RecordedRequest {
                        method: parts.method.to_string(),
                        path: parts.uri.path().to_owned(),
                        authorization: parts.headers["authorization"].to_str().unwrap().to_owned(),
                        body: if bytes.is_empty() {
                            Value::Null
                        } else {
                            serde_json::from_slice(&bytes).unwrap()
                        },
                    })
                    .unwrap();
                let (status, body) = responses.lock().unwrap().pop_front().unwrap_or((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({"error": "unexpected retry"}),
                ));
                (status, axum::Json(body)).into_response()
            }
        }));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            base_url: format!("http://{address}/gateway/v1/"),
            requests,
            server,
        }
    }

    fn client(&self, provider: LLMProvider) -> LLMClient {
        let mut config = LLMClientConfig::new(provider, TEST_KEY);
        config.base_url = Some(self.base_url.clone());
        LLMClient::new(config).unwrap()
    }

    async fn request(&mut self, method: &str, path: &str) -> Value {
        let request = tokio::time::timeout(Duration::from_secs(2), self.requests.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(request.method, method);
        assert_eq!(request.path, format!("/gateway/v1/{path}"));
        assert_eq!(request.authorization, format!("Bearer {TEST_KEY}"));
        request.body
    }
}

fn completion(provider: LLMProvider) -> Value {
    match provider {
        LLMProvider::OpenAI => json!({
            "id": "resp_fixture", "object": "response", "created_at": 0,
            "status": "completed", "model": "fixture-model", "tools": [],
            "output": [{
                "type": "message", "id": "msg_fixture", "status": "completed", "role": "assistant",
                "content": [{"type": "output_text", "annotations": [], "text": "hello"}]
            }],
            "usage": {"input_tokens": 3, "output_tokens": 2, "total_tokens": 5}
        }),
        LLMProvider::DeepSeek => json!({
            "id": "chatcmpl_fixture", "object": "chat.completion", "created": 0,
            "model": "fixture-model",
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "hello"}, "finish_reason": "stop"}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 2, "total_tokens": 5}
        }),
    }
}

fn completion_path(provider: LLMProvider) -> &'static str {
    match provider {
        LLMProvider::OpenAI => "responses",
        LLMProvider::DeepSeek => "chat/completions",
    }
}

#[tokio::test]
async fn both_providers_list_live_models_with_the_configured_key_and_api_root() {
    for provider in PROVIDERS {
        let mut fixture = Fixture::new(vec![
            (
                StatusCode::OK,
                json!({
                    "object": "list", "data": [
                        {"id": "fixture-model", "created": 42, "owned_by": "fixture"},
                        {"id": "another-model"}
                    ]
                }),
            ),
            (StatusCode::OK, json!({"data": []})),
        ])
        .await;
        let client = fixture.client(provider);
        let models = client.list_models().await.unwrap();
        assert_eq!(models.len(), 2);
        assert_eq!(models.data[0].id, "fixture-model");
        assert_eq!(models.data[0].created_at, Some(42));
        assert_eq!(models.data[0].owned_by.as_deref(), Some("fixture"));
        assert_eq!(models.data[1].id, "another-model");
        fixture.request("GET", "models").await;
        assert!(client.list_models().await.unwrap().is_empty());
        fixture.request("GET", "models").await;
    }
}

#[tokio::test]
async fn both_providers_send_one_rig_completion_and_preserve_content_and_usage() {
    for provider in PROVIDERS {
        let mut fixture = Fixture::new(vec![(StatusCode::OK, completion(provider))]).await;
        let client = fixture.client(provider);
        let mut request = client.completion_request("fixture-model", "hi");
        request.chat_history.insert(0, Message::system("be brief"));
        request.temperature = Some(0.5);
        request.max_tokens = Some(64);
        let response = client.complete(request).await.unwrap();
        assert!(
            matches!(&response.choice[0], AssistantContent::Text(text) if text.text == "hello")
        );
        assert_eq!(response.usage.input_tokens, 3);
        assert_eq!(response.usage.output_tokens, 2);
        assert_eq!(response.usage.total_tokens, 5);
        assert!(response.response_id.is_some());
        let body = fixture.request("POST", completion_path(provider)).await;
        assert_eq!(body["model"], "fixture-model");
        assert_eq!(body["temperature"], 0.5);
        assert_ne!(body["stream"], true);
        match provider {
            LLMProvider::OpenAI => {
                assert_eq!(body["max_output_tokens"], 64);
                assert!(body["input"].to_string().contains("hi"));
                assert!(body.to_string().contains("be brief"));
            }
            LLMProvider::DeepSeek => {
                assert_eq!(body["max_tokens"], 64);
                assert_eq!(body["messages"][0]["content"], "be brief");
                assert_eq!(body["messages"][1]["content"], "hi");
            }
        }
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn prompt_works_for_both_providers() {
    for provider in PROVIDERS {
        let mut fixture = Fixture::new(vec![(StatusCode::OK, completion(provider))]).await;
        assert_eq!(
            fixture
                .client(provider)
                .prompt("fixture-model", "hi")
                .await
                .unwrap(),
            "hello"
        );
        fixture.request("POST", completion_path(provider)).await;
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn http_errors_are_classified_redacted_and_never_retried() {
    for provider in PROVIDERS {
        for (status, kind, retryable) in [
            (401, AdapterErrorKind::Authentication, false),
            (403, AdapterErrorKind::Authentication, false),
            (429, AdapterErrorKind::RateLimited, true),
            (503, AdapterErrorKind::Unavailable, true),
            (400, AdapterErrorKind::Protocol, false),
        ] {
            for listing in [true, false] {
                let mut fixture = Fixture::new(vec![(StatusCode::from_u16(status).unwrap(), json!({
                    "error": {"message": format!("{TEST_KEY}: private prompt"), "type": "fixture_error"}
                }))]).await;
                let client = fixture.client(provider);
                let error = if listing {
                    client.list_models().await.unwrap_err()
                } else {
                    client.prompt("fixture-model", "hi").await.unwrap_err()
                };
                assert_eq!(error.kind, kind);
                assert_eq!(error.retryable, retryable);
                assert_eq!(error.code, Some(status.to_string()));
                assert!(!format!("{error:?} {error}").contains(TEST_KEY));
                assert!(!error.message.contains("private prompt"));
                fixture
                    .request(
                        if listing { "GET" } else { "POST" },
                        if listing {
                            "models"
                        } else {
                            completion_path(provider)
                        },
                    )
                    .await;
                assert!(fixture.requests.try_recv().is_err());
            }
        }
    }
}

#[tokio::test]
async fn malformed_responses_fail_without_echoing_body_content() {
    for provider in PROVIDERS {
        let fixture = Fixture::new(vec![
            (StatusCode::OK, json!({"data": TEST_KEY})),
            (StatusCode::OK, json!({"choices": TEST_KEY})),
        ])
        .await;
        let client = fixture.client(provider);
        for error in [
            client.list_models().await.unwrap_err(),
            client.prompt("fixture-model", "hi").await.unwrap_err(),
        ] {
            assert_eq!(error.kind, AdapterErrorKind::Protocol);
            assert!(!format!("{error:?}").contains(TEST_KEY));
        }
    }
}

#[tokio::test]
async fn invalid_requests_are_rejected_before_network_io() {
    for provider in PROVIDERS {
        let mut fixture = Fixture::new(vec![]).await;
        let client = fixture.client(provider);
        assert_eq!(
            client.prompt(" ", "hi").await.unwrap_err().kind,
            AdapterErrorKind::InvalidConfiguration
        );
        let mut request = client.completion_request("fixture-model", "hi");
        request.chat_history.clear();
        assert_eq!(
            client.complete(request).await.unwrap_err().kind,
            AdapterErrorKind::InvalidConfiguration
        );
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[test]
fn construction_validates_options_and_redacts_debug() {
    for provider in PROVIDERS {
        let config = LLMClientConfig::new(provider, TEST_KEY);
        assert!(!format!("{config:?}").contains(TEST_KEY));
        let client = LLMClient::new(config).unwrap();
        assert_eq!(client.provider(), provider);
        assert!(!format!("{client:?}").contains(TEST_KEY));
        for key in ["", "  ", "bad\nkey", "bad key"] {
            assert_eq!(
                LLMClient::new(LLMClientConfig::new(provider, key))
                    .unwrap_err()
                    .kind,
                AdapterErrorKind::InvalidConfiguration
            );
        }
        for base_url in [
            "",
            "not a URL",
            "ftp://example.com",
            "https://user:key@example.com",
            "https://example.com?key=secret",
            "https://example.com#fragment",
        ] {
            let mut config = LLMClientConfig::new(provider, TEST_KEY);
            config.base_url = Some(base_url.to_owned());
            assert_eq!(
                LLMClient::new(config).unwrap_err().kind,
                AdapterErrorKind::InvalidConfiguration
            );
        }
        let mut config = LLMClientConfig::new(provider, TEST_KEY);
        config.timeout = Duration::ZERO;
        assert_eq!(
            LLMClient::new(config).unwrap_err().kind,
            AdapterErrorKind::InvalidConfiguration
        );
    }
}

#[tokio::test]
async fn tool_calls_are_returned_without_executing_an_agent_loop() {
    for provider in PROVIDERS {
        let mut body = completion(provider);
        match provider {
            LLMProvider::OpenAI => {
                body["output"] = json!([{
                    "type": "function_call", "id": "fc_fixture", "call_id": "call_fixture",
                    "name": "lookup", "arguments": "{\"query\":\"hello\"}", "status": "completed"
                }]);
            }
            LLMProvider::DeepSeek => {
                body["choices"][0]["message"] = json!({
                    "role": "assistant", "content": "", "tool_calls": [{
                        "id": "call_fixture", "type": "function", "index": 0,
                        "function": {"name": "lookup", "arguments": "{\"query\":\"hello\"}"}
                    }]
                });
                body["choices"][0]["finish_reason"] = json!("tool_calls");
            }
        }
        let mut fixture = Fixture::new(vec![(StatusCode::OK, body)]).await;
        let client = fixture.client(provider);
        let response = client
            .complete(client.completion_request("fixture-model", "hi"))
            .await
            .unwrap();
        assert!(
            response
                .choice
                .iter()
                .any(|content| matches!(content, AssistantContent::ToolCall(_)))
        );
        fixture.request("POST", completion_path(provider)).await;
        assert!(fixture.requests.try_recv().is_err());
    }
}

#[tokio::test]
async fn stalled_http_requests_obey_the_configured_timeout() {
    // Keeping the listener open without accepting lets TCP connect but never responds.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    for provider in PROVIDERS {
        let mut config = LLMClientConfig::new(provider, TEST_KEY);
        config.base_url = Some(format!("http://{}", listener.local_addr().unwrap()));
        config.timeout = Duration::from_millis(30);
        let client = LLMClient::new(config).unwrap();
        let errors = tokio::time::timeout(Duration::from_secs(2), async {
            [
                client.list_models().await.unwrap_err(),
                client.prompt("fixture-model", "hi").await.unwrap_err(),
            ]
        })
        .await
        .expect("configured timeout should end both requests");
        for error in errors {
            assert_eq!(error.kind, AdapterErrorKind::Unavailable);
            assert!(error.retryable);
            assert!(!format!("{error:?}").contains(TEST_KEY));
        }
    }
}
