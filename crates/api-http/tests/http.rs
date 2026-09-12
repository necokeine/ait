//! HTTP adapter acceptance coverage.

use std::sync::Arc;

use ait_application::LocalControlService;
use ait_contracts::Response;
use ait_storage_sqlite::SqliteControlStore;
use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn entity_operations_and_cursor_event_replay_share_the_application_service() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    )));
    let app = ait_api_http::router(service);
    let request = serde_json::json!({
        "id": "project-http",
        "name": "HTTP",
        "workdir": project_dir.display().to_string(),
    });
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/project/register")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&request).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(serde_json::from_slice::<Response>(&body).unwrap().ok);

    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/event/list?after=0")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let text = std::str::from_utf8(&body).unwrap();
    assert!(text.contains("id: 1"));
    assert!(text.contains("event: project.registered"));

    let response = app
        .oneshot(Request::get("/v1/metric/list").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let metrics: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let points = metrics.as_array().unwrap();
    assert!(points.iter().any(|point| {
        point["name"] == "api_operations_total"
            && point["project_id"] == "project-http"
            && point["call_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("api-"))
    }));
}

#[tokio::test]
async fn every_application_use_case_has_a_distinct_entity_operation_route() {
    let service = Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    )));
    let app = ait_api_http::router(service);
    let post_routes = [
        "/v1/project/register",
        "/v1/project/set-default-agent",
        "/v1/project/export",
        "/v1/project/import",
        "/v1/agent/register",
        "/v1/agent/update",
        "/v1/agent-provider/save",
        "/v1/agent-provider/discover-models",
        "/v1/agent-provider/refresh-models",
        "/v1/session/set-config",
        "/v1/session/create",
        "/v1/session/set-agent",
        "/v1/session/rename",
        "/v1/session/set-title",
        "/v1/session/generate-title",
        "/v1/session/send-message",
        "/v1/session/submit-message",
        "/v1/session/fork",
        "/v1/session/submit-fork",
        "/v1/session/derive",
        "/v1/session/submit-derive",
        "/v1/run/get",
        "/v1/run/cancel",
        "/v1/run/approval/resolve",
        "/v1/cron/create",
        "/v1/cron/set-enabled",
        "/v1/cron/trigger",
        "/v1/settings/save",
        "/v1/settings/reset",
    ];

    for route in post_routes {
        let response = app
            .clone()
            .oneshot(
                Request::post(route)
                    .header("content-type", "application/json")
                    .body(Body::from("{}"))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{route}");
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{route}");
    }

    for route in [
        "/v1/project/list",
        "/v1/agent/list",
        "/v1/agent-provider/list",
        "/v1/session/list?project_id=p",
        "/v1/message/list?project_id=p",
        "/v1/run/list?project_id=p",
        "/v1/cron/list",
        "/v1/settings",
        "/v1/event/list",
        "/v1/event/stream",
        "/v1/run/progress?project_id=p",
        "/v1/health",
        "/v1/metric/list",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_ne!(response.status(), StatusCode::NOT_FOUND, "{route}");
        assert_ne!(response.status(), StatusCode::METHOD_NOT_ALLOWED, "{route}");
    }

    let response = app
        .oneshot(
            Request::post("/v1/commands")
                .header("content-type", "application/json")
                .body(Body::from("{}"))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn project_runtime_reads_require_an_explicit_project_id() {
    let app = ait_api_http::router(Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    ))));
    for route in [
        "/v1/session/list",
        "/v1/message/list",
        "/v1/run/list",
        "/v1/run/progress",
    ] {
        let response = app
            .clone()
            .oneshot(Request::get(route).body(Body::empty()).unwrap())
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{route}");
    }
}

#[tokio::test]
async fn event_stream_replays_then_follows_an_event_committed_at_the_handoff() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("stream-project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    )));
    let app = ait_api_http::router(service);
    let project = serde_json::json!({
        "id": "stream-project",
        "name": "Stream",
        "workdir": project_dir.display().to_string(),
    });
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/project/register")
                .header("content-type", "application/json")
                .body(Body::from(project.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);

    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/event/stream?after=1")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let mut body = response.into_body();
    let agent = serde_json::json!({
        "id": "stream-agent",
        "name": "Stream agent",
        "config": {
            "provider_id": "builtin-codex",
            "model": "gpt-5.6-sol",
            "reasoning_effort": "low"
        }
    });
    let committed = app
        .oneshot(
            Request::post("/v1/agent/register")
                .header("content-type", "application/json")
                .body(Body::from(agent.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(committed.status(), StatusCode::OK);

    let frame = tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            let frame = body.frame().await.unwrap().unwrap();
            if let Some(data) = frame.data_ref()
                && String::from_utf8_lossy(data).contains("agent.registered")
            {
                break data.clone();
            }
        }
    })
    .await
    .expect("follow stream did not deliver the handoff event");
    assert!(String::from_utf8_lossy(&frame).contains("id: 2"));
}

#[tokio::test]
async fn removed_message_overrides_are_rejected_at_the_transport_boundary() {
    let app = ait_api_http::router(Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    ))));
    for extra in [
        serde_json::json!({"expected_version": 1}),
        serde_json::json!({"reasoning_effort": "high"}),
    ] {
        let mut body = serde_json::json!({"session_id": "session", "text": "hello"});
        body.as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/session/send-message")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
    }
}

