use std::{sync::Arc, time::Duration};

use anyhow::Context;
use serde_json::json;
use server_provider::service::agent_execution::AgentExecution;
use server_voice::{
    Error,
    ports::{Agents, Operation},
    service::Speech,
};
use tokio_util::sync::CancellationToken;

#[derive(Debug)]
pub(super) struct NativeAgents(pub AgentExecution);

pub(super) fn compose(execution: AgentExecution) -> anyhow::Result<Speech> {
    Speech::from_environment(Some(Arc::new(NativeAgents(execution))))
        .context("initialize speech backends")
}

impl Agents for NativeAgents {
    fn resolve<'a>(&'a self, identifier: &'a str) -> Operation<'a, String> {
        Box::pin(async move {
            let result = self
                .0
                .execute("agent.get.request", json!({"agentId":identifier}))
                .await
                .map_err(|_| Error::Agent)?;
            let agent = &result["agent"];
            if !agent["archivedAt"].is_null() || agent["status"] == "archived" {
                return Err(Error::Agent);
            }
            agent["id"].as_str().map(str::to_owned).ok_or(Error::Agent)
        })
    }

    fn turn<'a>(
        &'a self,
        agent: &'a str,
        text: &'a str,
        cancel: CancellationToken,
    ) -> Operation<'a, String> {
        Box::pin(async move {
            if cancel.is_cancelled() {
                return Err(Error::Cancelled);
            }
            // Await the admission receipt even if cancellation arrives during send: only an
            // accepted turn belongs to this voice operation and may be interrupted by it.
            let receipt = self
                .0
                .send_voice(agent, text, cancel.clone())
                .await
                .map_err(|_| Error::Agent)?;
            if receipt["accepted"] != true {
                return Err(Error::Agent);
            }
            let turn = receipt["turnId"].as_str().ok_or(Error::Agent)?;
            let result = tokio::select! { biased;
                () = cancel.cancelled() => Err(Error::Cancelled),
                result = self.wait(agent, turn) => result,
            };
            if matches!(result, Err(Error::Cancelled)) {
                let _ = tokio::time::timeout(Duration::from_secs(4), async {
                    self.0
                        .execute(
                            "internal.voice.cancel",
                            json!({"agentId":agent,"turnId":turn}),
                        )
                        .await
                        .map_err(|_| Error::Agent)?;
                    self.wait(agent, turn).await.map(|_| ())
                })
                .await;
            }
            result
        })
    }
}

impl NativeAgents {
    async fn wait(&self, agent: &str, turn: &str) -> Result<String, Error> {
        loop {
            let result = self
                .0
                .execute(
                    "internal.voice.status",
                    json!({"agentId":agent,"turnId":turn}),
                )
                .await
                .map_err(|_| Error::Agent)?;
            match result["status"].as_str() {
                Some("running") => {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                }
                Some("idle") => {
                    return Ok(result["lastMessage"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned());
                }
                _ => return Err(Error::Agent),
            }
        }
    }
}
