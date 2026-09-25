//! Paseo-shaped daemon configuration, status, diagnostics, update, and lifecycle messages.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Known mutable daemon configuration constraints were violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDaemonConfig;

/// Canonical daemon methods implemented by the independent server.
pub const CAPABILITIES: &[&str] = &[
    "daemon.get_status.request",
    "daemon.get_pairing_offer.request",
    "daemon.config.reload.request",
    "daemon.update.request",
    "diagnostics.request",
    "daemon.config.get.request",
    "daemon.config.set.request",
    "server.restart.request",
    "server.shutdown.request",
];

/// Request without method-specific parameters.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmptyRequest {}

/// One provider's availability in daemon status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderAvailability {
    /// Provider identifier.
    pub provider: String,
    /// Whether the provider can be started.
    pub available: bool,
    /// Safe diagnostic when unavailable.
    pub error: Option<String>,
}

/// Runtime relay configuration exposed by Paseo daemon status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelayRuntimeConfig {
    /// Whether relay connections are enabled.
    pub enabled: bool,
    /// Internal relay endpoint.
    pub endpoint: String,
    /// Endpoint clients should use.
    pub public_endpoint: String,
    /// Whether the internal endpoint uses TLS.
    pub use_tls: bool,
    /// Whether the public endpoint uses TLS.
    pub public_use_tls: bool,
}

/// Current daemon process identity and availability.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    /// Stable server identity.
    pub server_id: String,
    /// Running package version, when known.
    pub version: Option<String>,
    /// Operating-system process ID.
    pub pid: u32,
    /// Current executable path.
    pub node_path: String,
    /// RFC 3339 process start time.
    pub started_at: Option<String>,
    /// Bound listen address.
    pub listen: Option<String>,
    /// Relay runtime settings, absent when relay support is not installed.
    pub relay: Option<RelayRuntimeConfig>,
    /// Provider availability snapshot.
    pub providers: Vec<ProviderAvailability>,
}

/// Local pairing data for another client.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairingOffer {
    /// Pairing URL, empty when pairing is unavailable.
    pub url: String,
    /// Optional QR representation.
    pub qr: Option<String>,
    /// Whether the URL uses Paseo relay transport.
    pub relay_enabled: bool,
}

/// Classification returned after rereading daemon configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConfigReloadResult {
    /// Configuration paths applied to live owners.
    pub applied_paths: Vec<String>,
    /// Changed paths that require a process restart.
    pub restart_required_paths: Vec<String>,
    /// Changed paths owned by launch-time overrides.
    pub override_controlled_paths: Vec<String>,
}

/// Human-readable daemon diagnostic report.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosticsResult {
    /// Sanitized report without credentials or configuration values.
    pub diagnostic: String,
}

/// Result of attempting an installation-specific self-update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonUpdateResult {
    /// Whether an update was installed.
    pub success: bool,
    /// Safe failure description.
    pub error: Option<String>,
    /// Version before the attempt.
    pub previous_version: Option<String>,
    /// Installed version, when successful and discoverable.
    pub new_version: Option<String>,
}

