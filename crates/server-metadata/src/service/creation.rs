//! Durable creation progress shared by Workspace and Agent capabilities.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use server_model::ErrorCode;
use server_model::events::{EventHub, Subscription};
use server_model::outbound::Outbound;
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::protocol::creation::{Kind, Snapshot};
use crate::storage::registry::FileRegistry;

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Receipt {
    id: String,
    intent: Value,
    snapshot: Snapshot,
}

#[derive(Debug, Default)]
struct State {
    memory: BTreeMap<String, Receipt>,
    active: BTreeSet<String>,
}

/// Shared creation coordinator. Production uses an atomic file; embedded defaults are ephemeral.
#[derive(Debug, Clone, Default)]
pub struct Creations {
    file: Option<Arc<FileRegistry<Receipt>>>,
    state: Arc<Mutex<State>>,
    events: EventHub,
}

/// Admission result: only a newly accepted intent may perform resource side effects.
#[derive(Debug)]
pub struct Admission {
    /// Whether the caller owns the first attempt.
    pub execute: bool,
    /// Latest committed state, including IDs reserved before the side effect.
    pub snapshot: Snapshot,
}

impl Creations {
    /// Open atomic creation receipts at `path` and load them without retrying interrupted work.
    /// # Errors
    /// Returns registry errors for malformed or inaccessible receipt files.
    pub fn open(path: PathBuf) -> Result<Self, ErrorCode> {
        let file = FileRegistry::new(path, |receipt: &Receipt| &receipt.id);
        file.initialize().map_err(io)?;
        Ok(Self {
            file: Some(Arc::new(file)),
            ..Self::default()
        })
    }

    /// Atomically reserve a resource identity for an immutable creation intent.
    /// `intent` excludes the key and subscribe flag; retries must supply the same intent.
    /// # Errors
    /// Returns invalid keys, key conflicts or durable storage failures before any side effects.
    pub fn begin(&self, kind: Kind, key: &str, intent: Value) -> Result<Admission, ErrorCode> {
        validate_key(key)?;
        let id = identity(kind, key);
        let mut state = self.state.lock().map_err(io)?;
        if let Some(receipt) = self.read(&state, &id)? {
            if receipt.intent != intent {
                return Err(ErrorCode::IdempotencyConflict);
            }
            let snapshot = observed(receipt.snapshot, state.active.contains(&id));
            return Ok(Admission {
                execute: false,
                snapshot,
            });
        }
        let field = match kind {
            Kind::Agent => "agentId",
            Kind::Workspace => "workspaceId",
        };
        let resource_id = if let Some(id) = intent[field].as_str() {
            id.to_owned()
        } else {
            match kind {
                Kind::Agent => Uuid::new_v4().to_string(),
                Kind::Workspace => super::directory::generate_workspace_id().map_err(io)?,
            }
        };
        let snapshot = Snapshot {
            kind,
            idempotency_key: key.to_owned(),
            revision: 0,
            phase: "accepted".to_owned(),
            workspace_id: if kind == Kind::Workspace {
                Some(resource_id.clone())
            } else {
                intent["workspaceId"].as_str().map(str::to_owned)
            },
            agent_id: (kind == Kind::Agent).then_some(resource_id),
            error: None,
            outcome_unknown: false,
            workspace: None,
            agent: None,
        };
        self.write(
            &mut state,
            Receipt {
                id: id.clone(),
                intent,
                snapshot: snapshot.clone(),
            },
        )?;
        state.active.insert(id.clone());
        self.publish(&id, &snapshot);
        Ok(Admission {
            execute: true,
            snapshot,
        })
    }

