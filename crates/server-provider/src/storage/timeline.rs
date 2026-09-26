//! Durable append-only display projection. Native Provider history remains authoritative.

pub(crate) mod inbox;
mod progress;
mod subagents;

use std::path::Path;
use std::sync::{Arc, Mutex};

use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use server_model::ErrorCode;
use server_model::events::EventHub;
use uuid::Uuid;

use crate::protocol::timeline::NativeItem;

/// Immutable timeline row with a stable sequence in a durable generation.
#[derive(Debug, Clone)]
pub struct Row {
    /// Sequence assigned at the first committed observation.
    pub seq: u64,
    /// Provider that produced this display history.
    pub provider: String,
    /// Original normalized native or plugin display entry.
    pub entry: NativeItem,
}

impl Row {
    /// Project one canonical row without modifying or merging stored history.
    #[must_use]
    pub fn value(&self) -> Value {
        let mut value = json!({"provider":self.provider,"item":self.entry.item,
            "timestamp":self.entry.timestamp,"seqStart":self.seq,"seqEnd":self.seq,
            "sourceSeqRanges":[{"startSeq":self.seq,"endSeq":self.seq}],"collapsed":["identity"]});
        if let Some(turn) = &self.entry.turn_id {
            value["turnId"] = json!(turn);
        }
        value
    }
}

/// Shared SQLite projection and its post-commit observers.
#[derive(Debug, Clone)]
pub struct Timeline {
    database: Arc<Mutex<Connection>>,
    events: EventHub,
}

impl Timeline {
    /// Open a dedicated projection database at `path`, creating its parent directories.
    /// # Errors
    /// Returns Agent I/O errors for unreadable, incompatible or unwritable storage.
    pub fn open(path: &Path) -> Result<Self, ErrorCode> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        for candidate in [
            path.to_path_buf(),
            std::path::PathBuf::from(format!("{}-wal", path.display())),
            std::path::PathBuf::from(format!("{}-shm", path.display())),
        ] {
            match std::fs::symlink_metadata(candidate) {
                Ok(metadata) if !metadata.is_file() || metadata.file_type().is_symlink() => {
                    return Err(ErrorCode::AgentIo);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(_) => return Err(ErrorCode::AgentIo),
            }
        }
        Self::initialize(Connection::open(path).map_err(io)?)
    }

    /// Construct isolated in-memory storage for embedded hosts and tests.
    /// # Errors
    /// Returns Agent I/O errors if SQLite cannot allocate its database.
    pub fn memory() -> Result<Self, ErrorCode> {
        Self::initialize(Connection::open_in_memory().map_err(io)?)
    }

    fn initialize(database: Connection) -> Result<Self, ErrorCode> {
        let version: i64 = database
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .map_err(io)?;
        let application: i64 = database
            .query_row("PRAGMA application_id", [], |row| row.get(0))
            .map_err(io)?;
        let tables: i64 = database.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%'", [], |row|row.get(0)).map_err(io)?;
        if !(version == 0 && application == 0 && tables == 0
            || version == 1 && application == 0x4154_544C && tables == 2
            || version == 2 && application == 0x4154_544C && tables == 3
            || version == 3 && application == 0x4154_544C && tables == 4)
            && !(version == 4 && application == 0x4154_544C && tables == 5)
            && !(version == 5 && application == 0x4154_544C && tables == 6)
        {
            return Err(ErrorCode::UnsupportedFormat);
        }
        database
            .execute_batch(
                "PRAGMA journal_mode=WAL; PRAGMA synchronous=FULL; BEGIN IMMEDIATE;
            CREATE TABLE IF NOT EXISTS timelines(agent TEXT PRIMARY KEY, epoch TEXT NOT NULL);
            CREATE TABLE IF NOT EXISTS entries(agent TEXT NOT NULL, seq INTEGER NOT NULL,
                identity TEXT NOT NULL, provider TEXT NOT NULL, entry TEXT NOT NULL,
                PRIMARY KEY(agent,seq), UNIQUE(agent,identity));
            CREATE TABLE IF NOT EXISTS retired_entries(agent TEXT NOT NULL, epoch TEXT NOT NULL,
                seq INTEGER NOT NULL, identity TEXT NOT NULL, provider TEXT NOT NULL, entry TEXT NOT NULL,
                PRIMARY KEY(agent,epoch,seq));
            CREATE TABLE IF NOT EXISTS progress(agent TEXT NOT NULL, seq INTEGER NOT NULL,
                identity TEXT NOT NULL, native_key TEXT NOT NULL, provider TEXT NOT NULL,
                entry TEXT NOT NULL, PRIMARY KEY(agent,seq), UNIQUE(agent,identity));
            CREATE INDEX IF NOT EXISTS progress_native ON progress(agent,native_key,seq);
            CREATE TABLE IF NOT EXISTS input_receipts(seq INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL, message TEXT NOT NULL, fingerprint TEXT NOT NULL,
                prompt TEXT, state TEXT NOT NULL, UNIQUE(agent,message));
            CREATE INDEX IF NOT EXISTS queued_inputs ON input_receipts(state,agent,seq);
            CREATE TABLE IF NOT EXISTS provider_subagents(agent TEXT NOT NULL, child TEXT NOT NULL,
                descriptor TEXT NOT NULL, PRIMARY KEY(agent,child));
            PRAGMA user_version=5; PRAGMA application_id=1096045644; COMMIT;",
            )
            .map_err(io)?;
        Ok(Self {
            database: Arc::new(Mutex::new(database)),
            events: EventHub::default(),
        })
    }

