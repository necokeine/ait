use super::*;

#[test]
fn workflow_replay_requires_a_parent_tool_link_and_restores_terminal_output() {
    let root_dir = tempfile::tempdir().unwrap();
    let cwd = root_dir.path().to_str().unwrap();
    let mut client = ClaudeClient::new("unused".into());
    client.config_dir = Some(root_dir.path().join("config"));
    let root = Uuid::new_v4().to_string();
    let directory = history::project_dir(&client, cwd).unwrap();
    write(
        &directory.join(format!("{root}.jsonl")),
        &[
            record("user", json!("Start workflow"), &root, cwd),
            record(
                "assistant",
                json!([{"type":"tool_use","id":"workflow-call","name":"Workflow","input":{"description":"Build report"}}]),
                &root,
                cwd,
            ),
            record(
                "user",
                json!([{"type":"tool_result","tool_use_id":"workflow-call","content":"Workflow launched\nRun ID: wf_123"}]),
                &root,
                cwd,
            ),
        ],
    );
    let summaries = directory.join(&root).join("workflows");
    std::fs::create_dir_all(&summaries).unwrap();
    std::fs::write(summaries.join("wf_123.json"),json!({"runId":"wf_123","status":"completed","summary":"Build report","timestamp":"2026-09-26T00:00:00Z","result":{"output":"Final report"}}).to_string()).unwrap();
    std::fs::write(
        summaries.join("wf_foreign.json"),
        json!({"runId":"wf_foreign","result":"Not this session"}).to_string(),
    )
    .unwrap();
    let nested = directory
        .join(&root)
        .join("subagents/workflows/wf_123/nested");
    write(
        &nested.join("child.jsonl"),
        &[record(
            "assistant",
            json!([{"type":"text","text":"Nested workflow work"}]),
            &root,
            cwd,
        )],
    );
    let children = list(&client, cwd).unwrap();
    assert_eq!(children.len(), 1);
    assert_eq!(children[0].descriptor["status"], "completed");
    let history = read(&client, children[0].persistence.as_ref().unwrap(), cwd).unwrap();
    assert!(
        history
            .entries
            .iter()
            .any(|entry| entry.item["text"] == "Final report")
    );
    assert!(
        history
            .entries
            .iter()
            .any(|entry| entry.item["text"] == "Nested workflow work")
    );
    let mut live = live::Live::new(client.images.clone());
    live.restore(&client, &root, cwd).unwrap();
    assert_eq!(live.children()[0].id, "workflow-call");
    std::fs::write(
        summaries.join("wf_123.json"),
        json!({"runId":"wf_123","status":"running"}).to_string(),
    )
    .unwrap();
    assert_eq!(
        list(&client, cwd).unwrap()[0].descriptor["status"],
        "failed"
    );
}
