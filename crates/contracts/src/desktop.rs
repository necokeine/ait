use std::collections::BTreeMap;

use ait_domain::{Message, MessageKind, MessageRole, ProjectedMessage, Session};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// Desktop protocol revision. Major changes require a handshake failure.
pub const DESKTOP_PROTOCOL_VERSION: u32 = 1;

/// Read-only Project information needed by the desktop navigation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopProject {
    /// Stable Project identity.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Canonical local work directory.
    pub workdir: String,
    /// Optional descriptive context.
    pub description: String,
}

/// Runnable Agent choice exposed to desktop users.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentSummary {
    /// Stable Agent identity.
    pub id: String,
    /// Human-readable label.
    pub name: String,
    /// Provider-independent model label.
    pub model: String,
    /// Whether new Runs may be started.
    pub enabled: bool,
}

/// Session projection for lists and active-head markers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSession {
    /// Stable Session identity.
    pub id: String,
    /// Owning Project.
    pub project_id: String,
    /// Display title.
    pub title: String,
    /// Current immutable Message pointer.
    pub current_message_id: String,
    /// Fixed Agent binding.
    pub agent_id: String,
    /// Optimistic-lock version.
    pub version: u64,
    /// Whether a Run currently follows this Session.
    pub active: bool,
    /// Last update time in milliseconds since Unix epoch.
    pub updated_at: i64,
}

impl From<&Session> for DesktopSession {
    fn from(session: &Session) -> Self {
        Self {
            id: session.id.as_str().to_owned(),
            project_id: session.project_id.as_str().to_owned(),
            title: session
                .title
                .clone()
                .unwrap_or_else(|| session.name.clone()),
            current_message_id: session.current_message_id.to_string(),
            agent_id: session.agent_id.as_str().to_owned(),
            version: session.version,
            active: session.active_run_id.is_some(),
            updated_at: session.updated_at.get(),
        }
    }
}

/// One safe, renderer-facing Message content item.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum DesktopMessagePart {
    /// Plain text.
    Text {
        /// Visible text content.
        text: String,
    },
    /// Attachment reference without local file-system authority.
    File {
        /// Safe display name, not a local path.
        name: String,
        /// MIME media type.
        media_type: String,
    },
    /// Tool invocation with bounded JSON arguments.
    ToolUse {
        /// Provider-stable call identity.
        call_id: String,
        /// Registered tool name.
        tool_name: String,
        /// Bounded JSON arguments.
        arguments: String,
    },
    /// Typed structured data.
    Structured {
        /// Structured payload media type.
        media_type: String,
        /// Canonical bounded payload.
        value: String,
    },
    /// Redaction marker preserving graph shape.
    Redacted,
}

/// Immutable Message projection used to build a Project-wide tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopMessage {
    /// Stable Message identity.
    pub id: String,
    /// Owning Project.
    pub project_id: String,
    /// Parent edge, absent for a root.
    pub parent_message_id: Option<String>,
    /// Stable protocol role.
    pub role: MessageRole,
    /// Stable Message kind.
    pub kind: MessageKind,
    /// Ordered safe parts.
    pub parts: Vec<DesktopMessagePart>,
    /// Creation time in milliseconds since Unix epoch.
    pub created_at: i64,
}

impl From<ProjectedMessage> for DesktopMessage {
    fn from(projected: ProjectedMessage) -> Self {
        match projected {
            ProjectedMessage::Visible(message) => Self::from(message),
            ProjectedMessage::Redacted {
                id,
                project_id,
                parent_message_id,
                role,
            } => Self {
                id: id.to_string(),
                project_id: project_id.as_str().to_owned(),
                parent_message_id: parent_message_id.map(|id| id.to_string()),
                role,
                kind: MessageKind::Standard,
                parts: vec![DesktopMessagePart::Redacted],
                created_at: 0,
            },
        }
    }
}

impl From<Message> for DesktopMessage {
    fn from(message: Message) -> Self {
        let parts = message
            .sub_messages
            .into_iter()
            .map(|part| match part {
                ait_domain::SubMessage::Text { text } => DesktopMessagePart::Text { text },
                ait_domain::SubMessage::FileRef {
                    media_type, name, ..
                } => DesktopMessagePart::File {
                    name: name.unwrap_or_else(|| "Attachment".into()),
                    media_type,
                },
                ait_domain::SubMessage::ToolUse(tool) => DesktopMessagePart::ToolUse {
                    call_id: tool.call_id,
                    tool_name: tool.tool_name,
                    arguments: tool.arguments,
                },
                ait_domain::SubMessage::StructuredData { media_type, value } => {
                    DesktopMessagePart::Structured { media_type, value }
                }
            })
            .collect();
        Self {
            id: message.id.to_string(),
            project_id: message.project_id.as_str().to_owned(),
            parent_message_id: message.parent_message_id.map(|id| id.to_string()),
            role: message.role,
            kind: message.kind,
            parts,
            created_at: message.created_at.get(),
        }
    }
}

