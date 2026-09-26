use super::*;
use serde_json::json;

#[test]
fn patch_distinguishes_omission_from_null_and_preserves_unrelated_configuration() {
    let current = StoredAgentConfig {
        model: Some("previous".to_owned()),
        thinking_option_id: Some("high".to_owned()),
        system_prompt: Some("instructions".to_owned()),
        ..Default::default()
    };
    let patch: ConfigPatch = serde_json::from_value(json!({"modelId":null})).unwrap();
    let next = patch.apply(&current);
    assert_eq!(next.model, None);
    assert_eq!(next.thinking_option_id, current.thinking_option_id);
    assert_eq!(next.system_prompt, current.system_prompt);
    assert_eq!(ConfigPatch::default().apply(&current), current);
    let patch: ConfigPatch =
        serde_json::from_value(json!({"modelId":"next","thinkingOptionId":null})).unwrap();
    assert_eq!(patch.apply(&current).model.as_deref(), Some("next"));
    assert_eq!(patch.apply(&current).thinking_option_id, None);
    assert!(serde_json::from_value::<ConfigPatch>(json!({"unknown":true})).is_err());
}

#[test]
fn advanced_patch_replaces_or_clears_only_present_configuration() {
    let current: StoredAgentConfig = serde_json::from_value(json!({
        "mcpServers":{"old":{"type":"stdio","command":"old"}},
        "providerOptions":{"allowedTools":["Read"]},"toolPolicy":{"preapproved":[]},
        "systemPrompt":"old"}))
    .unwrap();
    let patch: ConfigPatch = serde_json::from_value(json!({"mcpServers":null,
        "providerOptions":{"disallowedTools":["Bash"]},"systemPrompt":null}))
    .unwrap();
    let next = patch.apply(&current);
    assert_eq!(next.mcp_servers, None);
    assert_eq!(next.system_prompt, None);
    assert_eq!(next.tool_policy, current.tool_policy);
    assert_eq!(
        serde_json::to_value(next.provider_options).unwrap(),
        json!({"disallowedTools":["Bash"]})
    );
    assert_eq!(ConfigPatch::default().apply(&current), current);
}
