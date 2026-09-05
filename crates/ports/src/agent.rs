use std::{collections::BTreeSet, path::PathBuf, pin::Pin, sync::Arc};

use ait_domain::{
    AgentId, CostMicros, DomainMetadata, DurationMs, ProjectedMessage, RunAttemptId, RunId,
    RunUsage, SubMessage,
};
use async_trait::async_trait;
use futures_core::Stream;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;

/// Ordered, transport-neutral output from one Agent invocation.
pub type AgentEventStream =
    Pin<Box<dyn Stream<Item = Result<AgentEvent, AgentError>> + Send + 'static>>;

/// Stable correlation identity for one low-level Agent call.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentCallId(String);

impl AgentCallId {
    /// Creates an externally assigned call identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Stable correlation identity for a direct application operation.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct OperationId(String);

impl OperationId {
    /// Creates an externally assigned operation identity.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the string representation.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Reference to host-managed credential material.
///
/// This value identifies a secret but never contains the resolved secret.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CredentialRef(String);

impl CredentialRef {
    /// Creates a reference such as a keychain or connection-block key.
    #[must_use]
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Returns the reference string without resolving it.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// A behavior an immutable Agent revision may expose.
#[derive(Clone, Copy, Debug, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentCapability {
    /// Produces text content.
    Text,
    /// Produces an ordered event stream.
    Streaming,
    /// Accepts an immutable root-to-head Message path.
    MessagePath,
    /// Produces a complete domain Message proposal.
    MessageOutput,
    /// Produces a structured JSON object.
    StructuredOutput,
    /// Enforces a caller-provided JSON schema.
    JsonSchema,
    /// Emits tool calls for execution by the host Run coordinator.
    HostManagedTools,
    /// May emit parallel host-managed tool calls.
    ParallelTools,
    /// Reports activity managed inside a complete coding harness.
    HarnessManagedActivity,
    /// May read an explicitly granted workspace root.
    WorkspaceRead,
    /// May modify an explicitly granted workspace root.
    WorkspaceWrite,
    /// Supports an interactive approval boundary.
    Approval,
    /// Can resume from compatible opaque checkpoints.
    Checkpoint,
    /// Reports normalized usage.
    Usage,
}

impl AgentCapability {
    /// Returns the stable wire-independent capability name.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Streaming => "streaming",
            Self::MessagePath => "message_path",
            Self::MessageOutput => "message_output",
            Self::StructuredOutput => "structured_output",
            Self::JsonSchema => "json_schema",
            Self::HostManagedTools => "host_managed_tools",
            Self::ParallelTools => "parallel_tools",
            Self::HarnessManagedActivity => "harness_managed_activity",
            Self::WorkspaceRead => "workspace_read",
            Self::WorkspaceWrite => "workspace_write",
            Self::Approval => "approval",
            Self::Checkpoint => "checkpoint",
            Self::Usage => "usage",
        }
    }
}

/// Deterministic capability set for an Agent revision or adapter.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AgentCapabilities(BTreeSet<AgentCapability>);

impl AgentCapabilities {
    /// Creates a capability set from unique values.
    #[must_use]
    pub fn new(capabilities: impl IntoIterator<Item = AgentCapability>) -> Self {
        Self(capabilities.into_iter().collect())
    }

    /// Returns whether the set contains a capability.
    #[must_use]
    pub fn contains(&self, capability: AgentCapability) -> bool {
        self.0.contains(&capability)
    }

    /// Returns the capabilities required here but absent from `available`.
    #[must_use]
    pub fn missing_from(&self, available: &Self) -> Vec<AgentCapability> {
        self.0.difference(&available.0).copied().collect()
    }

    fn insert(&mut self, capability: AgentCapability) {
        self.0.insert(capability);
    }
}

