//! Compatibility migration for legacy catalogs and Agent bindings.
use crate::control::catalog::{builtin_providers, invalid};
use crate::control::errors::serialization_error;
use ait_contracts::{AgentMode, ApiError, ProviderModel};
use serde_json::{Value, json};
use std::collections::{HashMap, HashSet};

/// Upgrade legacy JSON snapshots in memory; the next atomic commit stores v3.
pub(in crate::control) fn migrate_state(mut value: Value) -> Result<Value, ApiError> {
    if value.get("providers").is_some() {
        remove_unused_retired_builtins(&mut value);
        return Ok(value);
    }
    let mut providers = builtin_providers();
    if let Some(agents) = value.get_mut("agents").and_then(Value::as_array_mut) {
        for agent in agents {
            let kind: AgentMode =
                serde_json::from_value(agent["mode"].clone()).map_err(serialization_error)?;
            let model = agent["model"].as_str().unwrap_or("default").to_owned();
            let provider = providers
                .iter_mut()
                .find(|p| p.provider.kind == kind)
                .ok_or_else(|| invalid("legacy provider kind is unavailable"))?;
            if !provider.provider.models.iter().any(|m| m.id == model) {
                provider.provider.models.push(ProviderModel {
                    id: model.clone(),
                    name: model.clone(),
                    reasoning_efforts: Vec::new(),
                });
            }
            agent["config"] = json!({"provider_id": provider.provider.id, "model": model, "reasoning_effort": null});
            agent.as_object_mut().expect("Agent object").remove("model");
            agent.as_object_mut().expect("Agent object").remove("mode");
        }
    }
    let configs: HashMap<String, Value> = value["agents"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|a| Some((a["id"].as_str()?.into(), a["config"].clone())))
        .collect();
    if let Some(runs) = value.get_mut("runs").and_then(Value::as_array_mut) {
        for run in runs {
            let mut config = configs
                .get(run["agent_id"].as_str().unwrap_or_default())
                .cloned()
                .ok_or_else(|| invalid("legacy Run Agent is unavailable"))?;
            config["reasoning_effort"] = run["reasoning_effort"].clone();
            let provider = providers
                .iter()
                .find(|p| Some(p.provider.id.as_str()) == config["provider_id"].as_str())
                .expect("migrated provider");
            run["provider"] =
                serde_json::to_value(&provider.provider).map_err(serialization_error)?;
            run["config"] = config;
            run.as_object_mut()
                .expect("Run object")
                .remove("reasoning_effort");
        }
    }
    value["providers"] = serde_json::to_value(providers).map_err(serialization_error)?;
    Ok(value)
}

/// Older snapshots persist the whole builtin catalog, including unused adapters.
/// Retire only unreferenced entries; referenced or custom providers must still
/// decode normally so unsupported history is never silently dropped or rebound.
fn remove_unused_retired_builtins(value: &mut Value) {
    let mut referenced = HashSet::<String>::new();
    for collection in ["agents", "runs"] {
        for item in value[collection].as_array().into_iter().flatten() {
            for id in [
                item["config"]["provider_id"].as_str(),
                item["provider"]["id"].as_str(),
            ]
            .into_iter()
            .flatten()
            {
                referenced.insert(id.into());
            }
        }
    }
    if let Some(credentials) = value["provider_credentials"].as_object() {
        referenced.extend(credentials.keys().cloned());
    }
    if let Some(providers) = value["providers"].as_array_mut() {
        providers.retain(|view| {
            let (Some(id), Some(kind)) = (view["id"].as_str(), view["kind"].as_str()) else {
                return true;
            };
            id != format!("builtin-{kind}")
                || referenced.contains(id)
                || view["url"] != Value::Null
                || view["has_secret"] != false
                || serde_json::from_value::<AgentMode>(view["kind"].clone()).is_ok()
        });
    }
}
