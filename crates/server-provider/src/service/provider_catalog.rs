//! Bounded discovery cache over the same adapters used by native Agent execution.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, Instant};

use chrono::Utc;
use serde_json::{Value, json};
use server_metadata::protocol::session::SessionEventKind;
use server_metadata::service::session::SessionEvents;
use server_model::ErrorCode;
use sha2::{Digest, Sha256};

use crate::ports::agent_session::AgentClient;
use crate::protocol::provider::{Details, ListRequest, RefreshRequest, SnapshotRequest};

#[derive(Debug)]
struct Entry {
    value: Value,
    features: Vec<Value>,
}
#[derive(Debug)]
struct Snapshot {
    entries: BTreeMap<String, Entry>,
    fetched: Instant,
}

/// Cache scoped by canonical working directory, bounded to sixteen directories.
#[derive(Debug, Default)]
pub(crate) struct Catalog {
    snapshots: BTreeMap<String, Snapshot>,
}

impl Catalog {
    pub(crate) async fn execute(
        &mut self,
        clients: &BTreeMap<String, Box<dyn AgentClient>>,
        events: &SessionEvents,
        method: &str,
        params: Value,
    ) -> Result<Value, ErrorCode> {
        let (cwd, selected, if_none_match, refresh) = match method {
            "provider.available.list.request" => {
                crate::rpc::agent_execution::only(&params, &[])?;
                (None, None, None, false)
            }
            "provider.snapshot.get.request" => {
                let request: SnapshotRequest = decode(params)?;
                (request.cwd, None, request.if_none_match, false)
            }
            "provider.snapshot.refresh.request" => {
                let request: RefreshRequest = decode(params)?;
                if request.providers.as_ref().is_some_and(|providers| {
                    providers.len() > 32
                        || providers
                            .iter()
                            .any(|provider| !clients.contains_key(provider))
                }) {
                    return Err(ErrorCode::UnsupportedCapability);
                }
                (request.cwd, request.providers, None, true)
            }
            _ => {
                let request: ListRequest = decode(params)?;
                if !clients.contains_key(&request.provider) {
                    return Err(ErrorCode::UnsupportedCapability);
                }
                (request.cwd, Some(vec![request.provider]), None, false)
            }
        };
        let path = match cwd.as_deref() {
            Some(path) if std::path::Path::new(path).is_absolute() => {
                std::path::PathBuf::from(path)
            }
            Some(_) => return Err(ErrorCode::InvalidMessage),
            None => std::env::current_dir().map_err(|_| ErrorCode::AgentIo)?,
        };
        let path = path.canonicalize().map_err(|_| ErrorCode::InvalidMessage)?;
        if !path.is_dir() {
            return Err(ErrorCode::InvalidMessage);
        }
        let key = path.to_str().ok_or(ErrorCode::InvalidMessage)?.to_owned();
        let stale = self
            .snapshots
            .get(&key)
            .is_none_or(|snapshot| snapshot.fetched.elapsed() > Duration::from_secs(60));
        if stale || refresh {
            if self.snapshots.len() >= 16 && !self.snapshots.contains_key(&key) {
                let oldest = self
                    .snapshots
                    .iter()
                    .min_by_key(|(_, snapshot)| snapshot.fetched)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    self.snapshots.remove(&oldest);
                }
            }
            let selection = selected
                .as_ref()
                .map(|values| values.iter().collect::<BTreeSet<_>>());
            let snapshot = self
                .snapshots
                .entry(key.clone())
                .or_insert_with(|| Snapshot {
                    entries: BTreeMap::new(),
                    fetched: Instant::now(),
                });
            for (provider, client) in clients {
                if !stale
                    && selection
                        .as_ref()
                        .is_some_and(|selection| !selection.contains(provider))
                {
                    continue;
                }
                snapshot
                    .entries
                    .insert(provider.clone(), discover(client.as_ref(), &key).await);
            }
            snapshot.fetched = Instant::now();
        }
        let snapshot = self.snapshots.get(&key).ok_or(ErrorCode::AgentIo)?;
        response(
            snapshot,
            events,
            method,
            ReplyScope {
                key,
                selected,
                if_none_match,
                refresh,
            },
        )
    }
}

