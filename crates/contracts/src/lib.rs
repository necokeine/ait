//! Versioned transport DTOs shared by HTTP, CLI, IPC, and future UI clients.

use ait_domain::{
    ApprovalGrantScope, ErrorCode, NativeApprovalKind, NativeApprovalStatus, NativeApprovalTarget,
    RunPermissionProfile,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;

mod git_commit;
pub mod sensitive;
pub use git_commit::{RunCommitStatus, RunGitCommit};

/// Current command/event wire contract version.
pub const API_VERSION: u16 = 1;

/// Current portable Project archive format.
pub const PROJECT_EXPORT_VERSION: u16 = 3;

pub use ait_domain::{AgentConfiguration, AgentProvider, ProviderKind as AgentMode, ProviderModel};

/// Commands accepted by the shared application service.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum Command {
    /// Retry only a failed Git finalization, without replaying model work.
    RetryRunCommit {
        /// Completed Run identity.
        run_id: String,
    },
    /// Selects the `RegisterProject` variant.
    RegisterProject {
        /// Id value.
        id: String,
        /// Name value.
        name: String,
        /// Omitted/null creates a new directory under the host user's Documents.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        workdir: Option<String>,
        #[serde(default)]
        /// Repo url value.
        repo_url: Option<String>,
    },
    /// Cancels and drains this runtime's work, then releases Project ownership.
    CloseProject {
        /// Stable Project identity; disk contents and registration are retained.
        project_id: String,
    },
    /// Explicitly binds a saved Project Agent reference to a local preset.
    BindProjectAgent {
        /// Stable Project identity.
        project_id: String,
        /// Agent identity retained in the Project's history.
        source_agent_id: String,
        /// Enabled named preset in this host's catalog.
        agent_id: String,
    },
    /// Selects the `UpdateProject` variant.
    UpdateProject {
        /// Project identifier.
        project_id: String,
        /// Name value.
        name: String,
        /// Omitted/null preserves the current default Agent.
        #[serde(default)]
        agent_id: Option<String>,
    },
    /// Selects the `SetProjectDefaultAgent` variant.
    SetProjectDefaultAgent {
        /// Project identifier.
        project_id: String,
        /// Agent identifier.
        agent_id: String,
    },
    /// Selects the `RegisterAgent` variant.
    RegisterAgent {
        /// Id value.
        id: String,
        /// Name value.
        name: String,
        /// Config value.
        config: AgentConfiguration,
    },
    /// Selects the `SaveAgentProvider` variant.
    SaveAgentProvider {
        /// Provider value.
        provider: AgentProvider,
        #[serde(default)]
        /// Secret value.
        secret: Option<ProviderSecret>,
    },
    /// Selects the `DiscoverProviderModels` variant.
    DiscoverProviderModels {
        /// Provider value.
        provider: AgentProvider,
        #[serde(default)]
        /// Secret value.
        secret: Option<ProviderSecret>,
    },
    /// Selects the `RefreshProviderModels` variant.
    RefreshProviderModels {
        /// Provider identifier.
        provider_id: String,
    },
    /// Discovers native Codex Threads without importing their history.
    ListCodexThreads {
        /// Codex Provider catalog identity.
        provider_id: String,
        /// Optional Project scope; unbound Threads must have one unambiguous owner.
        #[serde(default)]
        project_id: Option<String>,
    },
    /// Imports or reconciles one native Codex Thread into an Ait Session.
    SyncCodexThread {
        /// Codex Provider catalog identity.
        provider_id: String,
        /// Native Codex Thread identity.
        thread_id: String,
        /// Explicit target Ait Project identity.
        project_id: String,
        /// Enabled Codex Agent bound to the imported Session.
        agent_id: String,
    },
    /// Selects the `UpdateAgent` variant.
    UpdateAgent {
        /// Id value.
        id: String,
        /// Name value.
        name: String,
        /// Config value.
        config: AgentConfiguration,
    },
    /// Selects the `SetSessionConfig` variant.
    SetSessionConfig {
        /// Session identifier.
        session_id: String,
        /// Config value.
        config: AgentConfiguration,
    },
    /// Selects the `CreateSession` variant.
    CreateSession {
        /// Id value.
        id: String,
        /// Project identifier.
        project_id: String,
        /// Empty/omitted resolves Project override, then the global Default Agent.
        #[serde(default)]
        agent_id: String,
        #[serde(default)]
        /// At message identifier.
        at_message_id: Option<String>,
    },
    /// Selects the `SetSessionAgent` variant.
    SetSessionAgent {
        /// Session identifier.
        session_id: String,
        /// Agent identifier.
        agent_id: String,
    },
    /// Selects the `RenameSession` variant.
    RenameSession {
        /// Session identifier.
        session_id: String,
        /// Name value.
        name: String,
    },
    /// Selects the `SetSessionArchived` variant.
    SetSessionArchived {
        /// Session identifier.
        session_id: String,
        /// Whether the Session is archived.
        archived: bool,
    },
    /// Selects the `SetSessionTitle` variant.
    SetSessionTitle {
        /// Session identifier.
        session_id: String,
        /// Title value.
        title: String,
    },
    /// Selects the `SendMessage` variant.
    SendMessage {
        /// Session identifier.
        session_id: String,
        /// Text value.
        text: String,
    },
    /// Selects the `ForkSession` variant.
    ForkSession {
        /// Id value.
        id: String,
        /// Project identifier.
        project_id: String,
        /// Empty/omitted resolves Project override, then the global Default Agent.
        #[serde(default)]
        agent_id: String,
        /// At message identifier.
        at_message_id: String,
        /// Text value.
        text: String,
    },
    /// Selects the `DeriveSession` variant.
    DeriveSession {
        /// Id value.
        id: String,
        /// Project identifier.
        project_id: String,
        /// Source session identifier.
        source_session_id: String,
        /// Empty/omitted resolves Project override, then the global Default Agent.
        #[serde(default)]
        agent_id: String,
        /// At message identifier.
        at_message_id: String,
        /// Text value.
        text: String,
    },
    /// Selects the `GetRun` variant.
    GetRun {
        /// Run identifier.
        run_id: String,
    },
    /// Selects the `CancelRun` variant.
    CancelRun {
        /// Run identifier.
        run_id: String,
    },
    /// Selects the `ResolveNativeApproval` variant.
    ResolveNativeApproval {
        /// Run identifier.
        run_id: String,
        /// Approval identifier.
        approval_id: String,
        /// Action value.
        action: NativeApprovalAction,
        #[serde(default)]
        /// Scope value.
        scope: Option<ApprovalGrantScope>,
    },
    /// Selects the `CreateCron` variant.
    CreateCron {
        /// Id value.
        id: String,
        /// Name value.
        name: String,
        /// Project identifier.
        project_id: String,
        /// Base message identifier.
        base_message_id: String,
        /// Empty/omitted resolves Project override, then the global Default Agent.
        #[serde(default)]
        agent_id: String,
        /// Schedule value.
        schedule: String,
        /// Timezone value.
        timezone: String,
    },
    /// Selects the `SetCronEnabled` variant.
    SetCronEnabled {
        /// Cron identifier.
        cron_id: String,
        /// Enabled value.
        enabled: bool,
    },
    /// Selects the `TriggerCron` variant.
    TriggerCron {
        /// Cron identifier.
        cron_id: String,
        /// Scheduled timestamp.
        scheduled_at: i64,
    },
    /// Selects the `ExportProject` variant.
    ExportProject {
        /// Project identifier.
        project_id: String,
    },
    /// Selects the `ImportProject` variant.
    ImportProject {
        /// Archive value.
        archive: ProjectExport,
        /// Workdir value.
        workdir: String,
    },
    /// Selects the `GetSettings` variant.
    GetSettings,
    /// Selects the `SaveSettings` variant.
    SaveSettings {
        /// Expected revision value.
        expected_revision: u64,
        /// Values value.
        values: desktop::SettingsDocument,
    },
    /// Selects the `ResetSettings` variant.
    ResetSettings,
    /// Selects the `ListProjects` variant.
    ListProjects,
    /// Selects the `ListAgents` variant.
    ListAgents,
    /// Selects the `ListAgentProviders` variant.
    ListAgentProviders,
    /// Selects the `ListSessions` variant.
    ListSessions {
        /// Project identifier.
        project_id: String,
    },
    /// Selects the `ListMessages` variant.
    ListMessages {
        /// Project identifier.
        project_id: String,
    },
    /// Selects the `ListRuns` variant.
    ListRuns {
        /// Project identifier.
        project_id: String,
    },
    /// Selects the `ListCrons` variant.
    ListCrons,
}

