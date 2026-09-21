use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use server_domain::agent::{AgentConfig, AgentSnapshot, AgentTarget, Revision};
use server_domain::{AgentId, OperationId};
use server_ports::agent::{
    AgentCatalog, AgentError, AgentReceipt, ConfigureAgent, DefaultReceipt, DefaultSelection,
    SelectDefault,
};

use crate::{SqliteCatalog, parse};

pub(super) const SCHEMA: &str = "
CREATE TABLE agents (
    id TEXT PRIMARY KEY NOT NULL, head INTEGER NOT NULL CHECK(head>0),
    FOREIGN KEY(id,head) REFERENCES agent_revisions(agent_id,revision) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE agent_revisions (
    agent_id TEXT NOT NULL REFERENCES agents(id), revision INTEGER NOT NULL CHECK(revision>0),
    name TEXT NOT NULL, driver TEXT NOT NULL CHECK(driver='codex'), model TEXT NOT NULL,
    credential_ref TEXT, enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
    recorded_at INTEGER NOT NULL CHECK(recorded_at>=0), PRIMARY KEY(agent_id,revision)
) STRICT;
CREATE TRIGGER agent_revisions_no_update BEFORE UPDATE ON agent_revisions
BEGIN SELECT RAISE(ABORT,'immutable Agent revision'); END;
CREATE TRIGGER agent_revisions_no_delete BEFORE DELETE ON agent_revisions
BEGIN SELECT RAISE(ABORT,'immutable Agent revision'); END;
CREATE TABLE agent_default (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1), agent_id TEXT REFERENCES agents(id),
    version INTEGER NOT NULL CHECK(version>=0)
) STRICT;
INSERT INTO agent_default VALUES(1,NULL,0);
CREATE TABLE agent_configure_receipts (
    key TEXT PRIMARY KEY NOT NULL, id TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL,
    agent_id TEXT NOT NULL, revision INTEGER NOT NULL,
    FOREIGN KEY(agent_id,revision) REFERENCES agent_revisions(agent_id,revision)
) STRICT;
CREATE TABLE agent_default_receipts (
    key TEXT PRIMARY KEY NOT NULL, id TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL,
    agent_id TEXT REFERENCES agents(id), version INTEGER NOT NULL CHECK(version>0)
) STRICT;
";
const COLUMNS: &str =
    "r.agent_id,r.revision,r.name,r.driver,r.model,r.credential_ref,r.enabled,r.recorded_at";

impl AgentCatalog for SqliteCatalog {
    fn configure(&mut self, command: &ConfigureAgent) -> Result<AgentReceipt, AgentError> {
        let fingerprint = configure_fingerprint(command);
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        let previous = transaction.query_row("SELECT id,fingerprint,agent_id,revision FROM agent_configure_receipts WHERE key=?1", [&command.key], |row| {
            Ok((row.get::<_,String>(1)?, AgentReceipt { operation_id: parse(row,0)?, agent_id: parse(row,2)?, revision: revision(row,3)? }))
        }).optional().map_err(|error| sql(&error))?;
        if let Some((stored, receipt)) = previous {
            return if stored == fingerprint {
                Ok(receipt)
            } else {
                Err(AgentError::IdempotencyConflict)
            };
        }
        let (id, revision) = match command.target {
            AgentTarget::Create => (AgentId::generate(), Revision::new(1)?),
            AgentTarget::Update { id, expected } => {
                if current_revision(&transaction, id)? != expected {
                    return Err(AgentError::RevisionConflict);
                }
                if !command.config.enabled() && read_default(&transaction)?.agent_id == Some(id) {
                    return Err(AgentError::IsDefault);
                }
                (id, expected.next()?)
            }
        };
        let snapshot =
            AgentSnapshot::new(id, revision, command.config.clone(), command.recorded_at)?;
        publish(&transaction, &snapshot)?;
        let receipt = AgentReceipt {
            operation_id: OperationId::generate(),
            agent_id: id,
            revision,
        };
        transaction
            .execute(
                "INSERT INTO agent_configure_receipts VALUES(?1,?2,?3,?4,?5)",
                params![
                    command.key,
                    receipt.operation_id.to_string(),
                    fingerprint,
                    id.to_string(),
                    revision.value()
                ],
            )
            .map_err(|error| sql(&error))?;
        transaction.commit().map_err(|error| sql(&error))?;
        Ok(receipt)
    }

