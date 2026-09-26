//! Fork-context regressions from Paseo's activity curator and streamed assistant projection.

use super::*;

fn fragment(seq: u64, id: &str, text: &str) -> Row {
    let mut result = row(
        seq,
        json!({"type":"assistant_message","messageId":id,"text":text}),
    );
    result.entry.key = format!("native:turn:{id}");
    result
}

fn attachment(rows: &[Row]) -> String {
    export(&request(), "epoch", rows, &json!({})).unwrap()["attachment"]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn streamed_assistant_fragments_reconstruct_words_without_inserting_line_breaks() {
    let text = attachment(&[fragment(1, "a", "hel"), fragment(2, "a", "lo")]);
    assert!(text.contains("[Assistant] hello\n"), "{text}");
    assert!(!text.contains("hel\nlo"));
}

#[test]
fn whitespace_only_stream_fragments_are_retained_inside_the_logical_message() {
    let text = attachment(&[
        fragment(1, "a", "Hello"),
        fragment(2, "a", "  \n"),
        fragment(3, "a", "world"),
    ]);
    assert!(text.contains("[Assistant] Hello  \nworld\n"), "{text}");
}

#[test]
fn distinct_assistant_messages_keep_separate_labels_and_boundaries() {
    let text = attachment(&[
        fragment(1, "first", "First"),
        fragment(2, "second", "Sec"),
        fragment(3, "second", "ond"),
    ]);
    assert!(
        text.contains("[Assistant] First\n[Assistant] Second\n"),
        "{text}"
    );
}

#[test]
fn interleaved_tool_rows_preserve_assistant_fragment_order() {
    let rows = [
        fragment(1, "a", "Before "),
        row(
            2,
            json!({"type":"tool_call","name":"Read","input":"private"}),
        ),
        fragment(3, "a", "after"),
    ];
    let text = attachment(&rows);
    assert!(
        text.contains("[Assistant] Before\n[Read]\n[Assistant] after\n"),
        "{text}"
    );
    assert!(!text.contains("private"));
}

#[test]
fn interleaved_user_message_preserves_assistant_fragment_order() {
    let text = attachment(&[
        fragment(1, "a", "Before "),
        row(2, json!({"type":"user_message","text":"followup"})),
        fragment(3, "a", "after"),
    ]);
    assert!(
        text.contains("[Assistant] Before\n[User] followup\n[Assistant] after\n"),
        "{text}"
    );
}

#[test]
fn another_assistant_message_interrupts_fragment_coalescing() {
    let text = attachment(&[
        fragment(1, "a", "Before "),
        fragment(2, "b", "independent"),
        fragment(3, "a", "after"),
    ]);
    assert!(
        text.contains("[Assistant] Before\n[Assistant] independent\n[Assistant] after\n"),
        "{text}"
    );
}

#[test]
fn source_sequence_gap_interrupts_fragment_coalescing() {
    let text = attachment(&[fragment(1, "a", "Before "), fragment(3, "a", "after")]);
    assert!(
        text.contains("[Assistant] Before\n[Assistant] after\n"),
        "{text}"
    );
}

#[test]
fn complete_attachment_at_the_byte_limit_is_accepted() {
    let wrapper = "<chat-history-summary>\nChat history from a previous Paseo agent.\n\n[User] \n</chat-history-summary>";
    let body = "x".repeat(512 * 1024 - wrapper.len());
    let text = attachment(&[row(1, json!({"type":"user_message","text":body}))]);
    assert_eq!(text.len(), 512 * 1024);
    assert!(text.ends_with("\n</chat-history-summary>"));
}

#[test]
fn complete_attachment_one_byte_over_the_limit_is_rejected() {
    let wrapper = "<chat-history-summary>\nChat history from a previous Paseo agent.\n\n[User] \n</chat-history-summary>";
    let body = "x".repeat(512 * 1024 - wrapper.len() + 1);
    let rows = [row(1, json!({"type":"user_message","text":body}))];
    assert_eq!(
        export(&request(), "epoch", &rows, &json!({})),
        Err(ErrorCode::ResourceExhausted)
    );
}

#[test]
fn cursor_boundary_merges_only_fragments_observed_by_that_position() {
    let rows = [
        fragment(1, "a", "part"),
        fragment(2, "a", "ial"),
        fragment(3, "a", " later"),
    ];
    let mut boundary = request();
    boundary.boundary_cursor = Some(Cursor {
        epoch: "epoch".to_owned(),
        seq: 2,
    });
    let result = export(&boundary, "epoch", &rows, &json!({})).unwrap();
    let text = result["attachment"]["text"].as_str().unwrap();
    assert!(text.contains("[Assistant] partial\n"), "{text}");
    assert!(!text.contains("later"));
    assert_eq!(result["itemCount"], 2);
}

#[test]
fn assistant_message_boundary_includes_all_its_fragments_and_excludes_later_messages() {
    let rows = [
        fragment(1, "a", "你好 "),
        fragment(2, "a", "世界"),
        fragment(3, "b", "later"),
    ];
    let mut boundary = request();
    boundary.boundary_message_id = Some("a".to_owned());
    let result = export(&boundary, "epoch", &rows, &json!({})).unwrap();
    let text = result["attachment"]["text"].as_str().unwrap();
    assert!(text.contains("[Assistant] 你好 世界\n"), "{text}");
    assert!(!text.contains("later"));
    assert_eq!(result["itemCount"], 2);
}

#[test]
fn identical_native_item_ids_from_different_turns_do_not_merge() {
    let first = fragment(1, "reused", "First turn");
    let mut second = fragment(2, "reused", "Second turn");
    second.entry.key = "native:second:reused".to_owned();
    second.entry.turn_id = Some("second".to_owned());
    let text = attachment(&[first, second]);
    assert!(
        text.contains("[Assistant] First turn\n[Assistant] Second turn\n"),
        "{text}"
    );
}

#[test]
fn aggregate_fragment_content_is_bounded_before_exporting_a_large_attachment() {
    let rows = [
        fragment(1, "a", &"x".repeat(300_000)),
        fragment(2, "a", &"y".repeat(300_000)),
    ];
    assert_eq!(
        export(&request(), "epoch", &rows, &json!({})),
        Err(ErrorCode::ResourceExhausted)
    );
}

#[test]
fn whitespace_only_message_and_private_reasoning_do_not_create_an_empty_assistant_label() {
    let rows = [
        fragment(1, "blank", " \n"),
        fragment(2, "blank", "\t "),
        row(3, json!({"type":"reasoning","text":"private"})),
    ];
    let text = attachment(&rows);
    assert!(text.contains("No chat history to display.\n"));
    assert!(!text.contains("[Assistant]"));
    assert!(!text.contains("private"));
}
