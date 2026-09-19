//! Scoped reads and catalog routing; ownership is retained for the open interval.
use super::{
    BTreeMap, BTreeSet, ControlFilter, ControlRead, ControlRecord, ControlStoreError,
    ControlVersion, Kind, OptionalExtension, State, Value, catalog_id, other, params, project,
    projection, read_filter, revision, sql_error, table,
};

pub(super) fn location(
    state: &State,
    kind: Kind,
    id: &str,
) -> Result<Option<String>, ControlStoreError> {
    state
        .catalog
        .query_row(
            "SELECT project_id FROM record_locations WHERE kind=?1 AND id=?2",
            params![table(kind), id],
            |row| row.get(0),
        )
        .optional()
        .map_err(sql_error)
}

pub(super) fn targets(
    state: &State,
    filter: &ControlFilter,
) -> Result<Option<Vec<String>>, ControlStoreError> {
    let target = match filter {
        ControlFilter::All(
            Kind::Message
            | Kind::Session
            | Kind::Run
            | Kind::RunCredential
            | Kind::WorkspaceRunJournal,
        ) => {
            return Ok(Some(state.projects.keys().cloned().collect()));
        }
        ControlFilter::Project { kind, project_id }
            if *kind != Kind::ProviderCredential && *kind != Kind::Settings =>
        {
            return Ok(Some(vec![project_id.clone()]));
        }
        ControlFilter::Id {
            kind: Kind::Project,
            id,
        } => {
            return Ok(state
                .catalog
                .query_row("SELECT id FROM projects WHERE id=?1", [id], |row| {
                    row.get::<_, String>(0)
                })
                .optional()
                .map_err(sql_error)?
                .map(|id| vec![id]));
        }
        ControlFilter::Id { kind, id }
            if matches!(
                kind,
                Kind::Message
                    | Kind::Session
                    | Kind::Run
                    | Kind::RunCredential
                    | Kind::WorkspaceRunJournal
                    | Kind::Cron
                    | Kind::Agent
            ) =>
        {
            location(state, *kind, id)?
        }
        ControlFilter::MessageAncestors { head_id } => location(state, Kind::Message, head_id)?,
        ControlFilter::MessageChildren { parent_id } => location(state, Kind::Message, parent_id)?,
        ControlFilter::RunsForSession { session_id } => location(state, Kind::Session, session_id)?,
        ControlFilter::RunsForCron { cron_id } => location(state, Kind::Cron, cron_id)?,
        _ => return Ok(None),
    };
    if let Some(target) = target {
        return Ok(Some(vec![target]));
    }
    if matches!(
        filter,
        ControlFilter::Id {
            kind: Kind::Agent,
            ..
        }
    ) {
        Ok(None)
    } else {
        Ok(Some(Vec::new()))
    }
}

pub(super) fn records(
    state: &mut State,
    filters: &[ControlFilter],
    runtime: &str,
) -> Result<ControlRead, ControlStoreError> {
    let mut projection_failed = false;
    for (id, owned) in &state.projects {
        projection_failed |= projection::refresh(&mut state.catalog, id, owned).is_err();
    }
    let mut version = ControlVersion {
        catalog_id: catalog_id(&state.catalog)?,
        catalog_revision: revision(&state.catalog)?,
        observes_catalog: false,
        projects: BTreeMap::new(),
    };
    let mut selected = BTreeSet::new();
    let mut plans = Vec::with_capacity(filters.len());
    for filter in filters {
        let targets = targets(state, filter)?;
        if projection_failed && targets.as_ref().is_some_and(Vec::is_empty) {
            return Err(other(
                "PROJECT_INDEX_REBUILDING: an index is unavailable; retry after Project projection recovers",
            ));
        }
        if let Some(ids) = &targets {
            selected.extend(ids.iter().cloned());
        }
        plans.push((filter, targets));
    }
    for id in &selected {
        project::acquire(state, id, runtime)?;
        let owned = state
            .projects
            .get(id)
            .ok_or_else(|| other("Project owner is missing"))?;
        version
            .projects
            .insert(id.clone(), project::version(&owned.connection)?);
    }
    let mut records = BTreeMap::new();
    for (filter, targets) in plans {
        if let Some(ids) = targets {
            for id in ids {
                let owned = &state.projects[&id];
                read_filter(&owned.connection, filter, &mut records)?;
                if let ControlFilter::Id {
                    kind: Kind::Agent,
                    id: agent_id,
                } = filter
                {
                    configuration_snapshot(
                        state,
                        &BTreeSet::from([id.clone()]),
                        filter,
                        agent_id,
                        &mut records,
                    )?;
                }
                if matches!(
                    filter,
                    ControlFilter::Id {
                        kind: Kind::Project,
                        ..
                    } | ControlFilter::Project {
                        kind: Kind::Project,
                        ..
                    }
                ) {
                    records.insert((Kind::Project, id.clone()), project::record(owned, &id)?);
                }
            }
        } else {
            if matches!(
                filter,
                ControlFilter::All(
                    Kind::Agent | Kind::Provider | Kind::ProviderCredential | Kind::Settings
                ) | ControlFilter::Id {
                    kind: Kind::Agent | Kind::Provider | Kind::ProviderCredential | Kind::Settings,
                    ..
                } | ControlFilter::AgentsForProvider { .. }
            ) {
                version.observes_catalog = true;
            }
            read_filter(&state.catalog, filter, &mut records)?;
            merge_agents(state, &selected, filter, &mut records)?;
            if let ControlFilter::Id {
                kind: Kind::Agent | Kind::Provider,
                id,
            } = filter
            {
                configuration_snapshot(state, &selected, filter, id, &mut records)?;
            }
        }
    }
    decorate_projects(state, &mut records)?;
    mask_crons(state, &version.catalog_id, &mut records);
    Ok(ControlRead {
        revision: version.catalog_revision,
        version,
        records: records.into_values().collect(),
    })
}

