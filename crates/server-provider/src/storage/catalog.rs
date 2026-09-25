use std::path::Path;

use rusqlite::Connection;

use crate::ports::agent::AgentError;
use crate::storage::open;

const FAMILY: i32 = 0x4153_5343;

/// Agent preset catalog, owned by the host data-directory lease.
#[derive(Debug)]
pub struct SqliteCatalog(pub(super) Connection);

impl SqliteCatalog {
    /// Open or initialize the Agent catalog without creating any Project tables.
    ///
    /// # Errors
    /// Rejects unsupported schemas, unsafe state files and storage failures.
    pub fn open(data_dir: &Path) -> Result<Self, AgentError> {
        let mut connection = open(
            &data_dir.join("catalog.sqlite3"),
            FAMILY,
            crate::storage::agents::SCHEMA,
            2,
        )?;
        crate::storage::migration::catalog(&mut connection, data_dir)?;
        Ok(Self(connection))
    }
}
