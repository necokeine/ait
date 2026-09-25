use serde_json::{Value, json};

use super::{ErrorCode, ExecutionState, map_manager, only};

impl ExecutionState {
    pub(super) async fn voice(&mut self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        if method == "internal.voice.send" {
            let mut receipt = self.send(params).await?;
            if receipt["accepted"] == true {
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
