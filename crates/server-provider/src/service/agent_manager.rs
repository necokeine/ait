//! Provider session registration and recovery for the independent server.

use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use server_domain::agent_runtime::{
    AgentRuntimeStatus, PersistedAgentRuntimeRecord, StoredAgentRuntimeInfo,
};

use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::ports::agent_session::{
    AgentClient, AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionSpec,
    AgentTurnEvent,
};

/// Application failure while managing a live Agent session.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AgentManagerError {
    /// An Agent ID or provider identity was invalid.
    #[error("invalid Agent session request")]
    InvalidRequest,
    /// The Agent or its durable state does not exist.
    #[error("Agent not found: {0}")]
    NotFound(String),
    /// A live or durable Agent already has this ID.
    #[error("Agent already exists: {0}")]
    AlreadyExists(String),
    /// No registered, available provider can operate this Agent.
    #[error("provider unavailable: {0}")]
    ProviderUnavailable(String),
    /// The durable Agent has no native session identity to resume.
    #[error("Agent has no resumable provider session: {0}")]
    MissingPersistence(String),
    /// The provider adapter failed or returned inconsistent facts.
    #[error("provider session failed")]
    Session,
    /// Durable Agent state could not be read or written.
    #[error("Agent runtime registry failed")]
    Registry,
    /// A turn is already active or the Agent is archived.
    #[error("Agent is busy or read-only")]
    Busy,
}

/// Metadata added when a new provider session is registered.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AgentRegistration {
    /// Workspace placement, if any.
    pub workspace_id: Option<String>,
    /// Initial user-visible title.
    pub title: Option<String>,
    /// Public and delegation labels.
    pub labels: BTreeMap<String, String>,
    /// Whether this is an internal runtime Agent.
    pub internal: bool,
}

#[derive(Debug)]
struct LiveAgent {
    session: Box<dyn AgentSession>,
    record: PersistedAgentRuntimeRecord,
    registered: bool,
    turn: Option<String>,
    last_message: Option<String>,
    pending: Option<AgentTurnEvent>,
}

/// Owns live provider sessions and their durable Paseo-compatible snapshots.
///
/// Callers serialize mutations to one manager. The registry and provider operations are fallible;
/// a failed registration attempts to close the unregistered session before returning an error.
#[derive(Debug)]
pub struct AgentManager {
    registry: Box<dyn AgentRuntimeRegistry>,
    clients: BTreeMap<String, Box<dyn AgentClient>>,
    live: BTreeMap<String, LiveAgent>,
}

impl AgentManager {
    /// Compose the manager with a durable Agent runtime registry.
    #[must_use]
    pub fn new(registry: Box<dyn AgentRuntimeRegistry>) -> Self {
        Self {
            registry,
            clients: BTreeMap::new(),
            live: BTreeMap::new(),
        }
    }

    /// Register one provider client. A provider identity can only be registered once.
    ///
    /// # Errors
    /// Returns `InvalidRequest` for an empty identity and `AlreadyExists` for a duplicate.
    pub fn register_client(
        &mut self,
        client: Box<dyn AgentClient>,
    ) -> Result<(), AgentManagerError> {
        let provider = client.provider().to_owned();
        if provider.trim().is_empty() || provider.chars().any(char::is_control) {
            return Err(AgentManagerError::InvalidRequest);
        }
        if self.clients.contains_key(&provider) {
            return Err(AgentManagerError::AlreadyExists(provider));
        }
        self.clients.insert(provider, client);
        Ok(())
    }

    /// Return the live session's latest registered snapshot, if this process owns it.
    #[must_use]
    pub fn live_snapshot(&self, agent_id: &str) -> Option<&PersistedAgentRuntimeRecord> {
        self.live
            .get(agent_id)
            .filter(|agent| agent.registered)
            .map(|agent| &agent.record)
    }

    /// Return the currently accepted native turn identity, when one is active.
    #[must_use]
    pub fn active_turn(&self, agent_id: &str) -> Option<&str> {
        self.live
            .get(agent_id)
            .and_then(|agent| agent.turn.as_deref())
    }

    /// Return the final assistant text from this process's latest completed turn.
    #[must_use]
    pub fn last_message(&self, agent_id: &str) -> Option<&str> {
        self.live
            .get(agent_id)
            .and_then(|agent| agent.last_message.as_deref())
    }

