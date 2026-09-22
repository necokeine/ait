//! Atomic JSON persistence for the independent daemon's mutable configuration.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard};

use serde_json::{Map, Value};
use server_ports::daemon::{DaemonConfigReload, DaemonConfigStore, DaemonConfigStoreError};

const MAX_CONFIG_BYTES: u64 = 4 * 1024 * 1024;
const PATCH_FIELDS: &[&str] = &[
    "relay",
    "mcp",
    "browserTools",
    "providers",
    "metadataGeneration",
    "autoArchiveAfterMerge",
    "enableTerminalAgentHooks",
    "appendSystemPrompt",
    "terminalProfiles",
    "agentProfiles",
    "pluginsEnabled",
    "plugins",
];

/// File-backed daemon configuration stored at the Paseo-compatible `config.json` path.
#[derive(Debug)]
pub struct FileDaemonConfigStore {
    path: PathBuf,
    default: Value,
    state: Mutex<Option<Value>>,
}

impl FileDaemonConfigStore {
    /// Create a lazy store with the independent server's Paseo-shaped defaults.
    #[must_use]
    pub fn with_defaults(path: PathBuf) -> Self {
        Self::new(
            path,
            serde_json::json!({
                "relay":{"enabled":false},
                "mcp":{"enabled":true,"injectIntoAgents":false},
                "browserTools":{"enabled":false},
                "providers":{},
                "metadataGeneration":{"providers":[]},
                "autoArchiveAfterMerge":false,
                "enableTerminalAgentHooks":false,
                "appendSystemPrompt":"",
                "cors":{"allowedOrigins":[]},
                "trustedProxies":["loopback"],
                "git":{"maxProcessesPerSecond":64,"maxProcessConcurrency":8},
                "pluginsEnabled":false,
                "plugins":{}
            }),
        )
    }

    /// Create a lazy store. The file is initialized on the first operation.
    #[must_use]
    pub fn new(path: PathBuf, default: Value) -> Self {
        Self {
            path,
            default,
            state: Mutex::new(None),
        }
    }

    fn lock(&self) -> Result<MutexGuard<'_, Option<Value>>, DaemonConfigStoreError> {
        self.state.lock().map_err(|_| DaemonConfigStoreError::Io)
    }

    fn current<'a>(
        &'a self,
        state: &'a mut Option<Value>,
    ) -> Result<&'a Value, DaemonConfigStoreError> {
        if state.is_none() {
            let value = match read(&self.path) {
                Ok(value) => value,
                Err(DaemonConfigStoreError::Io) if !self.path.exists() => {
                    write_atomic(&self.path, &self.default)?;
                    self.default.clone()
                }
                Err(error) => return Err(error),
            };
            *state = Some(value);
        }
        state.as_ref().ok_or(DaemonConfigStoreError::Io)
    }
}

impl DaemonConfigStore for FileDaemonConfigStore {
    fn get(&self) -> Result<Value, DaemonConfigStoreError> {
        let mut state = self.lock()?;
        self.current(&mut state).cloned()
    }

    fn patch(&self, patch: &Value) -> Result<Value, DaemonConfigStoreError> {
        let patch = patch.as_object().ok_or(DaemonConfigStoreError::Invalid)?;
        let mut state = self.lock()?;
        let mut next = self.current(&mut state)?.clone();
        let next_object = next
            .as_object_mut()
            .ok_or(DaemonConfigStoreError::Invalid)?;
        for field in PATCH_FIELDS {
            if let Some(value) = patch.get(*field) {
                if matches!(
                    *field,
                    "relay" | "mcp" | "browserTools" | "providers" | "metadataGeneration"
                ) {
                    merge_field(next_object, field, value)?;
                } else {
                    next_object.insert((*field).to_owned(), value.clone());
                }
            }
        }
        remove_providers(next_object, patch.get("removeProviders"))?;
        write_atomic(&self.path, &next)?;
        *state = Some(next.clone());
        Ok(next)
    }

    fn reload(&self) -> Result<DaemonConfigReload, DaemonConfigStoreError> {
        let next = read(&self.path)?;
        let mut state = self.lock()?;
        let current = self.current(&mut state)?;
        let mut changed = Vec::new();
        diff_paths(current, &next, "", &mut changed);
        let mut applied = BTreeSet::new();
        let mut restart = BTreeSet::new();
        for path in changed {
            if let Some(mapped) = reloadable_path(&path) {
                applied.insert(mapped);
            } else {
                restart.insert(path);
            }
        }
        *state = Some(next.clone());
        Ok(DaemonConfigReload {
            config: next,
            applied_paths: applied.into_iter().collect(),
            restart_required_paths: restart.into_iter().collect(),
            override_controlled_paths: Vec::new(),
        })
    }
}

