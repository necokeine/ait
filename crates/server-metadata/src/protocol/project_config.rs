//! Paseo project configuration RPC payloads and raw `paseo.json` schema.

use serde::{Deserialize, Deserializer, Serialize, Serializer};
use serde_json::{Map, Value};

/// Methods for editing a registered project's `paseo.json`.
pub const CAPABILITIES: &[&str] = &[
    "project.config.read.request",
    "project.config.write.request",
];

/// Raw project configuration with Paseo's passthrough and normalization behavior.
#[derive(Debug, Clone, PartialEq)]
pub struct PaseoConfigRaw(Value);

impl PaseoConfigRaw {
    /// Validate and normalize a JSON value using the source schema.
    ///
    /// # Errors
    /// Returns an error when a strict known field has the wrong shape.
    pub fn new(value: Value) -> Result<Self, &'static str> {
        normalize_config(value).map(Self)
    }

    /// Borrow the normalized JSON object.
    #[must_use]
    pub const fn value(&self) -> &Value {
        &self.0
    }

    /// Consume the wrapper and return its normalized JSON object.
    #[must_use]
    pub fn into_value(self) -> Value {
        self.0
    }
}

impl<'de> Deserialize<'de> for PaseoConfigRaw {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Self::new(Value::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

impl Serialize for PaseoConfigRaw {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.0.serialize(serializer)
    }
}

/// Optimistic concurrency revision for `paseo.json`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PaseoConfigRevision {
    /// Last modification time in Unix milliseconds.
    pub mtime_ms: f64,
    /// File size in bytes, represented as a JSON number by Paseo.
    pub size: f64,
}

/// Read the configuration of a known active project root.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigReadRequest {
    /// Registered project root or a realpath-equivalent alias.
    pub repo_root: String,
}

/// Write a validated configuration with an expected revision.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigWriteRequest {
    /// Registered project root or a realpath-equivalent alias.
    pub repo_root: String,
    /// Validated raw configuration.
    pub config: PaseoConfigRaw,
    /// Null means the file is expected not to exist.
    #[serde(deserialize_with = "required_nullable")]
    pub expected_revision: Option<PaseoConfigRevision>,
}

/// Successful project configuration read.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigReadSuccess {
    /// Canonical registered root.
    pub repo_root: String,
    /// Missing files return null.
    pub config: Option<PaseoConfigRaw>,
    /// Missing files return null.
    pub revision: Option<PaseoConfigRevision>,
}

/// Successful project configuration write.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectConfigWriteSuccess {
    /// Canonical registered root.
    pub repo_root: String,
    /// Normalized configuration written to disk.
    pub config: PaseoConfigRaw,
    /// Revision of the installed file.
    pub revision: PaseoConfigRevision,
}

/// Inline business error used by both configuration operations.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum ProjectConfigRpcError {
    /// The requested root is not an active registered project.
    ProjectNotFound,
    /// The saved or requested document violates the config schema.
    InvalidProjectConfig,
    /// The file changed since the caller read it.
    StaleProjectConfig {
        /// Current on-disk revision, or null when it was removed.
        #[serde(rename = "currentRevision")]
        current_revision: Option<PaseoConfigRevision>,
    },
    /// Atomic file installation failed.
    WriteFailed,
}

/// Read response matching Paseo's boolean-discriminated result.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectConfigReadResult {
    /// The file was read, including the missing-file state.
    Success {
        /// Canonical registered root.
        repo_root: String,
        /// Missing files return null.
        config: Option<PaseoConfigRaw>,
        /// Missing files return null.
        revision: Option<PaseoConfigRevision>,
    },
    /// The request was rejected without a transport failure.
    Failure {
        /// Request root, or canonical root after resolution.
        repo_root: String,
        /// Stable inline failure.
        error: ProjectConfigRpcError,
    },
}

impl Serialize for ProjectConfigReadResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Success<'a> {
            ok: bool,
            repo_root: &'a str,
            config: &'a Option<PaseoConfigRaw>,
            revision: &'a Option<PaseoConfigRevision>,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Failure<'a> {
            ok: bool,
            repo_root: &'a str,
            error: &'a ProjectConfigRpcError,
        }
        match self {
            Self::Success {
                repo_root,
                config,
                revision,
            } => Success {
                ok: true,
                repo_root,
                config,
                revision,
            }
            .serialize(serializer),
            Self::Failure { repo_root, error } => Failure {
                ok: false,
                repo_root,
                error,
            }
            .serialize(serializer),
        }
    }
}

