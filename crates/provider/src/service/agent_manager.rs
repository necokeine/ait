//! Provider session registration and recovery for the independent server.

mod auto_archive;
mod controls;
mod delivery;
mod generated_titles;
pub(crate) mod native_sessions;
mod resume;
mod streaming;
mod titles;

use std::collections::BTreeMap;

use chrono::{SecondsFormat, Utc};
use domain::agent_runtime::{
    AgentRuntimeStatus, PersistedAgentRuntimeRecord, StoredAgentConfig, StoredAgentRuntimeInfo,
};
use metadata::protocol::session::SessionEventKind;
use metadata::service::session::SessionEvents;
use serde_json::json;

use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::ports::agent_session::{
    AgentClient, AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionSpec,
    AgentTurnEvent,
};
use crate::protocol::agent_config::ConfigPatch;

enum Registration<'a> {
    Create,
    Read,
    Restore(&'a crate::protocol::resume::Overrides),
}

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
    last_message_observer: Option<tokio::sync::watch::Sender<Option<String>>>,
    latest_turn: Option<String>,
    pending: Option<AgentTurnEvent>,
    pending_runtime: Option<StoredAgentRuntimeInfo>,
    pending_input_at: Option<String>,
    exclusive: bool,
    interruption_requested: bool,
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
    events: SessionEvents,
    timeline: Option<crate::storage::timeline::Timeline>,
    catalog: super::provider_catalog::Catalog,
    creations: metadata::service::creation::Creations,
    loaded_timelines: std::collections::BTreeSet<String>,
    generated_titles: generated_titles::Titles,
    workspace_names: Option<metadata::service::workspace_names::WorkspaceNames>,
    auto_archives: auto_archive::AutoArchives,
}

impl AgentManager {
    pub(crate) fn validate_provider(&self, provider: &str) -> Result<(), AgentManagerError> {
        if self.clients.contains_key(provider) {
            Ok(())
        } else {
            Err(AgentManagerError::ProviderUnavailable(provider.to_owned()))
        }
    }

    /// Compose the manager with a durable Agent runtime registry.
    #[must_use]
    pub fn new(registry: Box<dyn AgentRuntimeRegistry>) -> Self {
        Self {
            registry,
            clients: BTreeMap::new(),
            live: BTreeMap::new(),
            events: SessionEvents::default(),
            timeline: None,
            catalog: super::provider_catalog::Catalog::default(),
            creations: metadata::service::creation::Creations::default(),
            loaded_timelines: std::collections::BTreeSet::new(),
            generated_titles: generated_titles::Titles::default(),
            workspace_names: None,
            auto_archives: auto_archive::AutoArchives::default(),
        }
    }

    /// Install the same creation receipt service used by Workspace metadata.
    #[must_use]
    pub fn with_creations(mut self, creations: metadata::service::creation::Creations) -> Self {
        self.creations = creations;
        self
    }

    /// Return the metadata-owned creation coordinator shared with this manager.
    #[must_use]
    pub fn creations(&self) -> metadata::service::creation::Creations {
        self.creations.clone()
    }

    /// Install durable display history before admitting requests or creating native sessions.
    #[must_use]
    pub fn with_timeline(mut self, timeline: crate::storage::timeline::Timeline) -> Self {
        self.timeline = Some(timeline);
        self
    }

    /// Install shared auxiliary generation; foreground sessions retain their own lifecycle.
    #[must_use]
    pub fn with_metadata_generation(
        mut self,
        generator: std::sync::Arc<dyn metadata::ports::generation::MetadataGenerator>,
    ) -> Self {
        self.generated_titles.generator = Some(generator);
        self
    }

    /// Share workspace naming for workspaces created before their first Agent prompt arrives.
    #[must_use]
    pub fn with_workspace_names(
        mut self,
        names: metadata::service::workspace_names::WorkspaceNames,
    ) -> Self {
        self.workspace_names = Some(names);
        self
    }

    /// Return the installed display projection, independent of live session ownership.
    #[must_use]
    pub fn timeline(&self) -> Option<crate::storage::timeline::Timeline> {
        self.timeline.clone()
    }

