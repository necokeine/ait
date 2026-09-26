//! Provider-owned Agent sessions used by the independent server.
//!
//! These contracts mirror the create/resume, runtime inspection, persistence and close portion
//! of Paseo's `AgentClient` and `AgentSession`. A concrete provider adapter owns native execution.

use std::fmt::Debug;
use std::future::Future;
use std::pin::Pin;

use server_domain::agent_runtime::{
    AgentPersistenceHandle, StoredAgentConfig, StoredAgentRuntimeInfo,
};

/// A sendable provider operation borrowing its client or session.
pub type AgentSessionFuture<'a, T> =
    Pin<Box<dyn Future<Output = Result<T, AgentSessionError>> + Send + 'a>>;

/// Provider failure safe to expose at the application boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum AgentSessionError {
    /// The provider cannot currently start or resume a session.
    #[error("provider is unavailable")]
    Unavailable,
    /// The provider failed an operation or returned inconsistent facts.
    #[error("provider session operation failed")]
    Failed,
    /// The provider rejected a request while the transport remains usable.
    #[error("provider rejected the session operation")]
    Rejected,
}

/// Provider-independent inputs required to construct or resume a native session.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentSessionSpec {
    /// Provider identity.
    pub provider: String,
    /// Native session working directory.
    pub cwd: String,
    /// Persistable provider configuration.
    pub config: StoredAgentConfig,
}

/// Whether a resumed native session may accept a new foreground turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentResumePurpose {
    /// Resume an active Agent for interaction.
    Interactive,
    /// Read the history of an archived Agent without claiming an interactive writer.
    History,
}

/// Native foreground-turn progress or result. This is not a host Run or Message tree.
#[derive(Debug, Clone, PartialEq)]
pub enum AgentTurnEvent {
    /// A native autonomous foreground turn started, for example while pursuing a Codex goal.
    Started(String),
    /// Native child progress or lifecycle, independent of the foreground turn's completion.
    Subagent(super::controls::SubagentEvent),
    /// One incremental display event with a retry-stable observation identity.
    Progress {
        /// Unique identity for this observation, distinct from the native item identity.
        observation: String,
        /// Text delta or tool snapshot, keyed by the eventual completed native item.
        entry: crate::protocol::timeline::NativeItem,
    },
    /// A live native approval; the request payload is never written to display history.
    PermissionRequested(serde_json::Value),
    /// The native provider withdrew or resolved a previously published permission request.
    PermissionResolved(String),
    /// One immutable normalized item completed by the native provider.
    Timeline(crate::protocol::timeline::NativeItem),
    /// Complete current usage snapshot; consumers replace rather than sum repeated observations.
    Usage(crate::protocol::usage::AgentUsage),
    /// Native initialization resolved the session's actual model or runtime settings.
    RuntimeInfo(StoredAgentRuntimeInfo),
    /// The native provider drained its turn successfully.
    Completed(Option<String>),
    /// The native provider acknowledged interruption.
    Cancelled,
    /// The native provider failed, exited, or requested unsupported interaction.
    Failed,
}

/// Live provider session. Closing releases resources without deleting native history.
pub trait AgentSession: Debug + Send {
    /// Native foreground work was accepted but its start notification has not arrived yet.
    fn pending_foreground(&self) -> bool {
        false
    }