/// Immutable, non-secret configuration fixed before an invocation begins.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentRevisionSnapshot {
    /// Selected Agent identity.
    pub agent_id: AgentId,
    /// Monotonic Agent-local revision.
    pub revision: u64,
    /// Composition-root lookup key; never a concrete protocol type.
    pub adapter_key: String,
    /// Provider-neutral model identifier.
    pub model: String,
    /// Optional non-secret endpoint override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    /// Optional reference resolved only for the minimum invocation scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<CredentialRef>,
    /// Capabilities explicitly granted to this revision.
    #[serde(default)]
    pub capabilities: AgentCapabilities,
    /// Non-secret parameters fixed with this revision.
    #[serde(default)]
    pub parameters: DomainMetadata,
    /// Digest of the canonical non-secret configuration.
    pub config_digest: String,
}

impl AgentRevisionSnapshot {
    fn validate(&self) -> Result<(), AgentError> {
        if self.agent_id.as_str().is_empty()
            || self.revision == 0
            || self.adapter_key.trim().is_empty()
            || self.model.trim().is_empty()
            || !is_sha256(&self.config_digest)
            || self
                .credential_ref
                .as_ref()
                .is_some_and(|reference| reference.as_str().trim().is_empty())
        {
            return Err(AgentError::invalid_configuration(
                "agent revision snapshot contains an invalid non-secret field",
            ));
        }
        Ok(())
    }
}

/// Why the application is making this low-level call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentPurpose {
    /// One attempt inside a durable domain Run.
    RunAttempt {
        /// Durable Run identity.
        run_id: RunId,
        /// Durable attempt identity.
        attempt_id: RunAttemptId,
    },
    /// A short, pure application operation that creates no Run or Message.
    Direct {
        /// Idempotency and audit identity owned by the application operation.
        operation_id: OperationId,
        /// Stable class of direct generation task.
        kind: DirectTaskKind,
    },
}

/// Application-owned direct generation task classes.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DirectTaskKind {
    /// Session display title and searchable description.
    SessionMetadata,
    /// Git commit subject and body generated from an already fixed input.
    CommitMessage,
    /// Another stable application task key.
    Other(String),
}

/// Provider-neutral input to one invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentInput {
    /// Immutable domain projection ordered from root to current head.
    MessagePath(Vec<ProjectedMessage>),
    /// A bounded caller-assembled prompt.
    Prompt(String),
}

/// Workspace permission granted to one invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WorkspaceAccess {
    /// No workspace access.
    None,
    /// Read access to one canonical Project root.
    ReadOnly {
        /// Canonical absolute Project root.
        root: PathBuf,
    },
    /// Read and write access to one canonical Project root.
    WorkspaceWrite {
        /// Canonical absolute Project root.
        root: PathBuf,
    },
}

/// Ownership model for tools used by one invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentToolMode {
    /// Tools are disabled.
    Disabled,
    /// Tool calls are proposed to and executed by the host Run coordinator.
    HostManaged,
    /// Internal coding-harness activity is executed by the adapter.
    HarnessManaged,
}

/// Approval boundary granted to one invocation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentApprovalMode {
    /// Approval requests are forbidden and must fail closed.
    Denied,
    /// The host supplies an explicit approval decision.
    HostManaged,
}

/// Explicit capability and permission profile for one call.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentExecutionProfile {
    /// Granted workspace boundary.
    pub workspace: WorkspaceAccess,
    /// Selected tool ownership model.
    pub tool_mode: AgentToolMode,
    /// Selected approval boundary.
    pub approval: AgentApprovalMode,
    /// Whether more than one host-managed tool may be proposed together.
    pub allow_parallel_tools: bool,
    /// Whether normalized harness activity events are requested.
    pub emit_activity: bool,
    /// Whether at least one normalized usage event is required.
    pub require_usage: bool,
}

impl Default for AgentExecutionProfile {
    fn default() -> Self {
        Self {
            workspace: WorkspaceAccess::None,
            tool_mode: AgentToolMode::Disabled,
            approval: AgentApprovalMode::Denied,
            allow_parallel_tools: false,
            emit_activity: false,
            require_usage: false,
        }
    }
}

/// Complete output shape the caller will validate and consume.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentOutputContract {
    /// One complete assistant Message proposal.
    Message,
    /// Plain text assembled from text deltas.
    Text,
    /// A single JSON object.
    JsonObject,
    /// A single JSON value conforming to the supplied schema.
    JsonSchema(Value),
}

