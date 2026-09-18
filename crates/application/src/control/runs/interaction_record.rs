//! Persisted user interaction records for one Run tool call.
use ait_contracts::ToolInteractionView;
use serde_json::Value;

#[derive(Clone, Debug, PartialEq)]
pub(in crate::control) struct ToolInteractionRecord {
    pub id: String,
    pub run_id: String,
    pub tool_name: String,
    pub request: Value,
    pub response: Option<Value>,
    pub status: String,
    pub lease_epoch: u64,
    pub expires_at: i64,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

impl ToolInteractionRecord {
    pub(in crate::control) fn view(&self) -> ToolInteractionView {
        ToolInteractionView {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            tool_name: self.tool_name.clone(),
            request: self.request.clone(),
            response: self.response.clone(),
            status: self.status.clone(),
            lease_epoch: self.lease_epoch,
            expires_at: self.expires_at,
            created_at: self.created_at,
            decided_at: self.decided_at,
        }
    }
}

impl From<ToolInteractionView> for ToolInteractionRecord {
    fn from(view: ToolInteractionView) -> Self {
        Self {
            id: view.id,
            run_id: view.run_id,
            tool_name: view.tool_name,
            request: view.request,
            response: view.response,
            status: view.status,
            lease_epoch: view.lease_epoch,
            expires_at: view.expires_at,
            created_at: view.created_at,
            decided_at: view.decided_at,
        }
    }
}
