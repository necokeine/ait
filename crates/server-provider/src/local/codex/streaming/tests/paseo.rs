//! Incremental cases from Paseo's Codex stream and timeline projection suites.

use super::*;

fn start(stream: &mut Stream, id: &str, kind: &str) -> NativeItem {
    item(
        stream
            .progress(
                "item/started",
                &json!({"turnId":"turn","item":{"id":id,"type":kind}}),
            )
            .unwrap(),
    )
}

#[test]
fn distinct_assistant_messages_keep_independent_text_limits_and_identity() {
    let mut stream = Stream::default();
    let first = item(
        stream
            .progress(
                "item/agentMessage/delta",
                &params("first", &"x".repeat(MAX_TEXT)),
            )
            .unwrap(),
    );
    let second = item(
        stream
            .progress(
                "item/agentMessage/delta",
                &params("second", "second message"),
            )
            .unwrap(),
    );
    assert_ne!(first.key, second.key);
    assert_eq!(second.item["messageId"], "second");
    assert_eq!(second.item["text"], "second message");
}

#[test]
fn text_limit_is_cumulative_and_counts_utf8_bytes() {
    let mut stream = Stream::default();
    stream
        .progress(
            "item/agentMessage/delta",
            &params("a", &"界".repeat(MAX_TEXT / 3)),
        )
        .unwrap();
    assert_eq!(
        stream.progress("item/agentMessage/delta", &params("a", "x")),
        Err(AgentSessionError::Failed)
    );
}

#[test]
fn identical_delta_payloads_have_distinct_observation_ids() {
    let mut stream = Stream::default();
    let first = stream
        .progress("item/agentMessage/delta", &params("a", "ha"))
        .unwrap()
        .unwrap();
    let second = stream
        .progress("item/agentMessage/delta", &params("a", "ha"))
        .unwrap()
        .unwrap();
    let (
        AgentTurnEvent::Progress {
            observation: left, ..
        },
        AgentTurnEvent::Progress {
            observation: right, ..
        },
    ) = (first, second)
    else {
        panic!("expected progress observations");
    };
    assert_ne!(left, right);
}

#[test]
fn reasoning_same_part_is_additive_and_each_new_part_adds_one_separator() {
    let mut stream = Stream::default();
    let mut combined = String::new();
    for (index, delta) in [
        (0, "First"),
        (0, " part"),
        (1, "Second"),
        (1, " part"),
        (2, "Third"),
    ] {
        let mut input = params("reasoning", delta);
        input["summaryIndex"] = json!(index);
        let entry = item(
            stream
                .progress("item/reasoning/summaryTextDelta", &input)
                .unwrap(),
        );
        assert_eq!(entry.item["type"], "reasoning");
        assert!(entry.item.get("messageId").is_none());
        combined.push_str(entry.item["text"].as_str().unwrap());
    }
    assert_eq!(combined, "First part\nSecond part\nThird");
}

#[test]
fn skipped_reasoning_part_is_rejected_instead_of_silently_losing_text() {
    let mut stream = Stream::default();
    let mut input = params("r", "missing earlier parts");
    input["summaryIndex"] = json!(2);
    assert_eq!(
        stream.progress("item/reasoning/summaryTextDelta", &input),
        Err(AgentSessionError::Failed)
    );
}

#[test]
fn non_numeric_reasoning_index_is_rejected_before_publishing() {
    let mut stream = Stream::default();
    let mut input = params("r", "reasoning");
    input["summaryIndex"] = json!("0");
    assert_eq!(
        stream.progress("item/reasoning/summaryTextDelta", &input),
        Err(AgentSessionError::Failed)
    );
}

#[test]
fn silent_shell_command_has_a_running_card_before_any_output() {
    let mut stream = Stream::default();
    let entry = start(&mut stream, "silent", "commandExecution");
    assert_eq!(entry.item["callId"], "silent");
    assert_eq!(entry.item["status"], "running");
    assert_eq!(entry.item["detail"]["output"], "");
    assert_eq!(entry.key, "native:turn:silent");
}

