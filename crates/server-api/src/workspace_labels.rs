use std::sync::Arc;

use serde_json::Value;
pub(super) use server_metadata::rpc::workspace_labels::PendingSubscription;
use server_metadata::rpc::workspace_labels::{DeliveryError, Dispatch};
use server_protocol::{ErrorCode, ServerMessage};

use crate::Shared;
use crate::outbound::Outbound;

pub(super) async fn dispatch(
    method: &str,
    params: Value,
    state: &Shared,
    outbound: Outbound,
) -> Result<Dispatch, ErrorCode> {
    let method = method.to_owned();
    let sink = Arc::new(move |params| {
        outbound
            .send(&ServerMessage::Event {
                method: "workspace.label.update".to_owned(),
                params,
            })
            .map_err(|_| DeliveryError::Closed)
    });
    crate::jobs::run(
        state,
        state.workspace_labels.clone(),
        ErrorCode::RegistryIo,
        move |labels| {
            server_metadata::rpc::workspace_labels::execute(labels, &method, params, sink)
                .map_err(Into::into)
        },
    )
    .await
}
