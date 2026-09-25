//! Label request handling and ordered subscription delivery.

use std::sync::{Arc, Mutex};

use crate::model::workspace_labels::{
    WorkspaceLabelColor as DomainColor, WorkspaceLabelDefinition as DomainDefinition,
};
use crate::protocol::workspace_labels::{
    WorkspaceLabelAffectedResult, WorkspaceLabelAssignmentSetRequest,
    WorkspaceLabelAssignmentSetResult, WorkspaceLabelColor, WorkspaceLabelDefinition,
    WorkspaceLabelDeleteRequest, WorkspaceLabelListRequest, WorkspaceLabelListResult,
    WorkspaceLabelLiveUpdate, WorkspaceLabelRemoval, WorkspaceLabelSyncCursor,
    WorkspaceLabelSyncMetadata, WorkspaceLabelSyncMode as ProtocolSyncMode,
    WorkspaceLabelUpdateRequest, WorkspaceLabelUpdateResult,
};
use crate::rpc::ErrorCode;
use crate::service::workspace_labels::{
    SequencedWorkspaceLabelChange, WorkspaceLabelChange, WorkspaceLabelCursor, WorkspaceLabelError,
    WorkspaceLabelSubscription, WorkspaceLabelSync, WorkspaceLabelSyncMode, WorkspaceLabels,
};
use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

/// Ephemeral delivery failure; the host closes a connection when its budget is exhausted.
#[derive(Debug, thiserror::Error)]
pub enum DeliveryError {
    /// An event could not be serialized.
    #[error("metadata event encoding failed")]
    Encode(#[from] serde_json::Error),
    /// The connection is closed or its send budget is exhausted.
    #[error("metadata event sink unavailable")]
    Closed,
}

/// Host-provided connection sink; metadata never owns a socket or transport task.
pub type EventSink = Arc<dyn Fn(Value) -> Result<(), DeliveryError> + Send + Sync>;

/// Result with a listener activated only after response admission.
pub struct Dispatch {
    /// Serialized business response.
    pub value: Value,
    /// Listener awaiting response admission.
    pub subscription: Option<PendingSubscription>,
}

/// Inactive listener and its buffered events, owned by one physical connection.
pub struct PendingSubscription {
    subscription_id: String,
    subscription: WorkspaceLabelSubscription,
    delivery: Arc<Mutex<Delivery>>,
    head_seq: u64,
}

struct Delivery {
    ready: bool,
    pending: Vec<(String, SequencedWorkspaceLabelChange)>,
    outbound: EventSink,
    subscription_id: String,
}

impl PendingSubscription {
    /// Activate after admitting the snapshot response into the host send queue.
    ///
    /// # Errors
    /// Returns a delivery error if an event cannot be sent.
    pub fn activate(self) -> Result<(String, WorkspaceLabelSubscription), DeliveryError> {
        let mut delivery = self
            .delivery
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        delivery.ready = true;
        let pending = std::mem::take(&mut delivery.pending);
        for change in pending
            .into_iter()
            .map(|(_, change)| change)
            .filter(|change| change.seq > self.head_seq)
        {
            delivery.send(&change)?;
        }
        drop(delivery);
        Ok((self.subscription_id, self.subscription))
    }
}

impl Delivery {
    fn accept(&mut self, change: SequencedWorkspaceLabelChange) {
        if self.ready {
            let _ = self.send(&change);
        } else {
            let key = change_key(&change);
            if let Some((_, pending)) = self
                .pending
                .iter_mut()
                .find(|(pending_key, _)| pending_key == &key)
            {
                *pending = change;
            } else {
                self.pending.push((key, change));
            }
        }
    }