    fn get_agent(
        &mut self,
        id: AgentId,
        revision: Option<Revision>,
    ) -> Result<AgentSnapshot, AgentError> {
        let head = current_revision(&self.0, id)?;
        self.0
            .query_row(
                &format!(
                    "SELECT {COLUMNS} FROM agent_revisions r WHERE r.agent_id=?1 AND r.revision=?2"
                ),
                params![id.to_string(), revision.unwrap_or(head).value()],
                read,
            )
            .optional()
            .map_err(|error| sql(&error))?
            .ok_or(AgentError::RevisionNotFound)
    }

    fn list_agents(
        &mut self,
        after: Option<AgentId>,
        limit: usize,
    ) -> Result<Vec<AgentSnapshot>, AgentError> {
        if !(1..=50).contains(&limit) {
            return Err(AgentError::Invalid);
        }
        self.0.prepare(&format!("SELECT {COLUMNS} FROM agents a JOIN agent_revisions r ON r.agent_id=a.id AND r.revision=a.head WHERE a.id>?1 ORDER BY a.id LIMIT ?2")).map_err(|error| sql(&error))?
            .query_map(params![after.map(|id|id.to_string()).unwrap_or_default(),limit],read).map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>,_>>().map_err(|error| sql(&error))
    }

    fn get_default(&mut self) -> Result<DefaultSelection, AgentError> {
        read_default(&self.0)
    }

    fn set_default(&mut self, command: &SelectDefault) -> Result<DefaultReceipt, AgentError> {
        let fingerprint = serde_json::json!([
            command.agent_id.map(|id| id.to_string()),
            command.expected_version
        ])
        .to_string();
        let transaction = self
            .0
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        let previous = transaction
            .query_row(
                "SELECT id,fingerprint,agent_id,version FROM agent_default_receipts WHERE key=?1",
                [&command.key],
                |row| {
                    Ok((
                        row.get::<_, String>(1)?,
                        DefaultReceipt {
                            operation_id: parse(row, 0)?,
                            selection: DefaultSelection {
                                agent_id: optional_id(row, 2)?,
                                version: row.get(3)?,
                            },
                        },
                    ))
                },
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some((stored, receipt)) = previous {
            return if stored == fingerprint {
                Ok(receipt)
            } else {
                Err(AgentError::IdempotencyConflict)
            };
        }
        let current = read_default(&transaction)?;
        if current.version != command.expected_version {
            return Err(AgentError::DefaultConflict);
        }
        if current.version == i64::MAX as u64 {
            return Err(AgentError::Invalid);
        }
        if let Some(id) = command.agent_id {
            let enabled:bool = transaction.query_row("SELECT r.enabled FROM agents a JOIN agent_revisions r ON r.agent_id=a.id AND r.revision=a.head WHERE a.id=?1",[id.to_string()],|row|row.get(0)).optional().map_err(|error| sql(&error))?.ok_or(AgentError::NotFound)?;
            if !enabled {
                return Err(AgentError::Disabled);
            }
        }
        let receipt = DefaultReceipt {
            operation_id: OperationId::generate(),
            selection: DefaultSelection {
                agent_id: command.agent_id,
                version: current.version + 1,
            },
        };
        let id = command.agent_id.map(|id| id.to_string());
        transaction
            .execute(
                "UPDATE agent_default SET agent_id=?1,version=?2 WHERE singleton=1",
                params![id, receipt.selection.version],
            )
            .map_err(|error| sql(&error))?;
        transaction
            .execute(
                "INSERT INTO agent_default_receipts VALUES(?1,?2,?3,?4,?5)",
                params![
                    command.key,
                    receipt.operation_id.to_string(),
                    fingerprint,
                    id,
                    receipt.selection.version
                ],
            )
            .map_err(|error| sql(&error))?;
        transaction.commit().map_err(|error| sql(&error))?;
        Ok(receipt)
    }
}

fn configure_fingerprint(command: &ConfigureAgent) -> String {
    let (id, expected) = match command.target {
        AgentTarget::Create => (None, None),
        AgentTarget::Update { id, expected } => (Some(id.to_string()), Some(expected.value())),
    };
    let config = &command.config;
    serde_json::json!([
        id,
        expected,
        config.name(),
        config.driver().to_string(),
        config.model(),
        config.credential_ref().map(ToString::to_string),
        config.enabled()
    ])
    .to_string()
}

fn current_revision(connection: &Connection, id: AgentId) -> Result<Revision, AgentError> {
    connection
        .query_row(
            "SELECT head FROM agents WHERE id=?1",
            [id.to_string()],
            |row| revision(row, 0),
        )
        .optional()
        .map_err(|error| sql(&error))?
        .ok_or(AgentError::NotFound)
}

fn publish(connection: &Connection, snapshot: &AgentSnapshot) -> Result<(), AgentError> {
    let id = snapshot.id().to_string();
    connection
        .execute(
            "INSERT INTO agents VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET head=excluded.head",
            params![id, snapshot.revision().value()],
        )
        .map_err(|error| sql(&error))?;
    let config = snapshot.config();
    connection
        .execute(
            "INSERT INTO agent_revisions VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![
                id,
                snapshot.revision().value(),
                config.name(),
                config.driver().to_string(),
                config.model(),
                config.credential_ref().map(ToString::to_string),
                config.enabled(),
                snapshot.recorded_at()
            ],
        )
        .map_err(|error| sql(&error))?;
    Ok(())
}

