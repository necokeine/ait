//! Project native legacy and ID-based task tools into immutable todo snapshots.

use crate::ports::agent_session::AgentSessionError;
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};

#[derive(Debug, Default)]
pub(super) struct Tasks {
    items: BTreeMap<String, Value>,
    calls: BTreeMap<String, (String, Value)>,
    applied: BTreeSet<String>,
}

impl Tasks {
    pub(super) fn observe(
        &mut self,
        record: &Value,
    ) -> Result<Vec<(String, Value)>, AgentSessionError> {
        let mut snapshots = Vec::new();
        for block in record["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if block["type"] == "tool_use" {
                let Some(name) = block["name"].as_str().filter(|name| {
                    matches!(
                        *name,
                        "TodoWrite" | "TaskCreate" | "TaskUpdate" | "TaskList"
                    )
                }) else {
                    continue;
                };
                let id = text(block, "id").ok_or(AgentSessionError::Failed)?;
                if self.applied.contains(id) || self.calls.contains_key(id) {
                    continue;
                }
                if self.calls.len() >= 1024 {
                    return Err(AgentSessionError::Failed);
                }
                self.calls
                    .insert(id.to_owned(), (name.to_owned(), block["input"].clone()));
                if name == "TodoWrite" {
                    self.replace(&block["input"]["todos"], true)?;
                    snapshots.push((format!("todo:{id}"), self.snapshot()));
                }
            } else if block["type"] == "tool_result" {
                let Some(id) = text(block, "tool_use_id") else {
                    continue;
                };
                let Some((name, input)) = self.calls.remove(id) else {
                    continue;
                };
                if self.applied.len() >= 4096 {
                    return Err(AgentSessionError::Failed);
                }
                if !self.applied.insert(id.to_owned()) || block["is_error"] == true {
                    continue;
                }
                let result = record
                    .get("toolUseResult")
                    .or_else(|| record.get("tool_use_result"))
                    .unwrap_or(&Value::Null);
                if result["success"] == false {
                    continue;
                }
                if self.apply(&name, &input, result)? {
                    snapshots.push((format!("todo:{id}"), self.snapshot()));
                }
            }
        }
        Ok(snapshots)
    }

    fn snapshot(&self) -> Value {
        json!({"type":"todo","items":self.items.values().collect::<Vec<_>>()})
    }

    fn replace(&mut self, values: &Value, legacy: bool) -> Result<(), AgentSessionError> {
        let values = values.as_array().map_or(&[][..], Vec::as_slice);
        if values.len() > 1024 {
            return Err(AgentSessionError::Failed);
        }
        self.items.clear();
        for (index, value) in values.iter().enumerate() {
            if let Some(mut item) = item(value) {
                let id = text(&item, "id")
                    .map(str::to_owned)
                    .or_else(|| legacy.then(|| format!("legacy:{index}")));
                if let Some(id) = id {
                    item["id"] = json!(id);
                    self.items.insert(id, item);
                }
            }
        }
        Ok(())
    }

    fn apply(
        &mut self,
        name: &str,
        input: &Value,
        result: &Value,
    ) -> Result<bool, AgentSessionError> {
        match name {
            "TaskCreate" => {
                let Some(id) = text(&result["task"], "id").or_else(|| text(result, "taskId"))
                else {
                    return Ok(false);
                };
                let Some(subject) =
                    text(&result["task"], "subject").or_else(|| text(input, "subject"))
                else {
                    return Ok(false);
                };
                if self.items.len() >= 1024 {
                    return Err(AgentSessionError::Failed);
                }
                let mut value =
                    json!({"id":id,"text":subject,"status":"pending","completed":false});
                if let Some(active) = text(input, "activeForm") {
                    value["activeForm"] = json!(active);
                }
                self.items.insert(id.to_owned(), value);
            }
            "TaskUpdate" => {
                let Some(id) = text(input, "taskId").or_else(|| text(result, "taskId")) else {
                    return Ok(false);
                };
                let Some(current) = self.items.get_mut(id) else {
                    return Ok(false);
                };
                let status = input["status"]
                    .as_str()
                    .or_else(|| result["statusChange"]["to"].as_str());
                if status == Some("deleted") {
                    self.items.remove(id);
                    return Ok(true);
                }
                if let Some(status) = status {
                    let status = normalize(status);
                    current["status"] = json!(status);
                    current["completed"] = json!(status == "completed");
                }
                if let Some(subject) = text(input, "subject") {
                    current["text"] = json!(subject);
                }
                if let Some(active) = text(input, "activeForm") {
                    current["activeForm"] = json!(active);
                }
            }
            "TaskList" if result["tasks"].is_array() => self.replace(&result["tasks"], false)?,
            _ => return Ok(false),
        }
        Ok(true)
    }
}

fn item(value: &Value) -> Option<Value> {
    let subject = text(value, "subject")
        .or_else(|| text(value, "content"))
        .or_else(|| text(value, "text"))?;
    if value["status"] == "deleted" {
        return None;
    }
    let status = normalize(value["status"].as_str().unwrap_or_default());
    let mut item = json!({"text":subject,"status":status,"completed":status=="completed"});
    if let Some(id) = text(value, "id").or_else(|| text(value, "taskId")) {
        item["id"] = json!(id);
    }
    if let Some(active) = text(value, "activeForm").or_else(|| text(value, "active_form")) {
        item["activeForm"] = json!(active);
    }
    Some(item)
}

fn normalize(status: &str) -> &'static str {
    match status {
        "completed" => "completed",
        "in_progress" => "in_progress",
        _ => "pending",
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Option<&'a str> {
    value[key]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty() && text.len() <= 8192)
}

#[cfg(test)]
mod tests;
