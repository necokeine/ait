use super::*;

#[test]
fn rich_blocks_keep_chat_history_before_user_text_and_map_native_images() {
    let prompt: AgentPrompt = serde_json::from_value(
        json!({"text":"Inspect","images":[{"data":"aGVsbG8=","mimeType":"image/png"}],
        "attachments":[{"type":"text","mimeType":"text/plain","text":"after"},
            {"type":"text","mimeType":"text/plain","contextKind":"chat_history","text":"prior"}]}),
    )
    .unwrap();
    let blocks = prompt.codex_input().unwrap();
    assert_eq!(blocks[0]["text"], "prior");
    assert_eq!(blocks[1]["text"], "Inspect");
    assert_eq!(blocks[2]["url"], "data:image/png;base64,aGVsbG8=");
    assert_eq!(blocks[3]["text"], "after");
    let blocks = prompt.claude_content().unwrap();
    assert_eq!(
        blocks[2],
        json!({"type":"image","source":{"type":"base64","media_type":"image/png","data":"aGVsbG8="}})
    );
    assert!(!prompt.is_plain_text());
    let text = AgentPrompt::text("hello");
    assert!(text.is_plain_text());
    assert_eq!(text.claude_content().unwrap(), "hello");
}

#[test]
fn attachments_preserve_forge_review_and_uploaded_file_context_without_reading_paths() {
    let prompt: AgentPrompt = serde_json::from_value(json!({"attachments":[
        {"type":"forge_change_request","number":12,"title":"Fix","url":"https://example.com/12","body":"Review this","projectPath":"group/repo","baseRefName":"main","headRefName":"fix"},
        {"type":"uploaded_file","id":"file-1","fileName":"file.txt","path":"/does-not-exist/file.txt","size":3,"mimeType":"text/plain"},
        {"type":"review","mimeType":"application/paseo-review","cwd":"/project","mode":"base","baseRef":"main","comments":[{
            "filePath":"a.rs","side":"new","lineNumber":2,"body":"Check this","context":{"hunkHeader":"@@ -1 +1 @@",
            "targetLine":{"oldLineNumber":null,"newLineNumber":2,"type":"add","content":"new"},
            "lines":[{"oldLineNumber":null,"newLineNumber":2,"type":"add","content":"new"}]}}]}]})).unwrap();
    let blocks = prompt.blocks().unwrap();
    assert!(
        blocks[0]["text"]
            .as_str()
            .unwrap()
            .contains("group/repo\nBase: main\nHead: fix\n\nReview this")
    );
    assert!(
        blocks[1]["text"]
            .as_str()
            .unwrap()
            .contains("/does-not-exist/file.txt")
    );
    assert!(blocks[2]["text"].as_str().unwrap().contains(">  -  2 +new"));
}

#[test]
fn malformed_inputs_are_rejected_atomically_before_admission() {
    for value in [
        json!({}),
        json!({"text":" "}),
        json!({"text":"x","clientMessageId":""}),
        json!({"text":"x","outputSchema":true}),
        json!({"images":[{"data":"!bad","mimeType":"image/png"}]}),
        json!({"images":[{"data":"aA==","mimeType":"image/svg+xml"}]}),
        json!({"attachments":[{"type":"unknown"}]}),
        json!({"attachments":[{"type":"text","mimeType":"text/plain","text":"x".repeat(193*1024)}]}),
    ] {
        let prompt: AgentPrompt = serde_json::from_value(value).unwrap();
        assert_eq!(prompt.validate(), Err(AgentSessionError::Rejected));
    }
    let prompt: AgentPrompt =
        serde_json::from_value(json!({"images":[{"data":"aA==","mimeType":"image/png"}]})).unwrap();
    assert!(prompt.validate().is_ok());
}
