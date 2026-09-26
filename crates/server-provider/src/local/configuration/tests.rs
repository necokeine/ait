use super::*;

fn config(value: Value) -> StoredAgentConfig {
    serde_json::from_value(value).unwrap()
}

#[test]
fn codex_translates_transports_and_limits_preapprovals_to_named_tools() {
    let config = config(json!({"thinkingOptionId":"high", "providerOptions":{
        "sandbox_mode":"workspace-write", "web_search":"live",
        "features":{"network_proxy":{"domains":{"example.com":"allow"}}}},
        "mcpServers":{
            "local":{"type":"stdio","command":"node","args":["server.js"],"env":{"DEMO":"test"}},
            "remote":{"type":"http","url":"https://example.com/mcp","headers":{"X-Test":"fixture"}},
            "events":{"type":"sse","url":"http://localhost:3210/events"}},
        "toolPolicy":{"preapproved":[{"kind":"mcp","server":"local","tool":"read"},
            {"kind":"mcp","server":"local","tool":"read"},
            {"kind":"mcp","server":"inherited","tool":"search"}]}}));
    let native = codex(&config).unwrap();
    assert_eq!(native["mcp_servers"]["local"]["command"], "node");
    assert_eq!(native["mcp_servers"]["local"]["env"]["DEMO"], "test");
    assert_eq!(
        native["mcp_servers"]["local"]["enabled_tools"],
        json!(["read"])
    );
    assert_eq!(
        native["mcp_servers"]["inherited"]["tools"]["search"]["approval_mode"],
        "approve"
    );
    assert_eq!(
        native["mcp_servers"]["inherited"]["default_tools_approval_mode"],
        "prompt"
    );
    assert_eq!(
        native["mcp_servers"]["remote"]["http_headers"]["X-Test"],
        "fixture"
    );
    assert_eq!(
        native["mcp_servers"]["events"]["url"],
        "http://localhost:3210/events"
    );
    assert_eq!(native["model_reasoning_effort"], "high");
    assert_eq!(native["web_search"], "live");
    assert!(native["mcp_servers"]["local"].get("type").is_none());
}

#[test]
fn claude_merges_sandbox_settings_without_losing_permission_rules() {
    let config = config(
        json!({"featureValues":{"fast_mode":true},"providerOptions":{
        "allowedTools":["Read"],"disallowedTools":["Bash"],"additionalDirectories":["/tmp/a b"],
        "settings":{"permissions":{"ask":["Edit"]},"sandbox":{"network":{"allowedDomains":["example.com"]}}},
        "sandbox":{"enabled":true,"network":{"allowLocalBinding":true}}},
        "toolPolicy":{"preapproved":[{"kind":"mcp","server":"docs","tool":"search"}]},
        "mcpServers":{"docs":{"type":"stdio","command":"node","args":["literal$(value)"]}}}),
    );
    let args = claude(&config).unwrap();
    assert!(args.contains(&"--allowedTools=Read,mcp__docs__search".to_owned()));
    assert!(args.contains(&"--disallowedTools=Bash".to_owned()));
    assert!(args.contains(&"--add-dir=/tmp/a b".to_owned()));
    let settings: Value = serde_json::from_str(
        args.iter()
            .find_map(|arg| arg.strip_prefix("--settings="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(settings["permissions"]["ask"], json!(["Edit"]));
    assert_eq!(
        settings["sandbox"]["network"]["allowedDomains"],
        json!(["example.com"])
    );
    assert_eq!(settings["sandbox"]["network"]["allowLocalBinding"], true);
    assert_eq!(settings["fastMode"], true);
    let mcp: Value = serde_json::from_str(
        args.iter()
            .find_map(|arg| arg.strip_prefix("--mcp-config="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        mcp["mcpServers"]["docs"]["args"],
        json!(["literal$(value)"])
    );
}

#[test]
fn rejects_unknown_invalid_or_overbroad_advanced_configuration() {
    for (provider, value) in [
        ("codex", json!({"providerOptions":{"unknown":true}})),
        ("codex", json!({"providerOptions":{"approval_policy":{}}})),
        (
            "codex",
            json!({"providerOptions":{"features":{"network_proxy":{"domains":{"a":"maybe"}}}}}),
        ),
        (
            "claude",
            json!({"providerOptions":{"sandbox":{"network":{"httpProxyPort":65536}}}}),
        ),
        (
            "claude",
            json!({"providerOptions":{"sandbox":{"ripgrep":{}}}}),
        ),
        (
            "claude",
            json!({"providerOptions":{"settings":{"permissions":{"trust":true}}}}),
        ),
        (
            "claude",
            json!({"mcpServers":{"a":{"type":"stdio","command":"node","url":"https://a"}}}),
        ),
        (
            "codex",
            json!({"mcpServers":{"a":{"type":"http","url":"file:///etc/passwd"}}}),
        ),
        (
            "claude",
            json!({"toolPolicy":{"preapproved":[{"kind":"mcp","server":"a","tool":"*"}]}}),
        ),
        (
            "claude",
            json!({"toolPolicy":{"preapproved":[{"kind":"mcp","server":"a__b","tool":"c"}]}}),
        ),
        (
            "codex",
            json!({"toolPolicy":{"preapproved":[],"allowAll":true}}),
        ),
        ("codex", json!({"systemPrompt":"bad\u{0}prompt"})),
    ] {
        assert_eq!(
            validate(&config(value.clone()), provider),
            Err(AgentSessionError::Rejected),
            "{provider}: {value}"
        );
    }
    let oversized = config(json!({"providerOptions":{"allowedTools":["a".repeat(256*1024)]}}));
    assert_eq!(
        validate(&oversized, "claude"),
        Err(AgentSessionError::Rejected)
    );
}

#[test]
fn empty_and_inherited_configuration_does_not_add_cli_arguments() {
    assert!(claude(&StoredAgentConfig::default()).unwrap().is_empty());
    assert_eq!(codex(&StoredAgentConfig::default()).unwrap(), json!({}));
    assert!(
        grants(&config(json!({"toolPolicy":null})))
            .unwrap()
            .is_empty()
    );
}