    /// Commit a progress transition and publish it after durable installation.
    /// # Errors
    /// Returns absent receipts, invalid transitions or storage failures. Terminal receipts cannot change.
    pub fn advance(
        &self,
        snapshot: &Snapshot,
        phase: &str,
        result: Option<Value>,
        error: Option<String>,
    ) -> Result<Snapshot, ErrorCode> {
        if !matches!(
            phase,
            "workspace_ready" | "agent_ready" | "prompt_started" | "completed" | "failed"
        ) {
            return Err(ErrorCode::InvalidMessage);
        }
        let id = identity(snapshot.kind, &snapshot.idempotency_key);
        let mut state = self.state.lock().map_err(io)?;
        let mut receipt = self.read(&state, &id)?.ok_or(ErrorCode::InvalidMessage)?;
        if terminal(&receipt.snapshot.phase) || receipt.snapshot.revision != snapshot.revision {
            return Err(ErrorCode::IdempotencyConflict);
        }
        receipt.snapshot.revision = receipt
            .snapshot
            .revision
            .checked_add(1)
            .ok_or(ErrorCode::ResourceExhausted)?;
        phase.clone_into(&mut receipt.snapshot.phase);
        receipt.snapshot.error = error;
        if let Some(result) = result {
            if let Some(agent) = result.get("agent").filter(|value| !value.is_null()) {
                receipt.snapshot.agent = Some(agent.clone());
            }
            if let Some(workspace) = result.get("workspace").filter(|value| !value.is_null()) {
                receipt.snapshot.workspace = Some(workspace.clone());
            }
        }
        let snapshot = receipt.snapshot.clone();
        self.write(&mut state, receipt)?;
        if terminal(phase) {
            state.active.remove(&id);
        }
        self.publish(&id, &snapshot);
        Ok(snapshot)
    }

    /// Read the latest receipt, reporting interrupted side effects as uncertain after restart.
    /// # Errors
    /// Returns invalid keys or registry failures.
    pub fn snapshot(&self, kind: Kind, key: &str) -> Result<Option<Snapshot>, ErrorCode> {
        validate_key(key)?;
        let state = self.state.lock().map_err(io)?;
        let id = identity(kind, key);
        Ok(self
            .read(&state, &id)?
            .map(|receipt| observed(receipt.snapshot, state.active.contains(&id))))
    }

    /// Atomically capture a snapshot and install a paused observer, including for an unknown key.
    /// # Errors
    /// Returns invalid keys or storage failures. Activate only after sending the response.
    pub fn subscribe(
        &self,
        kind: Kind,
        key: &str,
        outbound: Outbound,
    ) -> Result<(Option<Snapshot>, Subscription), ErrorCode> {
        validate_key(key)?;
        let state = self.state.lock().map_err(io)?;
        let id = identity(kind, key);
        let snapshot = self
            .read(&state, &id)?
            .map(|receipt| observed(receipt.snapshot, state.active.contains(&id)));
        let subscription =
            self.events
                .subscribe(Uuid::new_v4().to_string(), BTreeSet::from([id]), outbound);
        Ok((snapshot, subscription))
    }

    fn read(&self, state: &State, id: &str) -> Result<Option<Receipt>, ErrorCode> {
        match &self.file {
            Some(file) => file.get(id).map_err(io),
            None => Ok(state.memory.get(id).cloned()),
        }
    }

    fn write(&self, state: &mut State, receipt: Receipt) -> Result<(), ErrorCode> {
        if let Some(file) = &self.file {
            file.mutate(|records| {
                records.insert(receipt.id.clone(), receipt.clone());
                Ok(((), true))
            })
            .map_err(io)?;
        } else {
            state.memory.insert(receipt.id.clone(), receipt);
        }
        Ok(())
    }

    fn publish(&self, id: &str, snapshot: &Snapshot) {
        let method = match snapshot.kind {
            Kind::Agent => "agent.create.update",
            Kind::Workspace => "workspace.create.update",
        };
        self.events.publish(id, method, &json!(snapshot));
    }
}

/// Validate a bounded durable key without assigning it filesystem path semantics.
/// # Errors
/// Rejects empty, oversized or control-containing keys.
pub fn validate_key(key: &str) -> Result<(), ErrorCode> {
    if key.is_empty() || key.len() > 512 || key.chars().any(char::is_control) {
        Err(ErrorCode::InvalidMessage)
    } else {
        Ok(())
    }
}

fn identity(kind: Kind, key: &str) -> String {
    format!("{}:{:x}", kind.name(), Sha256::digest(key.as_bytes()))
}
fn terminal(phase: &str) -> bool {
    matches!(phase, "completed" | "failed")
}
fn observed(mut snapshot: Snapshot, active: bool) -> Snapshot {
    if !terminal(&snapshot.phase) && !active {
        snapshot.outcome_unknown = true;
        "failed".clone_into(&mut snapshot.phase);
        snapshot.error = Some("Creation was interrupted; inspect the reserved resource before retrying with a new key".to_owned());
    }
    snapshot
}
fn io<T>(_: T) -> ErrorCode {
    ErrorCode::RegistryIo
}

#[cfg(test)]
mod tests;
