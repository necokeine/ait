use serde_json::json;

use super::transport::{connect, request};
use super::{ready, start, terminate};

const METHODS: &[&str] = &[
    "agent.skills.get_status.request",
    "agent.skills.reconcile.request",
    "agent.skills.uninstall.request",
    "agent.skills.save_selection.request",
    "agent.skills.import_legacy_selection.request",
];

#[tokio::test]
async fn skills_are_installed_confirmed_and_persisted_through_websocket() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("state");
    let bundle = data.join("skills-bundle/example");
    std::fs::create_dir_all(&bundle).unwrap();
    std::fs::write(bundle.join("SKILL.md"), "example skill").unwrap();
    let log = root.path().join("server.log");
    let mut process = start(&data, &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    let status = request(&mut socket, METHODS[0], json!({})).await;
    assert_eq!(status["result"]["state"], "not-installed", "{status}");
    assert_eq!(status["result"]["available"], json!(["example"]));
    assert!(!root.path().join(".agents/skills").exists());
    let imported = request(
        &mut socket,
        METHODS[4],
        json!({"selection":{"mode":"custom","skills":[" example "]}}),
    )
    .await;
    assert_eq!(imported["result"]["imported"], true);
    let installed = request(&mut socket, METHODS[1], json!({})).await;
    assert_eq!(installed["result"]["state"], "up-to-date", "{installed}");
    for target in [".agents", ".claude", ".codex"] {
        assert_eq!(
            std::fs::read_to_string(root.path().join(target).join("skills/example/SKILL.md"))
                .unwrap(),
            "example skill"
        );
    }
    let pending = request(
        &mut socket,
        METHODS[3],
        json!({"selection":{"mode":"custom","skills":[]}}),
    )
    .await;
    assert_eq!(
        pending["result"]["confirmationRequired"],
        json!({"removals":["example"]})
    );
    let confirmed = request(
        &mut socket,
        METHODS[3],
        json!({"selection":{"mode":"custom","skills":[]},"confirmedRemovals":["example"]}),
    )
    .await;
    assert_eq!(confirmed["result"]["state"], "not-installed");
    assert!(confirmed["result"]["confirmationRequired"].is_null());
    drop(socket);
    terminate(&mut process).await;
    let mut process = start(&data, &log);
    let address = ready(&mut process, &log).await;
    let mut socket = connect(&address, METHODS).await;
    assert_eq!(
        request(&mut socket, METHODS[0], json!({})).await["result"]["selection"],
        json!({"mode":"custom","skills":[]})
    );
    assert_eq!(
        request(&mut socket, METHODS[4], json!({"selection":{"mode":"all"}})).await["result"]["imported"],
        false
    );
    assert_eq!(
        request(&mut socket, METHODS[3], json!({"selection":{"mode":"all"}})).await["result"]["state"],
        "up-to-date"
    );
    assert_eq!(
        request(&mut socket, METHODS[2], json!({})).await["result"]["state"],
        "not-installed"
    );
    assert_eq!(
        request(&mut socket, METHODS[0], json!({"agentId":"invalid"})).await["code"],
        "invalid_message"
    );
    drop(socket);
    terminate(&mut process).await;
}