/// Member action on one pending Codex-native approval request.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NativeApprovalAction {
    /// Selects the `Approve` variant.
    Approve,
    /// Selects the `Deny` variant.
    Deny,
    /// Selects the `Cancel` variant.
    Cancel,
}

/// Member decision for one API host-tool request. Approval always means once.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolApprovalAction {
    /// Selects the `Approve` variant.
    Approve,
    /// Selects the `Deny` variant.
    Deny,
    /// Selects the `Cancel` variant.
    Cancel,
}

/// Stable API error envelope.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiError {
    /// Code value.
    pub code: ErrorCode,
    /// Message value.
    pub message: String,
    /// Retryable value.
    pub retryable: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `ProjectView`.
pub struct ProjectView {
    /// Current runtime acquisition; send as `x-ait-project-owner` on scoped requests.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner: Option<Box<ait_domain::ProjectOwner>>,
    /// A retained prior worker prevents execution; committed history remains readable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_blocked: Option<String>,
    /// Stable identifier.
    pub id: String,
    /// Name value.
    pub name: String,
    /// Workdir value.
    pub workdir: String,
    /// Root message identifier.
    pub root_message_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Repo url value.
    pub repo_url: Option<String>,
    /// Immutable repository HEAD captured when the Project was registered.
    #[serde(default)]
    pub base_commit: String,
    #[serde(default)]
    /// Default agent identifier.
    pub default_agent_id: Option<String>,
    #[serde(default = "default_revision")]
    /// Revision value.
    pub revision: u64,
}

