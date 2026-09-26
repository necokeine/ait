use super::*;
use std::fmt::Write as _;

mod workflows;

fn record(kind: &str, content: Value, root: &str, cwd: &str) -> Value {
    let mut record = json!({"type":kind,"uuid":Uuid::new_v4().to_string(),"sessionId":root,"cwd":cwd,
        "timestamp":"2026-09-26T00:00:00Z","message":{"id":Uuid::new_v4().to_string(),"stop_reason":"end_turn"}});
    record["message"]["content"] = content;
    record
}

fn write(path: &Path, records: &[Value]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut content = String::new();
    for record in records {
        writeln!(content, "{record}").unwrap();
    }
    std::fs::write(path, content).unwrap();
}

#[test]
fn child_history_requires_a_parent_declaration_and_preserves_nested_ancestry() {
    let root_dir = tempfile::tempdir().unwrap();
    let cwd = root_dir.path().to_str().unwrap();
    let mut client = ClaudeClient::new("unused".into());
    client.config_dir = Some(root_dir.path().join("config"));
    let root = Uuid::new_v4().to_string();
    let directory = history::project_dir(&client, cwd).unwrap();
    write(
        &directory.join(format!("{root}.jsonl")),
        &[
            record("user", json!("parent prompt"), &root, cwd),
            record(
                "assistant",
                json!([{"type":"tool_use","id":"call-parent","name":"Agent","input":{"description":"Find references","subagent_type":"Explore"}}]),
                &root,
                cwd,
            ),
        ],
    );
    let subagents = directory.join(&root).join("subagents");
    write(
        &subagents.join("agent-child.jsonl"),
        &[
            record("user", json!("child prompt"), &root, cwd),
            record(
                "assistant",
                json!([{"type":"text","text":"child reply"}, {"type":"tool_use","id":"call-nested","name":"Task","input":{}}]),
                &root,
                cwd,
            ),
        ],
    );
    std::fs::write(
        subagents.join("agent-child.meta.json"),
        r#"{"toolUseId":"call-parent","agentType":"Explore"}"#,
    )
    .unwrap();
    write(
        &subagents.join("agent-nested.jsonl"),
        &[record(
            "assistant",
            json!([{"type":"text","text":"nested reply"}]),
            &root,
            cwd,
        )],
    );
    std::fs::write(
        subagents.join("agent-nested.meta.json"),
        r#"{"toolUseId":"call-nested"}"#,
    )
    .unwrap();
    write(
        &subagents.join("agent-unrelated.jsonl"),
        &[record(
            "assistant",
            json!([{"type":"text","text":"hidden"}]),
            &root,
            cwd,
        )],
    );
    std::fs::write(
        subagents.join("agent-unrelated.meta.json"),
        r#"{"toolUseId":"no-declaration"}"#,
    )
    .unwrap();
    let children = list(&client, cwd).unwrap();
    assert_eq!(children.len(), 2);
    let child = children
        .iter()
        .find(|child| child.id == "call-parent")
        .unwrap();
    assert_eq!(child.parent_id, root);
    assert_eq!(child.descriptor["title"], "Explore");
    assert_resumed_alias(&client, &root, cwd);
    assert_eq!(
        children
            .iter()
            .find(|child| child.id == "call-nested")
            .unwrap()
            .parent_id,
        "call-parent"
    );
    let history = read(&client, child.persistence.as_ref().unwrap(), &child.cwd).unwrap();
    assert_eq!(history.parent_id.as_deref(), Some(root.as_str()));
    assert!(
        history
            .entries
            .iter()
            .any(|entry| entry.item["text"] == "child reply")
    );
    let mut foreign = child.persistence.clone().unwrap();
    foreign.session_id = "no-declaration".into();
    assert!(read(&client, &foreign, cwd).is_err());
    assert!(!valid_component("../../child"));
}

#[test]
fn legacy_agent_links_are_used_only_for_declared_native_task_calls() {
    let mut declarations = BTreeMap::new();
    let mut links = BTreeMap::new();
    let records = [
        json!({"message":{"content":[{"type":"tool_use","name":"Task","id":"call","input":{"description":"Inspect"}}]}}),
        json!({"message":{"content":[{"type":"tool_result","tool_use_id":"call","content":"result\nagentId: child (resume here)"}]}}),
    ];
    collect_declarations(&records, "root", false, &mut declarations, &mut links).unwrap();
    assert_eq!(links["child"], "call");
    assert_eq!(declarations["call"].owner, "root");
    assert!(
        collect_declarations(
            &records[..1],
            "foreign",
            false,
            &mut declarations,
            &mut links
        )
        .is_err()
    );
}

fn assert_resumed_alias(client: &ClaudeClient, root: &str, cwd: &str) {
    let mut resumed = live::Live::new(client.images.clone());
    resumed.restore(client, root, cwd).unwrap();
    resumed.observe(&json!({"type":"system","subtype":"task_started","task_type":"local_agent","task_id":"child","tool_use_id":"new-call"}),root,cwd).unwrap();
    assert_eq!(resumed.children().len(), 2);
    let reply = record(
        "assistant",
        json!([{"type":"text","text":"Resumed answer"}]),
        root,
        cwd,
    );
    let mut reply = reply;
    reply["parent_tool_use_id"] = json!("new-call");
    let updates = resumed.observe(&reply, root, cwd).unwrap();
    assert!(updates.iter().any(|event|matches!(event,crate::ports::agent_session::AgentTurnEvent::Subagent(crate::ports::controls::SubagentEvent::Timeline {id,entry}) if id=="call-parent" && entry.item["text"]=="Resumed answer")));
}