fn read(path: &Path) -> Result<Value, DaemonConfigStoreError> {
    reject_symlink(path)?;
    let metadata = path.metadata().map_err(|_| DaemonConfigStoreError::Io)?;
    if !metadata.is_file() || metadata.len() > MAX_CONFIG_BYTES {
        return Err(DaemonConfigStoreError::Invalid);
    }
    let value: Value =
        serde_json::from_reader(File::open(path).map_err(|_| DaemonConfigStoreError::Io)?)
            .map_err(|_| DaemonConfigStoreError::Invalid)?;
    if !value.is_object() {
        return Err(DaemonConfigStoreError::Invalid);
    }
    validate_config(&value)?;
    Ok(value)
}

fn write_atomic(path: &Path, value: &Value) -> Result<(), DaemonConfigStoreError> {
    validate_config(value)?;
    reject_symlink(path)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent).map_err(|_| DaemonConfigStoreError::Io)?;
    let mut temporary =
        tempfile::NamedTempFile::new_in(parent).map_err(|_| DaemonConfigStoreError::Io)?;
    serde_json::to_writer_pretty(temporary.as_file_mut(), value)
        .map_err(|_| DaemonConfigStoreError::Invalid)?;
    temporary
        .as_file_mut()
        .write_all(b"\n")
        .and_then(|()| temporary.as_file_mut().sync_all())
        .map_err(|_| DaemonConfigStoreError::Io)?;
    temporary
        .persist(path)
        .map_err(|_| DaemonConfigStoreError::Io)?;
    #[cfg(unix)]
    File::open(parent)
        .and_then(|directory| directory.sync_all())
        .map_err(|_| DaemonConfigStoreError::Io)?;
    Ok(())
}

fn validate_config(value: &Value) -> Result<(), DaemonConfigStoreError> {
    let config = value.as_object().ok_or(DaemonConfigStoreError::Invalid)?;
    let mcp = object_field(config, "mcp")?;
    boolean_field(mcp, "injectIntoAgents", true)?;
    boolean_field(mcp, "enabled", false)?;
    let browser = object_field(config, "browserTools")?;
    boolean_field(browser, "enabled", true)?;
    let providers = object_field(config, "providers")?;
    if providers
        .iter()
        .any(|(id, provider)| id.is_empty() || !provider.is_object())
    {
        return Err(DaemonConfigStoreError::Invalid);
    }
    let metadata = object_field(config, "metadataGeneration")?;
    let candidates = metadata
        .get("providers")
        .and_then(Value::as_array)
        .ok_or(DaemonConfigStoreError::Invalid)?;
    if candidates.iter().any(|candidate| {
        candidate
            .as_object()
            .and_then(|candidate| candidate.get("provider"))
            .and_then(Value::as_str)
            .is_none_or(str::is_empty)
    }) {
        return Err(DaemonConfigStoreError::Invalid);
    }
    boolean_field(config, "autoArchiveAfterMerge", true)?;
    boolean_field(config, "enableTerminalAgentHooks", true)?;
    if !config
        .get("appendSystemPrompt")
        .is_some_and(Value::is_string)
    {
        return Err(DaemonConfigStoreError::Invalid);
    }
    if let Some(relay) = config.get("relay") {
        boolean_field(
            relay.as_object().ok_or(DaemonConfigStoreError::Invalid)?,
            "enabled",
            true,
        )?;
    }
    if let Some(git) = config.get("git") {
        let git = git.as_object().ok_or(DaemonConfigStoreError::Invalid)?;
        positive_integer_field(git, "maxProcessesPerSecond")?;
        positive_integer_field(git, "maxProcessConcurrency")?;
    }
    if config
        .get("catalogRefreshTimeoutMs")
        .is_some_and(|value| value.as_u64().is_none_or(|value| value == 0))
    {
        return Err(DaemonConfigStoreError::Invalid);
    }
    Ok(())
}

fn object_field<'a>(
    object: &'a Map<String, Value>,
    field: &str,
) -> Result<&'a Map<String, Value>, DaemonConfigStoreError> {
    object
        .get(field)
        .and_then(Value::as_object)
        .ok_or(DaemonConfigStoreError::Invalid)
}