const fn default_revision() -> u64 {
    1
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
/// Data carried by `AgentView`.
pub struct AgentView {
    /// Stable identifier.
    pub id: String,
    /// Name value.
    pub name: String,
    /// Config value.
    pub config: AgentConfiguration,
    #[serde(default)]
    /// Owner session identifier.
    pub owner_session_id: Option<String>,
    /// Revision value.
    pub revision: u64,
    /// Enabled value.
    pub enabled: bool,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `SessionView`.
pub struct SessionView {
    /// Stable identifier.
    pub id: String,
    /// Project identifier.
    pub project_id: String,
    /// Absolute manager-owned linked worktree used by this Session.
    #[serde(default)]
    pub workdir: String,
    /// Native or Ait-managed Session source semantics.
    #[serde(default)]
    pub source: ait_domain::SessionSource,
    #[serde(default)]
    /// Name value.
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Title value.
    pub title: Option<String>,
    #[serde(default)]
    /// Description value.
    pub description: String,
    #[serde(default)]
    /// Title generation started value.
    pub title_generation_started: bool,
    /// Session availability state.
    #[serde(default)]
    pub status: ait_domain::SessionStatus,
    /// Agent identifier.
    pub agent_id: String,
    /// Current message identifier.
    pub current_message_id: String,
    /// Active run identifier.
    pub active_run_id: Option<String>,
    /// Version value.
    pub version: u64,
}

/// Provider catalog projection for one discovered native Codex Thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CodexThreadView {
    /// Codex Provider catalog identity.
    pub provider_id: String,
    /// Native Thread identity.
    pub thread_id: String,
    /// Native session metadata identity.
    pub codex_session_id: String,
    /// Source Thread identity for a native fork.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_thread_id: Option<String>,
    /// Native working directory.
    pub cwd: String,
    /// User-assigned native Thread name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Bounded provider preview.
    #[serde(default)]
    pub preview: String,
    /// Forward-compatible native source metadata.
    pub source: Value,
    /// Native runtime status object.
    pub status: Value,
    /// Whether the native Thread is archived.
    pub archived: bool,
    /// Provider creation time in Unix seconds.
    pub created_at: i64,
    /// Provider update time in Unix seconds.
    pub updated_at: i64,
    /// Bounded forward-compatible native Thread metadata.
    pub native_metadata: Value,
    /// Materialized Ait Session, when already bound.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// Bound Ait Project, when already materialized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
/// Data carried by `MessageView`.
pub struct MessageView {
    /// Stable identifier.
    pub id: String,
    /// Project identifier.
    pub project_id: String,
    /// Parent message identifier.
    pub parent_message_id: Option<String>,
    /// Role value.
    pub role: String,
    /// Kind value.
    pub kind: String,
    /// Text value.
    pub text: Option<String>,
    /// Creation time in Unix milliseconds; zero means an older record has no timestamp.
    #[serde(default)]
    pub created_at: i64,
    /// Clean repository HEAD captured with interactive human input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Data value.
    pub data: Option<Value>,
}

/// String or integer JSON-RPC identity supplied by Codex app-server.
#[derive(Clone, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ProtocolRequestId {
    /// Selects the `String` variant.
    String(String),
    /// Selects the `Integer` variant.
    Integer(i64),
}

/// Explicit network permission set requested by Codex.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeNetworkPermissions {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Enabled value.
    pub enabled: Option<bool>,
}

