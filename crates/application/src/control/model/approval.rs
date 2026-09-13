use ait_contracts::{NativeApprovalView, NativePermissionProfile, ProtocolRequestId};
use ait_domain::{
    ApprovalGrantScope, NativeApprovalKind, NativeApprovalStatus, NativeApprovalTarget,
};
use serde::{Deserialize, Serialize};
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub(in crate::control) struct NativeApprovalState {
    pub id: String,
    pub run_id: String,
    pub protocol_request_id: ProtocolRequestId,
    pub method: String,
    pub kind: NativeApprovalKind,
    pub thread_id: String,
    pub turn_id: String,
    pub item_id: String,
    /// Bounded, non-secret authorization object shown after reconnect.
    pub target: NativeApprovalTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested_permissions: Option<NativePermissionProfile>,
    pub status: NativeApprovalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_scope: Option<ApprovalGrantScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub granted_permissions: Option<NativePermissionProfile>,
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decided_at: Option<i64>,
}
impl NativeApprovalState {
    pub(in crate::control) fn view(&self) -> NativeApprovalView {
        NativeApprovalView {
            id: self.id.clone(),
            run_id: self.run_id.clone(),
            protocol_request_id: self.protocol_request_id.clone(),
            method: self.method.clone(),
            kind: self.kind,
            thread_id: self.thread_id.clone(),
            turn_id: self.turn_id.clone(),
            item_id: self.item_id.clone(),
            target: self.target.clone(),
            requested_permissions: self.requested_permissions.clone(),
            status: self.status,
            granted_scope: self.granted_scope,
            granted_permissions: self.granted_permissions.clone(),
            created_at: self.created_at,
            decided_at: self.decided_at,
        }
    }
}
impl From<NativeApprovalView> for NativeApprovalState {
    fn from(view: NativeApprovalView) -> Self {
        Self {
            id: view.id,
            run_id: view.run_id,
            protocol_request_id: view.protocol_request_id,
            method: view.method,
            kind: view.kind,
            thread_id: view.thread_id,
            turn_id: view.turn_id,
            item_id: view.item_id,
            target: view.target,
            requested_permissions: view.requested_permissions,
            status: view.status,
            granted_scope: view.granted_scope,
            granted_permissions: view.granted_permissions,
            created_at: view.created_at,
            decided_at: view.decided_at,
        }
    }
}