    /// Discover models and capabilities through the registered native adapters.
    /// # Errors
    /// Returns invalid requests, unavailable adapter or safe discovery/cache errors.
    pub async fn providers(
        &mut self,
        method: &str,
        params: serde_json::Value,
    ) -> Result<serde_json::Value, model::ErrorCode> {
        self.catalog
            .execute(&self.clients, &self.events, method, params)
            .await
    }

    /// Load native history without claiming a writer, once per Agent in this process.
    /// # Errors
    /// Returns unavailable history, missing Agent, or durable projection failures.
    pub async fn load_timeline(&mut self, agent_id: &str) -> Result<(), model::ErrorCode> {
        use model::ErrorCode;
        if self.loaded_timelines.contains(agent_id) {
            return Ok(());
        }
        let timeline = self
            .timeline
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        let record = self
            .registry
            .get(agent_id)
            .map_err(|_| ErrorCode::AgentIo)?
            .ok_or(ErrorCode::AgentNotFound)?;
        let handle = record
            .persistence
            .as_ref()
            .ok_or(ErrorCode::UnsupportedCapability)?;
        if handle
            .metadata
            .as_ref()
            .and_then(|metadata| metadata.get(controls::REPLACEMENT_MARKER))
            == Some(&json!(true))
        {
            let history = self.inspect_native(handle, &record.cwd).await?;
            self.finish_replacement(agent_id, &history)?;
            self.loaded_timelines.insert(agent_id.to_owned());
            return Ok(());
        }
        let client = self
            .clients
            .get(&record.provider)
            .ok_or(ErrorCode::UnsupportedCapability)?;
        if !client.supports_history_replay() {
            self.loaded_timelines.insert(agent_id.to_owned());
            return Ok(());
        }
        let entries = client
            .history(handle, &record.cwd)
            .await
            .map_err(|_| ErrorCode::AgentIo)?;
        timeline.reconcile(agent_id, &record.provider, &entries)?;
        if titles::missing(&record) {
            self.fill_missing_title(agent_id, titles::from_entries(&entries))?;
        }
        self.loaded_timelines.insert(agent_id.to_owned());
        Ok(())
    }

