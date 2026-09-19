//! Explicit host configuration adoption, separate from immutable saved snapshots.
use super::{
    ControlStoreError, ControlVersion, Kind, OptionalExtension, State, Value, catalog_id, other,
    params, project, revision, sql_error,
};

pub(super) fn bind(
    state: &mut State,
    version: &ControlVersion,
    project_id: &str,
    source_id: &str,
    agent_id: &str,
) -> Result<(), ControlStoreError> {
    if version.catalog_id != catalog_id(&state.catalog)?
        || version.catalog_revision != revision(&state.catalog)?
    {
        return Err(ControlStoreError::Conflict);
    }
    let owned = state
        .projects
        .get_mut(project_id)
        .ok_or(ControlStoreError::Conflict)?;
    if version.projects.get(project_id) != Some(&project::version(&owned.connection)?) {
        return Err(ControlStoreError::Conflict);
    }
    project::validate_owned(owned)?;
    super::workers::assert_prior_quiescent(&owned.connection)?;
    let native: bool = owned.connection.query_row("SELECT EXISTS(SELECT 1 FROM sessions WHERE json_extract(body_json,'$.agent_id')=?1 AND json_extract(body_json,'$.source.type')='codex_thread')",[source_id],|row|row.get(0)).map_err(sql_error)?;
    if native {
        return Err(other(
            "NATIVE_SOURCE_UNRESOLVED: saved Codex Threads require verified native-store identity before a host binding can be changed; history remains available",
        ));
    }
    let source: String = owned
        .connection
        .query_row(
            "SELECT body_json FROM agents WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let agent: String = state
        .catalog
        .query_row(
            "SELECT body_json FROM agents WHERE id=?1",
            [agent_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let agent_value: Value = serde_json::from_str(&agent).map_err(super::json_error)?;
    if agent_value.get("enabled").and_then(Value::as_bool) != Some(true)
        || agent_value
            .get("owner_session_id")
            .is_some_and(|value| !value.is_null())
    {
        return Err(other("Select an enabled named Agent preset"));
    }
    let provider_id = agent_value
        .pointer("/config/provider_id")
        .and_then(Value::as_str)
        .ok_or_else(|| other("Agent provider missing"))?;
    let provider: String = state
        .catalog
        .query_row(
            "SELECT body_json FROM agent_providers WHERE id=?1",
            [provider_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let saved_agent: Value = serde_json::from_str(&source).map_err(super::json_error)?;
    let prior_id = saved_agent
        .pointer("/config/provider_id")
        .and_then(Value::as_str)
        .ok_or_else(|| other("Saved Agent provider missing"))?;
    let prior: String = owned
        .connection
        .query_row(
            "SELECT body_json FROM agent_providers WHERE id=?1",
            [prior_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let prior: Value = serde_json::from_str(&prior).map_err(super::json_error)?;
    let replacement: Value = serde_json::from_str(&provider).map_err(super::json_error)?;
    if prior.get("kind").is_none() || prior.get("kind") != replacement.get("kind") {
        return Err(other(
            "Project Agent binding requires the same provider kind",
        ));
    }
    let transaction = owned.connection.transaction().map_err(sql_error)?;
    transaction.execute("INSERT INTO configuration_bindings VALUES(?1,?2,?3,?4,?5) ON CONFLICT(source_agent_id) DO UPDATE SET catalog_id=excluded.catalog_id,agent_id=excluded.agent_id,agent_json=excluded.agent_json,provider_json=excluded.provider_json",params![source_id,version.catalog_id,agent_id,agent,provider]).map_err(sql_error)?;
    transaction
        .execute(
            "UPDATE control_metadata SET revision=revision+1 WHERE singleton=1",
            [],
        )
        .map_err(sql_error)?;
    transaction.execute("INSERT INTO durable_events(kind,entity_id,body_json,created_at) VALUES('project.binding_updated',?1,?2,unixepoch()*1000)",
        params![project_id,serde_json::json!({"project_id":project_id,"source_agent_id":source_id,"agent_id":agent_id}).to_string()]).map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

pub(super) fn resolved(
    state: &State,
    project_id: &str,
    source_id: &str,
) -> Result<Option<super::ControlRecord>, ControlStoreError> {
    let project = &state.projects[project_id].connection;
    let binding: Option<(String,String,String,String)> = project.query_row("SELECT catalog_id,agent_id,agent_json,provider_json FROM configuration_bindings WHERE source_agent_id=?1",[source_id],|row|Ok((row.get(0)?,row.get(1)?,row.get(2)?,row.get(3)?))).optional().map_err(sql_error)?;
    let Some((catalog, agent_id, snapshot, provider_snapshot)) = binding else {
        return Ok(None);
    };
    if catalog != catalog_id(&state.catalog)? {
        return Ok(None);
    }
    let current: Option<String> = state
        .catalog
        .query_row(
            "SELECT body_json FROM agents WHERE id=?1",
            [&agent_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if current.as_deref() != Some(&snapshot) {
        return Ok(None);
    }
    let mut value: Value = serde_json::from_str(&snapshot).map_err(super::json_error)?;
    let provider_id = value
        .pointer("/config/provider_id")
        .and_then(Value::as_str)
        .ok_or_else(|| other("Bound provider missing"))?;
    let provider: Option<String> = state
        .catalog
        .query_row(
            "SELECT body_json FROM agent_providers WHERE id=?1",
            [provider_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)?;
    if provider.as_deref() != Some(&provider_snapshot) {
        return Ok(None);
    }
    let original: String = project
        .query_row(
            "SELECT body_json FROM agents WHERE id=?1",
            [source_id],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let original: Value = serde_json::from_str(&original).map_err(super::json_error)?;
    value["id"] = Value::String(source_id.into());
    value["owner_session_id"] = original
        .get("owner_session_id")
        .cloned()
        .unwrap_or(Value::Null);
    Ok(Some(super::ControlRecord {
        kind: Kind::Agent,
        id: source_id.into(),
        project_id: None,
        value,
    }))
}

pub(super) fn resolved_provider(
    state: &State,
    project_id: &str,
    provider_id: &str,
) -> Result<Option<super::ControlRecord>, ControlStoreError> {
    let mut query = state.projects[project_id].connection.prepare(
        "SELECT source_agent_id FROM configuration_bindings WHERE json_extract(agent_json,'$.config.provider_id')=?1"
    ).map_err(sql_error)?;
    let ids = query
        .query_map([provider_id], |row| row.get::<_, String>(0))
        .map_err(sql_error)?;
    for id in ids {
        if resolved(state, project_id, &id.map_err(sql_error)?)?.is_some() {
            let json: String = state
                .catalog
                .query_row(
                    "SELECT body_json FROM agent_providers WHERE id=?1",
                    [provider_id],
                    |row| row.get(0),
                )
                .map_err(sql_error)?;
            return Ok(Some(super::ControlRecord {
                kind: Kind::Provider,
                id: provider_id.into(),
                project_id: None,
                value: serde_json::from_str(&json).map_err(super::json_error)?,
            }));
        }
    }
    Ok(None)
}
