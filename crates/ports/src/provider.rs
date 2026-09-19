//! Credential storage and remote LLM operations behind the application boundary.

use ait_domain::{AgentConfiguration, AgentProvider, DomainError, ProviderModel};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// A provider adapter resolves credential references without exposing secrets to state.
#[async_trait]
pub trait AgentProviderGateway: Send + Sync {
    /// Resolves a minimum-scope, in-memory grant for a supervised worker.
    /// This value must only cross the private pipe, never logs or durable state.
    async fn credential_grant(&self, _reference: &str) -> Result<String, DomainError> {
        Err(DomainError::invariant(
            ait_domain::ErrorCode::InvalidConfiguration,
            "provider does not support worker credential grants",
        ))
    }
    /// Store a credential under an opaque, immutable reference.
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError>;
    /// Delete an unused credential after a failed configuration transaction.
    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError>;
    /// Fetch model identifiers; advertised effort metadata can be absent.
    async fn list_models(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
    ) -> Result<Vec<ProviderModel>, DomainError>;
    /// Preview models using an unsaved credential, without writing it to storage.
    async fn list_models_with_secret(
        &self,
        provider: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError>;
    /// Generate one structured turn. The host owns persistence and tool execution.
    async fn complete_turn(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
        config: &AgentConfiguration,
        request: crate::AgentInvocation,
        executable_tools: Vec<String>,
    ) -> Result<crate::AgentResponse, DomainError> {
        if !executable_tools.is_empty() {
            return Err(DomainError::invariant(
                ait_domain::ErrorCode::InvalidConfiguration,
                "provider gateway does not support host tools",
            ));
        }
        let messages = request
            .message_path
            .into_iter()
            .filter_map(|entry| {
                let ait_domain::ProjectedMessage::Visible(message) = entry else {
                    return None;
                };
                Some(ProviderMessage {
                    role: serde_json::to_value(message.role).ok()?.as_str()?.into(),
                    text: message
                        .sub_messages
                        .into_iter()
                        .filter_map(|part| match part {
                            ait_domain::SubMessage::Text { text } => Some(text),
                            _ => None,
                        })
                        .collect(),
                })
            })
            .collect();
        Ok(crate::AgentResponse {
            sub_messages: vec![ait_domain::SubMessage::Text {
                text: self
                    .complete(provider, credential_ref, config, messages)
                    .await?,
            }],
            usage: ait_domain::RunUsage::default(),
        })
    }
    /// Invoke one LLM turn against the fixed Agent configuration.
    async fn complete(
        &self,
        provider: &AgentProvider,
        credential_ref: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError>;
}

/// Model discovery for providers authenticated by a host application rather
/// than an API credential managed by Ait.
#[async_trait]
pub trait HostProviderModelCatalog: Send + Sync {
    /// Fetch the provider's picker-visible models and their advertised
    /// capabilities without changing persisted configuration.
    async fn discover_models(
        &self,
        provider: &AgentProvider,
    ) -> Result<Vec<ProviderModel>, DomainError>;
}

/// Text history projected for a single LLM call, without an SDK dependency.
#[derive(Clone, Debug)]
pub struct ProviderMessage {
    /// Domain role: system, user, or assistant.
    pub role: String,
    /// Text content.
    pub text: String,
}

/// Codex source categories accepted by `thread/list` in the pinned stable schema.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
pub enum CodexThreadSourceKind {
    /// Interactive CLI thread.
    #[serde(rename = "cli")]
    Cli,
    /// VS Code extension thread.
    #[serde(rename = "vscode")]
    Vscode,
    /// Non-interactive `codex exec` thread.
    #[serde(rename = "exec")]
    Exec,
    /// app-server client thread.
    #[serde(rename = "appServer")]
    AppServer,
    /// Any sub-agent thread.
    #[serde(rename = "subAgent")]
    SubAgent,
    /// Review sub-agent thread.
    #[serde(rename = "subAgentReview")]
    SubAgentReview,
    /// Compaction sub-agent thread.
    #[serde(rename = "subAgentCompact")]
    SubAgentCompact,
    /// Spawned sub-agent thread.
    #[serde(rename = "subAgentThreadSpawn")]
    SubAgentThreadSpawn,
    /// Other sub-agent thread.
    #[serde(rename = "subAgentOther")]
    SubAgentOther,
    /// Unknown or forward-compatible source.
    #[serde(rename = "unknown")]
    Unknown,
}

/// Completeness marker returned with one Codex Turn's `items` field.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CodexItemsView {
    /// Items were not loaded.
    NotLoaded,
    /// Items contain only a display summary.
    Summary,
    /// Items contain the complete persisted projection.
    #[default]
    Full,
}

/// One native Codex Turn read from persisted app-server history.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnSnapshot {
    /// Native Turn identity.
    pub id: String,
    /// Native terminal or active status.
    pub status: String,
    /// Persisted `ThreadItems` in provider order.
    #[serde(default)]
    pub items: Vec<Value>,
    /// Completeness of `items`.
    #[serde(default)]
    pub items_view: CodexItemsView,
    /// Native failure payload, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<Value>,
    /// Provider start time in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    /// Provider completion time in Unix seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_at: Option<i64>,
}

/// Complete metadata and ordered Turn history for one native Codex Thread.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadSnapshot {
    /// Local ownership evidence, never accepted from provider JSON or persisted as a lease.
    #[serde(skip)]
    pub writer_confirmed: bool,
    /// Native Thread identity.
    pub id: String,
    /// Native session metadata; it is not an Ait lineage identity.
    pub session_id: String,
    /// Source Thread when this Thread was forked.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub forked_from_id: Option<String>,
    /// Working directory captured by Codex.
    pub cwd: String,
    /// Native app-server project metadata.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_id: Option<String>,
    /// User-assigned Thread name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Provider preview text.
    #[serde(default)]
    pub preview: String,
    /// String or structured native source value.
    pub source: Value,
    /// Native history storage contract.
    #[serde(default)]
    pub history_mode: String,
    /// Current process-local runtime status.
    pub status: Value,
    /// Whether this entry came from the archived listing.
    #[serde(default)]
    pub archived: bool,
    /// Provider creation time in Unix seconds.
    pub created_at: i64,
    /// Provider update time in Unix seconds.
    pub updated_at: i64,
    /// Ordered native Turns; list results normally leave this empty.
    #[serde(default)]
    pub turns: Vec<CodexTurnSnapshot>,
    /// Forward-compatible Thread metadata not modeled by the stable Ait port.
    #[serde(default, flatten)]
    pub metadata: Map<String, Value>,
}

/// Read-only authoritative Codex history boundary.
#[async_trait]
pub trait CodexHistorySource: Send + Sync {
    /// Lists every archived and non-archived Thread for explicit source categories.
    async fn list_threads(
        &self,
        source_kinds: &[CodexThreadSourceKind],
    ) -> Result<Vec<CodexThreadSnapshot>, DomainError>;

    /// Reads metadata and every Turn with `itemsView=full` in provider order.
    async fn read_thread(&self, thread_id: &str) -> Result<CodexThreadSnapshot, DomainError>;
}