    /// Create a native session and persist its initial Agent snapshot.
    ///
    /// # Errors
    /// Rejects invalid or duplicate IDs, unavailable providers, session failures, and storage
    /// failures. If registration fails after creation, the new session is closed.
    pub async fn create(
        &mut self,
        agent_id: &str,
        spec: &AgentSessionSpec,
        registration: AgentRegistration,
    ) -> Result<PersistedAgentRuntimeRecord, AgentManagerError> {
        validate_identity(agent_id, spec)?;
        if self.live.len() >= 32 {
            return Err(AgentManagerError::Busy);
        }
        if self.live.contains_key(agent_id)
            || self.registry.get(agent_id).map_err(map_registry)?.is_some()
        {
            return Err(AgentManagerError::AlreadyExists(agent_id.to_owned()));
        }
        let client = self.available_client(&spec.provider).await?;
        let session = client.create_session(spec).await.map_err(map_session)?;
        let now = now_timestamp();
        let record = PersistedAgentRuntimeRecord {
            id: agent_id.to_owned(),
            provider: spec.provider.clone(),
            cwd: spec.cwd.clone(),
            workspace_id: registration.workspace_id,
            created_at: now.clone(),
            updated_at: now,
            last_activity_at: None,
            last_user_message_at: None,
            title: registration.title,
            labels: registration.labels,
            last_status: AgentRuntimeStatus::Idle,
            last_mode_id: spec.config.mode_id.clone(),
            config: Some(spec.config.clone()),
            runtime_info: None,
            features: Vec::new(),
            persistence: None,
            last_error: None,
            requires_attention: false,
            attention_reason: None,
            attention_timestamp: None,
            internal: registration.internal,
            archived_at: None,
            owner: None,
        };
        self.register_session(agent_id, session, record, true).await
    }

