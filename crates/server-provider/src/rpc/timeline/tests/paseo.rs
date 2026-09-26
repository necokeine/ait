//! Cases ported from Paseo's timeline store, prompt index and chat search tests.

use super::*;

fn row(seq: u64, key: &str, kind: &str, text: &str) -> Row {
    Row {
        seq,
        provider: "codex".to_owned(),
        entry: NativeItem {
            key: key.to_owned(),
            turn_id: Some("turn".to_owned()),
            timestamp: format!("2026-09-26T00:00:{seq:02}Z"),
            item: json!({"type":kind,"text":text}),
        },
    }
}

fn query(text: &str) -> SearchRequest {
    SearchRequest {
        agent_id: "agent".to_owned(),
        query: text.to_owned(),
        cursor: None,
    }
}

fn retained_rows() -> Vec<Row> {
    (5..=7)
        .map(|seq| row(seq, &seq.to_string(), "assistant_message", "answer"))
        .collect()
}

#[test]
fn overshooting_before_cursor_clamps_to_the_bounded_tail_without_reset() {
    let page = fetch(
        &request(json!({"agentId":"agent","direction":"before", "cursor":{"epoch":"e","seq":100},"limit":2})),
        "e",
        &retained_rows(),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["reset"], false);
    assert_eq!(page["gap"], false);
    assert_eq!(page["staleCursor"], false);
    assert_eq!(page["window"], json!({"minSeq":5,"maxSeq":7,"nextSeq":8}));
    assert_eq!(page["entries"][0]["seqStart"], 6);
    assert_eq!(page["entries"][1]["seqStart"], 7);
    assert_eq!(page["hasOlder"], true);
    assert_eq!(page["hasNewer"], false);
}

#[test]
fn after_cursor_behind_retained_history_returns_a_bounded_reset_window() {
    let page = fetch(
        &request(
            json!({"agentId":"agent","direction":"after","cursor":{"epoch":"e","seq":1},"limit":1}),
        ),
        "e",
        &retained_rows(),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["reset"], true);
    assert_eq!(page["gap"], true);
    assert_eq!(page["staleCursor"], false);
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["seqStart"], 7);
    assert_eq!(page["hasOlder"], true);
    assert_eq!(page["hasNewer"], false);
}

#[test]
fn after_cursor_immediately_before_retained_history_is_contiguous() {
    let page = fetch(
        &request(json!({"agentId":"agent","cursor":{"epoch":"e","seq":4},"limit":1})),
        "e",
        &retained_rows(),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["gap"], false);
    assert_eq!(page["reset"], false);
    assert_eq!(page["entries"][0]["seqStart"], 5);
    assert_eq!(page["hasNewer"], true);
}

#[test]
fn after_cursor_ahead_of_native_history_resets_to_bounded_tail() {
    let page = fetch(
        &request(json!({"agentId":"agent","cursor":{"epoch":"e","seq":8},"limit":1})),
        "e",
        &retained_rows(),
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["gap"], true);
    assert_eq!(page["entries"][0]["seqStart"], 7);
}

#[test]
fn empty_incremental_window_keeps_epoch_and_reports_no_gap() {
    let page = fetch(
        &request(json!({"agentId":"agent","cursor":{"epoch":"e","seq":0}})),
        "e",
        &[],
        &json!({"id":"agent"}),
    )
    .unwrap();
    assert_eq!(page["epoch"], "e");
    assert_eq!(page["agent"]["id"], "agent");
    assert_eq!(page["gap"], false);
    assert_eq!(page["entries"], json!([]));
    assert_eq!(page["window"], json!({"minSeq":0,"maxSeq":0,"nextSeq":1}));
    assert!(page["endCursor"].is_null());
}

#[test]
fn later_source_chunks_do_not_mutate_an_already_fetched_window() {
    let mut rows = vec![row(1, "message", "assistant_message", "A")];
    let request = request(json!({"agentId":"agent"}));
    let before = fetch(&request, "e", &rows, &Value::Null).unwrap();
    rows.push(row(2, "message", "assistant_message", "B"));
    let after = fetch(&request, "e", &rows, &Value::Null).unwrap();
    assert_eq!(before["entries"][0]["item"]["text"], "A");
    assert_eq!(before["endCursor"]["seq"], 1);
    assert_eq!(after["endCursor"]["seq"], 2);
    assert_eq!(
        before["entries"][0]["sourceSeqRanges"],
        json!([{"startSeq":1,"endSeq":1}])
    );
}

