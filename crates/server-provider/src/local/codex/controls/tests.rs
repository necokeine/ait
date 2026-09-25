use super::*;
use crate::ports::agent_session::AgentClient;

#[test]
fn skills_fail_closed_on_errors_and_relative_paths() {
    let root = tempfile::tempdir().unwrap();
    let cwd = root.path().to_str().unwrap();
    let mut response = json!({"data":[{"cwd":cwd,"errors":[],"skills":[{"name":"one","enabled":true,"path":"/one/SKILL.md","description":"One"}]}]});
    assert_eq!(skills(&response, cwd).unwrap()[0].name, "one");
    response["data"][0]["skills"][0]["path"] = json!("relative");
    assert!(skills(&response, cwd).is_err());
    response["data"][0]["errors"] = json!(["bad"]);
    assert!(skills(&response, cwd).is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn remote_selection_checks_model_tier_and_skill_invocation_uses_native_input() {
    let fixture = crate::test_support::Fixture::new();
    let client = fixture.client();
    let mut spec = fixture.spec();
    spec.config.feature_values = Some(BTreeMap::from([("fast_mode".to_owned(), json!(true))]));
    spec.config.mode_id = Some("auto".to_owned());
    client.validate_selection(&spec).await.unwrap();
    let commands = client.commands(&spec).await.unwrap();
    assert_eq!(commands.len(), 1);
    assert_eq!(commands[0]["name"], "review");
    let mut session = client.create_session(&spec).await.unwrap();
    assert!(session.start_turn("/unknown", &spec.config).await.is_err());
    session
        .start_turn("/review check this", &spec.config)
        .await
        .unwrap();
    let request = fixture
        .requests()
        .into_iter()
        .find(|request| request["method"] == "turn/start")
        .unwrap();
    assert_eq!(request["params"]["serviceTier"], "fast");
    assert_eq!(request["params"]["approvalPolicy"], "on-request");
    assert_eq!(request["params"]["sandboxPolicy"]["type"], "workspaceWrite");
    assert_eq!(request["params"]["input"][0]["type"], "skill");
    assert_eq!(request["params"]["input"][1]["text"], "check this");
    session.close().await.unwrap();
    fixture.mode("no-fast");
    assert!(client.validate_selection(&spec).await.is_err());
    spec.config.feature_values = None;
    client.validate_selection(&spec).await.unwrap();
    assert_eq!(policy(&spec.config).0, "on-request");
    spec.config.mode_id = Some("full-access".to_owned());
    assert_eq!(policy(&spec.config).1, "danger-full-access");
}

#[cfg(unix)]
#[tokio::test]
async fn child_pages_deduplicate_and_reject_parent_conflicts_and_cursor_loops() {
    let fixture = crate::test_support::Fixture::new();
    let child = json!({"id":"child","cwd":fixture.cwd,"parentThreadId":"root","createdAt":1_700_000_000,"updatedAt":1_700_000_000,"status":{"type":"active"}});
    let mut pages = json!({"first":{"data":[child],"nextCursor":"next"},"next":{"data":[child],"nextCursor":null}});
    let path = fixture.cwd.join("session-pages.json");
    std::fs::write(&path, pages.to_string()).unwrap();
    let client = fixture.client();
    let children = client.subagents(&fixture.spec().cwd).await.unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].descriptor["status"], "running");
    pages["next"]["data"][0]["parentThreadId"] = json!("other");
    std::fs::write(&path, pages.to_string()).unwrap();
    assert!(client.subagents(&fixture.spec().cwd).await.is_err());
    pages["next"]["data"] = json!([]);
    pages["next"]["nextCursor"] = json!("next");
    std::fs::write(&path, pages.to_string()).unwrap();
    assert!(client.subagents(&fixture.spec().cwd).await.is_err());
}
