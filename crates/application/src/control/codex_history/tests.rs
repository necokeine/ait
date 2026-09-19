use ait_domain::{
    MessageRole, ProviderHistoryCompleteness, ProviderRelationshipState, SessionSource,
};
use ait_ports::{CodexItemsView, CodexThreadSnapshot, CodexTurnSnapshot};
use serde_json::{Value, json};

use crate::control::{conversation::CodexImportContext, project::ProjectRecord};

use super::{CodexImportIdentity, matching_projects, materialize_thread};

fn item(id: &str, item_type: &str, body: Value) -> Value {
    let mut value = body;
    value["id"] = json!(id);
    value["type"] = json!(item_type);
    value
}

fn user(id: &str, text: &str) -> Value {
    item(
        id,
        "userMessage",
        json!({"content": [{"type": "text", "text": text}]}),
    )
}

fn turn(id: &str, items: Vec<Value>) -> CodexTurnSnapshot {
    CodexTurnSnapshot {
        id: id.into(),
        status: "completed".into(),
        items,
        items_view: CodexItemsView::Full,
        error: None,
        started_at: Some(10),
        completed_at: Some(11),
    }
}

fn snapshot(id: &str, turns: Vec<CodexTurnSnapshot>) -> CodexThreadSnapshot {
    CodexThreadSnapshot {
        id: id.into(),
        session_id: format!("session-{id}"),
        forked_from_id: None,
        cwd: "/tmp/project".into(),
        project_id: Some("native-project".into()),
        name: Some("Imported Thread".into()),
        preview: "preview".into(),
        source: json!({"custom": "fixture"}),
        history_mode: "paginated".into(),
        status: json!({"type": "notLoaded"}),
        archived: false,
        created_at: 1,
        updated_at: 12,
        turns,
        metadata: serde_json::Map::new(),
    }
}

fn identity() -> CodexImportIdentity<'static> {
    CodexImportIdentity {
        provider_id: "builtin-codex",
        project_id: "project",
        agent_id: "agent",
    }
}

#[test]
fn one_turn_can_project_multiple_user_and_assistant_segments() {
    let projected = materialize_thread(
        &snapshot(
            "thread",
            vec![turn(
                "turn",
                vec![
                    user("user-1", "first"),
                    item("agent-1", "agentMessage", json!({"text": "answer"})),
                    user("user-2", "steer"),
                    item("agent-2", "agentMessage", json!({"text": "final"})),
                ],
            )],
        ),
        identity(),
        &[],
        &[],
    )
    .unwrap();

    let roles = projected
        .messages
        .iter()
        .skip(1)
        .map(|message| message.role)
        .collect::<Vec<_>>();
    assert_eq!(
        roles,
        vec![
            MessageRole::User,
            MessageRole::Assistant,
            MessageRole::User,
            MessageRole::Assistant,
        ]
    );
    assert_eq!(projected.messages[1].text.as_deref(), Some("first"));
    assert_eq!(projected.messages[3].text.as_deref(), Some("steer"));
    assert_eq!(
        projected.messages[2]
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/origin")
            .and_then(Value::as_str),
        Some("provider")
    );
    assert_eq!(
        projected.messages[2]
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/sub_messages/0/type")
            .and_then(Value::as_str),
        Some("provider_item")
    );
    assert_eq!(
        projected.messages[2]
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/sub_messages/0/ordinal")
            .and_then(Value::as_u64),
        Some(1)
    );
    assert_eq!(
        projected.messages[4]
            .data
            .as_ref()
            .unwrap()
            .pointer("/native_message/sub_messages/0/ordinal")
            .and_then(Value::as_u64),
        Some(3)
    );
}

#[test]
fn repeated_snapshot_reuses_session_and_immutable_messages() {
    let snapshot = snapshot("thread", vec![turn("turn", vec![user("user", "hello")])]);
    let first = materialize_thread(&snapshot, identity(), &[], &[]).unwrap();
    let second = materialize_thread(
        &snapshot,
        identity(),
        std::slice::from_ref(&first.session),
        &first.messages,
    )
    .unwrap();

    assert_eq!(second.session.id, first.session.id);
    assert_eq!(
        second.session.current_message_id(),
        first.session.current_message_id()
    );
    assert!(second.messages.is_empty());
}

