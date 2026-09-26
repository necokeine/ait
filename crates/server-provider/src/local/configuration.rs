//! Shared, bounded configuration validation before invoking native providers.

mod options;

use std::collections::BTreeMap;

use serde_json::{Map, Value, json};
use server_domain::agent_runtime::StoredAgentConfig;

use crate::ports::agent_session::AgentSessionError;

/// Validate advanced settings without exposing their contents in errors.
pub(super) fn validate(
    config: &StoredAgentConfig,
    provider: &str,
) -> Result<(), AgentSessionError> {
    let encoded = serde_json::to_vec(config).map_err(|_| AgentSessionError::Rejected)?;
    if encoded.len() > 256 * 1024
        || config
            .system_prompt
            .as_ref()
            .is_some_and(|text| !text_value(text))
    {
        return Err(AgentSessionError::Rejected);
    }
    if let Some(options) = &config.provider_options {
        options::validate(options, provider)?;
    }
    if let Some(servers) = &config.mcp_servers {
        if servers.len() > 128 {
            return Err(AgentSessionError::Rejected);
        }
        for (name, server) in servers {
            if !identifier(name) || !mcp_server(server) {
                return Err(AgentSessionError::Rejected);
            }
        }
    }
    grants(config)?;
    Ok(())
}

fn text_value(text: &str) -> bool {
    text.len() <= 65536 && !text.contains('\0')
}

fn identifier(text: &str) -> bool {
    !text.trim().is_empty() && text.len() <= 256 && text.chars().all(|value| !value.is_control())
}

fn string_map(value: &Value) -> bool {
    value.as_object().is_some_and(|values| {
        values.len() <= 256
            && values
                .iter()
                .all(|(key, value)| identifier(key) && value.as_str().is_some_and(text_value))
    })
}

fn strings(value: &Value) -> bool {
    value.as_array().is_some_and(|values| {
        values.len() <= 1024
            && values
                .iter()
                .all(|value| value.as_str().is_some_and(text_value))
    })
}

fn mcp_server(value: &Value) -> bool {
    let Some(fields) = value.as_object() else {
        return false;
    };
    let stdio = value["type"] == "stdio";
    if !stdio && value["type"] != "http" && value["type"] != "sse" {
        return false;
    }
    let required = if stdio { "command" } else { "url" };
    if !value[required].as_str().is_some_and(|text| {
        identifier(text) && (stdio || text.starts_with("https://") || text.starts_with("http://"))
    }) {
        return false;
    }
    fields.iter().all(|(key, value)| match key.as_str() {
        "type" => true,
        "command" if stdio => value.as_str().is_some_and(identifier),
        "url" if !stdio => value.as_str().is_some_and(identifier),
        "args" if stdio => strings(value),
        "env" if stdio => string_map(value),
        "headers" if !stdio => string_map(value),
        "alwaysLoad" => value.is_boolean(),
        _ => false,
    })
}

/// Exact MCP grants, grouped deterministically and deduplicated per server.
pub(super) fn grants(
    config: &StoredAgentConfig,
) -> Result<BTreeMap<&str, Vec<&str>>, AgentSessionError> {
    let mut grants: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    let Some(policy) = config
        .tool_policy
        .as_ref()
        .filter(|policy| !policy.is_null())
    else {
        return Ok(grants);
    };
    let fields = policy.as_object().ok_or(AgentSessionError::Rejected)?;
    let entries = policy["preapproved"]
        .as_array()
        .filter(|entries| entries.len() <= 1024)
        .ok_or(AgentSessionError::Rejected)?;
    if fields.len() != 1 {
        return Err(AgentSessionError::Rejected);
    }
    for entry in entries {
        let fields = entry.as_object().ok_or(AgentSessionError::Rejected)?;
        let exact_name = |key: &str| {
            entry[key].as_str().filter(|name| {
                identifier(name)
                    && !name.contains(['*', '?', '[', ']', ',', '(', ')'])
                    && !name.contains("__")
                    && name.trim() == *name
            })
        };
        let server = exact_name("server").ok_or(AgentSessionError::Rejected)?;
        let tool = exact_name("tool").ok_or(AgentSessionError::Rejected)?;
        if fields.len() != 3 || entry["kind"] != "mcp" {
            return Err(AgentSessionError::Rejected);
        }
        let tools = grants.entry(server).or_default();
        if !tools.contains(&tool) {
            tools.push(tool);
        }
    }
    Ok(grants)
}

