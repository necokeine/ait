//! Paseo's native-history identity, timestamp and import-list contracts.

use super::*;

#[test]
fn persisted_assistant_items_retain_their_native_message_and_turn_ids() {
    let parsed = history(&thread()).unwrap();
    assert_eq!(parsed.entries[0].item["messageId"], "reply");
    assert_eq!(parsed.entries[0].key, "native:turn:reply");
    assert_eq!(parsed.entries[0].turn_id.as_deref(), Some("turn"));
}

#[test]
fn repeated_item_ids_in_different_turns_remain_distinct_history_entries() {
    let mut native = thread();
    let mut second = native["turns"][0].clone();
    second["id"] = json!("second-turn");
    second["items"][0]["text"] = json!("second answer");
    native["turns"].as_array_mut().unwrap().push(second);
    let parsed = history(&native).unwrap();
    assert_eq!(parsed.entries.len(), 2);
    assert_ne!(parsed.entries[0].key, parsed.entries[1].key);
    assert_eq!(parsed.entries[1].item["text"], "second answer");
}

#[test]
fn timestamp_less_items_use_their_native_turn_start_time() {
    let mut native = thread();
    native["turns"][0]["startedAt"] = json!(1_700_000_005);
    let parsed = history(&native).unwrap();
    assert_eq!(parsed.entries[0].timestamp, "2023-11-14T22:13:25+00:00");
    assert_eq!(parsed.created_at, "2023-11-14T22:13:20+00:00");
}

#[test]
fn missing_turn_time_uses_persisted_creation_time_without_inventing_now() {
    let parsed = history(&thread()).unwrap();
    assert_eq!(parsed.entries[0].timestamp, parsed.created_at);
    assert_eq!(parsed.entries[0].timestamp, "2023-11-14T22:13:20+00:00");
}

#[test]
fn failed_and_interrupted_turns_preserve_their_completed_native_items() {
    let mut native = thread();
    native["turns"][0]["status"] = json!("failed");
    let mut second = native["turns"][0].clone();
    second["id"] = json!("interrupted");
    second["status"] = json!("interrupted");
    native["turns"].as_array_mut().unwrap().push(second);
    let parsed = history(&native).unwrap();
    assert!(!parsed.active);
    assert_eq!(parsed.entries.len(), 2);
    assert_eq!(parsed.entries[1].turn_id.as_deref(), Some("interrupted"));
}

#[test]
fn active_turns_are_excluded_from_immutable_history_and_mark_session_active() {
    let mut native = thread();
    let mut active = native["turns"][0].clone();
    active["id"] = json!("active");
    active["status"] = json!("inProgress");
    active["items"][0]["text"] = json!("not complete");
    native["turns"].as_array_mut().unwrap().push(active);
    let parsed = history(&native).unwrap();
    assert!(parsed.active);
    assert_eq!(parsed.entries.len(), 1);
    assert_eq!(parsed.entries[0].item["text"], "hello");
}

#[test]
fn native_parent_source_and_direct_parent_field_resolve_the_same_identity() {
    let mut native = thread();
    native["source"] = json!({"subAgent":{"thread_spawn":{"parent_thread_id":"root"}}});
    assert_eq!(history(&native).unwrap().parent_id.as_deref(), Some("root"));
    native["parentThreadId"] = json!("explicit");
    assert_eq!(
        history(&native).unwrap().parent_id.as_deref(),
        Some("explicit")
    );
    native["parentThreadId"] = Value::Null;
    assert_eq!(history(&native).unwrap().parent_id.as_deref(), Some("root"));
}

#[test]
fn invalid_parent_identity_fails_instead_of_becoming_an_unrelated_root_session() {
    let mut native = thread();
    native["source"] = json!({"subAgent":{"thread_spawn":{"parent_thread_id":"bad\nparent"}}});
    assert!(history(&native).is_err());
    native["parentThreadId"] = json!(42);
    assert!(history(&native).is_err());
}

#[test]
fn whitespace_only_native_titles_and_previews_remain_absent() {
    let mut native = thread();
    native["name"] = json!(" \n ");
    native["preview"] = json!("\t ");
    let descriptor = descriptor(&native).unwrap();
    assert!(descriptor.title.is_none());
    assert!(descriptor.first_prompt_preview.is_none());
}

#[cfg(unix)]
#[tokio::test]
async fn import_list_excludes_nested_native_subagents_without_hydrating_history() {
    let fixture = crate::test_support::Fixture::new();
    let mut child = thread();
    child["id"] = json!("nested-child");
    child["source"] = json!({"subAgent":{"thread_spawn":{"parent_thread_id":"root"}}});
    let pages = json!({"first":{"data":[thread(),child],"nextCursor":null}});
    std::fs::write(fixture.cwd.join("session-pages.json"), pages.to_string()).unwrap();
    let sessions = fixture
        .client()
        .list_native(&ListOptions {
            cwd: Some(fixture.spec().cwd),
            scan_limit: 100,
        })
        .await
        .unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0].provider_handle_id, "native");
    assert!(fixture.requests().iter().all(|request| !matches!(
        request["method"].as_str(),
        Some("thread/read" | "thread/resume" | "thread/start")
    )));
}
