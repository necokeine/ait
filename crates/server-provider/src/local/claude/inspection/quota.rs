//! Claude quota windows preserve native model/surface identities across response shapes.

use std::collections::BTreeSet;

use serde_json::{Value, json};

use crate::ports::agent_session::AgentSessionError;

struct Scoped {
    dimension: &'static str,
    id: Option<String>,
    name: String,
    used: Option<f64>,
    reset: Option<String>,
}

pub(super) fn project(response: &Value, plan: Option<&str>) -> Result<Value, AgentSessionError> {
    if !response.is_object() {
        return Err(AgentSessionError::Failed);
    }
    let mut windows = Vec::new();
    for (key, id, label) in [
        ("five_hour", "five_hour", "Session"),
        ("seven_day", "weekly", "Weekly"),
    ] {
        if response[key].is_null() {
            continue;
        }
        let used = number(&response[key]["utilization"]).ok_or(AgentSessionError::Failed)?;
        windows.push(window(
            id,
            label,
            Some(used),
            response[key]["resets_at"].as_str(),
        ));
    }
    let mut scoped = Vec::new();
    for (key, name) in [
        ("seven_day_opus", "Opus"),
        ("seven_day_omelette", "Omelette"),
    ] {
        if response[key].is_null() {
            continue;
        }
        let used = number(&response[key]["utilization"]).ok_or(AgentSessionError::Failed)?;
        scoped.push(Scoped {
            dimension: "model",
            id: None,
            name: name.to_owned(),
            used: Some(used),
            reset: response[key]["resets_at"].as_str().map(str::to_owned),
        });
    }
    for entry in response["limits"]
        .as_array()
        .into_iter()
        .flatten()
        .take(512)
    {
        if entry["kind"] != "weekly_scoped" {
            continue;
        }
        let Some(mut limit) = scoped_entry(entry) else {
            continue;
        };
        let previous = scoped.iter().position(|previous| {
            previous.dimension == limit.dimension
                && match (&previous.id, &limit.id) {
                    (Some(left), Some(right)) => left == right,
                    _ => normalize(&previous.name) == normalize(&limit.name),
                }
        });
        if let Some(index) = previous {
            if limit.used.is_none() {
                limit.used = scoped[index].used;
            }
            if limit.reset.is_none() {
                limit.reset.clone_from(&scoped[index].reset);
            }
            scoped[index] = limit;
        } else {
            scoped.push(limit);
        }
    }
    let mut taken = BTreeSet::new();
    for limit in scoped {
        let base = format!(
            "weekly_{}_{}",
            limit.dimension,
            limit.id.unwrap_or_else(|| normalize(&limit.name))
        );
        let mut id = base.clone();
        let mut suffix = 2;
        while !taken.insert(id.clone()) {
            id = format!("{base}_{suffix}");
            suffix += 1;
        }
        windows.push(window(
            &id,
            &format!("Weekly · {}", limit.name),
            limit.used,
            limit.reset.as_deref(),
        ));
    }
    let details: Vec<_> = response["extra_usage"]["is_enabled"].as_bool().into_iter().map(|enabled|
        json!({"id":"extra_usage","label":"Extra usage","value":if enabled {"Enabled"} else {"Disabled"}})).collect();
    Ok(
        json!({"providerId":"claude","displayName":"Claude","status":if windows.is_empty() {"unavailable"} else {"available"},
        "planLabel":plan,"windows":windows,"balances":[],"details":details,"error":null,
        "sourceLabel":"Claude OAuth usage","fetchedAt":chrono::Utc::now().to_rfc3339(),"nextRefreshAt":null}),
    )
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        .filter(|value| value.is_finite() && *value >= 0.0)
}

fn scoped_entry(entry: &Value) -> Option<Scoped> {
    if !entry["percent"].is_null() && number(&entry["percent"]).is_none() {
        return None;
    }
    for dimension in ["model", "surface"] {
        let id = entry["scope"][dimension]["id"]
            .as_str()
            .map(str::trim)
            .filter(|id| !id.is_empty() && id.len() <= 256);
        let name = entry["scope"][dimension]["display_name"]
            .as_str()
            .map(str::trim)
            .filter(|name| !name.is_empty() && name.len() <= 256)
            .or(id);
        if let Some(name) = name {
            return Some(Scoped {
                dimension,
                id: id.map(str::to_owned),
                name: name.to_owned(),
                used: number(&entry["percent"]),
                reset: entry["resets_at"].as_str().map(str::to_owned),
            });
        }
    }
    None
}

fn normalize(name: &str) -> String {
    name.to_ascii_lowercase()
        .split(|character: char| !character.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

fn window(id: &str, label: &str, used: Option<f64>, reset: Option<&str>) -> Value {
    json!({"id":id,"label":label,"usedPct":used,"remainingPct":used.map(|used|(100.0-used).clamp(0.0,100.0)),
        "resetsAt":reset,"tone":match used { Some(used) if used>90.0=>"danger",Some(used) if used>=70.0=>"warning",Some(_)=>"ok",None=>"default"}})
}