/// Hard limits applying to one low-level invocation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentCallLimits {
    /// Wall-clock deadline relative to invocation start.
    pub timeout: DurationMs,
    /// Maximum serialized input size.
    pub max_input_bytes: u64,
    /// Maximum assembled output size.
    pub max_output_bytes: u64,
    /// Optional model output-token cap.
    pub max_output_tokens: Option<u64>,
    /// Optional cost cap owned by the caller.
    pub max_cost: Option<CostMicros>,
}

impl Default for AgentCallLimits {
    fn default() -> Self {
        Self {
            timeout: DurationMs(30_000),
            max_input_bytes: 128 * 1024,
            max_output_bytes: 1024 * 1024,
            max_output_tokens: None,
            max_cost: None,
        }
    }
}

/// Transport-neutral description of one host-managed tool.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDescriptor {
    /// Stable registered tool name.
    pub name: String,
    /// Human-readable behavior description.
    pub description: String,
    /// JSON schema for canonical arguments.
    pub input_schema: Value,
}

/// Compatibility binding carried beside an opaque checkpoint token.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentCheckpointCompatibility {
    /// Adapter lookup key that created the checkpoint.
    pub adapter_key: String,
    /// Agent whose immutable revision created the checkpoint.
    pub agent_id: AgentId,
    /// Exact immutable revision that created the checkpoint.
    pub revision: u64,
    /// Digest of the caller-assembled invocation context.
    pub context_digest: String,
}

/// Opaque adapter checkpoint that is not a Session, Message, Run, or SDK item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AgentCheckpoint {
    /// Adapter-owned opaque token. It must not contain credential material.
    pub opaque_id: String,
    /// Non-secret compatibility binding checked before resume.
    pub compatibility: AgentCheckpointCompatibility,
}

/// One complete invocation request independent of any provider protocol.
#[derive(Clone, Debug, PartialEq)]
pub struct AgentRequest {
    /// Stable idempotency and correlation identity.
    pub call_id: AgentCallId,
    /// Durable or direct ownership semantics.
    pub purpose: AgentPurpose,
    /// Provider-neutral input.
    pub input: AgentInput,
    /// Digest of the exact caller-assembled context.
    pub context_digest: String,
    /// Explicit permissions and requested behavior.
    pub profile: AgentExecutionProfile,
    /// Host-managed tools available to the call.
    pub tools: Vec<ToolDescriptor>,
    /// Output shape the caller requires.
    pub output: AgentOutputContract,
    /// Hard per-call limits.
    pub limits: AgentCallLimits,
    /// Compatible checkpoint to resume, when supported.
    pub resume_from: Option<AgentCheckpoint>,
    /// Cooperative cancellation shared with the caller.
    pub cancellation: CancellationToken,
}

impl AgentRequest {
    /// Computes the capabilities implied by this request.
    #[must_use]
    pub fn required_capabilities(&self) -> AgentCapabilities {
        let mut required = AgentCapabilities::new([AgentCapability::Streaming]);
        match &self.input {
            AgentInput::MessagePath(_) => required.insert(AgentCapability::MessagePath),
            AgentInput::Prompt(_) => required.insert(AgentCapability::Text),
        }
        match self.output {
            AgentOutputContract::Message => required.insert(AgentCapability::MessageOutput),
            AgentOutputContract::Text => required.insert(AgentCapability::Text),
            AgentOutputContract::JsonObject => required.insert(AgentCapability::StructuredOutput),
            AgentOutputContract::JsonSchema(_) => {
                required.insert(AgentCapability::StructuredOutput);
                required.insert(AgentCapability::JsonSchema);
            }
        }
        match self.profile.workspace {
            WorkspaceAccess::None => {}
            WorkspaceAccess::ReadOnly { .. } => required.insert(AgentCapability::WorkspaceRead),
            WorkspaceAccess::WorkspaceWrite { .. } => {
                required.insert(AgentCapability::WorkspaceRead);
                required.insert(AgentCapability::WorkspaceWrite);
            }
        }
        match self.profile.tool_mode {
            AgentToolMode::Disabled => {}
            AgentToolMode::HostManaged => required.insert(AgentCapability::HostManagedTools),
            AgentToolMode::HarnessManaged => {
                required.insert(AgentCapability::HarnessManagedActivity);
            }
        }
        if self.profile.allow_parallel_tools {
            required.insert(AgentCapability::ParallelTools);
        }
        if self.profile.approval == AgentApprovalMode::HostManaged {
            required.insert(AgentCapability::Approval);
        }
        if self.profile.emit_activity {
            required.insert(AgentCapability::HarnessManagedActivity);
        }
        if self.profile.require_usage {
            required.insert(AgentCapability::Usage);
        }
        if self.resume_from.is_some() {
            required.insert(AgentCapability::Checkpoint);
        }
        required
    }
}

