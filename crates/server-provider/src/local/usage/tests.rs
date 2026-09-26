use super::*;
use serde_json::json;

#[test]
fn codex_uses_last_request_not_session_totals_and_does_not_invent_cost() {
    let usage = codex(&json!({"last":{"inputTokens":100,"cachedInputTokens":30,
        "outputTokens":12,"totalTokens":112},"total":{"totalTokens":99999},"modelContextWindow":200_000})).unwrap();
    assert_eq!(usage.context_window_used_tokens, Some(112));
    assert_eq!(usage.input_tokens, Some(100));
    assert_eq!(usage.cached_input_tokens, Some(30));
    assert_eq!(usage.context_window_max_tokens, Some(200_000));
    assert_eq!(usage.total_cost_usd, None);
    assert_eq!(
        codex(&json!({"last":{"total_tokens":0},"model_context_window":123}))
            .unwrap()
            .context_window_used_tokens,
        Some(0)
    );
    assert!(codex(&json!({"last":{"totalTokens":u64::MAX}})).is_none());
    assert!(codex(&Value::Null).is_none());
}

#[test]
fn claude_tracks_current_request_cache_compaction_and_result_cost_without_double_counting() {
    let mut state = ClaudeUsage::default();
    let start = json!({"type":"stream_event","event":{"type":"message_start","message":{"usage":{
        "input_tokens":10,"cache_read_input_tokens":30,"cache_creation_input_tokens":20}}}});
    assert_eq!(
        state.observe(&start).unwrap().context_window_used_tokens,
        Some(60)
    );
    assert!(state.observe(&start).is_none());
    let delta =
        json!({"type":"stream_event","event":{"type":"message_delta","usage":{"output_tokens":7}}});
    assert_eq!(
        state.observe(&delta).unwrap().context_window_used_tokens,
        Some(67)
    );
    let result = json!({"type":"result","usage":{"input_tokens":4000,"output_tokens":120,"cache_read_input_tokens":5000},
        "total_cost_usd":0.5,"modelUsage":{"a":{"contextWindow":200_000},"b":{"contextWindow":1_000_000}}});
    let usage = state.observe(&result).unwrap();
    assert_eq!(usage.input_tokens, Some(4000));
    assert_eq!(usage.context_window_used_tokens, Some(67));
    assert_eq!(usage.context_window_max_tokens, Some(1_000_000));
    assert_eq!(usage.total_cost_usd, Some(0.5));
    assert!(state.observe(&result).is_none());
    state.begin();
    let usage = state.observe(&json!({"type":"system","subtype":"compact_boundary","compact_metadata":{"post_tokens":30}})).unwrap();
    assert_eq!(usage.context_window_used_tokens, Some(30));
    assert!(state.observe(&result).is_none());
    let mut child = start.clone();
    child["parent_tool_use_id"] = json!("child");
    assert!(state.observe(&child).is_none());
    let usage = state
        .observe(&json!({"type":"system","subtype":"compact_boundary"}))
        .unwrap();
    assert_eq!(usage.context_window_used_tokens, None);
}

#[test]
fn claude_prefers_last_iteration_and_rejects_overflow_without_corrupting_previous_usage() {
    let mut state = ClaudeUsage::default();
    let usage = state.observe(&json!({"type":"result","usage":{"input_tokens":8000,"output_tokens":50,
        "iterations":[{"input_tokens":100,"output_tokens":3},{"input_tokens":20,"output_tokens":4}]} })).unwrap();
    assert_eq!(usage.context_window_used_tokens, Some(24));
    assert!(
        state
            .observe(&json!({"type":"result","usage":{"input_tokens":u64::MAX}}))
            .is_none()
    );
    let usage = state
        .observe(
            &json!({"type":"assistant","message":{"usage":{"input_tokens":7,"output_tokens":1}}}),
        )
        .unwrap();
    assert_eq!(usage.context_window_used_tokens, Some(8));
    assert_eq!(usage.input_tokens, Some(8000));
}

#[test]
fn resumed_usage_does_not_treat_cumulative_result_counts_as_current_context() {
    let mut usage = ClaudeUsage::restore(Some(
        &json!({"contextWindowUsedTokens":42,"contextWindowMaxTokens":200_000,"totalCostUsd":1.2}),
    ))
    .unwrap();
    usage.begin();
    let result = usage
        .observe(&json!({"type":"result","usage":{"input_tokens":10000,"output_tokens":500}}))
        .unwrap();
    assert_eq!(result.context_window_used_tokens, Some(42));
    usage.observe(
        &json!({"type":"assistant","message":{"usage":{"input_tokens":11,"output_tokens":3}}}),
    );
    assert_eq!(
        usage
            .observe(&json!({"type":"result","usage":{"input_tokens":20000,"output_tokens":800}}))
            .unwrap()
            .context_window_used_tokens,
        Some(14)
    );
    assert!(ClaudeUsage::restore(Some(&json!({"inputTokens":u64::MAX}))).is_err());
}