    /// Resume a persisted provider session, using history-only mode for archived Agents.
    ///
    /// Restoring a session does not change its durable activity and update timestamps. A record
    /// without a provider handle cannot be resumed and is left untouched.
    ///
    /// # Errors
    /// Returns missing-state, unavailable-provider, provider, or registry failures.
    pub async fn resume(
        &mut self,
        agent_id: &str,
    ) -> Result<PersistedAgentRuntimeRecord, AgentManagerError> {
        if let Some(agent) = self.live.get(agent_id) {
            return if agent.registered {
                self.registry
                    .get(agent_id)
                    .map_err(map_registry)?
                    .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))
            } else {
                Err(AgentManagerError::Session)
            };
        }
        if self.live.len() >= 32 {
            return Err(AgentManagerError::Busy);
        }
        let record = self
            .registry
            .get(agent_id)
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or_else(|| AgentManagerError::MissingPersistence(agent_id.to_owned()))?;
        if handle.provider != record.provider {
            return Err(AgentManagerError::InvalidRequest);
        }
        let spec = AgentSessionSpec {
            provider: record.provider.clone(),
            cwd: record.cwd.clone(),
            config: record.config.clone().unwrap_or_default(),
        };
        validate_identity(agent_id, &spec)?;
        let purpose = if record.archived_at.is_some() {
            AgentResumePurpose::History
        } else {
            AgentResumePurpose::Interactive
        };
        let client = self.available_client(&record.provider).await?;
        let session = client
            .resume_session(handle, &spec, purpose)
            .await
            .map_err(map_session)?;
        self.register_session(agent_id, session, record, false)
            .await
    }

    /// Close one live session while retaining its provider history and durable Agent record.
    ///
    /// A native close failure retains manager ownership so a later attempt can retry. Missing
    /// live sessions are idempotent.
    ///
    /// # Errors
    /// Returns provider or registry failure.
    pub async fn close(&mut self, agent_id: &str) -> Result<(), AgentManagerError> {
        let Some(agent) = self.live.get_mut(agent_id) else {
            return Ok(());
        };
        agent.session.close().await.map_err(map_session)?;
        if !agent.registered {
            self.live.remove(agent_id);
            return Ok(());
        }
        // Metadata and attention may have changed through other narrow ports. Never replace
        // their latest record with the manager's creation-time snapshot or resurrect a deletion.
        self.registry
            .update(agent_id, &|current| {
                let mut next = current.clone();
                next.last_status = AgentRuntimeStatus::Closed;
                next
            })
            .map_err(map_registry)?;
        self.live.remove(agent_id);
        Ok(())
    }

    /// Start one native text turn. Busy Agents reject additional work explicitly.
    ///
    /// # Errors
    /// Returns resume, provider, storage, or busy/read-only failures.
    pub async fn send(&mut self, agent_id: &str, text: &str) -> Result<(), AgentManagerError> {
        if text.trim().is_empty() || text.len() > 65536 {
            return Err(AgentManagerError::InvalidRequest);
        }
        let record = self.resume(agent_id).await?;
        if record.archived_at.is_some() || self.active_turn(agent_id).is_some() {
            return Err(AgentManagerError::Busy);
        }
        let now = now_timestamp();
        self.registry
            .update(agent_id, &|current| {
                let mut next = current.clone();
                next.last_status = AgentRuntimeStatus::Running;
                next.updated_at.clone_from(&now);
                next.last_user_message_at = Some(now.clone());
                next.last_activity_at = Some(now.clone());
                next.last_error = None;
                next.requires_attention = false;
                next.attention_reason = None;
                next.attention_timestamp = None;
                next
            })
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        let agent = self
            .live
            .get_mut(agent_id)
            .ok_or(AgentManagerError::Session)?;
        agent.last_message = None;
        if let Ok(turn) = agent.session.start_turn(text).await {
            agent.turn = Some(turn);
            Ok(())
        } else {
            agent.pending = Some(AgentTurnEvent::Failed);
            self.poll().await?;
            Err(AgentManagerError::Session)
        }
    }

    /// Request interruption; completion is acknowledged only by the provider's terminal event.
    ///
    /// # Errors
    /// Returns native interruption failures. An idle Agent is an idempotent success.
    pub async fn cancel(&mut self, agent_id: &str) -> Result<(), AgentManagerError> {
        let Some(agent) = self.live.get_mut(agent_id) else {
            return Ok(());
        };
        if let Some(turn) = &agent.turn {
            agent.session.cancel_turn(turn).await.map_err(map_session)?;
        }
        Ok(())
    }

    /// Persist queued native terminal events before exposing completion to clients.
    ///
    /// A failed write retains its event for retry. Native failures close the process before
    /// publishing an error, so unsupported interactions cannot keep executing invisibly.
    ///
    /// # Errors
    /// Returns the first storage or close failure; later calls retry pending work.
    pub async fn poll(&mut self) -> Result<(), AgentManagerError> {
        let ids = self.live.keys().cloned().collect::<Vec<_>>();
        for id in ids {
            let Some(agent) = self.live.get_mut(&id) else {
                continue;
            };
            if !agent.registered {
                continue;
            }
            if agent.pending.is_none() {
                agent.pending = match agent.session.poll_turn() {
                    Ok(event) => event,
                    Err(_) => Some(AgentTurnEvent::Failed),
                };
            }
            let Some(event) = &agent.pending else {
                continue;
            };
            if matches!(event, AgentTurnEvent::Failed) {
                agent.session.close().await.map_err(map_session)?;
            }
            let failed = matches!(event, AgentTurnEvent::Failed);
            let cancelled = matches!(event, AgentTurnEvent::Cancelled);
            let now = now_timestamp();
            self.registry
                .update(&id, &|current| {
                    let mut next = current.clone();
                    next.last_status = if failed {
                        AgentRuntimeStatus::Error
                    } else {
                        AgentRuntimeStatus::Idle
                    };
                    next.last_error = failed.then(|| "Provider execution failed".to_owned());
                    next.updated_at.clone_from(&now);
                    next.last_activity_at = Some(now.clone());
                    next.requires_attention = !cancelled;
                    next.attention_reason = (!cancelled).then_some(if failed {
                        server_domain::agent_runtime::AgentAttentionReason::Error
                    } else {
                        server_domain::agent_runtime::AgentAttentionReason::Finished
                    });
                    next.attention_timestamp = (!cancelled).then(|| now.clone());
                    next
                })
                .map_err(map_registry)?;
            if let AgentTurnEvent::Completed(message) = event {
                agent.last_message.clone_from(message);
            }
            agent.turn = None;
            agent.pending = None;
            if failed {
                self.live.remove(&id);
            }
        }
        Ok(())
    }

    /// Close writers whose durable records were archived or removed by metadata operations.
    ///
    /// # Errors
    /// Returns registry or native-close errors; ownership remains available for retry.
    pub async fn reconcile(&mut self) -> Result<(), AgentManagerError> {
        let mut close = Vec::new();
        for (id, live) in &self.live {
            let current = self.registry.get(id).map_err(map_registry)?;
            if current.is_none()
                || (live.record.archived_at.is_none()
                    && current
                        .as_ref()
                        .is_some_and(|record| record.archived_at.is_some()))
            {
                close.push(id.clone());
            }
        }
        for id in close {
            self.close(&id).await?;
        }
        Ok(())
    }

    /// Close every live session. Failed closes retain ownership for retry.
    ///
    /// # Errors
    /// Returns the first provider or registry failure after attempting every close.
    pub async fn close_all(&mut self) -> Result<(), AgentManagerError> {
        let ids = self.live.keys().cloned().collect::<Vec<_>>();
        let mut first_error = None;
        for id in ids {
            if let Err(error) = self.close(&id).await {
                first_error.get_or_insert(error);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    async fn available_client(
        &self,
        provider: &str,
    ) -> Result<&dyn AgentClient, AgentManagerError> {
        let client = self
            .clients
            .get(provider)
            .ok_or_else(|| AgentManagerError::ProviderUnavailable(provider.to_owned()))?;
        if !client.is_available().await.map_err(map_session)? {
            return Err(AgentManagerError::ProviderUnavailable(provider.to_owned()));
        }
        Ok(client.as_ref())
    }

    async fn register_session(
        &mut self,
        agent_id: &str,
        mut session: Box<dyn AgentSession>,
        mut record: PersistedAgentRuntimeRecord,
        creating: bool,
    ) -> Result<PersistedAgentRuntimeRecord, AgentManagerError> {
        let inspected = session.runtime_info().await;
        let runtime_info = match inspected {
            Ok(info) if valid_runtime_info(&info, session.provider(), &record.provider) => info,
            Ok(_) | Err(_) => return Err(self.reject_session(agent_id, session, record).await),
        };
        let persistence = session.persistence().or(record.persistence.clone());
        if persistence.as_ref().is_some_and(|handle| {
            handle.provider != record.provider
                || runtime_info
                    .session_id
                    .as_ref()
                    .is_some_and(|session_id| session_id != &handle.session_id)
        }) {
            return Err(self.reject_session(agent_id, session, record).await);
        }
        record.last_mode_id.clone_from(&runtime_info.mode_id);
        record.runtime_info = Some(runtime_info);
        record.persistence = persistence;
        record.last_status = AgentRuntimeStatus::Idle;
        record.last_error = None;
        let stored = if creating {
            self.registry.upsert(&record).map(|()| Some(record.clone()))
        } else {
            self.registry.update(agent_id, &|current| {
                let mut next = current.clone();
                next.runtime_info.clone_from(&record.runtime_info);
                next.last_mode_id.clone_from(&record.last_mode_id);
                next.persistence.clone_from(&record.persistence);
                next.last_status = AgentRuntimeStatus::Idle;
                next.last_error = None;
                next
            })
        };
        if let Ok(Some(current)) = stored {
            record = current;
        } else {
            let close_result = session.close().await;
            if close_result.is_err() {
                self.live.insert(
                    agent_id.to_owned(),
                    LiveAgent {
                        session,
                        record,
                        registered: false,
                        turn: None,
                        last_message: None,
                        pending: None,
                    },
                );
                return Err(AgentManagerError::Session);
            }
            return Err(AgentManagerError::Registry);
        }
        self.live.insert(
            agent_id.to_owned(),
            LiveAgent {
                session,
                record: record.clone(),
                registered: true,
                turn: None,
                last_message: None,
                pending: None,
            },
        );
        Ok(record)
    }

    async fn reject_session(
        &mut self,
        agent_id: &str,
        mut session: Box<dyn AgentSession>,
        record: PersistedAgentRuntimeRecord,
    ) -> AgentManagerError {
        if session.close().await.is_err() {
            self.live.insert(
                agent_id.to_owned(),
                LiveAgent {
                    session,
                    record,
                    registered: false,
                    turn: None,
                    last_message: None,
                    pending: None,
                },
            );
        }
        AgentManagerError::Session
    }
}

fn valid_runtime_info(
    info: &StoredAgentRuntimeInfo,
    session_provider: &str,
    expected: &str,
) -> bool {
    info.provider == expected && session_provider == expected
}

fn validate_identity(agent_id: &str, spec: &AgentSessionSpec) -> Result<(), AgentManagerError> {
    let fields = [agent_id, spec.provider.as_str(), spec.cwd.as_str()];
    if agent_id.len() > 128
        || fields.iter().any(|value| {
            value.is_empty() || value.len() > 4096 || value.chars().any(char::is_control)
        })
    {
        return Err(AgentManagerError::InvalidRequest);
    }
    Ok(())
}

fn now_timestamp() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

const fn map_registry(
    _: crate::ports::agent_runtime::AgentRuntimeRegistryError,
) -> AgentManagerError {
    AgentManagerError::Registry
}

const fn map_session(_: AgentSessionError) -> AgentManagerError {
    AgentManagerError::Session
}

#[cfg(test)]
mod tests;
