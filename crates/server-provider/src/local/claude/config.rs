use std::path::Path;

use serde_json::{Value, json};
use server_domain::agent_runtime::StoredAgentConfig;

use crate::ports::agent_session::{AgentSessionError, AgentSessionSpec};

pub(super) fn validate_spec(spec: &AgentSessionSpec) -> Result<(), AgentSessionError> {
    if spec.provider != "claude"
        || !Path::new(&spec.cwd).is_absolute()
        || !Path::new(&spec.cwd).is_dir()
    {
        return Err(AgentSessionError::Unavailable);
    }
    validate_shape(&spec.config)
}

pub(super) fn validate(config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
    validate_shape(config)?;
    let (fast, disabled) = model_capabilities(config.model.as_deref());
    if (config
        .feature_values
        .as_ref()
        .and_then(|values| values.get("fast_mode"))
        == Some(&Value::Bool(true))
        && !fast)
        || (config.thinking_option_id.as_deref() == Some("off") && !disabled)
    {
        return Err(AgentSessionError::Rejected);
    }
    Ok(())
}

pub(super) fn validate_shape(config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
    if config.model.as_ref().is_some_and(|model| {
        model.trim().is_empty() || model.len() > 256 || model.chars().any(char::is_control)
    }) || !matches!(
        config.mode_id.as_deref(),
        None | Some("default" | "plan" | "acceptEdits" | "auto" | "bypassPermissions")
    ) || !matches!(
        config.thinking_option_id.as_deref(),
        None | Some("off" | "low" | "medium" | "high" | "xhigh" | "max" | "ultracode")
    ) || config.feature_values.as_ref().is_some_and(|values| {
        values
            .iter()
            .any(|(key, value)| key != "fast_mode" || !value.is_boolean())
    }) || config
        .system_prompt
        .as_ref()
        .is_some_and(|prompt| prompt.len() > 65536 || prompt.contains('\0'))
    {
        return Err(AgentSessionError::Rejected);
    }
    crate::local::configuration::validate(config, "claude")
}

pub(super) fn modes() -> Vec<Value> {
    [
        ("default", "Always Ask", "Shield", "safe"),
        ("plan", "Plan Mode", "ShieldEllipsis", "planning"),
        ("acceptEdits", "Accept File Edits", "ShieldPlus", "moderate"),
        ("auto", "Auto mode", "ShieldCheck", "moderate"),
        ("bypassPermissions", "Bypass", "ShieldOff", "dangerous"),
    ]
    .into_iter()
    .map(|(id, label, icon, tier)| {
        json!({"id":id,"label":label,"icon":icon,
        "colorTier":tier,"isDefault":id=="default","isUnattended":id=="bypassPermissions"})
    })
    .collect()
}

pub(super) fn features(config: &StoredAgentConfig) -> Vec<Value> {
    if !model_capabilities(config.model.as_deref()).0 {
        return Vec::new();
    }
    vec![json!({"id":"fast_mode","type":"toggle","label":"Fast mode",
        "description":"Use Claude Code fast mode on a supported model",
        "value":config.feature_values.as_ref().and_then(|values|values.get("fast_mode")) == Some(&Value::Bool(true))})]
}

// Capabilities follow the pinned Paseo manifest. Custom gateway aliases do not inherit
// first-party billing or thinking guarantees merely by containing a model name.
fn model_capabilities(model: Option<&str>) -> (bool, bool) {
    let normalized = model
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
        .replace(['_', ' ', '.'], "-");
    let normalized = normalized
        .strip_prefix("claude-")
        .unwrap_or(&normalized)
        .replace("[1m]", "");
    let normalized = normalized
        .rsplit_once('-')
        .filter(|(_, suffix)| {
            suffix.len() == 8 && suffix.bytes().all(|value| value.is_ascii_digit())
        })
        .map_or(normalized.as_str(), |(base, _)| base);
    let fast = matches!(normalized, "opus-5" | "opus-4-8" | "opus-4-7" | "opus-4-6");
    (
        fast,
        fast || matches!(normalized, "sonnet-5" | "sonnet-4-6"),
    )
}

pub(super) fn models(response: &Value) -> Result<Vec<Value>, AgentSessionError> {
    let models = response["models"]
        .as_array()
        .filter(|models| !models.is_empty() && models.len() <= 4096)
        .ok_or(AgentSessionError::Failed)?;
    models
        .iter()
        .map(|model| {
            let id = text(model, "value")?;
            let label = text(model, "displayName")?;
            let efforts = model.get("supportedEffortLevels").and_then(Value::as_array);
            let mut options = efforts
                .into_iter()
                .flatten()
                .map(|effort| {
                    let id = effort.as_str().ok_or(AgentSessionError::Failed)?;
                    if !matches!(id, "low" | "medium" | "high" | "xhigh" | "max") {
                        return Err(AgentSessionError::Failed);
                    }
                    Ok(json!({"id":id,"label":id}))
                })
                .collect::<Result<Vec<_>, AgentSessionError>>()?;
            let (fast, disabled) = model_capabilities(model["resolvedModel"].as_str().or(Some(id)));
            if disabled {
                options.insert(0, json!({"id":"off","label":"Off"}));
            }
            if options.iter().any(|option| option["id"] == "xhigh") {
                options.push(json!({"id":"ultracode","label":"Ultra Code"}));
            }
            Ok(json!({"provider":"claude","id":id,"label":label,
            "description":model["description"].as_str().unwrap_or(""),"isSelectable":true,
            "isDefault":id=="default","thinkingOptions":options,"supportsFastMode":fast}))
        })
        .collect()
}

pub(super) fn commands(response: &Value) -> Result<Vec<Value>, AgentSessionError> {
    let mut commands = response["commands"].as_array().filter(|values| values.len() <= 4096)
        .ok_or(AgentSessionError::Failed)?.iter().map(|command| {
            Ok(json!({"name":text(command,"name")?,"description":command["description"].as_str().unwrap_or(""),
                "argumentHint":command["argumentHint"].as_str().unwrap_or("")}))
        }).collect::<Result<Vec<_>, AgentSessionError>>()?;
    if !commands.iter().any(|command| command["name"] == "rewind") {
        commands.push(json!({"name":"rewind","description":"Restore files to a previous user checkpoint","argumentHint":"[message-id]"}));
    }
    Ok(commands)
}

pub(super) fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, AgentSessionError> {
    value[key]
        .as_str()
        .filter(|text| !text.is_empty() && text.len() <= 1024)
        .ok_or(AgentSessionError::Failed)
}

pub(super) fn executable(program: &Path) -> bool {
    let runnable = |path: &Path| {
        let Ok(metadata) = path.metadata() else {
            return false;
        };
        if !metadata.is_file() {
            return false;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            metadata.permissions().mode() & 0o111 != 0
        }
        #[cfg(not(unix))]
        {
            true
        }
    };
    if program.components().count() > 1 {
        return runnable(program);
    }
    std::env::var_os("PATH").is_some_and(|paths| {
        std::env::split_paths(&paths).any(|path| {
            let path = path.join(program);
            #[cfg(windows)]
            let path = path.with_extension("exe");
            runnable(&path)
        })
    })
}