async fn discover(client: &dyn AgentClient, cwd: &str) -> Entry {
    let available = client.is_available().await;
    let (status, error, details) = if matches!(available, Ok(true)) {
        match client.discover(cwd).await {
            Ok(details) => ("ready", None, details),
            Err(_) => (
                "error",
                Some("Provider discovery failed"),
                Details::default(),
            ),
        }
    } else {
        (
            "unavailable",
            Some("Provider executable is unavailable"),
            Details::default(),
        )
    };
    let mut value = json!({"provider":client.provider(),"status":status,"enabled":true,"source":"builtin",
        "models":details.models,"modes":details.modes,"fetchedAt":Utc::now().to_rfc3339()});
    if let Some(error) = error {
        value["error"] = json!(error);
    }
    Entry {
        value,
        features: details.features,
    }
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(value).map_err(|_| ErrorCode::InvalidMessage)
}

#[cfg(test)]
mod tests;

struct ReplyScope {
    key: String,
    selected: Option<Vec<String>>,
    if_none_match: Option<String>,
    refresh: bool,
}

fn response(
    snapshot: &Snapshot,
    events: &SessionEvents,
    method: &str,
    scope: ReplyScope,
) -> Result<Value, ErrorCode> {
    let ReplyScope {
        key,
        selected,
        if_none_match,
        refresh,
    } = scope;

    let entries: Vec<_> = snapshot
        .entries
        .values()
        .map(|entry| entry.value.clone())
        .collect();
    let fetched_at = Utc::now().to_rfc3339();
    if method == "provider.available.list.request" {
        let providers: Vec<_> = entries.iter().map(|entry|json!({"provider":entry["provider"],"available":entry["status"]=="ready","error":entry.get("error")})).collect();
        return Ok(json!({"providers":providers,"error":null,"fetchedAt":fetched_at}));
    }
    if matches!(
        method,
        "provider.snapshot.get.request" | "provider.snapshot.refresh.request"
    ) {
        let hashable: Vec<_> = entries
            .iter()
            .map(|entry| {
                let mut entry = entry.clone();
                if let Some(object) = entry.as_object_mut() {
                    object.remove("fetchedAt");
                }
                entry
            })
            .collect();
        let bytes = serde_json::to_vec(&hashable).map_err(|_| ErrorCode::AgentIo)?;
        let hash = format!("{:x}", Sha256::digest(bytes));
        let mut payload =
            json!({"cwd":key,"entries":entries,"snapshotHash":hash,"generatedAt":fetched_at});
        if refresh {
            events.publish(SessionEventKind::ProvidersSnapshot, &payload);
            return Ok(json!({"acknowledged":true}));
        }
        let unchanged = if_none_match.as_deref() == Some(&hash);
        payload["notModified"] = json!(unchanged);
        if unchanged {
            payload["entries"] = json!([]);
        }
        return crate::rpc::timeline::bounded(payload);
    }
    let provider = selected
        .as_ref()
        .and_then(|values| values.first())
        .ok_or(ErrorCode::InvalidMessage)?;
    let entry = snapshot
        .entries
        .get(provider)
        .ok_or(ErrorCode::UnsupportedCapability)?;
    let field = match method {
        "provider.models.list.request" => "models",
        "provider.modes.list.request" => "modes",
        "provider.features.list.request" => "features",
        _ => return Err(ErrorCode::MethodNotFound),
    };
    let values = if field == "features" {
        json!(entry.features)
    } else {
        entry.value[field].clone()
    };
    crate::rpc::timeline::bounded(
        json!({"provider":provider,(field):values,"error":entry.value.get("error"),"fetchedAt":entry.value["fetchedAt"]}),
    )
}
