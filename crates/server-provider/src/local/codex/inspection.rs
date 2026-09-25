use chrono::{DateTime, Utc};
use serde_json::{Value, json};

use super::CodexClient;
use crate::ports::agent_session::{AgentClient, AgentSessionError};

impl CodexClient {
    pub(super) async fn native_diagnostic(&self) -> Result<String, AgentSessionError> {
        if !self.is_available().await? {
            return Ok("Codex executable: unavailable".to_owned());
        }
        let cwd = current_dir()?;
        let Ok(account) = self
            .query(&cwd, "account/read", json!({"refreshToken":false}))
            .await
        else {
            return Ok(
                "Codex executable: found\nNative account inspection: unavailable".to_owned(),
            );
        };
        let authentication = match account.pointer("/account/type").and_then(Value::as_str) {
            Some("chatgpt") => "ChatGPT login",
            Some("apiKey") => "API key",
            Some("amazonBedrock") => "Amazon Bedrock",
            _ => "not authenticated",
        };
        Ok(format!(
            "Codex executable: found\nApp-server protocol: available\nAuthentication: {authentication}"
        ))
    }

    pub(super) async fn native_usage(&self) -> Result<Value, AgentSessionError> {
        let cwd = current_dir()?;
        let response = self
            .query(&cwd, "account/rateLimits/read", json!({}))
            .await?;
        usage(&response)
    }
}

fn current_dir() -> Result<String, AgentSessionError> {
    std::env::current_dir()
        .map_err(|_| AgentSessionError::Failed)?
        .into_os_string()
        .into_string()
        .map_err(|_| AgentSessionError::Failed)
}

fn usage(response: &Value) -> Result<Value, AgentSessionError> {
    let mut windows = Vec::new();
    let mut plan = None;
    let snapshots: Vec<_> = if let Some(buckets) = response["rateLimitsByLimitId"]
        .as_object()
        .filter(|buckets| !buckets.is_empty())
    {
        buckets
            .iter()
            .map(|(id, snapshot)| (id.as_str(), snapshot))
            .collect()
    } else {
        vec![("codex", &response["rateLimits"])]
    };
    for (bucket, snapshot) in snapshots {
        if !snapshot.is_object() {
            return Err(AgentSessionError::Failed);
        }
        if plan.is_none() {
            plan = snapshot["planType"].as_str();
        }
        for (id, label) in [("primary", "Primary"), ("secondary", "Secondary")] {
            let window = &snapshot[id];
            if window.is_null() {
                continue;
            }
            let used = window["usedPercent"]
                .as_i64()
                .filter(|percent| *percent >= 0)
                .ok_or(AgentSessionError::Failed)?;
            let reset = window["resetsAt"]
                .as_i64()
                .map(|time| {
                    DateTime::from_timestamp(time, 0)
                        .ok_or(AgentSessionError::Failed)
                        .map(|time| time.to_rfc3339())
                })
                .transpose()?;
            windows.push(json!({"id":format!("{bucket}:{id}"),"label":format!("{bucket} {label}"),"usedPct":used,"remainingPct":(100-used).max(0),
                "resetsAt":reset,"tone":if used>=95 {"danger"} else if used>=80 {"warning"} else {"ok"}}));
        }
    }
    Ok(
        json!({"providerId":"codex","displayName":"Codex","status":if windows.is_empty(){"unavailable"}else{"available"},"planLabel":plan,
        "sourceLabel":"Codex account/rateLimits/read","fetchedAt":Utc::now().to_rfc3339(),"nextRefreshAt":null,"windows":windows,"balances":[],"details":[],"error":null}),
    )
}

#[cfg(test)]
mod tests;