/// Normalized progress activity; never a provider SDK item.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentActivity {
    /// Stable activity category.
    pub kind: AgentActivityKind,
    /// Safe, bounded summary for progress and audit projections.
    pub summary: String,
    /// Non-secret structured attributes.
    pub metadata: DomainMetadata,
}

/// Stable activity categories independent of any coding harness protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentActivityKind {
    /// General progress without an external effect.
    Progress,
    /// Command execution managed by a coding harness.
    Command,
    /// Workspace file inspection or change managed by a coding harness.
    File,
    /// An approval boundary was reached.
    Approval,
    /// Another normalized activity category.
    Other(String),
}

/// Stable reason one invocation turn stopped producing events.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStopReason {
    /// The requested turn completed normally.
    EndTurn,
    /// A complete host-managed tool call is ready for the coordinator.
    ToolUse,
    /// A hard per-call limit stopped generation.
    LimitReached,
    /// A content policy stopped generation.
    ContentFiltered,
    /// Cooperative cancellation stopped generation cleanly.
    Cancelled,
    /// Another stable, safe reason supplied by an adapter.
    Other(String),
}

/// One normalized event emitted by any Agent implementation.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AgentEvent {
    /// Incremental plain text; not itself a durable domain fact.
    TextDelta {
        /// Newly emitted text.
        text: String,
    },
    /// One complete assistant Message candidate for domain validation.
    ProposedMessage {
        /// Ordered complete sub-messages.
        sub_messages: Vec<SubMessage>,
    },
    /// Non-overlapping usage delta accumulated by the caller.
    Usage(RunUsage),
    /// A new opaque recovery checkpoint.
    Checkpoint(AgentCheckpoint),
    /// Normalized progress or audit activity.
    Activity(AgentActivity),
    /// Exactly one terminal event, which must be the final stream item.
    Completed {
        /// Why this low-level turn stopped. It does not complete a domain Run.
        stop_reason: AgentStopReason,
    },
}

/// Stable cross-adapter error category.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentErrorKind {
    /// Caller input violates the port contract.
    InvalidRequest,
    /// A fixed revision or adapter configuration is invalid.
    InvalidConfiguration,
    /// The fixed revision no longer exists.
    RevisionNotFound,
    /// The fixed revision cannot satisfy a requested capability.
    CapabilityUnsupported,
    /// Credential resolution or authentication failed.
    Authentication,
    /// The authenticated principal lacks permission.
    PermissionDenied,
    /// The adapter observed an incompatible or malformed protocol response.
    Protocol,
    /// A service rate limit was reached.
    RateLimited,
    /// A connection could not be established or was interrupted.
    Connection,
    /// The bounded call timed out.
    Timeout,
    /// A remote or local execution service is temporarily unavailable.
    Unavailable,
    /// The caller cancelled the invocation.
    Cancelled,
    /// An uncategorized host-side failure occurred.
    Internal,
}

