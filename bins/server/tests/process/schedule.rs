use super::{
    ready, start_with_path, terminate,
    transport::{Socket, connect, request},
};
use serde_json::{Value, json};
const METHODS: &[&str] = &[
    "schedule.create.request",
    "schedule.list.request",
    "schedule.inspect.request",
    "schedule.logs.request",
    "schedule.update.request",
    "schedule.pause.request",
    "schedule.resume.request",
    "schedule.delete.request",
    "schedule.run_once.request",
    "agent.get.request",
];
async fn success(socket: &mut Socket, method: &str, params: Value) -> Value {
    let reply = request(socket, method, params).await;
    assert_eq!(reply["type"], "response", "{method}: {reply}");
    reply["result"].clone()
}
#[tokio::test]
async fn production_schedule_runs_real_provider_turn_and_preserves_state_across_restart() {
    let fixture = super::native::NativeFixture::new();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let created=success(&mut socket,"schedule.create.request",json!({"prompt":"scheduled hello","cadence":{"type":"every","everyMs":3_600_000},"target":{"type":"new-agent","config":{"provider":"codex","cwd":fixture.cwd,"archiveOnFinish":false}},"runOnCreate":false})).await;
    let id = created["schedule"]["id"].clone();
    assert_eq!(
        success(&mut socket, "schedule.list.request", json!({})).await["schedules"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    success(
        &mut socket,
        "schedule.pause.request",
        json!({"scheduleId":id}),
    )
    .await;
    let ran = success(
        &mut socket,
        "schedule.run_once.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(ran["schedule"]["status"], "paused", "{ran}");
    assert_eq!(ran["schedule"]["runs"][0]["status"], "succeeded", "{ran}");
    assert!(ran["schedule"]["runs"][0]["output"].is_string());
    let agent = ran["schedule"]["runs"][0]["agentId"].clone();
    assert_automatic_existing_run(&mut socket, &agent).await;
    success(
        &mut socket,
        "schedule.update.request",
        json!({"scheduleId":id,"name":"persisted","maxRuns":3}),
    )
    .await;
    success(
        &mut socket,
        "schedule.resume.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(
        success(
            &mut socket,
            "schedule.logs.request",
            json!({"scheduleId":id})
        )
        .await["runs"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    terminate(&mut process).await;
    drop(socket);
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let record = success(
        &mut socket,
        "schedule.inspect.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(record["schedule"]["name"], "persisted");
    assert_eq!(record["schedule"]["runs"][0]["status"], "succeeded");
    success(
        &mut socket,
        "schedule.delete.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(
        request(
            &mut socket,
            "schedule.inspect.request",
            json!({"scheduleId":id})
        )
        .await["code"],
        "schedule_request_failed"
    );
    terminate(&mut process).await;
}

async fn assert_automatic_existing_run(socket: &mut Socket, agent: &Value) {
    let existing=success(socket,"schedule.create.request",json!({"prompt":"existing scheduled hello","cadence":{"type":"every","everyMs":3_600_000},"target":{"type":"agent","agentId":agent},"maxRuns":1})).await;
    let existing_id = existing["schedule"]["id"].clone();
    let record = tokio::time::timeout(std::time::Duration::from_secs(10), async {
        loop {
            let record = success(
                socket,
                "schedule.inspect.request",
                json!({"scheduleId":existing_id}),
            )
            .await;
            if record["schedule"]["status"] == "completed" {
                break record;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        record["schedule"]["runs"][0]["status"], "succeeded",
        "{record}"
    );
}

#[tokio::test]
async fn scheduled_worktree_is_isolated_and_archived_after_execution() {
    let fixture = super::native::NativeFixture::new();
    for args in [
        vec!["init", "-b", "main"],
        vec![
            "-c",
            "user.name=Fixture",
            "-c",
            "user.email=fixture@example.test",
            "commit",
            "--allow-empty",
            "-m",
            "initial",
        ],
    ] {
        let result = std::process::Command::new("git")
            .current_dir(&fixture.cwd)
            .args(args)
            .output()
            .unwrap();
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
    }
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let created=success(&mut socket,"schedule.create.request",json!({"prompt":"worktree job","cadence":{"type":"every","everyMs":3_600_000},"target":{"type":"new-agent","config":{"provider":"codex","cwd":fixture.cwd,"isolation":"worktree"}},"runOnCreate":false})).await;
    let ran = success(
        &mut socket,
        "schedule.run_once.request",
        json!({"scheduleId":created["schedule"]["id"]}),
    )
    .await;
    assert_eq!(ran["schedule"]["runs"][0]["status"], "succeeded", "{ran}");
    let agent = success(
        &mut socket,
        "agent.get.request",
        json!({"agentId":ran["schedule"]["runs"][0]["agentId"]}),
    )
    .await;
    assert!(agent["agent"]["archivedAt"].is_string(), "{agent}");
    assert_ne!(agent["agent"]["cwd"], json!(fixture.cwd));
    terminate(&mut process).await;
}

#[tokio::test]
async fn permission_wait_fails_and_restart_recovers_interrupted_workspace() {
    let fixture = super::native::NativeFixture::new();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let created=success(&mut socket,"schedule.create.request",json!({"prompt":"permit-command","cadence":{"type":"every","everyMs":3_600_000},"target":{"type":"new-agent","config":{"provider":"codex","cwd":fixture.cwd,"archiveOnFinish":false}},"runOnCreate":false})).await;
    let id = created["schedule"]["id"].clone();
    let ran = success(
        &mut socket,
        "schedule.run_once.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(ran["schedule"]["runs"][0]["status"], "failed", "{ran}");
    assert!(
        ran["schedule"]["runs"][0]["error"]
            .as_str()
            .unwrap()
            .contains("permission"),
        "{ran}"
    );
    terminate(&mut process).await;
    drop(socket);
    // Reproduce a crash after identity checkpoint, before final settlement.
    let path = state.join("schedules/schedules.json");
    let mut doc: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    doc["schedules"][0]["runs"][0]["status"] = json!("running");
    doc["schedules"][0]["runs"][0]["endedAt"] = Value::Null;
    doc["schedules"][0]["target"]["config"]["archiveOnFinish"] = json!(true);
    std::fs::write(&path, doc.to_string()).unwrap();
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let restored = success(
        &mut socket,
        "schedule.inspect.request",
        json!({"scheduleId":id}),
    )
    .await;
    assert_eq!(restored["schedule"]["runs"][0]["status"], "failed");
    assert!(
        restored["schedule"]["runs"][0]["error"]
            .as_str()
            .unwrap()
            .contains("restarted")
    );
    let agent = success(
        &mut socket,
        "agent.get.request",
        json!({"agentId":ran["schedule"]["runs"][0]["agentId"]}),
    )
    .await;
    assert!(agent["agent"]["archivedAt"].is_string());
    terminate(&mut process).await;
}
