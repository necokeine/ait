//! Serialized orchestration skill selection and deletion consent.

use serde_json::{Value, json};
use server_model::ErrorCode;

use crate::ports::skills::{ApplyMode, SkillStore};
use crate::protocol::skills::{self, ImportRequest, Kind, Operation, SaveRequest};

/// Filesystem-owned skill coordinator; the host serializes access under its shared service lock.
#[derive(Debug)]
pub struct Skills {
    store: Box<dyn SkillStore>,
}

impl Skills {
    /// Install a backend. Recovery runs on each admitted operation before any status is exposed.
    #[must_use]
    pub fn new(store: Box<dyn SkillStore>) -> Self {
        Self { store }
    }

    /// Handle the canonical skill operations using fresh scans and explicit deletion consent.
    /// # Errors
    /// Returns validation or safe filesystem errors; partial mutations remain recoverable.
    pub fn execute(&mut self, method: &str, params: Value) -> Result<Value, ErrorCode> {
        self.store.recover()?;
        match method {
            skills::SAVE_SELECTION => self.save(decode(params)?),
            skills::IMPORT_LEGACY_SELECTION => {
                let mut request: ImportRequest = decode(params)?;
                request.selection.normalize();
                if let Some(selection) = self.store.selection()? {
                    return Ok(json!({"imported":false,"selection":selection}));
                }
                self.store.import(&request.selection)?;
                Ok(json!({"imported":true,"selection":request.selection}))
            }
            skills::GET_STATUS | skills::RECONCILE | skills::UNINSTALL => {
                if !params.as_object().is_some_and(serde_json::Map::is_empty) {
                    return Err(ErrorCode::InvalidMessage);
                }
                let selection = self.store.selection()?.unwrap_or_default();
                let status = self.store.scan(&selection)?;
                let (ops, mode): (Vec<Operation>, ApplyMode) = match method {
                    skills::GET_STATUS => return encode(status),
                    skills::UNINSTALL => (
                        status
                            .installed
                            .into_iter()
                            .map(|name| Operation {
                                kind: Kind::Delete,
                                name,
                            })
                            .collect(),
                        ApplyMode::Uninstall,
                    ),
                    _ => (
                        status
                            .ops
                            .into_iter()
                            .filter(|op| op.kind != Kind::Delete)
                            .collect(),
                        ApplyMode::Reconcile,
                    ),
                };
                self.store.apply(&selection, &ops, mode)?;
                encode(self.store.scan(&selection)?)
            }
            _ => Err(ErrorCode::MethodNotFound),
        }
    }

    fn save(&mut self, mut request: SaveRequest) -> Result<Value, ErrorCode> {
        request.selection.normalize();
        let previous = self.store.selection()?.unwrap_or_default();
        let plan = self.store.scan(&request.selection)?;
        let removals: Vec<_> = plan
            .ops
            .iter()
            .filter(|op| op.kind == Kind::Delete)
            .map(|op| op.name.clone())
            .collect();
        let confirmed: std::collections::BTreeSet<_> = request
            .confirmed_removals
            .iter()
            .map(|name| name.trim())
            .collect();
        if removals
            .iter()
            .any(|name| !confirmed.contains(name.as_str()))
        {
            let mut value = encode(self.store.scan(&previous)?)?;
            value["confirmationRequired"] = json!({"removals":removals});
            return Ok(value);
        }
        self.store
            .apply(&request.selection, &plan.ops, ApplyMode::Save)?;
        let mut value = encode(self.store.scan(&request.selection)?)?;
        value["confirmationRequired"] = Value::Null;
        Ok(value)
    }
}

fn decode<T: serde::de::DeserializeOwned>(params: Value) -> Result<T, ErrorCode> {
    serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)
}

fn encode(value: impl serde::Serialize) -> Result<Value, ErrorCode> {
    serde_json::to_value(value).map_err(|_| ErrorCode::RegistryIo)
}

#[cfg(test)]
mod tests;
