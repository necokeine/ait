use serde_json::Value;

use crate::protocol::usage::AgentUsage;

pub(super) fn codex(value: &Value) -> Option<AgentUsage> {
    let last = value.get("last")?.as_object()?;
    let usage = AgentUsage {
        input_tokens: last
            .get("inputTokens")
            .or_else(|| last.get("input_tokens"))
            .and_then(Value::as_u64),
        cached_input_tokens: last
            .get("cachedInputTokens")
            .or_else(|| last.get("cached_input_tokens"))
            .and_then(Value::as_u64),
        output_tokens: last
            .get("outputTokens")
            .or_else(|| last.get("output_tokens"))
            .and_then(Value::as_u64),
        context_window_max_tokens: value
            .get("modelContextWindow")
            .or_else(|| value.get("model_context_window"))
            .and_then(Value::as_u64),
        context_window_used_tokens: last
            .get("totalTokens")
            .or_else(|| last.get("total_tokens"))
            .and_then(Value::as_u64),
        total_cost_usd: None,
    };
    usage.is_valid().then_some(usage)
}

#[derive(Debug, Default)]
pub(super) struct ClaudeUsage {
    snapshot: AgentUsage,
    request_input: Option<u64>,
    request_output: Option<u64>,
    completed_results: u64,
}

impl ClaudeUsage {
    pub(super) fn restore(
        value: Option<&Value>,
    ) -> Result<Self, crate::ports::agent_session::AgentSessionError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let snapshot: AgentUsage = serde_json::from_value(value.clone())
            .map_err(|_| crate::ports::agent_session::AgentSessionError::Failed)?;
        if !snapshot.is_valid() {
            return Err(crate::ports::agent_session::AgentSessionError::Failed);
        }
        Ok(Self {
            snapshot,
            completed_results: 1,
            ..Self::default()
        })
    }

    pub(super) fn saved(&self) -> Option<Value> {
        (self.snapshot != AgentUsage::default()).then(|| serde_json::json!(self.snapshot))
    }

    pub(super) fn begin(&mut self) {
        self.request_input = None;
        self.request_output = None;
    }

    pub(super) fn observe(&mut self, record: &Value) -> Option<AgentUsage> {
        if record
            .get("parent_tool_use_id")
            .is_some_and(|id| !id.is_null())
            || record["isSidechain"] == true
        {
            return None;
        }
        let previous = self.snapshot.clone();
        match record["type"].as_str() {
            Some("stream_event") => {
                let event = &record["event"];
                match event["type"].as_str() {
                    Some("message_start") => {
                        let usage = &event["message"]["usage"];
                        self.request_input = input_total(usage);
                        self.request_output = Some(0);
                    }
                    Some("message_delta") => {
                        self.request_output = event["usage"]["output_tokens"].as_u64();
                    }
                    _ => return None,
                }
                self.snapshot.context_window_used_tokens = self.request_total();
            }
            Some("assistant") => {
                if let Some(total) = total(&record["message"]["usage"]) {
                    self.request_input = input_total(&record["message"]["usage"]);
                    self.request_output = record["message"]["usage"]["output_tokens"].as_u64();
                    self.snapshot.context_window_used_tokens = Some(total);
                }
            }
            Some("system") if record["subtype"] == "compact_boundary" => {
                self.begin();
                self.snapshot.context_window_used_tokens =
                    record["compact_metadata"]["post_tokens"].as_u64();
            }
            Some("result") => {
                let usage = &record["usage"];
                self.snapshot.input_tokens = usage["input_tokens"].as_u64();
                self.snapshot.cached_input_tokens = usage["cache_read_input_tokens"].as_u64();
                self.snapshot.output_tokens = usage["output_tokens"].as_u64();
                if let Some(cost) = record["total_cost_usd"]
                    .as_f64()
                    .filter(|cost| cost.is_finite() && *cost >= 0.0)
                {
                    self.snapshot.total_cost_usd = Some(cost);
                }
                if let Some(maximum) = record["modelUsage"]
                    .as_object()
                    .into_iter()
                    .flat_map(|models| models.values())
                    .filter_map(|model| model["contextWindow"].as_u64())
                    .filter(|size| *size > 0)
                    .max()
                {
                    self.snapshot.context_window_max_tokens = Some(maximum);
                }
                let iteration = usage["iterations"]
                    .as_array()
                    .and_then(|items| items.iter().rev().find_map(total));
                if let Some(used) = self.request_total().or(iteration).or_else(|| {
                    (self.completed_results == 0)
                        .then(|| total(usage))
                        .flatten()
                }) {
                    self.snapshot.context_window_used_tokens = Some(used);
                }
                self.completed_results = self.completed_results.saturating_add(1);
            }
            _ => return None,
        }
        if !self.snapshot.is_valid() {
            self.snapshot = previous;
            return None;
        }
        (self.snapshot != previous).then(|| self.snapshot.clone())
    }

    fn request_total(&self) -> Option<u64> {
        self.request_input?.checked_add(self.request_output?)
    }
}

fn input_total(value: &Value) -> Option<u64> {
    value["input_tokens"]
        .as_u64()?
        .checked_add(
            value["cache_creation_input_tokens"]
                .as_u64()
                .unwrap_or_default(),
        )?
        .checked_add(
            value["cache_read_input_tokens"]
                .as_u64()
                .unwrap_or_default(),
        )
}

fn total(value: &Value) -> Option<u64> {
    input_total(value)?.checked_add(value["output_tokens"].as_u64().unwrap_or_default())
}

#[cfg(test)]
mod tests;