#[test]
fn incremental_fetch_returns_only_new_fragments_with_original_source_positions() {
    // Rust currently exposes identity rows; clients append the new fragment to their prior text.
    let rows = vec![
        row(1, "message", "assistant_message", "A"),
        row(2, "message", "assistant_message", "B"),
    ];
    let page = fetch(
        &request(json!({"agentId":"agent","cursor":{"epoch":"e","seq":1}})),
        "e",
        &rows,
        &Value::Null,
    )
    .unwrap();
    assert_eq!(page["entries"].as_array().unwrap().len(), 1);
    assert_eq!(page["entries"][0]["item"]["text"], "B");
    assert_eq!(page["entries"][0]["seqStart"], 2);
    assert_eq!(page["hasOlder"], true);
}

#[test]
fn search_finds_literal_punctuation_without_matching_tools_reasoning_or_task_state() {
    let rows = vec![
        row(
            1,
            "a",
            "assistant_message",
            "```ts\nconst value = 'a.b';\n```",
        ),
        row(2, "b", "reasoning", "a.b"),
        row(3, "c", "todo", "a.b"),
        row(4, "d", "user_message", "axb"),
        row(5, "e", "user_message", "a.b"),
        row(6, "f", "tool_call", "a.b"),
    ];
    assert_eq!(
        search(&query("a.b"), "e", &rows).unwrap()["locations"],
        json!([{"seq":1,"role":"assistant"},{"seq":5,"role":"user"}])
    );
}

#[test]
fn search_joins_same_message_deltas_across_interleaved_tools() {
    let rows = vec![
        row(1, "message", "assistant_message", "hel"),
        row(2, "tool", "tool_call", "other"),
        row(3, "message", "assistant_message", "lo "),
        row(4, "second", "assistant_message", "separate"),
        row(5, "message", "assistant_message", "world"),
    ];
    let found = search(&query("HELLO world"), "e", &rows).unwrap();
    assert_eq!(found["locations"], json!([{"seq":1,"role":"assistant"}]));
    assert!(
        search(&query("hello separate"), "e", &rows).unwrap()["locations"]
            .as_array()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn search_does_not_join_distinct_messages_or_return_duplicate_locations() {
    let rows = vec![
        row(1, "first", "assistant_message", "first "),
        row(2, "second", "assistant_message", "second"),
        row(3, "first", "assistant_message", "target target"),
    ];
    assert_eq!(
        search(&query("target"), "e", &rows).unwrap()["locations"],
        json!([{"seq":1,"role":"assistant"}])
    );
    assert_eq!(
        search(&query("first second"), "e", &rows).unwrap()["locations"],
        json!([])
    );
}

#[test]
fn search_empty_query_has_no_locations_and_large_query_is_rejected() {
    let rows = vec![row(1, "a", "user_message", "target")];
    let empty = search(&query(" \n\t "), "e", &rows).unwrap();
    assert_eq!(empty["locations"], json!([]));
    assert!(empty["nextCursor"].is_null());
    assert_eq!(
        search(&query(&"界".repeat(1366)), "e", &rows),
        Err(ErrorCode::InvalidMessage)
    );
}

#[test]
fn search_final_page_preserves_remaining_locations_and_clears_cursor() {
    let rows: Vec<_> = (1..=202)
        .map(|seq| row(seq, &seq.to_string(), "user_message", "target"))
        .collect();
    let first = search(&query("target"), "e", &rows).unwrap();
    assert_eq!(first["locations"].as_array().unwrap().len(), 200);
    assert_eq!(first["nextCursor"], 200);
    let second = search(
        &SearchRequest {
            cursor: Some(200),
            ..query("target")
        },
        "e",
        &rows,
    )
    .unwrap();
    assert_eq!(
        second["locations"],
        json!([{"seq":201,"role":"user"},{"seq":202,"role":"user"}])
    );
    assert!(second["nextCursor"].is_null());
}

#[test]
fn prompt_index_preserves_sparse_source_positions_and_original_timestamps() {
    let rows = vec![
        row(3, "first", "user_message", "  First\n\n   prompt  "),
        row(4, "reply", "assistant_message", "response"),
        row(8, "second", "user_message", "Second prompt"),
    ];
    assert_eq!(
        prompts("agent", "e", &rows).unwrap()["prompts"],
        json!([
            {"seq":3,"timestamp":"2026-09-26T00:00:03Z","preview":"First prompt"},
            {"seq":8,"timestamp":"2026-09-26T00:00:08Z","preview":"Second prompt"}
        ])
    );
}

#[test]
fn exactly_bounded_prompt_is_not_truncated_and_unicode_space_is_collapsed() {
    assert_eq!(preview(&"x".repeat(120)), "x".repeat(120));
    assert_eq!(
        preview("first\u{2003}second\tthird\r\nfourth"),
        "first second third fourth"
    );
    assert_eq!(preview(" \n\t"), "");
}