impl AgentErrorKind {
    /// Returns a stable machine-readable error code.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "AGENT_INVALID_REQUEST",
            Self::InvalidConfiguration => "AGENT_INVALID_CONFIGURATION",
            Self::RevisionNotFound => "AGENT_REVISION_NOT_FOUND",
            Self::CapabilityUnsupported => "AGENT_CAPABILITY_UNSUPPORTED",
            Self::Authentication => "AGENT_AUTHENTICATION_FAILED",
            Self::PermissionDenied => "AGENT_PERMISSION_DENIED",
            Self::Protocol => "AGENT_PROTOCOL_ERROR",
            Self::RateLimited => "AGENT_RATE_LIMITED",
            Self::Connection => "AGENT_CONNECTION_FAILED",
            Self::Timeout => "AGENT_TIMEOUT",
            Self::Unavailable => "AGENT_UNAVAILABLE",
            Self::Cancelled => "AGENT_CANCELLED",
            Self::Internal => "AGENT_INTERNAL",
        }
    }
}

/// Retry advice emitted by an adapter but interpreted by the caller's policy.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "delay", rename_all = "snake_case")]
pub enum RetryDirective {
    /// The same request must not be retried automatically.
    Never,
    /// Use the caller's bounded exponential-backoff policy.
    Backoff,
    /// Wait the supplied server-directed duration before reconsidering retry.
    After(DurationMs),
}

/// An invalid pairing of stable error kind and retry directive.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentErrorClassificationError {
    kind: AgentErrorKind,
    retry: RetryDirective,
}

impl std::fmt::Display for AgentErrorClassificationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "retry directive {:?} is invalid for {}",
            self.retry,
            self.kind.code()
        )
    }
}

impl std::error::Error for AgentErrorClassificationError {}

/// Stable, safe Agent invocation failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentError {
    kind: AgentErrorKind,
    message: String,
    retry: RetryDirective,
}

impl AgentError {
    /// Creates an error only when its retry directive matches the stable class.
    ///
    /// # Errors
    ///
    /// Returns [`AgentErrorClassificationError`] for unsafe retry advice.
    pub fn classified(
        kind: AgentErrorKind,
        message: impl Into<String>,
        retry: RetryDirective,
    ) -> Result<Self, AgentErrorClassificationError> {
        let retry_is_valid = match kind {
            AgentErrorKind::RateLimited => {
                matches!(retry, RetryDirective::Backoff | RetryDirective::After(_))
            }
            AgentErrorKind::Connection | AgentErrorKind::Timeout | AgentErrorKind::Unavailable => {
                retry == RetryDirective::Backoff
            }
            AgentErrorKind::InvalidRequest
            | AgentErrorKind::InvalidConfiguration
            | AgentErrorKind::RevisionNotFound
            | AgentErrorKind::CapabilityUnsupported
            | AgentErrorKind::Authentication
            | AgentErrorKind::PermissionDenied
            | AgentErrorKind::Protocol
            | AgentErrorKind::Cancelled
            | AgentErrorKind::Internal => retry == RetryDirective::Never,
        };
        if !retry_is_valid {
            return Err(AgentErrorClassificationError { kind, retry });
        }
        Ok(Self {
            kind,
            message: message.into(),
            retry,
        })
    }

    /// Creates a non-retryable invalid-request failure.
    #[must_use]
    pub fn invalid_request(message: impl Into<String>) -> Self {
        Self::fixed(
            AgentErrorKind::InvalidRequest,
            message,
            RetryDirective::Never,
        )
    }

    /// Creates a non-retryable invalid-revision failure.
    #[must_use]
    pub fn invalid_configuration(message: impl Into<String>) -> Self {
        Self::fixed(
            AgentErrorKind::InvalidConfiguration,
            message,
            RetryDirective::Never,
        )
    }

    /// Creates a non-retryable capability failure with the domain-stable code.
    #[must_use]
    pub fn capability_unsupported(message: impl Into<String>) -> Self {
        Self::fixed(
            AgentErrorKind::CapabilityUnsupported,
            message,
            RetryDirective::Never,
        )
    }

    /// Creates a non-retryable cancellation failure.
    #[must_use]
    pub fn cancelled() -> Self {
        Self::fixed(
            AgentErrorKind::Cancelled,
            "agent invocation cancelled",
            RetryDirective::Never,
        )
    }

