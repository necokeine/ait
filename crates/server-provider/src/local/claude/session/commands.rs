//! Native checkpoint commands run as foreground control turns without model inference.

use super::{
    AgentSessionError, AgentTurnEvent, ClaudeSession, Uuid, Value, history, json, streaming,
};
use crate::protocol::prompt::AgentPrompt;

pub(super) fn is_rewind(text: &str) -> bool {
    text.split_whitespace().next() == Some("/rewind")
}

impl ClaudeSession {
    pub(super) async fn start_rewind(
        &mut self,
        prompt: &AgentPrompt,
    ) -> Result<String, AgentSessionError> {
        if !prompt.images.is_empty()
            || !prompt.attachments.is_empty()
            || prompt.output_schema.is_some()
        {
            return Err(AgentSessionError::Rejected);
        }
        let handle = self.handle();
        let history = history::read(&self.client, &handle, &self.spec.cwd)?
            .ok_or(AgentSessionError::Rejected)?;
        let requested = prompt
            .text
            .trim()
            .strip_prefix("/rewind")
            .ok_or(AgentSessionError::Rejected)?
            .trim();
        let candidates: Vec<_> = history
            .entries
            .iter()
            .rev()
            .filter(|entry| entry.item["type"] == "user_message")
            .filter_map(|entry| entry.item["messageId"].as_str())
            .filter(|id| requested.is_empty() || *id == requested)
            .take(128)
            .collect();
        if candidates.is_empty() {
            return Err(AgentSessionError::Rejected);
        }
        if self.transport.is_none() {
            self.launch().await?;
        }
        let turn = Uuid::new_v4().to_string();
        for candidate in candidates {
            let entry = streaming::entry(
                &format!("control:{turn}"),
                json!({"type":"assistant_message","messageId":format!("control:{turn}"),
                "text":format!("Rewound tracked files to message {candidate}.")}),
                &Value::Null,
            );
            let mut notes = self.notes.clone();
            notes.push(entry.clone())?;
            let response = self
                .transport
                .as_mut()
                .ok_or(AgentSessionError::Failed)?
                .request(
                    json!({"subtype":"rewind_files","user_message_id":candidate,"dry_run":false}),
                )
                .await;
            let response = match response {
                Ok(response) => response,
                Err(AgentSessionError::Rejected) if requested.is_empty() => continue,
                Err(error) => return Err(error),
            };
            if response["canRewind"] == false
                || response["can_rewind"] == false
                || response["error"]
                    .as_str()
                    .is_some_and(|error| !error.is_empty())
            {
                if requested.is_empty() {
                    continue;
                }
                return Err(AgentSessionError::Rejected);
            }
            self.notes = notes;
            let text = entry.item["text"].as_str().map(str::to_owned);
            self.stream
                .events
                .push_back(AgentTurnEvent::Timeline(entry));
            self.stream
                .events
                .push_back(AgentTurnEvent::Completed(text));
            self.active = Some(turn.clone());
            return Ok(turn);
        }
        Err(AgentSessionError::Rejected)
    }
}