    /// Return the event producer shared by this projection's subscribers.
    #[must_use]
    pub fn events(&self) -> EventHub {
        self.events.clone()
    }

    /// Read the immutable generation and ordered rows, creating an empty generation if necessary.
    /// # Errors
    /// Returns storage or invalid persisted-data errors.
    pub fn read(&self, agent: &str) -> Result<(String, Vec<Row>), ErrorCode> {
        let database = self.database.lock().map_err(io)?;
        let epoch = epoch(&database, agent)?;
        Ok((epoch, progress::read(&database, agent)?))
    }

    /// Atomically reconcile a complete native history, retaining plugin rows and retired generations.
    /// An unchanged native prefix keeps its cursor; rewrites retire the old generation and publish
    /// a replacement event only after the transaction commits. No domain Message is changed.
    /// # Errors
    /// Returns conflicts, oversize data or storage errors without replacing the visible generation.
    pub fn reconcile(
        &self,
        agent: &str,
        provider: &str,
        entries: &[NativeItem],
    ) -> Result<String, ErrorCode> {
        let mut database = self.database.lock().map_err(io)?;
        let transaction = database.transaction().map_err(io)?;
        let old_epoch = epoch(&transaction, agent)?;
        let previous = read_rows(&transaction, agent)?;
        let native: Vec<_> = previous
            .iter()
            .filter(|row| row.entry.item["type"] != "plugin")
            .collect();
        let prefix = progress::covered(&transaction, agent, entries)?
            && native.len() <= entries.len()
            && native.iter().zip(entries).all(|(old, new)| {
                old.entry.key == new.key
                    && old.entry.item == new.item
                    && old.entry.turn_id == new.turn_id
            });
        if prefix {
            let appended = append_rows(&transaction, agent, provider, entries)?;
            transaction.commit().map_err(io)?;
            self.publish(agent, provider, &old_epoch, &appended.entries);
            return Ok(old_epoch);
        }
        transaction.execute(
            "INSERT INTO retired_entries SELECT agent,?,seq,identity,provider,entry FROM entries WHERE agent=?",
            params![old_epoch,agent],
        ).map_err(io)?;
        transaction.execute(
            "INSERT INTO retired_entries SELECT agent,?,seq,identity,provider,entry FROM progress WHERE agent=?",
            params![old_epoch,agent],
        ).map_err(io)?;
        transaction
            .execute("DELETE FROM progress WHERE agent=?", [agent])
            .map_err(io)?;
        transaction
            .execute("DELETE FROM entries WHERE agent=?", [agent])
            .map_err(io)?;
        let replacement = Uuid::new_v4().to_string();
        transaction
            .execute(
                "UPDATE timelines SET epoch=? WHERE agent=?",
                params![replacement, agent],
            )
            .map_err(io)?;
        append_rows(&transaction, agent, provider, entries)?;
        let plugins: Vec<_> = previous
            .into_iter()
            .filter(|row| row.entry.item["type"] == "plugin")
            .map(|row| row.entry)
            .collect();
        append_rows(&transaction, agent, provider, &plugins)?;
        transaction.commit().map_err(io)?;
        self.events.publish(
            agent,
            "agent.timeline.replacement",
            &json!({"agentId":agent,"epoch":replacement}),
        );
        Ok(replacement)
    }