fn read_default(connection: &Connection) -> Result<DefaultSelection, AgentError> {
    connection
        .query_row(
            "SELECT agent_id,version FROM agent_default WHERE singleton=1",
            [],
            |row| {
                Ok(DefaultSelection {
                    agent_id: optional_id(row, 0)?,
                    version: row.get(1)?,
                })
            },
        )
        .map_err(|error| sql(&error))
}

fn optional_id(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Option<AgentId>> {
    row.get::<_, Option<String>>(index)?
        .map(|value| value.parse().map_err(|_| rusqlite::Error::InvalidQuery))
        .transpose()
}

fn revision(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<Revision> {
    Revision::new(row.get(index)?).map_err(|_| rusqlite::Error::InvalidQuery)
}

fn read(row: &rusqlite::Row<'_>) -> rusqlite::Result<AgentSnapshot> {
    let credential = row
        .get::<_, Option<String>>(5)?
        .map(|value| value.parse())
        .transpose()
        .map_err(|_| rusqlite::Error::InvalidQuery)?;
    let config = AgentConfig::new(
        row.get(2)?,
        parse(row, 3)?,
        row.get(4)?,
        credential,
        row.get(6)?,
    )
    .map_err(|_| rusqlite::Error::InvalidQuery)?;
    AgentSnapshot::new(parse(row, 0)?, revision(row, 1)?, config, row.get(7)?)
        .map_err(|_| rusqlite::Error::InvalidQuery)
}

fn sql(error: &rusqlite::Error) -> AgentError {
    match error {
        rusqlite::Error::InvalidQuery
        | rusqlite::Error::FromSqlConversionFailure(..)
        | rusqlite::Error::IntegralValueOutOfRange(..) => AgentError::UnsupportedFormat,
        _ => match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked) => {
                AgentError::Busy
            }
            Some(
                rusqlite::ErrorCode::NotADatabase
                | rusqlite::ErrorCode::DatabaseCorrupt
                | rusqlite::ErrorCode::ConstraintViolation,
            ) => AgentError::UnsupportedFormat,
            _ => AgentError::Io,
        },
    }
}

#[cfg(test)]
mod tests;