/// Relay fields mutable through the daemon configuration RPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutableRelayConfig {
    /// Whether relay connections should be enabled.
    pub enabled: bool,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// MCP fields mutable through the daemon configuration RPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableMcpConfig {
    /// Whether the server's MCP endpoint is enabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// Whether configured MCP tools are injected into agents.
    pub inject_into_agents: bool,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Browser-tools fields mutable through the daemon configuration RPC.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutableBrowserToolsConfig {
    /// Whether browser tools are enabled.
    pub enabled: bool,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Configured CORS origins.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableCorsConfig {
    /// Exact origins allowed by the daemon.
    pub allowed_origins: Vec<String>,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Git command concurrency limits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableGitConfig {
    /// Maximum process starts per second.
    pub max_processes_per_second: u32,
    /// Maximum simultaneous Git processes.
    pub max_process_concurrency: u32,
}

/// Client application settings surfaced with daemon configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableAppConfig {
    /// Application base URL.
    pub base_url: String,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Additional model registered for a provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableProviderModel {
    /// Provider-specific model ID.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Optional description.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Whether this model is the provider default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_default: Option<bool>,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Mutable configuration for one provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableProviderConfig {
    /// Paseo tool policy, whose provider-specific shape remains extensible.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub paseo_tools: Option<Value>,
    /// Explicit provider enablement.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enabled: Option<bool>,
    /// User-defined models.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub additional_models: Option<Vec<MutableProviderModel>>,
    /// Forward-compatible provider fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// One provider candidate used for structured metadata generation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MutableMetadataProvider {
    /// Provider identifier.
    pub provider: String,
    /// Optional model ID.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    /// Optional provider thinking option.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thinking_option_id: Option<String>,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// Structured metadata generation settings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct MutableMetadataGenerationConfig {
    /// Ordered provider candidates.
    pub providers: Vec<MutableMetadataProvider>,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// A boolean or a list of host/proxy names, as accepted by Paseo.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum BooleanOrStrings {
    /// Enable the built-in set.
    Enabled(bool),
    /// Explicit values.
    Values(Vec<String>),
}

/// Mutable daemon configuration returned to clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfig {
    /// Relay configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay: Option<MutableRelayConfig>,
    /// MCP configuration.
    pub mcp: MutableMcpConfig,
    /// Allowed host names.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hostnames: Option<BooleanOrStrings>,
    /// CORS configuration.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cors: Option<MutableCorsConfig>,
    /// Trusted reverse proxies.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trusted_proxies: Option<BooleanOrStrings>,
    /// Git process limits.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub git: Option<MutableGitConfig>,
    /// Client application settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub app: Option<MutableAppConfig>,
    /// Provider catalog refresh timeout.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub catalog_refresh_timeout_ms: Option<u64>,
    /// Browser tool configuration.
    pub browser_tools: MutableBrowserToolsConfig,
    /// Provider overrides.
    pub providers: BTreeMap<String, MutableProviderConfig>,
    /// Metadata generation settings.
    pub metadata_generation: MutableMetadataGenerationConfig,
    /// Whether merged workspaces are archived automatically.
    pub auto_archive_after_merge: bool,
    /// Whether terminal agent hooks are enabled.
    pub enable_terminal_agent_hooks: bool,
    /// Text appended to the system prompt.
    pub append_system_prompt: String,
    /// Paseo terminal profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_profiles: Option<Vec<Value>>,
    /// Paseo agent profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_profiles: Option<Vec<Value>>,
    /// Skill selection settings.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub skills: Option<Value>,
    /// Global plugin switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins_enabled: Option<bool>,
    /// Plugin source definitions.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<BTreeMap<String, Value>>,
    /// Forward-compatible Paseo fields.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        Self {
            relay: Some(MutableRelayConfig {
                enabled: false,
                extra: Map::new(),
            }),
            mcp: MutableMcpConfig {
                enabled: Some(true),
                inject_into_agents: false,
                extra: Map::new(),
            },
            hostnames: None,
            cors: Some(MutableCorsConfig {
                allowed_origins: Vec::new(),
                extra: Map::new(),
            }),
            trusted_proxies: Some(BooleanOrStrings::Values(vec!["loopback".to_owned()])),
            git: Some(MutableGitConfig {
                max_processes_per_second: 64,
                max_process_concurrency: 8,
            }),
            app: None,
            catalog_refresh_timeout_ms: None,
            browser_tools: MutableBrowserToolsConfig {
                enabled: false,
                extra: Map::new(),
            },
            providers: BTreeMap::new(),
            metadata_generation: MutableMetadataGenerationConfig {
                providers: Vec::new(),
                extra: Map::new(),
            },
            auto_archive_after_merge: false,
            enable_terminal_agent_hooks: false,
            append_system_prompt: String::new(),
            terminal_profiles: None,
            agent_profiles: None,
            skills: None,
            plugins_enabled: Some(false),
            plugins: Some(BTreeMap::new()),
            extra: Map::new(),
        }
    }
}

impl DaemonConfig {
    /// Validate constraints enforced by Paseo's mutable configuration schema.
    ///
    /// # Errors
    /// Returns an error for empty IDs or non-positive limits.
    pub fn validate(&self) -> Result<(), InvalidDaemonConfig> {
        if self.git.as_ref().is_some_and(|git| {
            git.max_processes_per_second == 0 || git.max_process_concurrency == 0
        }) || self.catalog_refresh_timeout_ms == Some(0)
            || self.providers.iter().any(|(id, config)| {
                id.is_empty()
                    || config.additional_models.as_ref().is_some_and(|models| {
                        models
                            .iter()
                            .any(|model| model.id.is_empty() || model.label.is_empty())
                    })
            })
            || self.metadata_generation.providers.iter().any(|entry| {
                entry.provider.is_empty()
                    || entry.model.as_ref().is_some_and(String::is_empty)
                    || entry
                        .thinking_option_id
                        .as_ref()
                        .is_some_and(String::is_empty)
            })
        {
            return Err(InvalidDaemonConfig);
        }
        Ok(())
    }
}

