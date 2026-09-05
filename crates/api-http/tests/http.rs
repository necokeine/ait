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
        "/v1/session/create",
        "/v1/session/set-agent",
        "/v1/session/rename",
        "/v1/session/set-title",
        "/v1/session/generate-title",
        "/v1/session/send-message",
        "/v1/session/fork",
        "/v1/run/get",
        "/v1/run/cancel",
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
        "/v1/workspace/snapshot",
        "/v1/settings",
        "/v1/event/list",
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
