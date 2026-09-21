use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use server_domain::{OperationId, ProjectId};
use server_ports::{Catalog, CatalogEntry, OpenIntent, ProjectError, Receipt};

use crate::{open, parse, sql};

const FAMILY: i32 = 0x4153_5343;
const SCHEMA: &str = "
CREATE TABLE projects (
    id TEXT PRIMARY KEY NOT NULL, path TEXT NOT NULL UNIQUE, name TEXT NOT NULL,
    base_commit TEXT NOT NULL, root_id TEXT NOT NULL, created_at INTEGER NOT NULL
) STRICT;
CREATE TABLE operations (
    method TEXT NOT NULL CHECK(method IN ('open','close')), key TEXT NOT NULL,
    id TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL, project_id TEXT,
    PRIMARY KEY(method,key)
) STRICT;
";
const ENTRY_COLUMNS: &str = "id,path,name,base_commit,root_id,created_at";

/// Durable catalog index and operation receipts, owned by the data-directory instance lease.
#[derive(Debug)]
pub struct SqliteCatalog(Connection);

impl SqliteCatalog {
    /// Open/create `data_dir/catalog.sqlite3` in the independent catalog schema family.
    ///
    /// # Errors
    /// Rejects foreign/newer schemas, symlink state files, and storage failures.
    pub fn open(data_dir: &Path) -> Result<Self, ProjectError> {
        Ok(Self(open(
            &data_dir.join("catalog.sqlite3"),
            FAMILY,
            SCHEMA,
        )?))
    }
}

impl Catalog for SqliteCatalog {
    fn begin_open(&mut self, key: &str, path: &Path) -> Result<OpenIntent, ProjectError> {
        let fingerprint = path.to_str().ok_or(ProjectError::Invalid)?;
        if let Some((id, previous, project)) = operation(&self.0, "open", key)? {
            if previous != fingerprint {
                return Err(ProjectError::IdempotencyConflict);
            }
            return Ok(OpenIntent {
                operation_id: id,
                path: path.to_owned(),
                receipt: project.map(|project_id| Receipt {
                    operation_id: id,
                    project_id,
                }),
            });
        }
        let id = OperationId::generate();
        self.0
            .execute(
                "INSERT INTO operations VALUES('open',?1,?2,?3,NULL)",
                params![key, id.to_string(), fingerprint],
            )
            .map_err(|error| sql(&error))?;
        Ok(OpenIntent {
            operation_id: id,
            path: path.to_owned(),
            receipt: None,
        })
    }

    fn finish_open(
        &mut self,
        intent: &OpenIntent,
        entry: &CatalogEntry,
    ) -> Result<Receipt, ProjectError> {
        if intent.path != entry.path {
            return Err(ProjectError::IdentityConflict);
        }
        let transaction = self
            .0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        let path = entry.path.to_str().ok_or(ProjectError::Invalid)?;
        let existing = transaction
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM projects WHERE id=?1"),
                [entry.id.to_string()],
                read_entry,
            )
            .optional()
            .map_err(|error| sql(&error))?;
        if let Some(mut existing) = existing {
            existing.path.clone_from(&entry.path);
            if existing != *entry {
                return Err(ProjectError::IdentityConflict);
            }
        }
        transaction.execute("INSERT INTO projects VALUES(?1,?2,?3,?4,?5,?6) ON CONFLICT(id) DO UPDATE SET path=excluded.path",
            params![entry.id.to_string(), path, entry.name, entry.base_commit.to_string(), entry.root_message_id.to_string(), entry.created_at]).map_err(|error| sql(&error))?;
        let changed = transaction.execute("UPDATE operations SET project_id=?1 WHERE id=?2 AND method='open' AND fingerprint=?3 AND (project_id IS NULL OR project_id=?1)",
            params![entry.id.to_string(), intent.operation_id.to_string(), path]).map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(ProjectError::IdempotencyConflict);
        }
        transaction.commit().map_err(|error| sql(&error))?;
        Ok(Receipt {
            operation_id: intent.operation_id,
            project_id: entry.id,
        })
    }

    fn get(&mut self, id: ProjectId) -> Result<CatalogEntry, ProjectError> {
        self.0
            .query_row(
                &format!("SELECT {ENTRY_COLUMNS} FROM projects WHERE id=?1"),
                [id.to_string()],
                read_entry,
            )
            .optional()
            .map_err(|error| sql(&error))?
            .ok_or(ProjectError::NotFound)
    }

    fn list(
        &mut self,
        after: Option<ProjectId>,
        limit: usize,
    ) -> Result<Vec<CatalogEntry>, ProjectError> {
        if !(1..=50).contains(&limit) {
            return Err(ProjectError::Invalid);
        }
        self.0
            .prepare(&format!(
                "SELECT {ENTRY_COLUMNS} FROM projects WHERE id>?1 ORDER BY id LIMIT ?2"
            ))
            .map_err(|error| sql(&error))?
            .query_map(
                params![after.map(|id| id.to_string()).unwrap_or_default(), limit],
                read_entry,
            )
            .map_err(|error| sql(&error))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|error| sql(&error))
    }

    fn close_receipt(&mut self, key: &str, id: ProjectId) -> Result<Option<Receipt>, ProjectError> {
        operation(&self.0, "close", key)?
            .map(|(operation_id, fingerprint, project)| {
                if fingerprint != id.to_string() || project != Some(id) {
                    return Err(ProjectError::IdempotencyConflict);
                }
                Ok(Receipt {
                    operation_id,
                    project_id: id,
                })
            })
            .transpose()
    }

    fn finish_close(&mut self, key: &str, id: ProjectId) -> Result<Receipt, ProjectError> {
        if let Some(receipt) = self.close_receipt(key, id)? {
            return Ok(receipt);
        }
        let operation_id = OperationId::generate();
        self.0
            .execute(
                "INSERT INTO operations VALUES('close',?1,?2,?3,?3)",
                params![key, operation_id.to_string(), id.to_string()],
            )
            .map_err(|error| sql(&error))?;
        Ok(Receipt {
            operation_id,
            project_id: id,
        })
    }
}

fn operation(
    connection: &Connection,
    method: &str,
    key: &str,
) -> Result<Option<(OperationId, String, Option<ProjectId>)>, ProjectError> {
    connection
        .query_row(
            "SELECT id,fingerprint,project_id FROM operations WHERE method=?1 AND key=?2",
            params![method, key],
            |row| {
                let project = row
                    .get::<_, Option<String>>(2)?
                    .map(|id| id.parse())
                    .transpose()
                    .map_err(|_| rusqlite::Error::InvalidQuery)?;
                Ok((parse(row, 0)?, row.get(1)?, project))
            },
        )
        .optional()
        .map_err(|error| sql(&error))
}

fn read_entry(row: &rusqlite::Row<'_>) -> rusqlite::Result<CatalogEntry> {
    Ok(CatalogEntry {
        id: parse(row, 0)?,
        path: row.get::<_, String>(1)?.into(),
        name: row.get(2)?,
        base_commit: parse(row, 3)?,
        root_message_id: parse(row, 4)?,
        created_at: row.get(5)?,
    })
}
