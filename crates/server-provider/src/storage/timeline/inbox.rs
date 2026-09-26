//! Durable admission receipts. A claimed native write is never automatically replayed.

use rusqlite::{OptionalExtension, params};
use sha2::{Digest, Sha256};

use super::{ErrorCode, Timeline, io};
use crate::protocol::prompt::AgentPrompt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Receipt {
    New,
    Accepted,
    Uncertain,
    Rejected,
}

#[derive(Debug)]
pub(crate) struct QueuedInput {
    pub(crate) agent: String,
    pub(crate) message: String,
    pub(crate) prompt: AgentPrompt,
}

impl Timeline {
    /// Read an existing receipt before current ownership checks, without admitting new work.
    pub(crate) fn input_receipt(
        &self,
        agent: &str,
        message: &str,
        prompt: &AgentPrompt,
        policy: &str,
    ) -> Result<Option<Receipt>, ErrorCode> {
        let fingerprint = format!(
            "{:x}",
            Sha256::digest(format!(
                "{policy}\n{}",
                serde_json::to_string(prompt).map_err(io)?
            ))
        );
        let database = self.database.lock().map_err(io)?;
        let existing: Option<(String, String)> = database
            .query_row(
                "SELECT fingerprint,state FROM input_receipts WHERE agent=? AND message=?",
                params![agent, message],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(io)?;
        existing
            .map(|(previous, state)| {
                if previous != fingerprint {
                    return Err(ErrorCode::IdempotencyConflict);
                }
                match state.as_str() {
                    "queued" | "accepted" => Ok(Receipt::Accepted),
                    "claimed" | "uncertain" => Ok(Receipt::Uncertain),
                    "rejected" | "cancelled" => Ok(Receipt::Rejected),
                    _ => Err(ErrorCode::UnsupportedFormat),
                }
            })
            .transpose()
    }

    /// Fence an immediate question answer. Definitive rejections may be retried by the caller;
    /// answers are never put in the interrupt queue or replayed on process recovery.
    pub(crate) fn claim_answer(
        &self,
        agent: &str,
        message: &str,
        prompt: &AgentPrompt,
    ) -> Result<Receipt, ErrorCode> {
        prompt.validate().map_err(|_| ErrorCode::InvalidMessage)?;
        let encoded = serde_json::to_string(prompt).map_err(io)?;
        let fingerprint = format!("{:x}", Sha256::digest(format!("answer\n{encoded}")));
        let mut database = self.database.lock().map_err(io)?;
        let tx = database.transaction().map_err(io)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint,state FROM input_receipts WHERE agent=? AND message=?",
                params![agent, message],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(io)?;
        if let Some((previous, state)) = existing {
            if previous != fingerprint {
                return Err(ErrorCode::IdempotencyConflict);
            }
            match state.as_str() {
                "accepted" => return Ok(Receipt::Accepted),
                "claimed" | "uncertain" => return Ok(Receipt::Uncertain),
                "rejected" => {
                    tx.execute(
                        "UPDATE input_receipts SET state='claimed' WHERE agent=? AND message=?",
                        params![agent, message],
                    )
                    .map_err(io)?;
                }
                _ => return Err(ErrorCode::InvalidMessage),
            }
        } else {
            tx.execute("INSERT INTO input_receipts(agent,message,fingerprint,prompt,state) VALUES(?,?,?,NULL,'claimed')",
                params![agent,message,fingerprint]).map_err(io)?;
        }
        tx.commit().map_err(io)?;
        Ok(Receipt::New)
    }

    /// Reserve input before any native effect; exact duplicates reuse their committed receipt.
    pub(crate) fn reserve_input(
        &self,
        agent: &str,
        message: &str,
        prompt: &AgentPrompt,
        policy: &str,
    ) -> Result<Receipt, ErrorCode> {
        prompt.validate().map_err(|_| ErrorCode::InvalidMessage)?;
        let encoded = serde_json::to_string(prompt).map_err(io)?;
        let fingerprint = format!("{:x}", Sha256::digest(format!("{policy}\n{encoded}")));
        let mut database = self.database.lock().map_err(io)?;
        let tx = database.transaction().map_err(io)?;
        let existing: Option<(String, String)> = tx
            .query_row(
                "SELECT fingerprint,state FROM input_receipts WHERE agent=? AND message=?",
                params![agent, message],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .map_err(io)?;
        if let Some((previous, state)) = existing {
            if previous != fingerprint {
                return Err(ErrorCode::IdempotencyConflict);
            }
            return Ok(match state.as_str() {
                "queued" | "accepted" => Receipt::Accepted,
                "claimed" | "uncertain" => Receipt::Uncertain,
                "rejected" | "cancelled" => Receipt::Rejected,
                _ => return Err(ErrorCode::UnsupportedFormat),
            });
        }
        let (count, bytes): (u64,u64) = tx.query_row(
            "SELECT count(*),coalesce(sum(length(prompt)),0) FROM input_receipts WHERE state='queued' AND agent=?",
            [agent], |row|Ok((row.get(0)?,row.get(1)?))).map_err(io)?;
        let total: u64 = tx
            .query_row(
                "SELECT coalesce(sum(length(prompt)),0) FROM input_receipts WHERE state='queued'",
                [],
                |row| row.get(0),
            )
            .map_err(io)?;
        if count >= 32
            || bytes.saturating_add(encoded.len() as u64) > 4 * 1024 * 1024
            || total.saturating_add(encoded.len() as u64) > 32 * 1024 * 1024
        {
            return Err(ErrorCode::ResourceExhausted);
        }
        tx.execute("INSERT INTO input_receipts(agent,message,fingerprint,prompt,state) VALUES(?,?,?,?,'queued')",
            params![agent,message,fingerprint,encoded]).map_err(io)?;
        tx.commit().map_err(io)?;
        Ok(Receipt::New)
    }

    /// Inspect queued work in FIFO order without changing admission state.
    pub(crate) fn queued_inputs(&self) -> Result<Vec<QueuedInput>, ErrorCode> {
        let database = self.database.lock().map_err(io)?;
        let mut query = database.prepare("SELECT agent,message,prompt FROM input_receipts WHERE state='queued' ORDER BY seq LIMIT 1024").map_err(io)?;
        query
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .map_err(io)?
            .map(|row| {
                let (agent, message, prompt) = row.map_err(io)?;
                Ok(QueuedInput {
                    agent,
                    message,
                    prompt: serde_json::from_str(&prompt).map_err(io)?,
                })
            })
            .collect()
    }

    /// Whether unsubmitted work remains for this Agent.
    pub(crate) fn has_queued_input(&self, agent: &str) -> Result<bool, ErrorCode> {
        self.database
            .lock()
            .map_err(io)?
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM input_receipts WHERE agent=? AND state='queued')",
                [agent],
                |row| row.get(0),
            )
            .map_err(io)
    }

