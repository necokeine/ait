//! Durable, leased push subscriptions. Connection lifetime does not revoke a token.

use std::collections::BTreeMap;
use std::fmt;

use chrono::DateTime;
use serde_json::{Value, json};

/// Paseo renews push subscriptions for forty-eight hours.
pub const LEASE_MS: i64 = 48 * 60 * 60 * 1000;
const MAX_TOKENS: usize = 4096;

/// Storage contract and safe persistence errors.
pub use crate::ports::push::{PushError, TokenStore};

/// Process-wide token leases. Debug output deliberately excludes token contents.
pub struct PushTokens {
    store: Box<dyn TokenStore>,
    subscriptions: BTreeMap<String, i64>,
}

impl fmt::Debug for PushTokens {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushTokens")
            .field("count", &self.subscriptions.len())
            .finish_non_exhaustive()
    }
}

impl PushTokens {
    /// Load leases and durably migrate the legacy `tokens` array using the supplied UTC time.
    /// # Errors
    /// Returns a storage, malformed-document or capacity error without disclosing tokens.
    pub fn open(store: Box<dyn TokenStore>, now_ms: i64) -> Result<Self, PushError> {
        let value = store.load()?;
        if !value.is_object() {
            return Err(PushError::Invalid);
        }
        let mut subscriptions = BTreeMap::new();
        if let Some(entries) = value["subscriptions"].as_array() {
            for entry in entries {
                if let (Some(token), Some(expiry)) =
                    (entry["token"].as_str(), entry["expiresAt"].as_str())
                    && let Ok(expiry) = DateTime::parse_from_rfc3339(expiry)
                    && valid_token(token.trim())
                {
                    subscriptions.insert(token.trim().to_owned(), expiry.timestamp_millis());
                }
            }
        }
        let mut migrated = false;
        if let Some(tokens) = value["tokens"].as_array() {
            for token in tokens
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|token| valid_token(token))
            {
                subscriptions.insert(token.to_owned(), now_ms.saturating_add(LEASE_MS));
                migrated = true;
            }
        }
        if subscriptions.len() > MAX_TOKENS {
            return Err(PushError::Capacity);
        }
        let result = Self {
            store,
            subscriptions,
        };
        if migrated {
            result.store.save(&document(&result.subscriptions)?)?;
        }
        Ok(result)
    }

    /// Renew a nonblank token; avoid writes while more than half of its lease remains.
    /// # Errors
    /// Returns invalid-input, capacity or storage errors; failed writes leave memory unchanged.
    pub fn renew(&mut self, token: &str, now_ms: i64) -> Result<(), PushError> {
        let token = token.trim();
        if token.is_empty() {
            return Ok(());
        }
        if !valid_token(token) {
            return Err(PushError::Invalid);
        }
        if self
            .subscriptions
            .get(token)
            .is_some_and(|expiry| expiry.saturating_sub(now_ms) > LEASE_MS / 2)
        {
            return Ok(());
        }
        let mut next = self.subscriptions.clone();
        next.retain(|_, expiry| *expiry > now_ms);
        if next.len() >= MAX_TOKENS && !next.contains_key(token) {
            return Err(PushError::Capacity);
        }
        next.insert(token.to_owned(), now_ms.saturating_add(LEASE_MS));
        self.commit(next)
    }

    /// Revoke a token idempotently, committing before removing it from memory.
    /// # Errors
    /// Returns storage errors without changing the active subscriptions.
    pub fn revoke(&mut self, token: &str) -> Result<(), PushError> {
        let token = token.trim();
        if !self.subscriptions.contains_key(token) {
            return Ok(());
        }
        let mut next = self.subscriptions.clone();
        next.remove(token);
        self.commit(next)
    }

    /// Return unexpired delivery targets; failed pruning never makes expired leases active.
    #[must_use]
    pub fn active(&mut self, now_ms: i64) -> Vec<String> {
        let active: BTreeMap<_, _> = self
            .subscriptions
            .iter()
            .filter(|(_, expiry)| **expiry > now_ms)
            .map(|(token, expiry)| (token.clone(), *expiry))
            .collect();
        let tokens = active.keys().cloned().collect();
        if active.len() != self.subscriptions.len() {
            let _ = self.commit(active);
        }
        tokens
    }

    fn commit(&mut self, next: BTreeMap<String, i64>) -> Result<(), PushError> {
        self.store.save(&document(&next)?)?;
        self.subscriptions = next;
        Ok(())
    }
}

fn valid_token(token: &str) -> bool {
    !token.is_empty() && token.len() <= 4096
}

fn document(subscriptions: &BTreeMap<String, i64>) -> Result<Value, PushError> {
    let entries: Result<Vec<_>, _> = subscriptions.iter().map(|(token, expiry)| {
        let date = DateTime::from_timestamp_millis(*expiry).ok_or(PushError::Invalid)?;
        Ok(json!({"token":token,"expiresAt":date.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)}))
    }).collect();
    Ok(json!({"subscriptions":entries?}))
}

#[cfg(test)]
mod tests;
