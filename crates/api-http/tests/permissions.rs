//! NEC-192: HTTP settings -> durable Run -> production Codex mapping / API gateway.
#![allow(clippy::pedantic)]

use ait_agent_adapters::{
    AdapterError, AgentAdapter, AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunStatus,
    AgentStream, SandboxMode, codex::CodexWorkspaceAgent,
};
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_contracts::{AgentConfiguration, AgentProvider, ProviderModel};
use ait_domain::{DomainError, SandboxAccess};
use ait_ports::{AgentProviderGateway, ProviderMessage};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use axum::{Router, body::Body, http::Request};
use http_body_util::BodyExt;
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tower::ServiceExt;

#[derive(Default)]
struct Native(Mutex<Vec<SandboxMode>>);
#[async_trait]
impl AgentAdapter for Native {
    fn driver(&self) -> &'static str {
        "http-permission-fixture"
    }
    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            thread_resume: false,
            approvals: true,
            command_execution: true,
            file_changes: true,
            usage: false,
        }
    }
    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError> {
        self.0.lock().unwrap().push(request.sandbox);
        Ok(Box::pin(futures_util::stream::iter([
            Ok(AgentEvent::ItemCompleted {
                item: json!({"type":"agentMessage","id":"answer","phase":"final_answer","text":"Read the project."}),
            }),
            Ok(AgentEvent::Completed {
                turn_id: "turn".into(),
                status: AgentRunStatus::Completed,
                error: None,
            }),
        ])))
    }
}
#[derive(Default)]
struct TextGateway(AtomicUsize);
#[async_trait]
impl AgentProviderGateway for TextGateway {
    async fn store_secret(&self, _: &str, _: &str) -> Result<(), DomainError> {
        Ok(())
    }
    async fn delete_secret(&self, _: &str) -> Result<(), DomainError> {
        Ok(())
    }
    async fn list_models(
        &self,
        provider: &AgentProvider,
        _: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        Ok(provider.models.clone())
    }
    async fn list_models_with_secret(
        &self,
        provider: &AgentProvider,
        _: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        Ok(provider.models.clone())
    }
    async fn complete(
        &self,
        _: &AgentProvider,
        _: &str,
        _: &AgentConfiguration,
        _: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        self.0.fetch_add(1, Ordering::Relaxed);
        Ok("Text only.".into())
    }
}
struct Fixture {
    app: Router,
    native: Arc<Native>,
    text: Arc<TextGateway>,
    _project: tempfile::TempDir,
}
async fn request(app: &Router, path: &str, body: Option<Value>) -> Value {
    let request = match body {
        Some(value) => Request::post(path)
            .header("content-type", "application/json")
            .body(Body::from(value.to_string())),
        None => Request::get(path).body(Body::empty()),
    }
    .unwrap();
    let response = app.clone().oneshot(request).await.unwrap();
    assert!(response.status().is_success());
    let bytes = response.into_body().collect().await.unwrap().to_bytes();
    serde_json::from_slice(&bytes).unwrap()
}
async fn ok(app: &Router, path: &str, body: Option<Value>) -> Value {
    let response = request(app, path, body).await;
    assert_eq!(response["ok"], true, "{path}: {response}");
    response["result"]["value"].clone()
}
async fn fixture(kind: &str, max_sandbox: SandboxAccess) -> Fixture {
    let native = Arc::new(Native::default());
    let text = Arc::new(TextGateway::default());
    let service = LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        Arc::new(CodexWorkspaceAgent::new(native.clone())),
    )
    .with_provider_gateway(text.clone())
    .with_permission_limits(PermissionPolicyLimits {
        max_sandbox,
        allow_session_approvals: false,
    });
    let app = ait_api_http::router_with_telemetry(
        Arc::new(service),
        ait_observability::Telemetry::new(Arc::new(ait_observability::JsonLogSink::new(
            std::io::sink(),
        ))),
    );
    let project = tempfile::tempdir().unwrap();
    let registered = ok(
        &app,
        "/v1/project/register",
        Some(json!({"id":"p","name":"Permissions","workdir":project.path()})),
    )
    .await;
    let config = if kind == "codex" {
        json!({"provider_id":"builtin-codex","model":"gpt-5.6-sol"})
    } else {
        ok(&app, "/v1/agent-provider/save", Some(json!({"provider":{"id":"api","name":"API","kind":kind,"models":[{"id":"chat","name":"Chat","reasoning_efforts":[]}]},"secret":"fixture-secret"}))).await;
        json!({"provider_id":"api","model":"chat"})
    };
    ok(
        &app,
        "/v1/agent/register",
        Some(json!({"id":"a","name":"Agent","config":config})),
    )
    .await;
    ok(
        &app,
        "/v1/session/create",
        Some(json!({"id":"s","project_id":"p","agent_id":"a"})),
    )
    .await;
    ok(&app, "/v1/cron/create", Some(json!({"id":"cron","name":"Cron","project_id":"p","base_message_id":registered["root_message_id"],"agent_id":"a","schedule":"0 * * * * *","timezone":"UTC"}))).await;
    Fixture {
        app,
        native,
        text,
        _project: project,
    }
}
async fn settings(app: &Router, sandbox: &str) -> Value {
    let settings = ok(app, "/v1/settings", None).await;
    let mut values = settings["values"].clone();
    values["permissions.sandbox"] = json!(sandbox);
    request(
        app,
        "/v1/settings/save",
        Some(json!({"expected_revision":settings["revision"],"values":values})),
    )
    .await
}

