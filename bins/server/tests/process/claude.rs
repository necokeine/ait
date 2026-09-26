use std::os::unix::fs::PermissionsExt;

use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start_with_path, terminate};

#[path = "claude/rewind.rs"]
mod rewind;

const METHODS: &[&str] = &[
    "workspace.open.request",
    "agent.create.request",
    "agent.message.send.request",
    "agent.finish.wait.request",
    "agent.timeline.get.request",
    "provider.models.list.request",
    "provider.snapshot.get.request",
    "agent.resume.request",
    "agent.get.request",
    "agent.rewind.request",
];

#[tokio::test]
async fn claude_provider_is_discovered_created_streamed_and_restored_over_websocket() {
    let fixture = fixture();
    let state = fixture.root.path().join("state");
    let log = fixture.root.path().join("server.log");
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    let models = request(
        &mut client,
        "provider.models.list.request",
        json!({"provider":"claude","cwd":fixture.cwd}),
    )
    .await;
    assert_eq!(models["result"]["models"][0]["id"], "sonnet", "{models}");
    let snapshot = request(
        &mut client,
        "provider.snapshot.get.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    assert!(
        snapshot["result"]["entries"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["provider"] == "claude" && entry["status"] == "ready")
    );
    request(
        &mut client,
        "workspace.open.request",
        json!({"cwd":fixture.cwd}),
    )
    .await;
    let created = request(&mut client,"agent.create.request",json!({"config":{"provider":"claude","cwd":fixture.cwd,"model":"sonnet","modeId":"default"}})).await;
    let id = created["result"]["agentId"]
        .as_str()
        .unwrap_or_else(|| panic!("{created}"))
        .to_owned();
    assert_eq!(
        created["result"]["agent"]["persistence"]["provider"],
        "claude"
    );
    let sent = request(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"first"}),
    )
    .await;
    assert_eq!(sent["result"]["accepted"], true, "{sent}");
    let finished = request(
        &mut client,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(
        finished["result"]["lastMessage"], "Claude: first",
        "{finished}"
    );
    let timeline = request(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(timeline["type"], "response", "{timeline}");
    terminate(&mut process).await;
    let mut process = start_with_path(&state, &log, Some(&fixture.path));
    let address = ready(&mut process, &log).await;
    let mut client = connect(&address, METHODS).await;
    let resumed = request(
        &mut client,
        "agent.resume.request",
        json!({"handle":created["result"]["agent"]["persistence"]}),
    )
    .await;
    assert_eq!(resumed["type"], "response", "{resumed}");
    request(
        &mut client,
        "agent.message.send.request",
        json!({"agentId":id,"text":"second"}),
    )
    .await;
    let finished = request(
        &mut client,
        "agent.finish.wait.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(
        finished["result"]["lastMessage"], "Claude: second",
        "{finished}"
    );
    let timeline = request(
        &mut client,
        "agent.timeline.get.request",
        json!({"agentId":id}),
    )
    .await;
    assert_eq!(timeline["type"], "response", "{timeline}");
    terminate(&mut process).await;
}

fn fixture() -> super::native::NativeFixture {
    let fixture = super::native::NativeFixture::new();
    let program = fixture.root.path().join("claude");
    std::fs::write(
        &program,
        include_str!("../../../../crates/server-provider/tests/fixtures/claude_code.py"),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o700)).unwrap();
    fixture
}
