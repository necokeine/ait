//! Durable counterparts of Paseo's stream-completion and projected-sequence tests.

use super::*;

fn text_item(id: &str, kind: &str, text: &str) -> NativeItem {
    NativeItem {
        key: format!("native:turn:{id}"),
        turn_id: Some("turn".to_owned()),
        timestamp: "2026-09-26T00:00:00Z".to_owned(),
        item: json!({"type":kind,"messageId":id,"text":text}),
    }
}

fn assistant(id: &str, text: &str) -> NativeItem {
    text_item(id, "assistant_message", text)
}

#[test]
fn fully_streamed_assistant_completion_does_not_replay_its_text() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "1", &assistant("m", "Hello "))
        .unwrap();
    timeline
        .progress("a", "codex", "2", &assistant("m", "world"))
        .unwrap();
    timeline
        .append("a", "codex", &[assistant("m", "Hello world")])
        .unwrap();
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[2].entry.item["text"], "");
    assert_eq!(rows[2].seq, 3);
    assert_eq!(rows[2].entry.key, rows[0].entry.key);
}

#[test]
fn reasoning_completion_emits_only_missing_summary_suffix() {
    let timeline = Timeline::memory().unwrap();
    let reasoning = |text| text_item("r", "reasoning", text);
    timeline
        .progress("a", "codex", "1", &reasoning("First part"))
        .unwrap();
    timeline
        .progress("a", "codex", "2", &reasoning("\nSecond"))
        .unwrap();
    timeline
        .append("a", "codex", &[reasoning("First part\nSecond part")])
        .unwrap();
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(rows[2].entry.item["text"], " part");
    assert_eq!(
        rows.iter()
            .map(|row| row.entry.item["text"].as_str().unwrap())
            .collect::<String>(),
        "First part\nSecond part"
    );
}

#[test]
fn interleaved_assistant_messages_keep_separate_prefixes_and_source_sequences() {
    let timeline = Timeline::memory().unwrap();
    for (observation, id, text) in [
        ("1", "first", "A"),
        ("2", "second", "B"),
        ("3", "first", "C"),
    ] {
        timeline
            .progress("a", "codex", observation, &assistant(id, text))
            .unwrap();
    }
    timeline
        .append(
            "a",
            "codex",
            &[assistant("second", "BD"), assistant("first", "ACE")],
        )
        .unwrap();
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(
        rows.iter().map(|row| row.seq).collect::<Vec<_>>(),
        [1, 2, 3, 4, 5]
    );
    assert_eq!(rows[3].entry.item["text"], "D");
    assert_eq!(rows[4].entry.item["text"], "E");
}

#[test]
fn tool_snapshots_keep_source_updates_and_the_final_native_output() {
    let timeline = Timeline::memory().unwrap();
    let mut running = assistant("tool", "");
    running.item =
        json!({"type":"tool_call","callId":"tool","status":"running","detail":{"output":"tail"}});
    timeline.progress("a", "codex", "1", &running).unwrap();
    timeline
        .progress("a", "codex", "2", &assistant("answer", "Answer"))
        .unwrap();
    let mut complete = running.clone();
    complete.item["status"] = json!("completed");
    complete.item["detail"]["output"] = json!("complete output, including tail");
    timeline.append("a", "codex", &[complete.clone()]).unwrap();
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(rows[0].entry.item["status"], "running");
    assert_eq!(rows[1].entry.item["text"], "Answer");
    assert_eq!(rows[2].entry.item, complete.item);
    assert_eq!(rows[2].seq, 3);
}

#[test]
fn identical_fragment_payloads_with_different_observations_are_both_preserved() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "first", &assistant("m", "ha"))
        .unwrap();
    timeline
        .progress("a", "codex", "second", &assistant("m", "ha"))
        .unwrap();
    timeline
        .append("a", "codex", &[assistant("m", "haha!")])
        .unwrap();
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(
        rows.iter()
            .map(|row| row.entry.item["text"].as_str().unwrap())
            .collect::<String>(),
        "haha!"
    );
}

#[test]
fn observation_identity_is_scoped_to_agent_and_not_shared_across_sessions() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("first", "codex", "shared", &assistant("m", "first"))
        .unwrap();
    timeline
        .progress("second", "codex", "shared", &assistant("m", "second"))
        .unwrap();
    let first = timeline.read("first").unwrap();
    let second = timeline.read("second").unwrap();
    assert_ne!(first.0, second.0);
    assert_eq!(first.1[0].seq, 1);
    assert_eq!(second.1[0].seq, 1);
    assert_eq!(first.1[0].entry.item["text"], "first");
    assert_eq!(second.1[0].entry.item["text"], "second");
}