    fn fixed(kind: AgentErrorKind, message: impl Into<String>, retry: RetryDirective) -> Self {
        Self::classified(kind, message, retry).expect("fixed Agent error classification is valid")
    }

    /// Returns the stable category.
    #[must_use]
    pub const fn kind(&self) -> AgentErrorKind {
        self.kind
    }

    /// Returns the stable code for the category.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        self.kind.code()
    }

    /// Returns a safe diagnostic that must not contain prompts or credentials.
    #[must_use]
    pub fn message(&self) -> &str {
        &self.message
    }

    /// Returns adapter advice for the caller-owned retry policy.
    #[must_use]
    pub const fn retry(&self) -> RetryDirective {
        self.retry
    }
}

impl std::fmt::Display for AgentError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}: {}", self.code(), self.message)
    }
}

impl std::error::Error for AgentError {}

/// Unified invocation boundary implemented by coding harnesses and model providers.
#[async_trait]
pub trait AgentInvoker: Send + Sync + 'static {
    /// Returns actual adapter capabilities before any external side effect.
    fn capabilities(&self) -> AgentCapabilities;

    /// Starts one low-level invocation and returns its ordered event stream.
    ///
    /// Implementations must call [`preflight_request`] before network access,
    /// process spawn, credential resolution, or workspace access. Composition
    /// roots should expose them through [`ResolvedAgent`], which enforces this
    /// ordering against the fixed revision even if an adapter is faulty.
    async fn invoke(&self, request: AgentRequest) -> Result<AgentEventStream, AgentError>;
}

/// Resolver from one fixed, non-secret revision to a preflight-guarded invoker.
#[async_trait]
pub trait AgentResolver: Send + Sync {
    /// Resolves an adapter without resolving credentials beyond call scope.
    async fn resolve(&self, revision: &AgentRevisionSnapshot) -> Result<ResolvedAgent, AgentError>;
}

/// Fixed-revision wrapper that enforces capability and cancellation preflight.
#[derive(Clone)]
pub struct ResolvedAgent {
    revision: AgentRevisionSnapshot,
    invoker: Arc<dyn AgentInvoker>,
}

impl ResolvedAgent {
    /// Binds an adapter to a fixed revision after verifying declared capability claims.
    ///
    /// # Errors
    ///
    /// Returns [`AgentErrorKind::InvalidConfiguration`] when the snapshot is
    /// invalid or claims a capability the adapter does not implement.
    pub fn new(
        revision: AgentRevisionSnapshot,
        invoker: Arc<dyn AgentInvoker>,
    ) -> Result<Self, AgentError> {
        revision.validate()?;
        let missing = revision.capabilities.missing_from(&invoker.capabilities());
        if !missing.is_empty() {
            return Err(AgentError::invalid_configuration(format!(
                "agent revision claims unsupported adapter capabilities: {}",
                capability_names(&missing)
            )));
        }
        Ok(Self { revision, invoker })
    }

    /// Returns the immutable non-secret revision used by this wrapper.
    #[must_use]
    pub const fn revision(&self) -> &AgentRevisionSnapshot {
        &self.revision
    }

    fn validate_checkpoint(&self, request: &AgentRequest) -> Result<(), AgentError> {
        let Some(checkpoint) = &request.resume_from else {
            return Ok(());
        };
        let compatibility = &checkpoint.compatibility;
        if compatibility.adapter_key != self.revision.adapter_key
            || compatibility.agent_id != self.revision.agent_id
            || compatibility.revision != self.revision.revision
            || compatibility.context_digest != request.context_digest
        {
            return Err(AgentError::invalid_request(
                "checkpoint is incompatible with the fixed Agent revision or invocation context",
            ));
        }
        Ok(())
    }
}