fn configuration_snapshot(
    state: &State,
    projects: &BTreeSet<String>,
    filter: &ControlFilter,
    id: &str,
    records: &mut BTreeMap<(Kind, String), ControlRecord>,
) -> Result<(), ControlStoreError> {
    let ControlFilter::Id { kind, .. } = filter else {
        return Ok(());
    };
    let current_catalog = catalog_id(&state.catalog)?;
    for project_id in projects {
        if *kind == Kind::Agent
            && let Some(record) = super::binding::resolved(state, project_id, id)?
        {
            records.insert((Kind::Agent, id.to_owned()), record);
            continue;
        }
        if *kind == Kind::Provider
            && let Some(record) = super::binding::resolved_provider(state, project_id, id)?
        {
            records.insert((Kind::Provider, id.to_owned()), record);
            continue;
        }
        let connection = &state.projects[project_id].connection;
        let source: Option<String> = connection
            .query_row(
                "SELECT catalog_id FROM configuration_sources WHERE kind=?1 AND id=?2",
                params![table(*kind), id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if source
            .as_ref()
            .is_some_and(|source| source != &current_catalog)
            || !records.contains_key(&(*kind, id.to_owned()))
        {
            let mut saved = BTreeMap::new();
            read_filter(connection, filter, &mut saved)?;
            for (key, mut record) in saved {
                if *kind == Kind::Agent {
                    record.value["enabled"] = Value::Bool(false);
                }
                records.insert(key, record);
            }
        }
    }
    Ok(())
}

fn mask_crons(state: &State, catalog: &str, records: &mut BTreeMap<(Kind, String), ControlRecord>) {
    for record in records
        .values_mut()
        .filter(|record| record.kind == Kind::Cron)
    {
        let available = record
            .project_id
            .as_ref()
            .and_then(|id| state.projects.get(id))
            .is_some_and(|owned| {
                owned
                    .connection
                    .query_row(
                        "SELECT catalog_id FROM configuration_sources WHERE kind='crons' AND id=?1",
                        [&record.id],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .ok()
                    .flatten()
                    .is_some_and(|source| source == catalog)
            });
        if !available {
            record.value["enabled"] = Value::Bool(false);
        }
    }
}

fn merge_agents(
    state: &State,
    selected: &BTreeSet<String>,
    filter: &ControlFilter,
    records: &mut BTreeMap<(Kind, String), ControlRecord>,
) -> Result<(), ControlStoreError> {
    if matches!(filter, ControlFilter::All(Kind::Agent)) {
        records.retain(|(kind, _), record| {
            *kind != Kind::Agent
                || record
                    .value
                    .get("owner_session_id")
                    .is_none_or(Value::is_null)
        });
        let available = if selected.is_empty() {
            state.projects.keys().cloned().collect()
        } else {
            selected.clone()
        };
        for project_id in &available {
            let mut saved = BTreeMap::new();
            read_filter(&state.projects[project_id].connection, filter, &mut saved)?;
            for ((_, id), record) in saved {
                if selected.is_empty()
                    && record
                        .value
                        .get("owner_session_id")
                        .is_none_or(Value::is_null)
                {
                    continue;
                }
                records.entry((Kind::Agent, id.clone())).or_insert(record);
                configuration_snapshot(
                    state,
                    &BTreeSet::from([project_id.clone()]),
                    &ControlFilter::id(Kind::Agent, &id),
                    &id,
                    records,
                )?;
            }
        }
    }
    Ok(())
}

fn decorate_projects(
    state: &State,
    records: &mut BTreeMap<(Kind, String), ControlRecord>,
) -> Result<(), ControlStoreError> {
    for record in records
        .values_mut()
        .filter(|record| record.kind == Kind::Project)
    {
        record.value["owner"] = if let Some(owned) = state.projects.get(&record.id) {
            serde_json::to_value(project::version(&owned.connection)?.owner(&record.id))
                .map_err(super::json_error)?
        } else {
            Value::Null
        };
    }
    Ok(())
}