#[test]
fn replay_of_a_committed_progress_observation_remains_idempotent_after_completion() {
    let timeline = Timeline::memory().unwrap();
    let fragment = assistant("m", "prefix");
    timeline.progress("a", "codex", "one", &fragment).unwrap();
    timeline
        .append("a", "codex", &[assistant("m", "prefix suffix")])
        .unwrap();
    timeline.progress("a", "codex", "one", &fragment).unwrap();
    assert_eq!(timeline.read("a").unwrap().1.len(), 2);
    assert_eq!(
        timeline.progress("a", "codex", "late", &fragment),
        Err(ErrorCode::IdempotencyConflict)
    );
}

#[test]
fn failed_completion_batch_rolls_back_earlier_valid_rows_and_cursor_allocation() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "one", &assistant("m", "prefix"))
        .unwrap();
    assert_eq!(
        timeline.append(
            "a",
            "codex",
            &[
                assistant("new", "would be valid"),
                assistant("m", "different")
            ]
        ),
        Err(ErrorCode::IdempotencyConflict)
    );
    assert_eq!(timeline.read("a").unwrap().1.len(), 1);
    let (_, positions) = timeline
        .append("a", "codex", &[assistant("m", "prefix suffix")])
        .unwrap();
    assert_eq!(positions, [2]);
}

#[test]
fn recovery_of_matching_native_history_keeps_the_epoch_and_committed_prefix() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("timeline.sqlite");
    let timeline = Timeline::open(&path).unwrap();
    timeline
        .progress("a", "codex", "one", &assistant("m", "partial"))
        .unwrap();
    let epoch = timeline.read("a").unwrap().0;
    drop(timeline);
    let reopened = Timeline::open(&path).unwrap();
    assert_eq!(
        reopened
            .reconcile("a", "codex", &[assistant("m", "partial recovered")])
            .unwrap(),
        epoch
    );
    let rows = reopened.read("a").unwrap().1;
    assert_eq!(rows[0].entry.item["text"], "partial");
    assert_eq!(rows[1].entry.item["text"], " recovered");
}

#[test]
fn changed_native_history_retires_progress_and_preserves_plugin_rows() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "one", &assistant("m", "old"))
        .unwrap();
    let mut plugin = assistant("plugin", "");
    plugin.key = "plugin:card".to_owned();
    plugin.item = json!({"type":"plugin","data":{"keep":true}});
    timeline.append("a", "codex", &[plugin]).unwrap();
    let previous = timeline.read("a").unwrap().0;
    let epoch = timeline
        .reconcile("a", "codex", &[assistant("m", "new")])
        .unwrap();
    assert_ne!(epoch, previous);
    let rows = timeline.read("a").unwrap().1;
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0].entry.item["text"], "new");
    assert_eq!(rows[1].entry.item["data"]["keep"], true);
    let retired: usize = timeline
        .database
        .lock()
        .unwrap()
        .query_row(
            "SELECT count(*) FROM retired_entries WHERE epoch=?",
            [previous],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(retired, 2);
}

#[test]
fn rewritten_turn_identity_resets_even_when_message_text_is_unchanged() {
    let timeline = Timeline::memory().unwrap();
    let original = assistant("m", "same text");
    let (before, _) = timeline
        .append("a", "codex", std::slice::from_ref(&original))
        .unwrap();
    let mut rewritten = original;
    rewritten.turn_id = Some("different-turn".to_owned());
    let after = timeline.reconcile("a", "codex", &[rewritten]).unwrap();
    assert_ne!(before, after);
    assert_eq!(
        timeline.read("a").unwrap().1[0].entry.turn_id.as_deref(),
        Some("different-turn")
    );
}

#[test]
fn oversized_history_replacement_leaves_progress_epoch_and_plugin_state_unchanged() {
    let timeline = Timeline::memory().unwrap();
    timeline
        .progress("a", "codex", "one", &assistant("m", "safe prefix"))
        .unwrap();
    let before = timeline.read("a").unwrap();
    assert_eq!(
        timeline.reconcile("a", "codex", &[assistant("m", &"x".repeat(300_000))]),
        Err(ErrorCode::ResourceExhausted)
    );
    let after = timeline.read("a").unwrap();
    assert_eq!(before.0, after.0);
    assert_eq!(after.1.len(), 1);
    assert_eq!(after.1[0].entry.item["text"], "safe prefix");
    let retired: usize = timeline
        .database
        .lock()
        .unwrap()
        .query_row("SELECT count(*) FROM retired_entries", [], |row| row.get(0))
        .unwrap();
    assert_eq!(retired, 0);
}