/// Complete, bounded desktop projection returned after every mutation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSnapshot {
    /// Protocol version understood by the Rust backend.
    pub protocol_version: u32,
    /// Monotonic state revision.
    pub revision: u64,
    /// Projects available to this local profile.
    pub projects: Vec<DesktopProject>,
    /// Configured Agents safe to display.
    pub agents: Vec<AgentSummary>,
    /// Session references.
    pub sessions: Vec<DesktopSession>,
    /// Immutable Message forest.
    pub messages: Vec<DesktopMessage>,
}

/// High-level settings section.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettingCategory {
    /// Provider and model selection.
    Models,
    /// Agent execution defaults.
    Agents,
    /// Runtime behavior and limits.
    Runtime,
    /// Approval and sandbox policy.
    Permissions,
    /// Project and work-directory defaults.
    Projects,
    /// Network and proxy behavior.
    Network,
    /// Diagnostic output.
    Logging,
    /// Desktop-only presentation preference.
    Interface,
}

/// Renderer control type. Validation remains in Rust.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SettingKind {
    /// Free-form string.
    Text,
    /// Bounded integral number.
    Number {
        /// Inclusive lower bound.
        min: i64,
        /// Inclusive upper bound.
        max: i64,
    },
    /// Boolean toggle.
    Boolean,
    /// Enumerated choice.
    Select {
        /// Allowed stable string values.
        options: Vec<String>,
    },
    /// Local directory selector.
    Path,
    /// Reference into the host credential store; never a secret value.
    CredentialReference,
}

/// One centrally defined setting and its default.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingDefinition {
    /// Stable dotted key.
    pub id: String,
    /// Settings section.
    pub category: SettingCategory,
    /// Short display label.
    pub label: String,
    /// Explanatory copy.
    pub description: String,
    /// Input and validation shape.
    pub kind: SettingKind,
    /// Backend default value.
    pub default_value: Value,
    /// Whether a restart is needed after saving.
    pub restart_required: bool,
}

/// Settings schema supplied by Rust rather than duplicated in Electron.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsSchema {
    /// Schema revision for compatibility and reset behavior.
    pub revision: u32,
    /// Ordered settings definitions.
    pub definitions: Vec<SettingDefinition>,
}

/// Persisted non-secret settings values.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SettingsDocument(pub BTreeMap<String, Value>);

/// Settings schema and values persisted by the daemon control plane.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    /// Rust-owned schema used to render and validate controls.
    pub schema: SettingsSchema,
    /// Current non-secret values.
    pub values: SettingsDocument,
    /// Settings-specific optimistic revision.
    pub revision: u64,
}

/// Atomic branch operation requested by the desktop.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ForkFromMessageRequest {
    /// Project owning the selected Message.
    pub project_id: String,
    /// Selected immutable Message.
    pub source_message_id: String,
    /// Agent to bind to the new Session.
    pub agent_id: String,
    /// First user input on the new branch.
    pub content: String,
}

/// Complete settings replacement guarded by the loaded revision.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsRequest {
    /// State revision observed by the editor.
    pub expected_revision: u64,
    /// Values keyed by schema identifier.
    pub values: SettingsDocument,
}

/// Returns the authoritative settings schema for the current core revision.
#[must_use]
pub fn settings_schema() -> SettingsSchema {
    let mut definitions = model_settings();
    definitions.extend(execution_settings());
    definitions.extend(environment_settings());
    definitions.extend(interface_settings());
    SettingsSchema {
        revision: 1,
        definitions,
    }
}

fn model_settings() -> Vec<SettingDefinition> {
    vec![
        setting(
            "models.default",
            SettingCategory::Models,
            "Default model",
            "Model used when a new Agent does not override it.",
            SettingKind::Text,
            json!("gpt-5.6-codex"),
            false,
        ),
        setting(
            "models.provider",
            SettingCategory::Models,
            "Provider",
            "Provider adapter used for new Agents.",
            SettingKind::Select {
                options: vec!["openai".into(), "openai_compatible".into(), "local".into()],
            },
            json!("openai"),
            true,
        ),
        setting(
            "models.endpoint",
            SettingCategory::Models,
            "Provider endpoint",
            "Optional OpenAI-compatible API endpoint.",
            SettingKind::Text,
            json!(""),
            true,
        ),
        setting(
            "models.credential_ref",
            SettingCategory::Models,
            "Credential reference",
            "Name of a host credential entry. Secret material is never returned to the renderer.",
            SettingKind::CredentialReference,
            json!(""),
            true,
        ),
    ]
}