    /// Commit the no-replay fence before writing to a native provider.
    pub(crate) fn claim_input(&self, agent: &str, message: &str) -> Result<(), ErrorCode> {
        let changed = self.database.lock().map_err(io)?.execute(
            "UPDATE input_receipts SET state='claimed' WHERE agent=? AND message=? AND state='queued'", params![agent,message]).map_err(io)?;
        if changed != 1 {
            return Err(ErrorCode::IdempotencyConflict);
        }
        Ok(())
    }

    /// Store an admission outcome and discard the potentially large prompt body.
    pub(crate) fn finish_input(
        &self,
        agent: &str,
        message: &str,
        result: Receipt,
    ) -> Result<(), ErrorCode> {
        let state = match result {
            Receipt::Accepted => "accepted",
            Receipt::Uncertain => "uncertain",
            Receipt::Rejected => "rejected",
            Receipt::New => return Err(ErrorCode::InvalidMessage),
        };
        let changed = self.database.lock().map_err(io)?.execute(
            "UPDATE input_receipts SET state=?,prompt=NULL WHERE agent=? AND message=? AND state IN ('queued','claimed')", params![state,agent,message]).map_err(io)?;
        if changed != 1 {
            return Err(ErrorCode::IdempotencyConflict);
        }
        Ok(())
    }

    /// A definitive native steering rejection permits fallback delivery of that same input.
    pub(crate) fn requeue_input(&self, agent: &str, message: &str) -> Result<(), ErrorCode> {
        let changed = self.database.lock().map_err(io)?.execute(
            "UPDATE input_receipts SET state='queued' WHERE agent=? AND message=? AND state='claimed' AND prompt IS NOT NULL", params![agent,message]).map_err(io)?;
        if changed != 1 {
            return Err(ErrorCode::IdempotencyConflict);
        }
        Ok(())
    }

    /// Explicit cancellation withdraws only inputs that never reached the native boundary.
    pub(crate) fn cancel_inputs(&self, agent: &str) -> Result<(), ErrorCode> {
        self.database.lock().map_err(io)?.execute("UPDATE input_receipts SET state='cancelled',prompt=NULL WHERE agent=? AND state='queued'", [agent]).map_err(io)?;
        Ok(())
    }

    /// Process recovery retains uncertain receipts and never replays a possibly submitted input.
    pub(crate) fn recover_inputs(&self) -> Result<(), ErrorCode> {
        self.database
            .lock()
            .map_err(io)?
            .execute(
                "UPDATE input_receipts SET state='uncertain',prompt=NULL WHERE state='claimed'",
                [],
            )
            .map_err(io)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests;
