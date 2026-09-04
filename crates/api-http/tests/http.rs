//! HTTP adapter acceptance coverage.

use std::sync::Arc;

use ait_application::LocalControlService;
use ait_contracts::{Command, Response};
use ait_storage_sqlite::SqliteControlStore;
use axum::{body::Body, http::Request};
use http_body_util::BodyExt;
use tempfile::TempDir;
use tower::ServiceExt;

#[tokio::test]
async fn commands_and_cursor_event_replay_share_the_application_service() {
    let temporary = TempDir::new().unwrap();
    let project_dir = temporary.path().join("project");
    std::fs::create_dir(&project_dir).unwrap();
    let service = Arc::new(LocalControlService::new(Arc::new(
        SqliteControlStore::in_memory().unwrap(),
    )));
    let app = ait_api_http::router(service);
    let command = Command::RegisterProject {
        id: "project-http".into(),
        name: "HTTP".into(),
        workdir: project_dir.display().to_string(),
    };
    let response = app
        .clone()
        .oneshot(
            Request::post("/v1/commands")
                .header("content-type", "application/json")
                .body(Body::from(serde_json::to_vec(&command).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    assert!(serde_json::from_slice::<Response>(&body).unwrap().ok);

    let response = app
        .clone()
        .oneshot(
            Request::get("/v1/events?after=0")
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
        .oneshot(Request::get("/v1/metrics").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let body = response.into_body().collect().await.unwrap().to_bytes();
    let metrics: serde_json::Value = serde_json::from_slice(&body).unwrap();
    let points = metrics.as_array().unwrap();
    assert!(points.iter().any(|point| {
        point["name"] == "api_commands_total"
            && point["project_id"] == "project-http"
            && point["call_id"]
                .as_str()
                .is_some_and(|id| id.starts_with("api-"))
    }));
}