#[test]
fn interleaved_tool_output_updates_only_its_own_running_card() {
    let mut stream = Stream::default();
    start(&mut stream, "first", "commandExecution");
    start(&mut stream, "second", "fileChange");
    stream
        .progress("item/commandExecution/outputDelta", &params("first", "one"))
        .unwrap();
    let second = item(
        stream
            .progress("item/fileChange/outputDelta", &params("second", "two"))
            .unwrap(),
    );
    let first = item(
        stream
            .progress(
                "item/commandExecution/outputDelta",
                &params("first", " more"),
            )
            .unwrap(),
    );
    assert_eq!(second.item["detail"]["output"], "two");
    assert_eq!(first.item["detail"]["output"], "one more");
}

#[test]
fn tool_tail_discards_old_bytes_without_splitting_multibyte_characters() {
    let mut stream = Stream::default();
    start(&mut stream, "tool", "commandExecution");
    stream
        .progress(
            "item/commandExecution/outputDelta",
            &params("tool", &"界".repeat(MAX_OUTPUT / 3)),
        )
        .unwrap();
    let entry = item(
        stream
            .progress("item/commandExecution/outputDelta", &params("tool", "ab"))
            .unwrap(),
    );
    let text = entry.item["detail"]["output"].as_str().unwrap();
    assert!(text.len() <= MAX_OUTPUT);
    assert_eq!(text, format!("{}ab", "界".repeat(MAX_OUTPUT / 3 - 1)));
}

#[test]
fn output_without_a_started_tool_does_not_create_an_unowned_timeline_row() {
    let mut stream = Stream::default();
    assert!(
        stream
            .progress(
                "item/commandExecution/outputDelta",
                &params("unknown", "out")
            )
            .unwrap()
            .is_none()
    );
    start(&mut stream, "tool", "commandExecution");
    assert!(
        stream
            .progress("item/commandExecution/outputDelta", &params("tool", ""))
            .unwrap()
            .is_none()
    );
}

#[test]
fn repeated_terminal_completion_remains_idempotent_at_capacity() {
    let mut stream = Stream::default();
    for index in 0..MAX_ITEMS {
        assert!(stream.complete(&json!({"id":index.to_string()})).unwrap());
    }
    assert!(!stream.complete(&json!({"id":"0"})).unwrap());
    assert_eq!(
        stream.complete(&json!({"id":"new"})),
        Err(AgentSessionError::Failed)
    );
}

#[test]
fn completing_a_text_item_releases_capacity_for_a_new_message() {
    let mut stream = Stream::default();
    for index in 0..MAX_ITEMS {
        stream
            .progress("item/agentMessage/delta", &params(&index.to_string(), "x"))
            .unwrap();
    }
    assert_eq!(
        stream.progress("item/agentMessage/delta", &params("new", "x")),
        Err(AgentSessionError::Failed)
    );
    stream.complete(&json!({"id":"0"})).unwrap();
    assert!(
        stream
            .progress("item/agentMessage/delta", &params("new", "x"))
            .unwrap()
            .is_some()
    );
    assert!(
        stream
            .progress("item/agentMessage/delta", &params("0", "late"))
            .unwrap()
            .is_none()
    );
}

#[test]
fn completing_a_tool_releases_running_card_capacity_and_cannot_reopen_it() {
    let mut stream = Stream::default();
    for index in 0..MAX_ITEMS {
        start(&mut stream, &index.to_string(), "commandExecution");
    }
    let extra = json!({"turnId":"turn","item":{"id":"new","type":"commandExecution"}});
    assert_eq!(
        stream.progress("item/started", &extra),
        Err(AgentSessionError::Failed)
    );
    stream.complete(&json!({"id":"0"})).unwrap();
    assert!(stream.progress("item/started", &extra).unwrap().is_some());
    let completed = json!({"turnId":"turn","item":{"id":"0","type":"commandExecution"}});
    assert!(
        stream
            .progress("item/started", &completed)
            .unwrap()
            .is_none()
    );
}