/// Partial daemon configuration accepted by `daemon.config.set.request`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DaemonConfigPatch {
    /// Relay patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub relay: Option<Value>,
    /// MCP patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mcp: Option<Value>,
    /// Browser tools patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub browser_tools: Option<Value>,
    /// Provider patches.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub providers: Option<BTreeMap<String, Value>>,
    /// Provider IDs to remove.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remove_providers: Option<Vec<String>>,
    /// Metadata generation patch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metadata_generation: Option<Value>,
    /// Automatic archive setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auto_archive_after_merge: Option<bool>,
    /// Terminal hook setting.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub enable_terminal_agent_hooks: Option<bool>,
    /// System prompt suffix.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub append_system_prompt: Option<String>,
    /// Replacement terminal profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub terminal_profiles: Option<Vec<Value>>,
    /// Replacement agent profiles.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent_profiles: Option<Vec<Value>>,
    /// Global plugin switch.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins_enabled: Option<bool>,
    /// Replacement plugin sources.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugins: Option<BTreeMap<String, Value>>,
    /// Unknown fields accepted by Paseo and ignored by its supported-patch picker.
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl DaemonConfigPatch {
    /// Validate the portions of the partial schema represented as extensible JSON.
    ///
    /// # Errors
    /// Returns an error when known fields have the wrong type or empty identifiers.
    pub fn validate(&self) -> Result<(), InvalidDaemonConfig> {
        validate_optional_object(self.relay.as_ref())?;
        validate_optional_object(self.mcp.as_ref())?;
        validate_optional_object(self.browser_tools.as_ref())?;
        validate_optional_object(self.metadata_generation.as_ref())?;
        validate_optional_bool(self.relay.as_ref(), "enabled")?;
        validate_optional_bool(self.mcp.as_ref(), "injectIntoAgents")?;
        validate_optional_bool(self.browser_tools.as_ref(), "enabled")?;
        if let Some(metadata) = self.metadata_generation.as_ref() {
            let metadata = metadata.as_object().ok_or(InvalidDaemonConfig)?;
            if let Some(providers) = metadata.get("providers") {
                let providers = providers.as_array().ok_or(InvalidDaemonConfig)?;
                if providers.iter().any(|provider| {
                    provider
                        .as_object()
                        .and_then(|provider| provider.get("provider"))
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
                }) {
                    return Err(InvalidDaemonConfig);
                }
            }
        }
        if self.providers.as_ref().is_some_and(|providers| {
            providers
                .iter()
                .any(|(id, value)| id.is_empty() || invalid_provider_patch(value))
        }) || self
            .remove_providers
            .as_ref()
            .is_some_and(|providers| providers.iter().any(String::is_empty))
        {
            return Err(InvalidDaemonConfig);
        }
        Ok(())
    }
}

fn validate_optional_object(value: Option<&Value>) -> Result<(), InvalidDaemonConfig> {
    if value.is_some_and(|value| !value.is_object()) {
        return Err(InvalidDaemonConfig);
    }
    Ok(())
}

fn validate_optional_bool(value: Option<&Value>, field: &str) -> Result<(), InvalidDaemonConfig> {
    if value
        .and_then(Value::as_object)
        .and_then(|value| value.get(field))
        .is_some_and(|value| !value.is_boolean())
    {
        return Err(InvalidDaemonConfig);
    }
    Ok(())
}

fn invalid_provider_patch(value: &Value) -> bool {
    let Some(provider) = value.as_object() else {
        return true;
    };
    if provider
        .get("enabled")
        .is_some_and(|value| !value.is_boolean())
    {
        return true;
    }
    provider.get("additionalModels").is_some_and(|models| {
        models.as_array().is_none_or(|models| {
            models.iter().any(|model| {
                let Some(model) = model.as_object() else {
                    return true;
                };
                model
                    .get("id")
                    .and_then(Value::as_str)
                    .is_none_or(str::is_empty)
                    || model
                        .get("label")
                        .and_then(Value::as_str)
                        .is_none_or(str::is_empty)
            })
        })
    })
}

/// Daemon configuration response.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfigResult {
    /// Current normalized mutable configuration.
    pub config: DaemonConfig,
}

/// Daemon configuration mutation request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonConfigSetRequest {
    /// Partial configuration to merge.
    pub config: DaemonConfigPatch,
}

/// Server restart request.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RestartRequest {
    /// Optional caller-supplied diagnostic reason.
    #[serde(default)]
    pub reason: Option<String>,
}

/// Accepted server lifecycle request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LifecycleResult {
    /// `restart_requested` or `shutdown_requested`.
    pub status: String,
    /// Normalized restart reason, when relevant.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[cfg(test)]
mod tests;