/// Write response matching Paseo's boolean-discriminated result.
#[derive(Debug, Clone, PartialEq)]
pub enum ProjectConfigWriteResult {
    /// The validated file was atomically installed.
    Success {
        /// Canonical registered root.
        repo_root: String,
        /// Normalized configuration written to disk.
        config: PaseoConfigRaw,
        /// Revision of the installed file.
        revision: PaseoConfigRevision,
    },
    /// The request was rejected without a transport failure.
    Failure {
        /// Request root, or canonical root after resolution.
        repo_root: String,
        /// Stable inline failure.
        error: ProjectConfigRpcError,
    },
}

impl Serialize for ProjectConfigWriteResult {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Success<'a> {
            ok: bool,
            repo_root: &'a str,
            config: &'a PaseoConfigRaw,
            revision: &'a PaseoConfigRevision,
        }
        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Failure<'a> {
            ok: bool,
            repo_root: &'a str,
            error: &'a ProjectConfigRpcError,
        }
        match self {
            Self::Success {
                repo_root,
                config,
                revision,
            } => Success {
                ok: true,
                repo_root,
                config,
                revision,
            }
            .serialize(serializer),
            Self::Failure { repo_root, error } => Failure {
                ok: false,
                repo_root,
                error,
            }
            .serialize(serializer),
        }
    }
}

fn normalize_config(value: Value) -> Result<Value, &'static str> {
    let Value::Object(mut config) = value else {
        return Err("project config must be an object");
    };
    if let Some(worktree) = config.get_mut("worktree") {
        normalize_worktree(worktree)?;
    }
    if let Some(scripts) = config.get("scripts") {
        let Value::Object(scripts) = scripts else {
            return Err("scripts must be an object");
        };
        if scripts.values().any(|entry| !entry.is_object()) {
            return Err("script entries must be objects");
        }
    }
    if let Some(metadata) = config.get_mut("metadataGeneration") {
        normalize_metadata(metadata);
    }
    Ok(Value::Object(config))
}

fn required_nullable<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::deserialize(deserializer)
}

fn normalize_worktree(value: &mut Value) -> Result<(), &'static str> {
    let Value::Object(worktree) = value else {
        return Err("worktree must be an object");
    };
    for key in ["setup", "teardown"] {
        if let Some(commands) = worktree.get(key)
            && !matches!(commands, Value::String(_))
            && !commands
                .as_array()
                .is_some_and(|commands| commands.iter().all(Value::is_string))
        {
            return Err("lifecycle commands must be a string or string array");
        }
    }
    if let Some(ports) = worktree.get_mut("servicePorts") {
        normalize_service_ports(ports)?;
    }
    Ok(())
}

fn normalize_service_ports(value: &mut Value) -> Result<(), &'static str> {
    let Value::Object(ports) = value else {
        return Err("servicePorts must be an object");
    };
    if ports
        .keys()
        .any(|key| !matches!(key.as_str(), "range" | "portScript"))
    {
        return Err("servicePorts contains an unknown field");
    }
    for key in ["range", "portScript"] {
        if let Some(value) = ports.get_mut(key) {
            let Value::String(text) = value else {
                return Err("servicePorts values must be strings");
            };
            *text = text.trim().to_owned();
            if text.is_empty() {
                return Err("servicePorts values cannot be empty");
            }
        }
    }
    if ports.is_empty() {
        return Err("servicePorts requires range or portScript");
    }
    if let Some(Value::String(range)) = ports.get("range") {
        let Some((start, end)) = range.split_once('-') else {
            return Err("invalid service port range");
        };
        if start.is_empty()
            || end.is_empty()
            || start.len() > 5
            || end.len() > 5
            || !start.bytes().all(|byte| byte.is_ascii_digit())
            || !end.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Err("invalid service port range");
        }
        let start = start
            .parse::<u32>()
            .map_err(|_| "invalid service port range")?;
        let end = end
            .parse::<u32>()
            .map_err(|_| "invalid service port range")?;
        if start == 0 || end > 65_535 || start > end {
            return Err("invalid service port range");
        }
    }
    Ok(())
}

fn normalize_metadata(value: &mut Value) {
    let Value::Object(metadata) = value else {
        *value = Value::Object(Map::new());
        return;
    };
    for key in ["title", "branchName", "commitMessage", "pullRequest"] {
        if let Some(entry) = metadata.get_mut(key) {
            let valid = entry
                .as_object()
                .is_some_and(|entry| entry.get("instructions").is_none_or(Value::is_string));
            if !valid {
                *entry = Value::Object(Map::new());
            }
        }
    }
}

#[cfg(test)]
mod tests;
