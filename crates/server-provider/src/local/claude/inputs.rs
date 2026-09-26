//! Host correlation IDs are independent of the UUID required by the Claude SDK.

use crate::ports::agent_session::AgentSessionError;
use crate::protocol::timeline::NativeItem;
use serde_json::Value;
use std::collections::BTreeMap;
use uuid::Uuid;

#[derive(Debug, Default)]
pub(super) struct Inputs(BTreeMap<String, String>);

impl Inputs {
    pub(super) fn restore(saved: Option<&Value>) -> Result<Self, AgentSessionError> {
        let Some(saved) = saved else {
            return Ok(Self::default());
        };
        let entries: BTreeMap<String, String> =
            serde_json::from_value(saved.clone()).map_err(|_| AgentSessionError::Failed)?;
        let inputs = Self(entries);
        inputs.validate()?;
        Ok(inputs)
    }

    fn validate(&self) -> Result<(), AgentSessionError> {
        if self.0.len() > 512
            || self.0.iter().any(|(native, host)| {
                Uuid::parse_str(native).is_err()
                    || host.is_empty()
                    || host.len() > 128
                    || host.chars().any(char::is_control)
            })
        {
            return Err(AgentSessionError::Rejected);
        }
        Ok(())
    }

    pub(super) fn admit(&mut self, client: Option<&str>) -> Result<String, AgentSessionError> {
        let id = client
            .and_then(|id| Uuid::parse_str(id).ok())
            .unwrap_or_else(Uuid::new_v4)
            .to_string();
        if let Some(client) = client.filter(|client| *client != id) {
            if self.0.len() >= 512 {
                return Err(AgentSessionError::Rejected);
            }
            self.0.insert(id.clone(), client.to_owned());
            if let Err(error) = self.validate() {
                self.0.remove(&id);
                return Err(error);
            }
        }
        Ok(id)
    }

    pub(super) fn saved(&self) -> Option<Value> {
        (!self.0.is_empty()).then(|| serde_json::json!(self.0))
    }

    pub(super) fn decorate(&self, item: &mut NativeItem) {
        if item.item["type"] != "user_message" {
            return;
        }
        if let Some(id) = item.item["messageId"].as_str() {
            item.item["clientMessageId"] =
                serde_json::json!(self.0.get(id).map_or(id, String::as_str));
        }
    }
}

#[cfg(test)]
mod tests;
