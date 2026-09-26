//! Durable display descriptors for native tasks whose output can outlive their parent turn.

use rusqlite::{OptionalExtension, params};

use super::{ErrorCode, Timeline, io};
use crate::ports::controls::NativeSubagent;

impl Timeline {
    pub(crate) fn store_subagent(
        &self,
        parent: &str,
        child: &NativeSubagent,
    ) -> Result<(), ErrorCode> {
        if child.id.is_empty()
            || child.id.len() > 256
            || child.parent_id.is_empty()
            || child.descriptor["id"] != child.id
            || child.cwd.len() > 16384
        {
            return Err(ErrorCode::InvalidMessage);
        }
        let encoded = serde_json::to_string(child).map_err(io)?;
        if encoded.len() > 65536 {
            return Err(ErrorCode::ResourceExhausted);
        }
        let mut database = self.database.lock().map_err(io)?;
        let tx = database.transaction().map_err(io)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT descriptor FROM provider_subagents WHERE agent=? AND child=?",
                params![parent, child.id],
                |row| row.get(0),
            )
            .optional()
            .map_err(io)?;
        if let Some(previous) = previous {
            let previous: NativeSubagent = serde_json::from_str(&previous).map_err(io)?;
            if previous.parent_id != child.parent_id {
                return Err(ErrorCode::IdempotencyConflict);
            }
        } else {
            let count: u64 = tx
                .query_row(
                    "SELECT count(*) FROM provider_subagents WHERE agent=?",
                    [parent],
                    |row| row.get(0),
                )
                .map_err(io)?;
            if count >= 4096 {
                return Err(ErrorCode::ResourceExhausted);
            }
        }
        tx.execute("INSERT INTO provider_subagents(agent,child,descriptor) VALUES(?,?,?) ON CONFLICT(agent,child) DO UPDATE SET descriptor=excluded.descriptor",
            params![parent,child.id,encoded]).map_err(io)?;
        tx.commit().map_err(io)?;
        Ok(())
    }

    pub(crate) fn subagents(&self, parent: &str) -> Result<Vec<NativeSubagent>, ErrorCode> {
        let database = self.database.lock().map_err(io)?;
        let mut query = database
            .prepare("SELECT descriptor FROM provider_subagents WHERE agent=? ORDER BY child")
            .map_err(io)?;
        query
            .query_map([parent], |row| row.get::<_, String>(0))
            .map_err(io)?
            .map(|row| serde_json::from_str(&row.map_err(io)?).map_err(io))
            .collect()
    }
}

#[cfg(test)]
mod tests;
