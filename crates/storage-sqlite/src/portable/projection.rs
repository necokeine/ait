//! Idempotent catalog indexes and event feed; Project commits do not depend on them.
use super::{
    BTreeMap, Connection, ControlChange, ControlFilter, ControlStoreError, Kind, OptionalExtension,
    OwnedProject, Value, apply_change, catalog_id, json_error, params, project, read_filter,
    replay_locked, revision, sql_error, table,
};

pub(super) const PROJECT_KINDS: [Kind; 8] = [
    Kind::Project,
    Kind::Message,
    Kind::Session,
    Kind::Run,
    Kind::RunCredential,
    Kind::WorkspaceRunJournal,
    Kind::Cron,
    Kind::Agent,
];

pub(super) fn refresh(
    catalog: &mut Connection,
    id: &str,
    owned: &OwnedProject,
) -> Result<(), ControlStoreError> {
    project::validate_owned(owned)?;
    let source = &owned.connection;
    let local_revision = revision(source)?;
    let stream: String = source
        .query_row(
            "SELECT stream_id FROM project_identity WHERE singleton=1",
            [],
            |row| row.get(0),
        )
        .map_err(sql_error)?;
    let watermark: Option<(String, u64, u64)> = catalog
        .query_row(
            "SELECT stream_id, revision, event_seq FROM projection_watermarks WHERE project_id=?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(sql_error)?;
    if watermark
        .as_ref()
        .is_some_and(|(saved, rev, _)| *saved == stream && *rev > local_revision)
    {
        return Err(super::other(
            "RESTORE_REQUIRED: Project revision moved backwards; restore requires a new event stream",
        ));
    }
    let rebuild = watermark
        .as_ref()
        .is_none_or(|(saved, rev, _)| *saved != stream || *rev > local_revision);
    let from_revision = if rebuild {
        0
    } else {
        watermark.as_ref().map_or(0, |(_, rev, _)| *rev)
    };
    let from_event = if rebuild {
        0
    } else {
        watermark.as_ref().map_or(0, |(_, _, seq)| *seq)
    };
    let transaction = catalog.transaction().map_err(sql_error)?;
    apply_change(
        &transaction,
        ControlChange::Put(project::record(owned, id)?),
    )?;
    refresh_crons(&transaction, source, id)?;
    if rebuild {
        transaction
            .execute("DELETE FROM record_locations WHERE project_id=?1", [id])
            .map_err(sql_error)?;
        for kind in PROJECT_KINDS {
            let mut rows = BTreeMap::new();
            read_filter(source, &ControlFilter::all(kind), &mut rows)?;
            for record in rows.values() {
                // Named Agent snapshots must not shadow global preset identities.
                if kind == Kind::Agent
                    && record
                        .value
                        .get("owner_session_id")
                        .is_none_or(Value::is_null)
                {
                    continue;
                }
                locate(&transaction, table(kind), &record.id, id, false)?;
            }
        }
    } else if from_revision != local_revision {
        let mut statement = source.prepare("SELECT kind,id,deleted FROM projection_changes WHERE revision>?1 ORDER BY revision").map_err(sql_error)?;
        let rows = statement
            .query_map([from_revision], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, bool>(2)?,
                ))
            })
            .map_err(sql_error)?;
        for row in rows {
            let (kind, record_id, deleted) = row.map_err(sql_error)?;
            locate(&transaction, &kind, &record_id, id, deleted)?;
        }
    }
    let cursor = project_events(&transaction, source, id, &stream, from_event)?;
    transaction.execute("INSERT INTO projection_watermarks VALUES(?1,?2,?3,?4) ON CONFLICT(project_id) DO UPDATE SET stream_id=excluded.stream_id,revision=excluded.revision,event_seq=excluded.event_seq",params![id,stream,local_revision,cursor]).map_err(sql_error)?;
    transaction.commit().map_err(sql_error)
}