impl std::fmt::Debug for ResolvedAgent {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ResolvedAgent")
            .field("revision", &self.revision)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl AgentInvoker for ResolvedAgent {
    fn capabilities(&self) -> AgentCapabilities {
        self.revision.capabilities.clone()
    }

    async fn invoke(&self, request: AgentRequest) -> Result<AgentEventStream, AgentError> {
        preflight_request(&request, &self.revision.capabilities)?;
        self.validate_checkpoint(&request)?;
        self.invoker.invoke(request).await
    }
}

/// Validates request shape, direct-call restrictions, cancellation, and capabilities.
///
/// Adapters must call this before network, process, credential, or filesystem
/// work. [`ResolvedAgent`] also calls it before dispatching to an adapter.
///
/// # Errors
///
/// Returns a stable non-retryable [`AgentError`] when validation fails.
pub fn preflight_request(
    request: &AgentRequest,
    available: &AgentCapabilities,
) -> Result<(), AgentError> {
    if request.cancellation.is_cancelled() {
        return Err(AgentError::cancelled());
    }
    if request.call_id.as_str().trim().is_empty()
        || !is_sha256(&request.context_digest)
        || request.limits.timeout.get() == 0
        || request.limits.max_input_bytes == 0
        || request.limits.max_output_bytes == 0
        || request.limits.max_output_tokens == Some(0)
        || request.limits.max_cost.is_some_and(|cost| cost.0 == 0)
    {
        return Err(AgentError::invalid_request(
            "agent request identity, context digest, or limits are invalid",
        ));
    }
    match &request.input {
        AgentInput::MessagePath(path) if path.is_empty() => {
            return Err(AgentError::invalid_request("message path cannot be empty"));
        }
        AgentInput::Prompt(prompt) if prompt.trim().is_empty() => {
            return Err(AgentError::invalid_request("prompt cannot be empty"));
        }
        AgentInput::MessagePath(_) | AgentInput::Prompt(_) => {}
    }
    if let AgentOutputContract::JsonSchema(schema) = &request.output
        && !schema.is_object()
    {
        return Err(AgentError::invalid_request(
            "JSON output schema must be an object",
        ));
    }
    match request.profile.tool_mode {
        AgentToolMode::HostManaged if request.tools.is_empty() => {
            return Err(AgentError::invalid_request(
                "host-managed tool mode requires at least one tool descriptor",
            ));
        }
        AgentToolMode::Disabled | AgentToolMode::HarnessManaged if !request.tools.is_empty() => {
            return Err(AgentError::invalid_request(
                "tool descriptors require host-managed tool mode",
            ));
        }
        AgentToolMode::Disabled | AgentToolMode::HostManaged | AgentToolMode::HarnessManaged => {}
    }
    if request.profile.allow_parallel_tools
        && request.profile.tool_mode != AgentToolMode::HostManaged
    {
        return Err(AgentError::invalid_request(
            "parallel tools require host-managed tool mode",
        ));
    }
    if let WorkspaceAccess::ReadOnly { root } | WorkspaceAccess::WorkspaceWrite { root } =
        &request.profile.workspace
        && !root.is_absolute()
    {
        return Err(AgentError::invalid_request(
            "workspace root must be an absolute canonical path",
        ));
    }
    if let AgentPurpose::Direct { operation_id, .. } = &request.purpose {
        if operation_id.as_str().trim().is_empty() {
            return Err(AgentError::invalid_request(
                "direct operation identity cannot be empty",
            ));
        }
        if matches!(
            request.profile.workspace,
            WorkspaceAccess::WorkspaceWrite { .. }
        ) || request.profile.tool_mode != AgentToolMode::Disabled
            || request.profile.approval != AgentApprovalMode::Denied
            || request.profile.allow_parallel_tools
            || request.profile.emit_activity
            || request.resume_from.is_some()
        {
            return Err(AgentError::invalid_request(
                "direct invocation cannot write a workspace, use tools, request approval or activity, or resume a checkpoint",
            ));
        }
    }
    let missing = request.required_capabilities().missing_from(available);
    if !missing.is_empty() {
        return Err(AgentError::capability_unsupported(format!(
            "agent lacks required capabilities: {}",
            capability_names(&missing)
        )));
    }
    Ok(())
}

fn capability_names(capabilities: &[AgentCapability]) -> String {
    capabilities
        .iter()
        .map(|capability| capability.as_str())
        .collect::<Vec<_>>()
        .join(", ")
}

fn is_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