/// Explicit filesystem permission set requested by Codex.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeFileSystemPermissions {
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// Entries value.
    pub entries: Vec<NativeFileSystemPermission>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Glob scan max depth value.
    pub glob_scan_max_depth: Option<u64>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// Read value.
    pub read: Vec<String>,
    #[serde(
        default,
        deserialize_with = "deserialize_null_default",
        skip_serializing_if = "Vec::is_empty"
    )]
    /// Write value.
    pub write: Vec<String>,
}

fn deserialize_null_default<'de, D, T>(deserializer: D) -> Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Option::<T>::deserialize(deserializer).map(Option::unwrap_or_default)
}

/// One filesystem path and its requested access.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NativeFileSystemPermission {
    /// Access value.
    pub access: NativeFileSystemAccess,
    /// Path value.
    pub path: NativeFileSystemPath,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
/// Variants represented by `NativeFileSystemAccess`.
pub enum NativeFileSystemAccess {
    /// Selects the `Read` variant.
    Read,
    /// Selects the `Write` variant.
    Write,
    /// Selects the `Deny` variant.
    Deny,
}

/// Member action on a pending API tool interaction.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolInteractionAction {
    /// Selects the `Submit` variant.
    Submit,
    /// Selects the `Approve` variant.
    Approve,
    /// Selects the `Deny` variant.
    Deny,
    /// Selects the `Cancel` variant.
    Cancel,
}

/// Provider path vocabulary retained without granting renderer filesystem authority.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeFileSystemPath {
    /// Selects the `Path` variant.
    Path {
        #[doc = "Path value."]
        path: String,
    },
    /// Selects the `GlobPattern` variant.
    GlobPattern {
        #[doc = "Pattern value."]
        pattern: String,
    },
    /// Selects the `Special` variant.
    Special {
        #[doc = "Value value."]
        value: NativeFileSystemSpecialPath,
    },
}

/// Special roots understood by the current Codex permission protocol.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum NativeFileSystemSpecialPath {
    /// Selects the `Root` variant.
    Root,
    /// Selects the `Minimal` variant.
    Minimal,
    /// Selects the `ProjectRoots` variant.
    ProjectRoots {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Subpath value.
        subpath: Option<String>,
    },
    /// Selects the `Tmpdir` variant.
    Tmpdir,
    /// Selects the `SlashTmp` variant.
    SlashTmp,
    /// Selects the `Unknown` variant.
    Unknown {
        /// Path value.
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        /// Subpath value.
        subpath: Option<String>,
    },
}

/// Exact additional capabilities requested or granted for a native operation.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativePermissionProfile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// File system value.
    pub file_system: Option<NativeFileSystemPermissions>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Network value.
    pub network: Option<NativeNetworkPermissions>,
}

/// Durable, non-secret audit record for one Codex-native approval request.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct NativeApprovalView {
    /// Stable identifier.
    pub id: String,
    /// Run identifier.
    pub run_id: String,
    /// Protocol request identifier.
    pub protocol_request_id: ProtocolRequestId,
    /// Method value.
    pub method: String,
    /// Kind value.
    pub kind: NativeApprovalKind,
    /// Thread identifier.
    pub thread_id: String,
    /// Turn identifier.
    pub turn_id: String,
    /// Item identifier.
    pub item_id: String,
    /// Bounded, non-secret authorization object shown after reconnect.
    pub target: NativeApprovalTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Requested permissions value.
    pub requested_permissions: Option<NativePermissionProfile>,
    /// Status value.
    pub status: NativeApprovalStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Granted scope value.
    pub granted_scope: Option<ApprovalGrantScope>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Granted permissions value.
    pub granted_permissions: Option<NativePermissionProfile>,
    /// Created timestamp.
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Decided timestamp.
    pub decided_at: Option<i64>,
}

