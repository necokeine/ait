//! Small durable native control results absent from a provider's transcript file.

use serde_json::Value;

use crate::ports::agent_session::AgentSessionError;
use crate::protocol::timeline::NativeItem;

#[derive(Debug, Clone, Default)]
pub(super) struct Notes(Vec<NativeItem>);

impl Notes {
    pub(super) fn restore(value: Option<&Value>) -> Result<Self, AgentSessionError> {
        let Some(value) = value else {
            return Ok(Self::default());
        };
        let entries: Vec<NativeItem> =
            serde_json::from_value(value.clone()).map_err(|_| AgentSessionError::Failed)?;
        let mut notes = Self::default();
        for entry in entries {
            notes.push(entry)?;
        }
        Ok(notes)
    }

    pub(super) fn push(&mut self, entry: NativeItem) -> Result<(), AgentSessionError> {
        if let Some(previous) = self.0.iter().find(|previous| previous.key == entry.key) {
            return if previous.item == entry.item {
                Ok(())
            } else {
                Err(AgentSessionError::Failed)
            };
        }
        if self.0.len() >= 128
            || serde_json::to_vec(&self.0)
                .map_err(|_| AgentSessionError::Failed)?
                .len()
                + serde_json::to_vec(&entry)
                    .map_err(|_| AgentSessionError::Failed)?
                    .len()
                > 64 * 1024
        {
            return Err(AgentSessionError::Rejected);
        }
        self.0.push(entry);
        Ok(())
    }

    pub(super) fn saved(&self) -> Option<Value> {
        (!self.0.is_empty()).then(|| serde_json::json!(self.0))
    }

    pub(super) fn history(&self, entries: &mut Vec<NativeItem>) {
        for entry in &self.0 {
            if !entries.iter().any(|existing| existing.key == entry.key) {
                let timestamp = chrono::DateTime::parse_from_rfc3339(&entry.timestamp).ok();
                let position = timestamp
                    .and_then(|timestamp| {
                        entries.iter().position(|existing| {
                            chrono::DateTime::parse_from_rfc3339(&existing.timestamp)
                                .is_ok_and(|existing| existing > timestamp)
                        })
                    })
                    .unwrap_or(entries.len());
                entries.insert(position, entry.clone());
            }
        }
    }

    pub(super) fn retain(&mut self, entries: &[NativeItem]) {
        self.0.retain(|note| {
            note.turn_id.as_ref().is_some_and(|turn| {
                entries
                    .iter()
                    .any(|entry| entry.turn_id.as_ref() == Some(turn))
            })
        });
    }
}

#[cfg(test)]
mod tests;
