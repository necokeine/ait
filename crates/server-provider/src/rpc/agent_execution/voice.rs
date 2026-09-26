use serde_json::{Value, json};

use super::{ErrorCode, ExecutionState, map_manager, only};

impl ExecutionState {
    pub(super) async fn voice(&mut self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        if method == "internal.voice.send" {
            only(&params, &["agentId", "text"])?;
            let request: crate::protocol::agent_execution::SendRequest = super::decode(params)?;
            let id = self.resolve(&request.agent_id)?;
            let record = self
                .registry
                .get(&id)
                .map_err(|_| ErrorCode::AgentIo)?
                .ok_or(ErrorCode::AgentNotFound)?;
            self.workspace(record.workspace_id.as_deref(), &record.cwd)?;
            let result = if self.manager.has_pending_input(&id)? {
                Err(super::AgentManagerError::Busy)
            } else {
                self.manager.send_input(&id, &request.into_prompt()).await
            };
            let mut receipt = json!({"agentId":id,"accepted":result.is_ok(),"error":result.err().map(|error|error.to_string())});
            if receipt["accepted"] == true {
                self.manager.claim_exclusive_turn(&id)?;
                let id = receipt["agentId"].as_str().ok_or(ErrorCode::AgentIo)?;
                let turn = self.manager.active_turn(id).ok_or(ErrorCode::AgentIo)?;
                receipt["turnId"] = json!(turn);
            }
            return Ok(receipt);
        }
        only(&params, &["agentId", "turnId"])?;
        let id = self.resolve(
            params["agentId"]
                .as_str()
                .ok_or(ErrorCode::InvalidMessage)?,
        )?;
        let turn = params["turnId"]
            .as_str()
            .filter(|id| server_model::valid_id(id))
            .ok_or(ErrorCode::InvalidMessage)?;
        let owns_latest = self.manager.latest_turn(&id) == Some(turn);
        let owns_active = self.manager.active_turn(&id) == Some(turn);
        if method == "internal.voice.cancel" {
            if owns_active {
                self.manager
                    .cancel(&id)
                    .await
                    .map_err(|error| map_manager(&error))?;
            }
            return Ok(json!({"cancelled":owns_active}));
        }
        if !owns_latest {
            return Err(ErrorCode::IdempotencyConflict);
        }
        let snapshot = self.snapshot(&id)?;
        let status = if owns_active {
            "running"
        } else if snapshot["status"] == "error" {
            "error"
        } else {
            "idle"
        };
        Ok(json!({"status":status,"lastMessage":self.manager.last_message(&id)}))
    }
}