/// Durable member response requested by an API host tool.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolInteractionView {
    /// Stable interaction and `ToolExecution` identity.
    pub id: String,
    /// Run identifier.
    pub run_id: String,
    /// Tool name value.
    pub tool_name: String,
    /// Bounded request arguments shown to the member.
    pub request: serde_json::Value,
    /// Submitted answer or plan decision, once resolved.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub response: Option<serde_json::Value>,
    /// `pending`, `answered`, `approved`, `denied`, `cancelled`, or `expired`.
    pub status: String,
    /// Lease epoch value.
    pub lease_epoch: u64,
    /// Expires timestamp.
    pub expires_at: i64,
    /// Created timestamp.
    pub created_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Decided timestamp.
    pub decided_at: Option<i64>,
}

/// Run result or query snapshot; this DTO never requests execution.
/// Synchronous command routes return the final state. Explicit asynchronous
/// submission routes and queries can expose an intermediate state.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RunView {
    /// Optional Ait Git finalization outcome, independent of model execution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub git_commit: Option<RunGitCommit>,
    /// Canonical host runtime state for API Providers; absent for native harness Runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution: Option<Box<ApiRunExecution>>,

    /// Stable identifier.
    pub id: String,
    /// Project identifier.
    pub project_id: String,
    /// Base message identifier.
    pub base_message_id: String,
    /// Last message identifier.
    pub last_message_id: Option<String>,
    /// Session identifier.
    pub session_id: Option<String>,
    /// Agent identifier.
    pub agent_id: String,
    /// Agent revision value.
    pub agent_revision: u64,
    /// Config value.
    pub config: AgentConfiguration,
    /// Provider value.
    pub provider: AgentProvider,
    /// Effective non-secret permission policy fixed when this Run was created.
    #[serde(default)]
    pub permission_profile: RunPermissionProfile,
    /// Codex-native approval audit records. They are not Ait ToolUse/ToolResult.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub native_approvals: Vec<NativeApprovalView>,
    /// Independent, durable API host-tool requests and one-operation grants.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_approvals: Vec<ait_domain::ToolApprovalRecord>,
    /// API tool questions and plan reviews awaiting or retaining a member response.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_interactions: Vec<ToolInteractionView>,
    /// Trigger value.
    pub trigger: String,
    /// Cron identifier.
    pub cron_id: Option<String>,
    /// Scheduled timestamp.
    pub scheduled_at: Option<i64>,
    /// Git baseline authorized for a workspace-writing Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_base_commit: Option<String>,
    /// Exact Git index tree authorized with the workspace baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_base_index_tree: Option<Box<str>>,
    /// Status value.
    pub status: String,
    /// Fine-grained durable phase used to explain and recover non-terminal work.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<Box<str>>,
    /// Stable identity of the workspace side-effect operation for this Run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation_id: Option<Box<str>>,
    /// Monotonic execution lease; late writers holding an older value are fenced.
    #[serde(default)]
    pub lease_epoch: u64,
    /// Error value.
    pub error: Option<ApiError>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `CronView`.
pub struct CronView {
    /// Stable identifier.
    pub id: String,
    /// Name value.
    pub name: String,
    /// Project identifier.
    pub project_id: String,
    /// Base message identifier.
    pub base_message_id: String,
    /// Agent identifier.
    pub agent_id: String,
    /// Schedule value.
    pub schedule: String,
    /// Timezone value.
    pub timezone: String,
    /// Enabled value.
    pub enabled: bool,
}

/// Durable coordinator state stored atomically with the public Run projection.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ApiRunExecution {
    /// Run value.
    pub run: ait_domain::Run,
    /// Attempts value.
    pub attempts: Vec<ait_domain::RunAttempt>,
    /// Tools value.
    pub tools: Vec<ait_domain::ToolExecution>,
    /// Current worker identity, fenced together with `RunView.lease_epoch`.
    #[serde(default)]
    pub worker_instance_id: Option<String>,
    /// Atomic mutation receipts; payloads are hashed, never copied into this journal.
    #[serde(default)]
    pub worker_receipts: std::collections::BTreeMap<String, WorkerCommitReceipt>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `WorkerCommitReceipt`.
pub struct WorkerCommitReceipt {
    /// Fingerprint value.
    pub fingerprint: String,
    /// Run value.
    pub run: ait_domain::Run,
    /// Completed value.
    pub completed: Option<bool>,
}

/// Portable, credential-free Project and Session archive.
///
/// Runtime attempts, active Run bindings, Cron registrations, attachment
/// bytes, and provider credentials are deliberately outside this format.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(try_from = "archive::ArchiveInput")]
pub struct ProjectExport {
    /// Format version value.
    pub format_version: u16,
    /// Source revision value.
    pub source_revision: u64,
    /// Project value.
    pub project: ProjectView,
    /// Agents value.
    pub agents: Vec<AgentView>,
    #[serde(default)]
    /// Providers value.
    pub providers: Vec<AgentProvider>,
    /// Sessions value.
    pub sessions: Vec<SessionView>,
    /// Messages value.
    pub messages: Vec<MessageView>,
}