    fn send(&self, change: &SequencedWorkspaceLabelChange) -> Result<(), DeliveryError> {
        let params = serde_json::to_value(live_update(&self.subscription_id, change))?;
        (self.outbound)(params)
    }
}

/// Decode and execute one label request using a connection-owned event sink.
///
/// # Errors
/// Returns validation, catalog, assignment or subscription errors.
pub fn execute(
    labels: &WorkspaceLabels,
    method: &str,
    params: Value,
    outbound: EventSink,
) -> Result<Dispatch, ErrorCode> {
    match method {
        "workspace.label.list.request" => list(labels, &decode(params)?, outbound),
        "workspace.label.assignment.set.request" => assignment(labels, decode(params)?),
        "workspace.label.update.request" => update(labels, &decode(params)?),
        "workspace.label.delete.inspect.request" => inspect_delete(labels, &decode(params)?),
        "workspace.label.delete.request" => delete(labels, &decode(params)?),
        _ => Err(ErrorCode::MethodNotFound),
    }
}

fn list(
    labels: &WorkspaceLabels,
    request: &WorkspaceLabelListRequest,
    outbound: EventSink,
) -> Result<Dispatch, ErrorCode> {
    if request
        .subscribe
        .as_ref()
        .is_some_and(|subscribe| subscribe.subscription_id.is_some())
    {
        return Err(ErrorCode::InvalidMessage);
    }
    let cursor = request.sync.as_ref().map(application_cursor);
    if request.subscribe.is_none() {
        let sync = labels.list(cursor.as_ref()).map_err(map_error)?;
        return Ok(Dispatch {
            value: encode(list_result(None, sync))?,
            subscription: None,
        });
    }
    let subscription_id = Uuid::new_v4().to_string();
    let delivery = Arc::new(Mutex::new(Delivery {
        ready: false,
        pending: Vec::new(),
        outbound,
        subscription_id: subscription_id.clone(),
    }));
    let listener_delivery = delivery.clone();
    let (sync, subscription) = labels
        .subscribe(
            cursor.as_ref(),
            Arc::new(move |change| {
                listener_delivery
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .accept(change);
            }),
        )
        .map_err(map_error)?;
    let head_seq = sync.sync.head_seq;
    Ok(Dispatch {
        value: encode(list_result(Some(subscription_id.clone()), sync))?,
        subscription: Some(PendingSubscription {
            subscription_id,
            subscription,
            delivery,
            head_seq,
        }),
    })
}

fn assignment(
    labels: &WorkspaceLabels,
    request: WorkspaceLabelAssignmentSetRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = labels
        .set_assignment(
            &request.workspace_id,
            &domain_definition(request.label),
            request.assigned,
            &timestamp(),
        )
        .map_err(map_error)?;
    value(WorkspaceLabelAssignmentSetResult {
        label: protocol_definition(result.label),
        workspace_labels: result.workspace_labels,
    })
}

fn update(
    labels: &WorkspaceLabels,
    request: &WorkspaceLabelUpdateRequest,
) -> Result<Dispatch, ErrorCode> {
    let result = labels
        .update(
            &request.name,
            request.new_name.as_deref(),
            request.color.map(domain_color),
            &timestamp(),
        )
        .map_err(map_error)?;
    value(WorkspaceLabelUpdateResult {
        label: protocol_definition(result.label),
        affected_workspace_count: result.affected_workspace_count,
    })
}

fn inspect_delete(
    labels: &WorkspaceLabels,
    request: &WorkspaceLabelDeleteRequest,
) -> Result<Dispatch, ErrorCode> {
    value(WorkspaceLabelAffectedResult {
        affected_workspace_count: labels.inspect_delete(&request.name).map_err(map_error)?,
    })
}

fn delete(
    labels: &WorkspaceLabels,
    request: &WorkspaceLabelDeleteRequest,
) -> Result<Dispatch, ErrorCode> {
    value(WorkspaceLabelAffectedResult {
        affected_workspace_count: labels
            .delete(&request.name, &timestamp())
            .map_err(map_error)?,
    })
}

fn value(value: impl Serialize) -> Result<Dispatch, ErrorCode> {
    Ok(Dispatch {
        value: encode(value)?,
        subscription: None,
    })
}

fn list_result(
    subscription_id: Option<String>,
    sync: WorkspaceLabelSync,
) -> WorkspaceLabelListResult {
    WorkspaceLabelListResult {
        subscription_id,
        labels: sync.labels.into_iter().map(protocol_definition).collect(),
        sync: WorkspaceLabelSyncMetadata {
            mode: match sync.sync.mode {
                WorkspaceLabelSyncMode::Snapshot => ProtocolSyncMode::Snapshot,
                WorkspaceLabelSyncMode::Changes => ProtocolSyncMode::Changes,
            },
            generation: sync.sync.generation,
            head_seq: sync.sync.head_seq,
            removals: sync
                .sync
                .removals
                .into_iter()
                .map(|removal| WorkspaceLabelRemoval {
                    name: removal.name,
                    seq: removal.seq,
                })
                .collect(),
        },
    }
}

fn live_update(
    subscription_id: &str,
    update: &SequencedWorkspaceLabelChange,
) -> WorkspaceLabelLiveUpdate {
    match &update.change {
        WorkspaceLabelChange::Upsert {
            label,
            previous_name,
        } => WorkspaceLabelLiveUpdate::Upsert {
            subscription_id: subscription_id.to_owned(),
            label: protocol_definition(label.clone()),
            previous_name: previous_name.clone(),
            generation: update.generation.clone(),
            seq: update.seq,
        },
        WorkspaceLabelChange::Remove { name } => WorkspaceLabelLiveUpdate::Remove {
            subscription_id: subscription_id.to_owned(),
            name: name.clone(),
            generation: update.generation.clone(),
            seq: update.seq,
        },
    }
}

fn change_key(update: &SequencedWorkspaceLabelChange) -> String {
    match &update.change {
        WorkspaceLabelChange::Upsert { label, .. } => label.name.to_lowercase(),
        WorkspaceLabelChange::Remove { name } => name.to_lowercase(),
    }
}

fn application_cursor(cursor: &WorkspaceLabelSyncCursor) -> WorkspaceLabelCursor {
    WorkspaceLabelCursor {
        generation: cursor.generation.clone(),
        after_seq: cursor.after_seq,
    }
}

fn domain_definition(definition: WorkspaceLabelDefinition) -> DomainDefinition {
    DomainDefinition {
        name: definition.name,
        color: domain_color(definition.color),
    }
}

fn protocol_definition(definition: DomainDefinition) -> WorkspaceLabelDefinition {
    WorkspaceLabelDefinition {
        name: definition.name,
        color: protocol_color(definition.color),
    }
}

const fn domain_color(color: WorkspaceLabelColor) -> DomainColor {
    match color {
        WorkspaceLabelColor::Violet => DomainColor::Violet,
        WorkspaceLabelColor::Sky => DomainColor::Sky,
        WorkspaceLabelColor::Emerald => DomainColor::Emerald,
        WorkspaceLabelColor::Orange => DomainColor::Orange,
        WorkspaceLabelColor::Pink => DomainColor::Pink,
        WorkspaceLabelColor::Indigo => DomainColor::Indigo,
        WorkspaceLabelColor::Teal => DomainColor::Teal,
        WorkspaceLabelColor::Red => DomainColor::Red,
        WorkspaceLabelColor::Amber => DomainColor::Amber,
        WorkspaceLabelColor::Blue => DomainColor::Blue,
    }
}

const fn protocol_color(color: DomainColor) -> WorkspaceLabelColor {
    match color {
        DomainColor::Violet => WorkspaceLabelColor::Violet,
        DomainColor::Sky => WorkspaceLabelColor::Sky,
        DomainColor::Emerald => WorkspaceLabelColor::Emerald,
        DomainColor::Orange => WorkspaceLabelColor::Orange,
        DomainColor::Pink => WorkspaceLabelColor::Pink,
        DomainColor::Indigo => WorkspaceLabelColor::Indigo,
        DomainColor::Teal => WorkspaceLabelColor::Teal,
        DomainColor::Red => WorkspaceLabelColor::Red,
        DomainColor::Amber => WorkspaceLabelColor::Amber,
        DomainColor::Blue => WorkspaceLabelColor::Blue,
    }
}

const fn map_error(error: WorkspaceLabelError) -> ErrorCode {
    match error {
        WorkspaceLabelError::NameEmpty => ErrorCode::LabelNameEmpty,
        WorkspaceLabelError::LabelNotFound => ErrorCode::LabelNotFound,
        WorkspaceLabelError::NameTaken => ErrorCode::LabelNameTaken,
        WorkspaceLabelError::WorkspaceNotFound => ErrorCode::WorkspaceNotFound,
        WorkspaceLabelError::Storage => ErrorCode::RegistryIo,
        WorkspaceLabelError::StorageUncertain => ErrorCode::WorkspaceLabelStorageUncertain,
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

#[cfg(test)]
mod tests;