    /// Withdraw accepted autonomous work that has not announced a foreground turn yet.
    /// # Errors
    /// Returns native transport failures; an uncertain cancellation is never reported successful.
    fn cancel_pending(&mut self) -> AgentSessionFuture<'_, ()> {
        Box::pin(async { Ok(()) })
    }

    /// Execute a provider command that does not interrupt or replace the active input turn.
    /// # Errors
    /// Returns rejected before mutation for unsupported commands, or uncertain transport failure.
    fn out_of_band<'a>(
        &'a mut self,
        _prompt: &'a crate::protocol::prompt::AgentPrompt,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Rejected) })
    }
    /// Current verified native descendants observed by this live session.
    fn subagents(&self) -> Vec<super::controls::NativeSubagent> {
        Vec::new()
    }
    /// Prepare a session-scoped question answer that requires durable user-input admission.
    /// This has no side effects; call `respond_permission` only after admission succeeds.
    /// # Errors
    /// Returns rejected for stale or invalid answers.
    fn prepare_permission_response(
        &self,
        _id: &str,
        _response: &serde_json::Value,
    ) -> Result<Option<crate::protocol::prompt::AgentPrompt>, AgentSessionError> {
        Ok(None)
    }

    /// Configuration explicitly authorized by an approval, applied before its follow-up input.
    /// # Errors
    /// Returns rejected for stale or invalid approval responses.
    fn permission_config_patch(
        &self,
        _id: &str,
        _response: &serde_json::Value,
    ) -> Result<Option<crate::protocol::agent_config::ConfigPatch>, AgentSessionError> {
        Ok(None)
    }

    /// Inspect pending approvals for reconnecting clients; IDs expire with this native session.
    fn pending_permissions(&self) -> Vec<serde_json::Value> {
        Vec::new()
    }

    /// Answer one pending approval without extending its authority to later calls.
    /// # Errors
    /// Returns rejected for stale/invalid answers, or a native transport failure.
    fn respond_permission<'a>(
        &'a mut self,
        _id: &'a str,
        _response: &'a serde_json::Value,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Rejected) })
    }
    /// Return the provider that owns this session.
    fn provider(&self) -> &str;

    /// Inspect current provider facts after creation or resume.
    ///
    /// # Errors
    /// Returns a provider error when the native runtime cannot be inspected.
    fn runtime_info(&mut self) -> AgentSessionFuture<'_, StoredAgentRuntimeInfo>;

    /// Describe the native identity required to resume this session, if available.
    fn persistence(&self) -> Option<AgentPersistenceHandle>;

    /// Start one text-only foreground turn and return its native identity.
    ///
    /// # Errors
    /// Returns an error for a busy, history-only, or unsupported session.
    fn start_turn<'a>(
        &'a mut self,
        _text: &'a str,
        _config: &'a StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Submit one validated rich input, preserving attachment order and output constraints.
    /// # Errors
    /// Legacy implementations reject non-text input instead of silently dropping it.
    fn start_input<'a>(
        &'a mut self,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
        config: &'a StoredAgentConfig,
    ) -> AgentSessionFuture<'a, String> {
        Box::pin(async move {
            prompt.validate()?;
            if !prompt.is_plain_text() {
                return Err(AgentSessionError::Rejected);
            }
            self.start_turn(&prompt.text, config).await
        })
    }

    /// Append text to exactly `turn_id`; success means admission at the adapter's native boundary.
    /// Codex acknowledges its RPC; Claude accepts input on its live SDK stream and echoes it later.
    /// # Errors
    /// Returns rejected for unsupported or definitively refused input. Other errors leave
    /// admission uncertain and must never trigger an automatic resubmission.
    fn steer_turn<'a>(
        &'a mut self,
        _turn_id: &'a str,
        _text: &'a str,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Rejected) })
    }

    /// Steer with complete rich input; unsupported providers must reject without partial admission.
    /// # Errors
    /// Returns definitive rejection or uncertain provider/transport failure.
    fn steer_input<'a>(
        &'a mut self,
        turn_id: &'a str,
        prompt: &'a crate::protocol::prompt::AgentPrompt,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            prompt.validate()?;
            if !prompt.is_plain_text() {
                return Err(AgentSessionError::Rejected);
            }
            self.steer_turn(turn_id, &prompt.text).await
        })
    }

    /// Interrupt the identified native turn without deleting session history.
    ///
    /// # Errors
    /// Returns an error when the provider cannot acknowledge interruption.
    fn cancel_turn<'a>(&'a mut self, _turn_id: &'a str) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Read a queued progress or terminal event without waiting for new provider output.
    ///
    /// # Errors
    /// Returns an error if the provider connection has failed.
    fn poll_turn(&mut self) -> Result<Option<AgentTurnEvent>, AgentSessionError> {
        Ok(None)
    }

    /// Release the live native runtime without deleting its durable history.
    ///
    /// # Errors
    /// Returns a provider error if runtime ownership remains uncertain.
    fn close(&mut self) -> AgentSessionFuture<'_, ()>;
}