/// Map options and transports to Codex's thread configuration overrides.
pub(super) fn codex(config: &StoredAgentConfig) -> Result<Value, AgentSessionError> {
    validate(config, "codex")?;
    let mut result: Map<String, Value> = config
        .provider_options
        .clone()
        .unwrap_or_default()
        .into_iter()
        .collect();
    let mut servers = Map::new();
    if let Some(approval) = result.get_mut("approval_policy") {
        complete_codex_approval(approval);
    }
    for (name, server) in config.mcp_servers.iter().flatten() {
        let mut native = Map::new();
        for (source, target) in [
            ("command", "command"),
            ("args", "args"),
            ("env", "env"),
            ("url", "url"),
            ("headers", "http_headers"),
        ] {
            if let Some(value) = server.get(source) {
                native.insert(target.to_owned(), value.clone());
            }
        }
        servers.insert(name.clone(), Value::Object(native));
    }
    for (server, tools) in grants(config)? {
        let native = servers
            .entry(server.to_owned())
            .or_insert_with(|| json!({}));
        native["enabled_tools"] = json!(tools);
        native["default_tools_approval_mode"] = json!("prompt");
        native["tools"] = Value::Object(
            tools
                .into_iter()
                .map(|tool| (tool.to_owned(), json!({"approval_mode":"approve"})))
                .collect(),
        );
    }
    if !servers.is_empty() {
        result.insert("mcp_servers".to_owned(), Value::Object(servers));
    }
    if let Some(effort) = &config.thinking_option_id {
        result.insert("model_reasoning_effort".to_owned(), json!(effort));
    }
    Ok(Value::Object(result))
}

/// Native JSON-RPC requires all three granular switches, unlike the optional config schema.
pub(super) fn complete_codex_approval(approval: &mut Value) {
    if let Some(granular) = approval.get_mut("granular").and_then(Value::as_object_mut) {
        for field in ["rules", "sandbox_approval", "mcp_elicitations"] {
            granular
                .entry(field.to_owned())
                .or_insert(Value::Bool(false));
        }
    }
}

/// Native Claude CLI argument pairs, without a shell or interpolation.
pub(super) fn claude(config: &StoredAgentConfig) -> Result<Vec<String>, AgentSessionError> {
    validate(config, "claude")?;
    let empty = BTreeMap::new();
    let options = config.provider_options.as_ref().unwrap_or(&empty);
    let mut args = Vec::new();
    let mut allowed = options
        .get("allowedTools")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default();
    for (server, tools) in grants(config)? {
        for tool in tools {
            let name = json!(format!("mcp__{server}__{tool}"));
            if !allowed.contains(&name) {
                allowed.push(name);
            }
        }
    }
    for (flag, values) in [
        ("--allowedTools", Some(&allowed)),
        (
            "--disallowedTools",
            options.get("disallowedTools").and_then(Value::as_array),
        ),
    ] {
        if let Some(values) = values.filter(|values| !values.is_empty()) {
            args.push(format!(
                "{flag}={}",
                values
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(",")
            ));
        }
    }
    for directory in options
        .get("additionalDirectories")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        if let Some(directory) = directory.as_str() {
            args.push(format!("--add-dir={directory}"));
        }
    }
    let mut settings = options
        .get("settings")
        .cloned()
        .unwrap_or_else(|| json!({}));
    if let Some(sandbox) = options.get("sandbox") {
        merge(&mut settings["sandbox"], sandbox);
    }
    if let Some(fast) = config
        .feature_values
        .as_ref()
        .and_then(|values| values.get("fast_mode"))
    {
        settings["fastMode"] = fast.clone();
    }
    if config.thinking_option_id.as_deref() == Some("ultracode") {
        settings["ultracode"] = json!(true);
    }
    if settings
        .as_object()
        .is_some_and(|settings| !settings.is_empty())
    {
        args.push(format!("--settings={settings}"));
    }
    if let Some(servers) = config
        .mcp_servers
        .as_ref()
        .filter(|servers| !servers.is_empty())
    {
        args.push(format!("--mcp-config={}", json!({"mcpServers":servers})));
    }
    Ok(args)
}

fn merge(target: &mut Value, source: &Value) {
    if let (Some(target), Some(source)) = (target.as_object_mut(), source.as_object()) {
        for (key, value) in source {
            merge(target.entry(key.clone()).or_insert(Value::Null), value);
        }
    } else {
        *target = source.clone();
    }
}

#[cfg(test)]
mod tests;