#[tokio::test]
async fn http_permission_profiles_reach_durable_runs_and_actual_codex_mapping() {
    for kind in ["codex", "openai", "deepseek"] {
        for (setting, snapshot, wire) in [
            ("read_only", "read_only", SandboxMode::ReadOnly),
            ("strict", "read_only", SandboxMode::ReadOnly),
            (
                "workspace_write",
                "workspace_write",
                SandboxMode::WorkspaceWrite,
            ),
            ("full_access", "full_access", SandboxMode::DangerFullAccess),
        ] {
            let fixture = fixture(kind, SandboxAccess::FullAccess).await;
            let initial = ok(&fixture.app, "/v1/settings", None).await;
            assert_eq!(initial["values"]["permissions.sandbox"], "read_only");
            assert_eq!(initial["values"]["permissions.approval"], "on_request");
            assert_eq!(settings(&fixture.app, setting).await["ok"], true);
            let run = ok(
                &fixture.app,
                "/v1/session/send-message",
                Some(json!({"session_id":"s","text":"Inspect."})),
            )
            .await;
            assert_eq!(run["status"], "completed", "{run}");
            assert_eq!(
                run["permission_profile"],
                json!({"sandbox":snapshot,"approval":"on_request"})
            );
            let cron = ok(
                &fixture.app,
                "/v1/cron/trigger",
                Some(json!({"cron_id":"cron","scheduled_at":42})),
            )
            .await;
            assert_eq!(cron["status"], "completed", "{cron}");
            assert_eq!(cron["permission_profile"], run["permission_profile"]);
            assert!(cron["session_id"].is_null());
            if kind == "codex" {
                assert_eq!(*fixture.native.0.lock().unwrap(), vec![wire, wire]);
                assert_eq!(fixture.text.0.load(Ordering::Relaxed), 0);
            } else {
                assert!(fixture.native.0.lock().unwrap().is_empty());
                assert_eq!(fixture.text.0.load(Ordering::Relaxed), 2);
            }
            ok(&fixture.app, "/v1/settings/reset", Some(json!({}))).await;
            let persisted = ok(
                &fixture.app,
                "/v1/run/get",
                Some(json!({"run_id":run["id"]})),
            )
            .await;
            assert_eq!(persisted, run);
            assert_eq!(
                ok(
                    &fixture.app,
                    "/v1/run/get",
                    Some(json!({"run_id":cron["id"]}))
                )
                .await,
                cron
            );
            assert!(!persisted.to_string().contains("fixture-secret"));
        }
    }
}

#[tokio::test]
async fn http_unknown_settings_and_excessive_run_permissions_have_no_side_effects() {
    for kind in ["codex", "openai", "deepseek"] {
        let fixture = fixture(kind, SandboxAccess::ReadOnly).await;
        let before = ok(&fixture.app, "/v1/message/list?project_id=p", None).await;
        for invalid in ["unknown", "danger-full-access", "fixture-secret"] {
            let result = settings(&fixture.app, invalid).await;
            assert_eq!(result["ok"], false);
            assert_eq!(result["error"]["code"], "INVALID_CONFIGURATION");
            assert!(!result.to_string().contains(invalid));
        }
        assert_eq!(settings(&fixture.app, "full_access").await["ok"], true);
        let result = request(
            &fixture.app,
            "/v1/session/send-message",
            Some(json!({"session_id":"s","text":"Rejected."})),
        )
        .await;
        assert_eq!(result["ok"], false);
        assert_eq!(result["error"]["code"], "INVALID_CONFIGURATION");
        let cron = request(
            &fixture.app,
            "/v1/cron/trigger",
            Some(json!({"cron_id":"cron","scheduled_at":42})),
        )
        .await;
        assert_eq!(cron["ok"], false);
        assert_eq!(cron["error"]["code"], "INVALID_CONFIGURATION");
        assert_eq!(
            ok(&fixture.app, "/v1/message/list?project_id=p", None).await,
            before
        );
        assert_eq!(
            ok(&fixture.app, "/v1/run/list?project_id=p", None).await,
            json!([])
        );
        assert!(fixture.native.0.lock().unwrap().is_empty());
        assert_eq!(fixture.text.0.load(Ordering::Relaxed), 0);
    }
}