    /// Atomically append new source identities, deduplicating replay without rewriting old rows.
    /// Existing identities must retain their payload. Timestamp differences on replay are ignored.
    /// # Errors
    /// Returns conflicts for changed payloads and storage errors before publishing any event.
    pub fn append(
        &self,
        agent: &str,
        provider: &str,
        entries: &[NativeItem],
    ) -> Result<(String, Vec<u64>), ErrorCode> {
        let mut database = self.database.lock().map_err(io)?;
        let transaction = database.transaction().map_err(io)?;
        let epoch = epoch(&transaction, agent)?;
        let appended = append_rows(&transaction, agent, provider, entries)?;
        transaction.commit().map_err(io)?;
        self.publish(agent, provider, &epoch, &appended.entries);
        Ok((epoch, appended.positions))
    }

    fn publish(&self, agent: &str, provider: &str, epoch: &str, entries: &[(u64, NativeItem)]) {
        // The caller retains the database lock so concurrent commits cannot reorder events.
        for (seq, entry) in entries {
            if let Some((parent, child)) = agent
                .strip_prefix("subagent:")
                .and_then(|scope| scope.split_once(':'))
            {
                self.events.publish(
                    parent,
                    "agent.provider_subagents.update",
                    &json!({
                    "kind":"timeline","parentAgentId":parent,"subagentId":child,"provider":provider,
                    "item":entry.item,"timestamp":entry.timestamp,"seq":seq,"epoch":epoch}),
                );
                continue;
            }
            let mut event = json!({"type":"timeline","provider":provider,"item":entry.item});
            if let Some(turn) = &entry.turn_id {
                event["turnId"] = json!(turn);
            }
            self.events.publish(
                agent,
                "agent_stream",
                &json!({"agentId":agent,"event":event,
                "timestamp":entry.timestamp,"seq":seq,"epoch":epoch}),
            );
        }
    }
}

fn read_rows(database: &Connection, agent: &str) -> Result<Vec<Row>, ErrorCode> {
    let mut query = database
        .prepare("SELECT seq, provider, entry FROM entries WHERE agent=? ORDER BY seq")
        .map_err(io)?;
    let rows = query
        .query_map([agent], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
            ))
        })
        .map_err(io)?;
    let entries = rows
        .map(|row| {
            let (seq, provider, entry) = row.map_err(io)?;
            Ok(Row {
                seq,
                provider,
                entry: serde_json::from_str(&entry).map_err(io)?,
            })
        })
        .collect::<Result<Vec<_>, ErrorCode>>()?;
    Ok(entries)
}

struct Appended {
    positions: Vec<u64>,
    entries: Vec<(u64, NativeItem)>,
}

fn append_rows(
    transaction: &Connection,
    agent: &str,
    provider: &str,
    entries: &[NativeItem],
) -> Result<Appended, ErrorCode> {
    let mut next = progress::next(transaction, agent)?;
    let mut positions = Vec::with_capacity(entries.len());
    let mut appended = Vec::new();
    for entry in entries {
        let existing: Option<(u64, String)> = transaction
            .query_row(
                "SELECT seq,entry FROM entries WHERE agent=? AND identity=?",
                params![agent, entry.key],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(io)?;
        if let Some((seq, previous)) = existing {
            let previous: NativeItem = serde_json::from_str(&previous).map_err(io)?;
            if previous.item != entry.item || previous.turn_id != entry.turn_id {
                return Err(ErrorCode::IdempotencyConflict);
            }
            positions.push(seq);
            continue;
        }
        let bytes = serde_json::to_string(entry).map_err(io)?;
        if bytes.len() > 256 * 1024 {
            return Err(ErrorCode::ResourceExhausted);
        }
        let projected = progress::completion(transaction, agent, entry)?;
        transaction
            .execute(
                "INSERT INTO entries(agent,seq,identity,provider,entry) VALUES(?,?,?,?,?)",
                params![agent, next, entry.key, provider, bytes],
            )
            .map_err(io)?;
        positions.push(next);
        appended.push((next, projected));
        next = next.checked_add(1).ok_or(ErrorCode::ResourceExhausted)?;
    }
    Ok(Appended {
        positions,
        entries: appended,
    })
}

fn epoch(database: &Connection, agent: &str) -> Result<String, ErrorCode> {
    if let Some(epoch) = database
        .query_row(
            "SELECT epoch FROM timelines WHERE agent=?",
            [agent],
            |row| row.get(0),
        )
        .optional()
        .map_err(io)?
    {
        return Ok(epoch);
    }
    let epoch = Uuid::new_v4().to_string();
    database
        .execute(
            "INSERT INTO timelines(agent,epoch) VALUES(?,?)",
            params![agent, epoch],
        )
        .map_err(io)?;
    Ok(epoch)
}

fn io<T>(_: T) -> ErrorCode {
    ErrorCode::AgentIo
}

#[cfg(test)]
mod tests;