fn boolean_field(
    object: &Map<String, Value>,
    field: &str,
    required: bool,
) -> Result<(), DaemonConfigStoreError> {
    match object.get(field) {
        Some(Value::Bool(_)) => Ok(()),
        None if !required => Ok(()),
        _ => Err(DaemonConfigStoreError::Invalid),
    }
}

fn positive_integer_field(
    object: &Map<String, Value>,
    field: &str,
) -> Result<(), DaemonConfigStoreError> {
    if object
        .get(field)
        .and_then(Value::as_u64)
        .is_some_and(|value| value > 0)
    {
        Ok(())
    } else {
        Err(DaemonConfigStoreError::Invalid)
    }
}

fn reject_symlink(path: &Path) -> Result<(), DaemonConfigStoreError> {
    match path.symlink_metadata() {
        Ok(metadata) if metadata.file_type().is_symlink() => Err(DaemonConfigStoreError::Io),
        Ok(_) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(DaemonConfigStoreError::Io),
    }
}

fn merge_field(
    config: &mut Map<String, Value>,
    field: &str,
    patch: &Value,
) -> Result<(), DaemonConfigStoreError> {
    let patch = patch.as_object().ok_or(DaemonConfigStoreError::Invalid)?;
    let current = config
        .entry(field.to_owned())
        .or_insert_with(|| Value::Object(Map::new()));
    deep_merge(current, patch)
}

fn deep_merge(
    target: &mut Value,
    patch: &Map<String, Value>,
) -> Result<(), DaemonConfigStoreError> {
    let target = target
        .as_object_mut()
        .ok_or(DaemonConfigStoreError::Invalid)?;
    for (key, value) in patch {
        if let (Some(current), Some(nested)) = (target.get_mut(key), value.as_object())
            && current.is_object()
        {
            deep_merge(current, nested)?;
        } else {
            target.insert(key.clone(), value.clone());
        }
    }
    Ok(())
}

fn remove_providers(
    config: &mut Map<String, Value>,
    remove: Option<&Value>,
) -> Result<(), DaemonConfigStoreError> {
    let Some(remove) = remove else {
        return Ok(());
    };
    let remove: HashSet<&str> = remove
        .as_array()
        .ok_or(DaemonConfigStoreError::Invalid)?
        .iter()
        .map(Value::as_str)
        .collect::<Option<_>>()
        .ok_or(DaemonConfigStoreError::Invalid)?;
    if let Some(providers) = config.get_mut("providers").and_then(Value::as_object_mut) {
        providers.retain(|provider, _| !remove.contains(provider.as_str()));
    }
    if let Some(providers) = config
        .get_mut("metadataGeneration")
        .and_then(Value::as_object_mut)
        .and_then(|metadata| metadata.get_mut("providers"))
        .and_then(Value::as_array_mut)
    {
        providers.retain(|entry| {
            entry
                .get("provider")
                .and_then(Value::as_str)
                .is_none_or(|provider| !remove.contains(provider))
        });
    }
    Ok(())
}

fn diff_paths(previous: &Value, next: &Value, prefix: &str, changed: &mut Vec<String>) {
    if previous == next {
        return;
    }
    let (Some(previous), Some(next)) = (previous.as_object(), next.as_object()) else {
        if !prefix.is_empty() {
            changed.push(prefix.to_owned());
        }
        return;
    };
    let keys: BTreeSet<&str> = previous
        .keys()
        .chain(next.keys())
        .map(String::as_str)
        .collect();
    for key in keys {
        let path = if prefix.is_empty() {
            key.to_owned()
        } else {
            format!("{prefix}.{key}")
        };
        match (previous.get(key), next.get(key)) {
            (Some(left), Some(right)) => diff_paths(left, right, &path, changed),
            _ => changed.push(path),
        }
    }
}

fn reloadable_path(path: &str) -> Option<String> {
    let (root, suffix) = path.split_once('.').unwrap_or((path, ""));
    let owner = match root {
        "relay"
        | "mcp"
        | "browserTools"
        | "hostnames"
        | "cors"
        | "trustedProxies"
        | "git"
        | "autoArchiveAfterMerge"
        | "enableTerminalAgentHooks"
        | "appendSystemPrompt"
        | "terminalProfiles"
        | "agentProfiles" => {
            format!("daemon.{path}")
        }
        "app" => format!("app.{suffix}"),
        "providers" | "catalogRefreshTimeoutMs" | "metadataGeneration" | "skills" => {
            format!("agents.{path}")
        }
        "pluginsEnabled" => "pluginsEnabled".to_owned(),
        _ => return None,
    };
    Some(owner)
}

#[cfg(test)]
mod tests;