#[test]
fn incomplete_tail_preserves_last_complete_turn() {
    let mut active = turn("active", vec![user("pending", "not published")]);
    active.status = "inProgress".into();
    active.completed_at = None;
    let projected = materialize_thread(
        &snapshot(
            "thread",
            vec![turn("complete", vec![user("user", "published")]), active],
        ),
        identity(),
        &[],
        &[],
    )
    .unwrap();

    let SessionSource::CodexThread(source) = &projected.session.source else {
        panic!("expected Codex Session")
    };
    assert_eq!(
        source.history_completeness,
        ProviderHistoryCompleteness::Partial
    );
    assert_eq!(projected.messages.len(), 2);
    assert_eq!(projected.messages[1].text.as_deref(), Some("published"));
}

#[test]
fn fork_with_verified_turn_prefix_reuses_parent_lineage() {
    let prefix = turn(
        "shared-turn",
        vec![
            user("shared-user", "hello"),
            item("shared-agent", "agentMessage", json!({"text": "answer"})),
        ],
    );
    let parent = materialize_thread(
        &snapshot("parent", vec![prefix.clone()]),
        identity(),
        &[],
        &[],
    )
    .unwrap();
    let mut child_snapshot = snapshot(
        "child",
        vec![prefix, turn("child-turn", vec![user("child-user", "next")])],
    );
    child_snapshot.session_id = "different-native-session".into();
    child_snapshot.forked_from_id = Some("parent".into());
    let child = materialize_thread(
        &child_snapshot,
        identity(),
        std::slice::from_ref(&parent.session),
        &parent.messages,
    )
    .unwrap();

    let SessionSource::CodexThread(parent_source) = &parent.session.source else {
        panic!("expected parent Codex Session")
    };
    let SessionSource::CodexThread(child_source) = &child.session.source else {
        panic!("expected child Codex Session")
    };
    assert_eq!(child_source.lineage_id, parent_source.lineage_id);
    assert_eq!(
        child_source.relationship_state,
        ProviderRelationshipState::Verified
    );
}

#[test]
fn provider_payload_redacts_sensitive_fields() {
    let mut native = snapshot(
        "thread",
        vec![turn(
            "turn",
            vec![item(
                "command",
                "commandExecution",
                json!({
                    "authorization": "Bearer private",
                    "nested": {"access_token": "private"},
                }),
            )],
        )],
    );
    native.metadata.insert("model".into(), json!("gpt-5.6-sol"));
    native
        .metadata
        .insert("session_token".into(), json!("private"));
    let projected = materialize_thread(&native, identity(), &[], &[]).unwrap();
    let payload = projected.messages[1]
        .data
        .as_ref()
        .unwrap()
        .pointer("/native_message/sub_messages/0/payload")
        .unwrap();
    assert_eq!(payload["authorization"], "[redacted]");
    assert_eq!(payload["nested"]["access_token"], "[redacted]");
    let SessionSource::CodexThread(source) = &projected.session.source else {
        panic!("expected Codex Session")
    };
    assert_eq!(source.native_metadata["model"], "gpt-5.6-sol");
    assert_eq!(source.native_metadata["session_token"], "[redacted]");
}

#[test]
fn nonexistent_native_cwd_does_not_bind_by_lexical_prefix() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = CodexImportContext::default();
    state.projects.push(ProjectRecord {
        id: "project".into(),
        name: "Project".into(),
        workdir: directory.path().display().to_string(),
        root_message_id: "root".into(),
        repo_url: None,
        base_commit: String::new(),
        defaults: ait_domain::ProjectDefaults::default(),
    });

    let matches = matching_projects(&state, &directory.path().join("missing-native-cwd"));

    assert!(matches.is_empty());
}
