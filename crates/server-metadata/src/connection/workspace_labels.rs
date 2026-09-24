use std::sync::Arc;

use crate::rpc::workspace_labels::{DeliveryError, Dispatch};
use serde_json::Value;
use server_model::{ErrorCode, ServerMessage};

use crate::dispatch::State as Shared;
use server_model::outbound::Outbound;

pub async fn dispatch(
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
    state
        .run(
            state.workspace_labels.clone(),
            ErrorCode::RegistryIo,
            move |labels| {
                crate::rpc::workspace_labels::execute(labels, &method, params, sink)
                    .map_err(Into::into)
            },
        )
        .await
}