pub(super) fn replay(
    state: &super::State,
    cursor: u64,
    limit: usize,
) -> Result<Vec<super::DurableEvent>, ControlStoreError> {
    let mut events = replay_locked(&state.catalog, cursor, limit)?;
    for event in &mut events {
        let location: Option<(String, String, u64)> = state
            .catalog
            .query_row(
                "SELECT project_id,stream_id,event_seq FROM projected_events WHERE feed_cursor=?1",
                [event.cursor],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .optional()
            .map_err(sql_error)?;
        let Some((id, stream, seq)) = location else {
            continue;
        };
        let payload = state.projects.get(&id).map(|owned| {
            owned.connection.query_row("SELECT body_json FROM durable_events WHERE cursor=?1 AND EXISTS(SELECT 1 FROM project_identity WHERE stream_id=?2)",params![seq,stream],|row|row.get::<_,String>(0)).optional().map_err(sql_error)
        }).transpose()?.flatten();
        if let Some(payload) = payload {
            event.body = serde_json::from_str(&payload).map_err(json_error)?;
        } else {
            event.kind = "project.refresh_required".into();
            event.entity_id = Some(id.clone());
            event.body = serde_json::json!({"project_id":id});
        }
    }
    Ok(events)
}

fn locate(
    connection: &Connection,
    kind: &str,
    id: &str,
    project_id: &str,
    deleted: bool,
) -> Result<(), ControlStoreError> {
    if deleted {
        connection
            .execute(
                "DELETE FROM record_locations WHERE kind=?1 AND id=?2 AND project_id=?3",
                params![kind, id, project_id],
            )
            .map_err(sql_error)?;
    } else {
        let existing: Option<String> = connection
            .query_row(
                "SELECT project_id FROM record_locations WHERE kind=?1 AND id=?2",
                params![kind, id],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)?;
        if existing.as_ref().is_some_and(|saved| saved != project_id) {
            return Err(super::other(
                "Project record identity conflicts with an existing catalog route",
            ));
        }
        connection
            .execute(
                "INSERT OR IGNORE INTO record_locations VALUES(?1,?2,?3)",
                params![kind, id, project_id],
            )
            .map_err(sql_error)?;
    }
    Ok(())
}

pub(super) fn snapshots(
    catalog: &Connection,
    project: &rusqlite::Transaction<'_>,
) -> Result<(), ControlStoreError> {
    let source = catalog_id(catalog)?;
    project
        .execute(
            "INSERT OR IGNORE INTO configuration_sources SELECT 'crons',id,?1 FROM crons",
            [&source],
        )
        .map_err(sql_error)?;
    let mut agent_ids = project.prepare("SELECT DISTINCT json_extract(body_json,'$.agent_id') FROM sessions UNION SELECT json_extract(body_json,'$.default_agent_id') FROM projects UNION SELECT json_extract(body_json,'$.agent_id') FROM crons").map_err(sql_error)?;
    let ids = agent_ids
        .query_map([], |row| row.get::<_, Option<String>>(0))
        .map_err(sql_error)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(sql_error)?;
    for id in ids.into_iter().flatten() {
        for kind in [Kind::Agent, Kind::Provider] {
            let target = if kind == Kind::Provider {
                project.query_row("SELECT json_extract(body_json,'$.config.provider_id') FROM agents WHERE id=?1",[&id],|row|row.get::<_,String>(0)).optional().map_err(sql_error)?
            } else {
                Some(id.clone())
            };
            let Some(target) = target else {
                continue;
            };
            let mut records = BTreeMap::new();
            read_filter(catalog, &ControlFilter::id(kind, &target), &mut records)?;
            for record in records.into_values() {
                let prior: Option<String> = project
                    .query_row(
                        "SELECT catalog_id FROM configuration_sources WHERE kind=?1 AND id=?2",
                        params![table(kind), record.id],
                        |row| row.get(0),
                    )
                    .optional()
                    .map_err(sql_error)?;
                if prior.as_ref().is_some_and(|saved| *saved != source) {
                    continue;
                }
                project
                    .execute(
                        "INSERT OR IGNORE INTO configuration_sources VALUES(?1,?2,?3)",
                        params![table(kind), record.id, source],
                    )
                    .map_err(sql_error)?;
                apply_change(project, ControlChange::Put(record))?;
            }
        }
    }
    Ok(())
}

fn refresh_crons(
    transaction: &rusqlite::Transaction<'_>,
    source: &Connection,
    id: &str,
) -> Result<(), ControlStoreError> {
    transaction
        .execute("DELETE FROM crons WHERE project_id=?1", [id])
        .map_err(sql_error)?;
    let mut crons = BTreeMap::new();
    read_filter(source, &ControlFilter::all(Kind::Cron), &mut crons)?;
    for cron in crons.into_values() {
        apply_change(transaction, ControlChange::Put(cron))?;
    }
    Ok(())
}

fn project_events(
    transaction: &rusqlite::Transaction<'_>,
    source: &Connection,
    id: &str,
    stream: &str,
    from_event: u64,
) -> Result<u64, ControlStoreError> {
    let mut cursor = from_event;
    loop {
        let events = replay_locked(source, cursor, 500)?;
        if events.is_empty() {
            break;
        }
        for event in events {
            cursor = event.cursor;
            let inserted = transaction
                .execute(
                    "INSERT OR IGNORE INTO projected_events(project_id,stream_id,event_seq) VALUES(?1,?2,?3)",
                    params![id, stream, cursor],
                )
                .map_err(sql_error)?;
            if inserted > 0 {
                transaction.execute("INSERT INTO durable_events(kind,entity_id,body_json,created_at,project_id) VALUES(?1,?2,'{}',?3,?4)", params![event.kind,event.entity_id,event.created_at,id]).map_err(sql_error)?;
                transaction.execute("UPDATE projected_events SET feed_cursor=last_insert_rowid() WHERE project_id=?1 AND stream_id=?2 AND event_seq=?3",params![id,stream,cursor]).map_err(sql_error)?;
            }
        }
    }
    Ok(cursor)
}