fn execution_settings() -> Vec<SettingDefinition> {
    vec![
        setting(
            "agents.max_steps",
            SettingCategory::Agents,
            "Maximum steps",
            "Default persisted Agent/tool step limit per Run.",
            SettingKind::Number {
                min: 1,
                max: 10_000,
            },
            json!(128),
            false,
        ),
        setting(
            "agents.parallel_tools",
            SettingCategory::Agents,
            "Parallel tools",
            "Maximum tool calls that may execute concurrently.",
            SettingKind::Number { min: 1, max: 32 },
            json!(4),
            false,
        ),
        setting(
            "runtime.shutdown_grace_seconds",
            SettingCategory::Runtime,
            "Shutdown grace",
            "Time allowed for supervised work to checkpoint before exit.",
            SettingKind::Number { min: 1, max: 300 },
            json!(20),
            true,
        ),
        setting(
            "runtime.recovery",
            SettingCategory::Runtime,
            "Recovery policy",
            "How interrupted Runs are handled after daemon restart.",
            SettingKind::Select {
                options: vec!["resume_safe".into(), "ask".into(), "fail".into()],
            },
            json!("resume_safe"),
            false,
        ),
        setting(
            "permissions.approval",
            SettingCategory::Permissions,
            "Approval mode",
            "Controls when tools require explicit approval.",
            SettingKind::Select {
                options: vec![
                    "on_request".into(),
                    "untrusted_only".into(),
                    "always".into(),
                ],
            },
            json!("on_request"),
            false,
        ),
        setting(
            "permissions.sandbox",
            SettingCategory::Permissions,
            "Sandbox profile",
            "Default operating-system isolation profile for tools.",
            SettingKind::Select {
                options: vec![
                    "workspace_write".into(),
                    "read_only".into(),
                    "strict".into(),
                ],
            },
            json!("workspace_write"),
            false,
        ),
    ]
}

fn environment_settings() -> Vec<SettingDefinition> {
    vec![
        setting(
            "projects.default_workdir",
            SettingCategory::Projects,
            "Default work directory",
            "Directory offered when creating a Project.",
            SettingKind::Path,
            json!(""),
            false,
        ),
        setting(
            "network.proxy",
            SettingCategory::Network,
            "HTTP proxy",
            "Optional proxy URL. Credentials should be stored as a credential reference.",
            SettingKind::Text,
            json!(""),
            true,
        ),
        setting(
            "logging.level",
            SettingCategory::Logging,
            "Log level",
            "Minimum structured diagnostic level.",
            SettingKind::Select {
                options: vec!["error".into(), "warn".into(), "info".into(), "debug".into()],
            },
            json!("info"),
            false,
        ),
        setting(
            "logging.retention_days",
            SettingCategory::Logging,
            "Log retention",
            "Days to retain local redacted diagnostic logs.",
            SettingKind::Number { min: 1, max: 90 },
            json!(14),
            false,
        ),
    ]
}

fn interface_settings() -> Vec<SettingDefinition> {
    vec![
        setting(
            "interface.theme",
            SettingCategory::Interface,
            "Theme",
            "Desktop color scheme.",
            SettingKind::Select {
                options: vec!["system".into(), "light".into(), "dark".into()],
            },
            json!("system"),
            false,
        ),
        setting(
            "interface.density",
            SettingCategory::Interface,
            "Density",
            "Information density of navigation and conversation surfaces.",
            SettingKind::Select {
                options: vec!["comfortable".into(), "compact".into()],
            },
            json!("compact"),
            false,
        ),
        setting(
            "interface.session_tree_open",
            SettingCategory::Interface,
            "Open session tree",
            "Show the auxiliary Message tree when the app starts.",
            SettingKind::Boolean,
            json!(true),
            false,
        ),
    ]
}

fn setting(
    id: &str,
    category: SettingCategory,
    label: &str,
    description: &str,
    kind: SettingKind,
    default_value: Value,
    restart_required: bool,
) -> SettingDefinition {
    SettingDefinition {
        id: id.into(),
        category,
        label: label.into(),
        description: description.into(),
        kind,
        default_value,
        restart_required,
    }
}

/// Produces the backend defaults matching [`settings_schema`].
#[must_use]
pub fn default_settings() -> SettingsDocument {
    SettingsDocument(
        settings_schema()
            .definitions
            .into_iter()
            .map(|definition| (definition.id, definition.default_value))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_cover_every_setting_once() {
        let schema = settings_schema();
        let defaults = default_settings();
        assert_eq!(schema.definitions.len(), defaults.0.len());
        assert!(
            schema
                .definitions
                .iter()
                .all(|item| defaults.0.contains_key(&item.id))
        );
    }

    #[test]
    fn credential_setting_is_a_reference_not_secret_material() {
        let credential = settings_schema()
            .definitions
            .into_iter()
            .find(|item| item.id == "models.credential_ref")
            .unwrap();
        assert_eq!(credential.kind, SettingKind::CredentialReference);
        assert_eq!(credential.default_value, json!(""));
    }
}