/// Factory and availability boundary for one independent provider adapter.
pub trait AgentClient: Debug + Send + Sync {
    /// Whether this text names a provider command that bypasses foreground input scheduling.
    fn handles_out_of_band(&self, _text: &str) -> bool {
        false
    }
    /// Restore durable session-scoped questions without opening a native writer.
    /// Native tool approvals are deliberately excluded because their transport has expired.
    fn persisted_permissions(&self, _handle: &AgentPersistenceHandle) -> Vec<serde_json::Value> {
        Vec::new()
    }
    /// Describe selectable modes/features and implemented native control flags without I/O.
    fn settings(&self, _config: &StoredAgentConfig) -> serde_json::Value {
        serde_json::json!({"availableModes":[],"features":[],"capabilities":{}})
    }
    /// Validate dynamic selections before committing configuration.
    /// # Errors
    /// Returns unsupported settings or unavailable native discovery.
    fn validate_selection<'a>(&'a self, spec: &'a AgentSessionSpec) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move { self.validate_config(&spec.config) })
    }

    /// Inspect launch and authentication availability without exposing credentials.
    /// # Errors
    /// Returns unsupported diagnostics or an inspection failure.
    fn diagnostic(&self) -> AgentSessionFuture<'_, String> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Read native account quota facts, not local estimates of token consumption.
    /// # Errors
    /// Returns unsupported accounting or a native query failure.
    fn usage(&self) -> AgentSessionFuture<'_, serde_json::Value> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// List commands and skills available in a validated working directory.
    /// # Errors
    /// Returns malformed discovery or unavailable native support.
    fn commands<'a>(
        &'a self,
        _spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Vec<serde_json::Value>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Discover native child-parent links without registering host Agents.
    /// # Errors
    /// Returns unavailable or incomplete native discovery.
    fn subagents<'a>(
        &'a self,
        _cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<super::controls::NativeSubagent>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Derive a new native history before the selected user message, preserving the old thread.
    /// # Errors
    /// Returns an invalid target, active history, or native fork/rollback failure.
    fn rewind<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _spec: &'a AgentSessionSpec,
        _message_id: &'a str,
    ) -> AgentSessionFuture<'a, super::native_history::SessionHistory> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Restore native tracked-file checkpoints at a verified user message, without rewriting history.
    /// # Errors
    /// Returns unsupported capability, an invalid checkpoint, or a native restore failure.
    fn rewind_files<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _spec: &'a AgentSessionSpec,
        _message_id: &'a str,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }
    /// Return the provider identity served by this client.
    fn provider(&self) -> &str;

    /// Validate persisted next-turn configuration without launching or mutating a session.
    ///
    /// # Errors
    /// Returns an error for unsupported settings. Validation does not verify model availability.
    fn validate_config(&self, _config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
        Err(AgentSessionError::Unavailable)
    }

    /// Check whether the provider can launch sessions now.
    ///
    /// # Errors
    /// Returns a provider error if availability cannot be determined.
    fn is_available(&self) -> AgentSessionFuture<'_, bool>;

    /// Discover native models and the modes/features supported by this adapter in `cwd`.
    /// # Errors
    /// Returns unavailable or safe native discovery failures.
    fn discover<'a>(
        &'a self,
        _cwd: &'a str,
    ) -> AgentSessionFuture<'a, crate::protocol::provider::Details> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Read existing native history without resuming or claiming an interactive writer.
    /// # Errors
    /// Returns unavailable, missing-history or malformed-provider errors.
    fn history<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<crate::protocol::timeline::NativeItem>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Discover existing sessions without resuming a writer or submitting input.
    /// # Errors
    /// Returns unsupported discovery, malformed native responses or transport failures.
    fn list_sessions<'a>(
        &'a self,
        _options: &'a super::native_history::ListOptions,
    ) -> AgentSessionFuture<'a, Vec<super::native_history::SessionDescriptor>> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Inspect one complete native history and configuration without claiming a writer.
    /// # Errors
    /// Returns unavailable history, mismatched identity, partial history or transport failures.
    fn inspect_session<'a>(
        &'a self,
        _handle: &'a AgentPersistenceHandle,
        _cwd: &'a str,
    ) -> AgentSessionFuture<'a, super::native_history::SessionHistory> {
        Box::pin(async { Err(AgentSessionError::Unavailable) })
    }

    /// Create a new native session using `spec`.
    ///
    /// # Errors
    /// Returns a provider error when construction fails.
    fn create_session<'a>(
        &'a self,
        spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>>;

    /// Resume the native session identified by `handle` for `purpose`.
    ///
    /// # Errors
    /// Returns a provider error when resume fails or the handle is no longer valid.
    fn resume_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>>;
}
