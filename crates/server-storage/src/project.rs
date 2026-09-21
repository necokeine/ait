use std::path::Path;

use rusqlite::{Connection, OptionalExtension, params};
use server_domain::{OwnerEpoch, Project, RootMessage};
use server_ports::{ProjectError, ProjectStorage, ProjectStore};

use crate::{open, parse, sql};

const FAMILY: i32 = 0x4153_5350;
const SCHEMA: &str = "
CREATE TABLE project (
    singleton INTEGER PRIMARY KEY CHECK(singleton=1),
    id TEXT NOT NULL UNIQUE, name TEXT NOT NULL, base_commit TEXT NOT NULL,
    root_id TEXT NOT NULL REFERENCES messages(id) DEFERRABLE INITIALLY DEFERRED
) STRICT;
CREATE TABLE messages (
    id TEXT PRIMARY KEY NOT NULL, project_id TEXT NOT NULL REFERENCES project(id),
    parent_id TEXT CHECK(parent_id IS NULL), role TEXT NOT NULL CHECK(role='system'),
    origin TEXT NOT NULL CHECK(origin='project'), text TEXT NOT NULL,
    created_at INTEGER NOT NULL CHECK(created_at>=0)
) STRICT;
CREATE TABLE ownership (singleton INTEGER PRIMARY KEY CHECK(singleton=1), epoch INTEGER NOT NULL CHECK(epoch>=0)) STRICT;
INSERT INTO ownership VALUES(1,0);
CREATE TRIGGER immutable_messages_update BEFORE UPDATE ON messages BEGIN SELECT RAISE(ABORT, 'immutable message'); END;
CREATE TRIGGER immutable_messages_delete BEFORE DELETE ON messages BEGIN SELECT RAISE(ABORT, 'immutable message'); END;
CREATE TRIGGER immutable_project_update BEFORE UPDATE ON project BEGIN SELECT RAISE(ABORT, 'immutable project creation'); END;
CREATE TRIGGER immutable_project_delete BEFORE DELETE ON project BEGIN SELECT RAISE(ABORT, 'immutable project creation'); END;
";

/// Factory for project databases held under independent workspace leases.
#[derive(Debug, Default)]
pub struct SqliteProjects;

impl ProjectStorage for SqliteProjects {
    fn open(&self, root: &Path) -> Result<Box<dyn ProjectStore>, ProjectError> {
        let connection = open(&root.join(".ait-server/project.sqlite3"), FAMILY, SCHEMA, 1)?;
        Ok(Box::new(Store(connection)))
    }
}

#[derive(Debug)]
struct Store(Connection);

impl ProjectStore for Store {
    fn initialize(&mut self, initial: &Project) -> Result<Project, ProjectError> {
        if let Some(project) = read(&self.0)? {
            return Ok(project);
        }
        let transaction = self
            .0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        transaction
            .execute(
                "INSERT INTO project VALUES(1,?1,?2,?3,?4)",
                params![
                    initial.id().to_string(),
                    initial.name(),
                    initial.base_commit().to_string(),
                    initial.root().id().to_string()
                ],
            )
            .map_err(|error| sql(&error))?;
        transaction
            .execute(
                "INSERT INTO messages VALUES(?1,?2,NULL,'system','project',?3,?4)",
                params![
                    initial.root().id().to_string(),
                    initial.id().to_string(),
                    initial.root().text(),
                    initial.root().created_at()
                ],
            )
            .map_err(|error| sql(&error))?;
        transaction.commit().map_err(|error| sql(&error))?;
        Ok(initial.clone())
    }

    fn owner_epoch(&mut self) -> Result<OwnerEpoch, ProjectError> {
        epoch(&self.0)
    }

    fn claim(&mut self, reserved: OwnerEpoch) -> Result<(), ProjectError> {
        let transaction = self
            .0
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|error| sql(&error))?;
        let changed = transaction
            .execute(
                "UPDATE ownership SET epoch=?1 WHERE singleton=1 AND epoch<?1",
                [reserved.value()],
            )
            .map_err(|error| sql(&error))?;
        if changed != 1 {
            return Err(ProjectError::StaleOwner);
        }
        transaction.commit().map_err(|error| sql(&error))?;
        Ok(())
    }

    fn check_owner(&mut self, expected: OwnerEpoch) -> Result<(), ProjectError> {
        if expected.value() == 0 || epoch(&self.0)? != expected {
            return Err(ProjectError::StaleOwner);
        }
        Ok(())
    }
}

fn epoch(connection: &Connection) -> Result<OwnerEpoch, ProjectError> {
    OwnerEpoch::new(
        connection
            .query_row("SELECT epoch FROM ownership WHERE singleton=1", [], |row| {
                row.get(0)
            })
            .map_err(|error| sql(&error))?,
    )
    .map_err(Into::into)
}

fn read(connection: &Connection) -> Result<Option<Project>, ProjectError> {
    connection.query_row("SELECT p.id,p.name,p.base_commit,m.id,m.text,m.created_at FROM project p JOIN messages m ON m.id=p.root_id AND m.project_id=p.id WHERE p.singleton=1",
        [], |row| {
            let root = RootMessage::new(parse(row,3)?, row.get(4)?, row.get(5)?).map_err(|_| rusqlite::Error::InvalidQuery)?;
            Project::new(parse(row,0)?, row.get(1)?, parse(row,2)?, root).map_err(|_| rusqlite::Error::InvalidQuery)
        }).optional().map_err(|error| sql(&error))
}
