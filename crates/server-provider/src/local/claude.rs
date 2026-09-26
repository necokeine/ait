//! Native Claude Code sessions over the CLI's Agent SDK streaming control protocol.

mod config;
mod history;
mod inputs;
mod inspection;
mod permissions;
mod rewind;
mod session;
mod streaming;
mod subagents;
mod tasks;
mod transport;

use std::path::PathBuf;
use std::time::Duration;

use serde_json::{Value, json};
use server_domain::agent_runtime::{AgentPersistenceHandle, StoredAgentConfig};

use crate::ports::agent_session::{
    AgentClient, AgentResumePurpose, AgentSession, AgentSessionError, AgentSessionFuture,
    AgentSessionSpec,
};
use crate::ports::native_history::{ListOptions, SessionDescriptor, SessionHistory};
use crate::protocol::{provider::Details, timeline::NativeItem};

/// Local Claude Code executable; credentials and native tools remain owned by Claude Code.
#[derive(Debug, Clone)]
pub struct ClaudeClient {
    program: PathBuf,
    config_dir: Option<PathBuf>,
    deadline: Duration,
    images: super::images::ImageStore,
    resolved_models: std::sync::Arc<std::sync::RwLock<std::collections::BTreeMap<String, String>>>,
}

impl ClaudeClient {
    /// Use `program` without a shell and inherit Claude Code's configuration and authentication.
    /// Control requests time out after thirty seconds; foreground turns have no time limit.
    #[must_use]
    pub fn new(program: PathBuf) -> Self {
        Self {
            program,
            config_dir: std::env::var_os("CLAUDE_CONFIG_DIR").map(PathBuf::from),
            deadline: Duration::from_secs(30),
            images: super::images::ImageStore::default(),
            resolved_models: std::sync::Arc::default(),
        }
    }

    /// Store decoded native image output in a private, persistent directory.
    #[must_use]
    pub fn with_image_directory(mut self, directory: PathBuf) -> Self {
        self.images = super::images::ImageStore::new(directory);
        self
    }

    async fn probe(&self, spec: &AgentSessionSpec) -> Result<Value, AgentSessionError> {
        config::validate_spec(spec)?;
        let mut transport = transport::Transport::spawn(self, spec, None, None)?;
        let result = transport.initialize().await;
        transport.close().await?;
        if let Ok(response) = &result {
            self.remember_models(response)?;
        }
        result
    }

    fn remember_models(&self, response: &Value) -> Result<(), AgentSessionError> {
        config::models(response)?;
        let mut models = self
            .resolved_models
            .write()
            .map_err(|_| AgentSessionError::Failed)?;
        models.clear();
        for model in response["models"].as_array().into_iter().flatten() {
            if let (Some(alias), Some(resolved)) = (
                model["value"].as_str(),
                model["resolvedModel"]
                    .as_str()
                    .filter(|value| value.len() <= 256),
            ) {
                models.insert(alias.to_owned(), resolved.to_owned());
            }
        }
        Ok(())
    }

    fn resolved_config(&self, config: &StoredAgentConfig) -> StoredAgentConfig {
        let mut resolved = config.clone();
        if let Some(model) = self.resolved_models.read().ok().and_then(|models| {
            models
                .get(config.model.as_deref().unwrap_or("default"))
                .cloned()
        }) {
            resolved.model = Some(model);
        }
        resolved
    }
}

impl AgentClient for ClaudeClient {
    fn provider(&self) -> &'static str {
        "claude"
    }

    fn validate_config(&self, config: &StoredAgentConfig) -> Result<(), AgentSessionError> {
        config::validate(&self.resolved_config(config))
    }

    fn validate_selection<'a>(&'a self, spec: &'a AgentSessionSpec) -> AgentSessionFuture<'a, ()> {
        Box::pin(async move {
            config::validate_spec(spec)?;
            if self.validate_config(&spec.config).is_err() {
                self.probe(&AgentSessionSpec {
                    config: StoredAgentConfig::default(),
                    ..spec.clone()
                })
                .await?;
            }
            self.validate_config(&spec.config)
        })
    }

    fn settings(&self, config: &StoredAgentConfig) -> Value {
        json!({"availableModes":config::modes(),"features":config::features(&self.resolved_config(config)),"capabilities":{
            "supportsMcpServers":true,
            "supportsDynamicModes":true,"supportsRewindConversation":true,"supportsRewindFiles":true,"supportsRewindBoth":true,
            "supportsStreaming":true,"supportsReasoningStream":true,"supportsSessionListing":true}})
    }

    fn is_available(&self) -> AgentSessionFuture<'_, bool> {
        Box::pin(async { Ok(config::executable(&self.program)) })
    }

    fn diagnostic(&self) -> AgentSessionFuture<'_, String> {
        Box::pin(self.native_diagnostic())
    }

    fn usage(&self) -> AgentSessionFuture<'_, Value> {
        Box::pin(self.native_usage())
    }

    fn discover<'a>(&'a self, cwd: &'a str) -> AgentSessionFuture<'a, Details> {
        Box::pin(async move {
            let response = self
                .probe(&AgentSessionSpec {
                    provider: "claude".to_owned(),
                    cwd: cwd.to_owned(),
                    config: StoredAgentConfig::default(),
                })
                .await?;
            Ok(Details {
                models: config::models(&response)?,
                modes: config::modes(),
                features: config::features(&self.resolved_config(&StoredAgentConfig::default())),
            })
        })
    }

    fn commands<'a>(&'a self, spec: &'a AgentSessionSpec) -> AgentSessionFuture<'a, Vec<Value>> {
        Box::pin(async move {
            let response = self.probe(spec).await?;
            config::commands(&response)
        })
    }

    fn create_session<'a>(
        &'a self,
        spec: &'a AgentSessionSpec,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(session::open(self, spec, None))
    }

    fn resume_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        purpose: AgentResumePurpose,
    ) -> AgentSessionFuture<'a, Box<dyn AgentSession>> {
        Box::pin(session::open(self, spec, Some((handle, purpose))))
    }

    fn history<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<NativeItem>> {
        Box::pin(async move {
            Ok(history::read(self, handle, cwd)?
                .ok_or(AgentSessionError::Unavailable)?
                .entries)
        })
    }

    fn inspect_session<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, SessionHistory> {
        Box::pin(async move {
            if handle
                .metadata
                .as_ref()
                .is_some_and(|metadata| metadata.contains_key(subagents::LOCATOR))
            {
                return subagents::read(self, handle, cwd);
            }
            history::read(self, handle, cwd)?.ok_or(AgentSessionError::Unavailable)
        })
    }

    fn subagents<'a>(
        &'a self,
        cwd: &'a str,
    ) -> AgentSessionFuture<'a, Vec<crate::ports::controls::NativeSubagent>> {
        Box::pin(async move { subagents::list(self, cwd) })
    }

    fn rewind<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        message: &'a str,
    ) -> AgentSessionFuture<'a, SessionHistory> {
        Box::pin(async move { rewind::conversation(self, handle, spec, message) })
    }

    fn rewind_files<'a>(
        &'a self,
        handle: &'a AgentPersistenceHandle,
        spec: &'a AgentSessionSpec,
        message: &'a str,
    ) -> AgentSessionFuture<'a, ()> {
        Box::pin(rewind::files(self, handle, spec, message))
    }

    fn list_sessions<'a>(
        &'a self,
        options: &'a ListOptions,
    ) -> AgentSessionFuture<'a, Vec<SessionDescriptor>> {
        Box::pin(async move { history::list(self, options) })
    }
}

#[cfg(test)]
mod tests;