#[cfg(not(all(feature = "dev-mock-provider", debug_assertions)))]
#[tokio::test]
async fn production_http_contract_rejects_mock_provider_payloads() {
    let app = ait_api_http::router(Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    ))));
    let body = serde_json::json!({
        "provider": {
            "id": "builtin-mock",
            "name": "Mock",
            "kind": "mock",
            "url": null,
            "models": [{"id": "mock-local", "name": "Mock Local", "reasoning_efforts": []}]
        }
    });
    let response = app
        .oneshot(
            Request::post("/v1/agent-provider/save")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn name_only_http_request_accepts_null_and_returns_stable_conflict() {
    let temporary = TempDir::new().unwrap();
    let documents = temporary.path().to_path_buf();
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()))
        .with_project_directory_creator(Arc::new(
            ait_project_local::DocumentsProjectDirectory::with_resolver(move || {
                Some(documents.clone())
            }),
        ));
    let app = ait_api_http::router(Arc::new(service));
    for (id, expected) in [
        ("first", None),
        ("second", Some("PROJECT_PATH_ALREADY_EXISTS")),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post("/v1/project/register")
                    .header("content-type", "application/json")
                    .body(Body::from(
                        serde_json::json!({"id":id, "name":"API project", "workdir":null})
                            .to_string(),
                    ))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = response.into_body().collect().await.unwrap().to_bytes();
        let body: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        if let Some(code) = expected {
            assert_eq!(body["ok"], false);
            assert_eq!(body["error"]["code"], code);
            assert_eq!(body["error"]["retryable"], false);
        } else {
            assert_eq!(body["ok"], true);
            assert_eq!(
                body["result"]["value"]["workdir"],
                temporary
                    .path()
                    .join("API project")
                    .canonicalize()
                    .unwrap()
                    .to_str()
                    .unwrap()
            );
        }
    }
}

#[tokio::test]
async fn malformed_approval_and_settings_requests_do_not_echo_secret_values() {
    let app = ait_api_http::router(Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    ))));
    for (path, body, status) in [
        (
            "/v1/run/approval/resolve",
            r#"{"run_id":"r","approval_id":"a","action":"fixture-secret"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/run/approval/resolve",
            r#"{"run_id":"r","approval_id":"a","action":"approve","scope":"fixture-secret"}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/run/approval/resolve",
            r#"{"run_id":"r","approval_id":"a","action":"approve","fixture-secret":true}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/run/approval/resolve",
            r#"{"run_id":"fixture-secret","#,
            StatusCode::BAD_REQUEST,
        ),
        (
            "/v1/settings/save",
            r#"{"expected_revision":"fixture-secret","values":{}}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/settings/save",
            r#"{"expected_revision":0,"values":["fixture-secret"]}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/settings/save",
            r#"{"expected_revision":0,"values":{},"fixture-secret":true}"#,
            StatusCode::UNPROCESSABLE_ENTITY,
        ),
        (
            "/v1/settings/save",
            r#"{"expected_revision":0,"values":{"key":"fixture-secret"},"#,
            StatusCode::BAD_REQUEST,
        ),
    ] {
        let response = app
            .clone()
            .oneshot(
                Request::post(path)
                    .header("content-type", "application/json")
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), status, "{path}");
        assert_eq!(response.headers()["content-type"], "application/json");
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
            serde_json::json!({
                "api_version": 1,
                "ok": false,
                "error": {
                    "code": "INVALID_CONFIGURATION",
                    "message": "invalid permission or settings request",
                    "retryable": false
                }
            }),
            "{path}"
        );
    }
}

#[tokio::test]
async fn valid_approval_json_preserves_application_error_response() {
    let app = ait_api_http::router(Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    ))));
    let response = app
        .oneshot(
            Request::post("/v1/run/approval/resolve")
                .header("content-type", "application/json")
                .body(Body::from(
                    r#"{"run_id":"fixture-secret","approval_id":"a","action":"approve"}"#,
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["content-type"], "application/json");
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&body).unwrap(),
        serde_json::json!({
            "api_version": 1,
            "ok": false,
            "error": {"code":"INVALID_RUN","message":"run not found","retryable":false}
        })
    );
}