    /// Return the shared connection event service used for committed attention notifications.
    #[must_use]
    pub fn events(&self) -> SessionEvents {
        self.events.clone()
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

    /// Whether an acknowledged interruption still awaits the active turn's terminal event.
    pub(crate) fn interruption_pending(&self, agent_id: &str) -> bool {
        self.live
            .get(agent_id)
            .is_some_and(|agent| agent.turn.is_some() && agent.interruption_requested)
    }

    /// Return the most recently accepted native turn, including after it finishes.
    /// Used to fence connection-owned cancellation and completion reads against later turns.
    #[must_use]
    pub fn latest_turn(&self, agent_id: &str) -> Option<&str> {
        self.live
            .get(agent_id)
            .and_then(|agent| agent.latest_turn.as_deref())
    }

    /// Return the final assistant text from this process's latest completed turn.
    #[must_use]
    pub fn last_message(&self, agent_id: &str) -> Option<&str> {
        self.live
            .get(agent_id)
            .and_then(|agent| agent.last_message.as_deref())
    }

    /// Retain this live turn's final text for an in-flight wait, even after native close.
    pub(crate) fn observe_last_message(
        &mut self,
        agent_id: &str,
    ) -> Option<tokio::sync::watch::Receiver<Option<String>>> {
        let agent = self.live.get_mut(agent_id)?;
        let sender = agent
            .last_message_observer
            .get_or_insert_with(|| tokio::sync::watch::channel(agent.last_message.clone()).0);
        Some(sender.subscribe())
    }

    /// Persist a validated patch for subsequent turns without altering an accepted turn.
    ///
    /// # Errors
    /// Returns validation, unavailable-provider, archived-Agent, or registry errors. All supplied
    /// fields commit together; failed writes leave the running session and durable config intact.
    pub async fn configure(
        &self,
        agent_id: &str,
        patch: &ConfigPatch,
    ) -> Result<(), AgentManagerError> {
        let record = self
            .registry
            .get(agent_id)
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        if record.archived_at.is_some() {
            return Err(AgentManagerError::Busy);
        }
        if matches!(
            patch.mode_id,
            crate::protocol::agent_config::NullableSetting::Clear
        ) {
            return Err(AgentManagerError::InvalidRequest);
        }
        let config = patch.apply(
            record
                .config
                .as_ref()
                .unwrap_or(&StoredAgentConfig::default()),
        );
        self.clients
            .get(&record.provider)
            .ok_or_else(|| AgentManagerError::ProviderUnavailable(record.provider.clone()))?
            .validate_selection(&AgentSessionSpec {
                provider: record.provider.clone(),
                cwd: record.cwd.clone(),
                config,
            })
            .await
            .map_err(|_| AgentManagerError::InvalidRequest)?;
        let now = now_timestamp();
        self.registry
            .update(agent_id, &|current| {
                let mut next = current.clone();
                next.config = Some(
                    patch.apply(
                        current
                            .config
                            .as_ref()
                            .unwrap_or(&StoredAgentConfig::default()),
                    ),
                );
                next.last_mode_id = next
                    .config
                    .as_ref()
                    .and_then(|config| config.mode_id.clone());
                next.updated_at.clone_from(&now);
                next
            })
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        Ok(())
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
        self.create_with_environment(
            agent_id,
            spec,
            registration,
            &crate::ports::environment::AgentEnvironment::default(),
        )
        .await
    }

    /// Create a native session with private per-process environment overrides.
    ///
    /// Environment values remain in the native adapter's memory and are never put in the registry.
    /// # Errors
    /// Returns identity, admission, native launch or registration failures with cleanup.
    pub async fn create_with_environment(
        &mut self,
        agent_id: &str,
        spec: &AgentSessionSpec,
        registration: AgentRegistration,
        environment: &crate::ports::environment::AgentEnvironment,
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
        let session = client
            .create_session_with_environment(spec, environment)
            .await
            .map_err(map_session)?;
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
            title_origin: None,
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
        let record = self
            .register_session(agent_id, session, record, Registration::Create)
            .await?;
        self.loaded_timelines.insert(agent_id.to_owned());
        Ok(record)
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
        let mut record = self
            .registry
            .get(agent_id)
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        if record
            .persistence
            .as_ref()
            .and_then(|handle| handle.metadata.as_ref())
            .and_then(|metadata| metadata.get(controls::REPLACEMENT_MARKER))
            == Some(&json!(true))
        {
            self.load_timeline(agent_id)
                .await
                .map_err(|_| AgentManagerError::Registry)?;
            record = self
                .registry
                .get(agent_id)
                .map_err(map_registry)?
                .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        }
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
        self.register_session(agent_id, session, record, Registration::Read)
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
        if agent.registered {
            streaming::persist_handle(self.registry.as_ref(), agent_id, agent)?;
        }
        agent.session.close().await.map_err(map_session)?;
        if agent.registered {
            for child in agent.session.subagents() {
                controls::publish_subagent(
                    self.timeline.as_ref(),
                    &self.events,
                    agent,
                    &crate::ports::controls::SubagentEvent::Upsert(child),
                )
                .map_err(|_| AgentManagerError::Registry)?;
            }
        }
        if !agent.registered {
            self.live.remove(agent_id);
            return Ok(());
        }
        // Metadata and attention may have changed through other narrow ports. Never replace
        // their latest record with the manager's creation-time snapshot or resurrect a deletion.
        let durable_permissions = agent.session.persistence().is_some_and(|handle| {
            self.clients
                .get(&agent.record.provider)
                .is_some_and(|client| !client.persisted_permissions(&handle).is_empty())
        });
        self.registry
            .update(agent_id, &|current| {
                let mut next = current.clone();
                next.last_status = AgentRuntimeStatus::Closed;
                if next.attention_reason
                    == Some(domain::agent_runtime::AgentAttentionReason::Permission)
                    && !durable_permissions
                {
                    next.requires_attention = false;
                    next.attention_reason = None;
                    next.attention_timestamp = None;
                }
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
        self.send_input(agent_id, &crate::protocol::prompt::AgentPrompt::text(text))
            .await
    }

    /// Submit one complete rich prompt while retaining exclusive native-turn admission.
    /// # Errors
    /// Returns invalid input, busy ownership, native admission or persistence failures.
    pub async fn send_input(
        &mut self,
        agent_id: &str,
        prompt: &crate::protocol::prompt::AgentPrompt,
    ) -> Result<(), AgentManagerError> {
        prompt
            .validate()
            .map_err(|_| AgentManagerError::InvalidRequest)?;
        if self.timeline.is_some() && !self.loaded_timelines.contains(agent_id) {
            self.load_timeline(agent_id)
                .await
                .map_err(|_| AgentManagerError::Registry)?;
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
                if titles::missing(current) {
                    next.title = titles::from_prompt(&prompt.text);
                    next.title_origin = next
                        .title
                        .as_ref()
                        .map(|_| domain::agent_runtime::TitleOrigin::Prompt);
                }
                next
            })
            .map_err(map_registry)?
            .ok_or_else(|| AgentManagerError::NotFound(agent_id.to_owned()))?;
        let agent = self
            .live
            .get_mut(agent_id)
            .ok_or(AgentManagerError::Session)?;
        agent.last_message = None;
        agent.last_message_observer = None;
        if let Some(names) = &self.workspace_names
            && let Some(workspace) = record.workspace_id.clone()
            && !record.internal
        {
            let context = self
                .timeline
                .as_ref()
                .and_then(|timeline| timeline.first_user_text(agent_id).ok().flatten())
                .unwrap_or_else(|| prompt.text.clone());
            names.schedule(
                workspace,
                context,
                Some(metadata::ports::generation::MetadataSelection {
                    provider: record.provider.clone(),
                    model: record
                        .config
                        .as_ref()
                        .and_then(|config| config.model.clone()),
                    thinking_option_id: record
                        .config
                        .as_ref()
                        .and_then(|config| config.thinking_option_id.clone()),
                }),
            );
        }
        let config = record.config.unwrap_or_default();
        if let Ok(turn) = agent.session.start_input(prompt, &config).await {
            agent.latest_turn = Some(turn.clone());
            agent.turn = Some(turn);
            match agent.session.runtime_info().await {
                Ok(info) => agent.pending_runtime = Some(info),
                Err(_) => agent.pending = Some(AgentTurnEvent::Failed),
            }
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
        if let Some(timeline) = &self.timeline {
            timeline
                .cancel_inputs(agent_id)
                .map_err(|_| AgentManagerError::Registry)?;
        }
        if self.active_turn(agent_id).is_none() {
            self.registry
                .update(agent_id, &|current| {
                    let mut next = current.clone();
                    if next.last_status == AgentRuntimeStatus::Running {
                        next.last_status = AgentRuntimeStatus::Idle;
                    }
                    next
                })
                .map_err(map_registry)?;
        }
        let Some(agent) = self.live.get_mut(agent_id) else {
            return Ok(());
        };
        if let Some(turn) = &agent.turn {
            agent.session.cancel_turn(turn).await.map_err(map_session)?;
            agent.interruption_requested = true;
        }
        agent.session.cancel_pending().await.map_err(map_session)?;
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
            persist_runtime(self.registry.as_ref(), &id, agent)?;
            streaming::persist_handle(self.registry.as_ref(), &id, agent)?;
            streaming::persist_input(self.registry.as_ref(), &id, agent)?;
            streaming::drain(
                self.registry.as_ref(),
                self.timeline.as_ref(),
                &self.events,
                &id,
                agent,
            )?;
            streaming::persist_handle(self.registry.as_ref(), &id, agent)?;
            let Some(event) = &agent.pending else {
                continue;
            };
            if matches!(event, AgentTurnEvent::Failed) {
                agent.session.close().await.map_err(map_session)?;
                controls::publish_children(self.timeline.as_ref(), &self.events, agent)?;
            }
            let failed = matches!(event, AgentTurnEvent::Failed);
            let cancelled = matches!(event, AgentTurnEvent::Cancelled);
            let permission = !agent.session.pending_permissions().is_empty();
            let queued = self
                .timeline
                .as_ref()
                .map(|timeline| timeline.has_queued_input(&id))
                .transpose()
                .map_err(|_| AgentManagerError::Registry)?
                .unwrap_or(false);
            let now = now_timestamp();
            let committed = self
                .registry
                .update(&id, &|current| {
                    let mut next = current.clone();
                    next.last_status = if queued {
                        AgentRuntimeStatus::Running
                    } else if failed {
                        AgentRuntimeStatus::Error
                    } else {
                        AgentRuntimeStatus::Idle
                    };
                    next.last_error = failed.then(|| "Provider execution failed".to_owned());
                    next.updated_at.clone_from(&now);
                    next.last_activity_at = Some(now.clone());
                    next.requires_attention = permission || (!cancelled && !queued);
                    next.attention_reason = next.requires_attention.then_some(if permission {
                        domain::agent_runtime::AgentAttentionReason::Permission
                    } else if failed {
                        domain::agent_runtime::AgentAttentionReason::Error
                    } else {
                        domain::agent_runtime::AgentAttentionReason::Finished
                    });
                    next.attention_timestamp = next.requires_attention.then(|| now.clone());
                    next
                })
                .map_err(map_registry)?;
            if let AgentTurnEvent::Completed(message) = event {
                agent.last_message.clone_from(message);
            }
            streaming::publish_terminal(
                self.timeline.as_ref(),
                &id,
                agent,
                event,
                committed.as_ref(),
            );
            agent.turn = None;
            agent.pending = None;
            agent.exclusive = false;
            agent.interruption_requested = false;
            if !permission
                && !cancelled
                && !queued
                && committed
                    .as_ref()
                    .is_some_and(|record| record.archived_at.is_none() && !record.internal)
            {
                publish_terminal_attention(&self.events, &id, &now, failed);
            }
            if failed && committed.as_ref().is_some_and(|record| !record.internal) {
                publish_failure_activity(&self.events, &id, &now);
            }
            if failed {
                self.live.remove(&id);
            }
            self.auto_archives.completed(&id);
        }
        self.archive_finished().await
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
        self.generated_titles.close().await;
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
        registration: Registration<'_>,
    ) -> Result<PersistedAgentRuntimeRecord, AgentManagerError> {
        let inspected = session.runtime_info().await;
        let mut runtime_info = match inspected {
            Ok(info) if valid_runtime_info(&info, session.provider(), &record.provider) => info,
            Ok(_) | Err(_) => return Err(self.reject_session(agent_id, session, record).await),
        };
        streaming::preserve_usage(&mut runtime_info, record.runtime_info.as_ref());
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
        let stored = if matches!(registration, Registration::Create) {
            self.registry.upsert(&record).map(|()| Some(record.clone()))
        } else {
            self.registry.update(agent_id, &|current| {
                let mut next = current.clone();
                if let Registration::Restore(overrides) = &registration {
                    overrides.apply(&mut next);
                    next.updated_at = now_timestamp();
                }
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
                        last_message_observer: None,
                        latest_turn: None,
                        pending: None,
                        pending_runtime: None,
                        pending_input_at: None,
                        exclusive: false,
                        interruption_requested: false,
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
                last_message_observer: None,
                latest_turn: None,
                pending: None,
                pending_runtime: None,
                pending_input_at: None,
                exclusive: false,
                interruption_requested: false,
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
                    last_message_observer: None,
                    latest_turn: None,
                    pending: None,
                    pending_runtime: None,
                    pending_input_at: None,
                    exclusive: false,
                    interruption_requested: false,
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

fn publish_failure_activity(events: &SessionEvents, agent_id: &str, timestamp: &str) {
    events.publish(
        SessionEventKind::ActivityLog,
        &json!({
            "id": uuid::Uuid::new_v4().to_string(),
            "timestamp": timestamp,
            "type": "error",
            "content": "Provider execution failed",
            "metadata": {"agentId": agent_id},
        }),
    );
}

fn publish_terminal_attention(
    events: &SessionEvents,
    agent_id: &str,
    timestamp: &str,
    failed: bool,
) {
    events.publish(
        SessionEventKind::AgentAttention,
        &json!({
            "agentId": agent_id,
            "reason": if failed { "error" } else { "finished" },
            "timestamp": timestamp,
        }),
    );
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

fn persist_runtime(
    registry: &dyn AgentRuntimeRegistry,
    id: &str,
    agent: &mut LiveAgent,
) -> Result<(), AgentManagerError> {
    if let Some(info) = &agent.pending_runtime {
        registry
            .update(id, &|current| {
                let mut next = current.clone();
                let mut info = info.clone();
                streaming::preserve_usage(&mut info, current.runtime_info.as_ref());
                next.runtime_info = Some(info);
                next
            })
            .map_err(map_registry)?;
        agent.pending_runtime = None;
    }
    Ok(())
}
