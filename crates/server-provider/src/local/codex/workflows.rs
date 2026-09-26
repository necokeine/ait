use std::sync::atomic::Ordering;

use serde_json::{Value, json};
use server_domain::agent_runtime::StoredAgentConfig;

use super::{CodexClient, Transport, controls};
use crate::ports::agent_session::AgentSessionError;

const PLAN: u8 = 1;
const REVIEW: u8 = 2;
const GOALS: u8 = 4;

impl CodexClient {
    pub(super) fn goals(&self) -> bool {
        self.capabilities.load(Ordering::Relaxed) & GOALS != 0
    }
    pub(super) async fn inspect_workflows(
        &self,
        transport: &mut Transport,
    ) -> Result<(), AgentSessionError> {
        let mut capabilities = 0;
        if let Some(version) = transport.user_agent.as_deref().and_then(version) {
            if version >= (0, 115, 0) {
                capabilities |= REVIEW;
            }
            if version >= (0, 128, 0) {
                capabilities |= GOALS;
            }
        }
        match transport.request("collaborationMode/list", json!({})).await {
            Ok(response) => {
                if response["data"]
                    .as_array()
                    .is_some_and(|modes| modes.iter().any(|mode| mode["mode"] == "plan"))
                {
                    capabilities |= PLAN;
                }
            }
            Err(AgentSessionError::Rejected) => {}
            Err(error) => return Err(error),
        }
        self.capabilities.store(capabilities, Ordering::Relaxed);
        Ok(())
    }

    pub(super) fn validate_workflows(
        &self,
        config: &StoredAgentConfig,
    ) -> Result<(), AgentSessionError> {
        let capabilities = self.capabilities.load(Ordering::Relaxed);
        if (config.mode_id.as_deref() == Some("auto-review") && capabilities & REVIEW == 0)
            || (config
                .feature_values
                .as_ref()
                .is_some_and(|values| values.contains_key("plan_mode"))
                && capabilities & PLAN == 0)
        {
            return Err(AgentSessionError::Rejected);
        }
        Ok(())
    }

    pub(super) fn modes(&self) -> Vec<Value> {
        let mut modes = controls::modes();
        if self.capabilities.load(Ordering::Relaxed) & REVIEW != 0 {
            modes.push(json!({"id":"auto-review","label":"Auto review","description":"Native Codex reviews approval requests within the workspace sandbox"}));
        }
        modes
    }

    pub(super) fn features(&self, config: &StoredAgentConfig) -> Vec<Value> {
        let mut features = controls::features(config);
        if self.capabilities.load(Ordering::Relaxed) & PLAN != 0 {
            features.push(json!({"id":"plan_mode","type":"toggle","label":"Plan mode",
                "description":"Use the native planning workflow","value":plan(config)}));
        }
        features
    }
}

pub(super) fn reviewer(config: &StoredAgentConfig) -> &'static str {
    if config.mode_id.as_deref() == Some("auto-review") {
        "auto_review"
    } else {
        "user"
    }
}

fn plan(config: &StoredAgentConfig) -> bool {
    config
        .feature_values
        .as_ref()
        .and_then(|values| values.get("plan_mode"))
        == Some(&Value::Bool(true))
}

pub(super) async fn collaboration(
    transport: &mut Transport,
    config: &StoredAgentConfig,
    fallback_model: Option<&str>,
    previously_configured: bool,
) -> Result<Option<Value>, AgentSessionError> {
    if !previously_configured
        && !config
            .feature_values
            .as_ref()
            .is_some_and(|values| values.contains_key("plan_mode"))
    {
        return Ok(None);
    }
    let selected = if plan(config) { "plan" } else { "default" };
    let response = transport
        .request("collaborationMode/list", json!({}))
        .await?;
    let preset = response["data"]
        .as_array()
        .and_then(|modes| modes.iter().find(|mode| mode["mode"] == selected))
        .ok_or(AgentSessionError::Rejected)?;
    let model = config
        .model
        .as_deref()
        .or_else(|| preset["model"].as_str())
        .or(fallback_model)
        .filter(|model| !model.is_empty())
        .ok_or(AgentSessionError::Rejected)?;
    Ok(Some(json!({"mode":selected,"settings":{"model":model,
        "reasoning_effort":config.thinking_option_id.as_deref().or_else(||preset["reasoning_effort"].as_str()),
        "developer_instructions":preset["developer_instructions"]}})))
}

fn version(agent: &str) -> Option<(u64, u64, u64)> {
    agent
        .split(|character: char| !character.is_ascii_digit() && character != '.')
        .find_map(|part| {
            let mut segments = part.split('.');
            let version = (
                segments.next()?.parse().ok()?,
                segments.next()?.parse().ok()?,
                segments.next()?.parse().ok()?,
            );
            segments.next().is_none().then_some(version)
        })
}

#[cfg(test)]
mod tests;
