//! Provider fixtures regression coverage.
#![allow(clippy::pedantic)]
#![allow(dead_code)]
#![allow(missing_docs)]

use ait_contracts::{AgentConfiguration, AgentProvider, ProviderModel};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{AgentProviderGateway, ProviderMessage};
use async_trait::async_trait;
use std::collections::HashMap;
use std::sync::Mutex;

#[derive(Default)]
pub(crate) struct Gateway {
    pub(crate) omit_models: std::sync::atomic::AtomicBool,
    pub(crate) secrets: Mutex<HashMap<String, String>>,
    pub(crate) calls: Mutex<Vec<(AgentConfiguration, Vec<String>)>>,
    pub(crate) reasoning_efforts: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentProviderGateway for Gateway {
    async fn store_secret(&self, reference: &str, secret: &str) -> Result<(), DomainError> {
        self.secrets
            .lock()
            .unwrap()
            .insert(reference.into(), secret.into());
        Ok(())
    }
    async fn delete_secret(&self, reference: &str) -> Result<(), DomainError> {
        self.secrets.lock().unwrap().remove(reference);
        Ok(())
    }
    async fn list_models(
        &self,
        _: &AgentProvider,
        reference: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        if self.omit_models.load(std::sync::atomic::Ordering::Relaxed) {
            return Ok(Vec::new());
        }
        let reasoning_efforts = self.reasoning_efforts.lock().unwrap().clone();
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: reasoning_efforts.clone(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts,
            },
        ])
    }
    async fn list_models_with_secret(
        &self,
        _: &AgentProvider,
        secret: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        if secret == "invalid-preview-secret" {
            return Err(DomainError::invariant(
                ErrorCode::ProviderFailed,
                "model discovery failed",
            ));
        }
        assert!(!secret.is_empty());
        let reasoning_efforts = self.reasoning_efforts.lock().unwrap().clone();
        Ok(vec![
            ProviderModel {
                id: "chat".into(),
                name: "Chat".into(),
                reasoning_efforts: reasoning_efforts.clone(),
            },
            ProviderModel {
                id: "new".into(),
                name: "New".into(),
                reasoning_efforts,
            },
        ])
    }
    async fn complete(
        &self,
        _: &AgentProvider,
        reference: &str,
        config: &AgentConfiguration,
        messages: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        assert!(self.secrets.lock().unwrap().contains_key(reference));
        self.calls.lock().unwrap().push((
            config.clone(),
            messages.into_iter().map(|m| m.role).collect(),
        ));
        Ok("API response".into())
    }
}

pub(crate) const RETIRED_BUILTINS: [&str; 4] =
    ["tool", "manual", "provider_failure", "approval_required"];