/// Successful command payload.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
#[allow(
    clippy::large_enum_variant,
    reason = "the versioned wire contract keeps result payloads directly serializable"
)]
pub enum CommandResult {
    /// Selects the `Project` variant.
    Project(ProjectView),
    /// Selects the `Agent` variant.
    Agent(AgentView),
    /// Selects the `AgentProvider` variant.
    AgentProvider(AgentProviderView),
    /// Selects the `ProviderModels` variant.
    ProviderModels(Vec<ProviderModel>),
    /// Selects the `CodexThreads` variant.
    CodexThreads(Vec<CodexThreadView>),
    /// Selects the `Session` variant.
    Session(SessionView),
    /// Selects the `Run` variant.
    Run(RunView),
    /// Selects the `Cron` variant.
    Cron(CronView),
    /// Selects the `ProjectExport` variant.
    ProjectExport(ProjectExport),
    /// Selects the `Settings` variant.
    Settings(desktop::SettingsView),
    /// Selects the `Projects` variant.
    Projects(Vec<ProjectView>),
    /// Selects the `Agents` variant.
    Agents(Vec<AgentView>),
    /// Selects the `AgentProviders` variant.
    AgentProviders(Vec<AgentProviderView>),
    /// Selects the `Sessions` variant.
    Sessions(Vec<SessionView>),
    /// Selects the `Messages` variant.
    Messages(Vec<MessageView>),
    /// Selects the `Runs` variant.
    Runs(Vec<RunView>),
    /// Selects the `Crons` variant.
    Crons(Vec<CronView>),
}

/// Response shared by every transport.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Response {
    /// Api version value.
    pub api_version: u16,
    /// Ok value.
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Result value.
    pub result: Option<CommandResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    /// Error value.
    pub error: Option<ApiError>,
}

impl Response {
    #[must_use]
    /// Creates a successful response carrying `result`.
    pub const fn success(result: CommandResult) -> Self {
        Self {
            api_version: API_VERSION,
            ok: true,
            result: Some(result),
            error: None,
        }
    }
    #[must_use]
    /// Creates a failed response carrying `error`.
    pub const fn failure(error: ApiError) -> Self {
        Self {
            api_version: API_VERSION,
            ok: false,
            result: None,
            error: Some(error),
        }
    }
}

/// Reconnectable durable event.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// Catalog and feed generation; sequence numbers are only comparable within it.
    #[serde(default)]
    pub namespace: String,
    /// Api version value.
    pub api_version: u16,
    /// Cursor value.
    pub cursor: u64,
    /// Kind value.
    pub kind: String,
    /// Entity identifier.
    pub entity_id: Option<String>,
    /// Body value.
    pub body: Value,
    /// Created timestamp.
    pub created_at: i64,
}

/// One bounded replay page plus retained-cursor validity metadata.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventPage {
    /// Namespace required when reconnecting with a nonzero cursor.
    #[serde(default)]
    pub namespace: String,
    /// Events value.
    pub events: Vec<Event>,
    /// Oldest cursor value.
    pub oldest_cursor: Option<u64>,
    /// Latest cursor value.
    pub latest_cursor: Option<u64>,
    /// Cursor valid value.
    pub cursor_valid: bool,
}

/// Versioned desktop workspace, settings, and branch-operation DTOs.
pub mod desktop;

pub use desktop::{
    AgentSummary, DESKTOP_PROTOCOL_VERSION, DesktopMessage, DesktopMessagePart, DesktopProject,
    DesktopSession, ForkFromMessageRequest, SaveSettingsRequest, SettingCategory,
    SettingDefinition, SettingKind, SettingsDocument, SettingsSchema, SettingsView,
    default_settings, settings_schema,
};

/// A write-only secret; Debug never prints its contents.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ProviderSecret(pub String);
impl std::fmt::Debug for ProviderSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
/// Data carried by `AgentProviderView`.
pub struct AgentProviderView {
    #[serde(flatten)]
    /// Provider value.
    pub provider: AgentProvider,
    /// Has secret value.
    pub has_secret: bool,
}

mod archive;
pub mod worker;
