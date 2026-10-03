//! Agent directory stream bootstrap and latest-state change delivery.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use model::outbound::QueueError;
use model::{Context, ErrorCode, Runtime, ServerMessage};
use serde_json::{Value, json};

use crate::dispatch::State;
use crate::service::agent_execution::AgentExecution;
use crate::service::agent_runtime::AgentRuntimeDirectory;

#[derive(Clone)]
struct Reader {
    runtime: Arc<Runtime>,
    execution: Option<AgentExecution>,
    directory: Option<Arc<Mutex<AgentRuntimeDirectory>>>,
}

impl Reader {
    async fn read(&self, params: Value) -> Result<Value, ErrorCode> {
        if let Some(execution) = &self.execution {
            return execution
                .execute("internal.agent.directory.prepare", params)
                .await
                .map_err(Into::into);
        }
        self.runtime
            .run_queued(
                self.directory.clone(),
                ErrorCode::AgentIo,
                move |directory| {
                    let request =
                        serde_json::from_value(params).map_err(|_| ErrorCode::InvalidMessage)?;
                    crate::rpc::agent_runtime::listing::prepare(directory, request)
                        .and_then(|listing| listing.finish(directory))
                        .map_err(Into::into)
                },
            )
            .await
    }
}

struct Observation {
    id: String,
    params: Value,
    previous: BTreeMap<String, Value>,
}

pub(crate) async fn subscribe(
    mut context: Context<'_>,
    state: &State,
    connection: &mut super::Connection,
) -> Result<(), QueueError> {
    if context.available_subscriptions == 0 {
        return context.respond(Err(ErrorCode::ResourceExhausted));
    }
    let reader = Reader {
        runtime: state.runtime.clone(),
        execution: state.agent_execution.clone(),
        directory: state.agent_runtime.clone(),
    };
    let changes = state
        .directory_changes
        .as_ref()
        .map(model::changes::Changes::subscribe);
    let mut params = std::mem::take(&mut context.request.params);
    let prepared = match reader.read(params.clone()).await {
        Ok(prepared) => prepared,
        Err(error) => return context.respond(Err(error)),
    };
    let (mut response, previous) = match unpack(prepared) {
        Ok(prepared) => prepared,
        Err(error) => return context.respond(Err(error)),
    };
    if response.get("sync").is_some() {
        params["sync"] = checkpoint(&response);
    }
    let id = uuid::Uuid::new_v4().to_string();
    response["subscriptionId"] = json!(id);
    let outbound = context.outbound.clone();
    context.respond(Ok(response))?;
    let observation = Arc::new(tokio::sync::Mutex::new(Observation {
        id: id.clone(),
        params,
        previous,
    }));
    let read = move || {
        let reader = reader.clone();
        let observation = observation.clone();
        async move {
            let mut observation = observation.lock().await;
            let prepared = reader.read(observation.params.clone()).await?;
            observation.update(prepared)
        }
    };
    let subscription = if let Some(changes) = changes {
        model::polling::Subscription::spawn_on_changes(
            state.runtime.clone(),
            outbound,
            changes,
            read,
        )
    } else {
        model::polling::Subscription::spawn(state.runtime.clone(), outbound, read)
    };
    connection.directories.insert(id, subscription);
    Ok(())
}

fn unpack(mut prepared: Value) -> Result<(Value, BTreeMap<String, Value>), ErrorCode> {
    if !prepared["response"]["entries"].is_array() {
        return Err(ErrorCode::AgentIo);
    }
    let Value::Array(entries) = prepared["entries"].take() else {
        return Err(ErrorCode::AgentIo);
    };
    let rows = entries
        .into_iter()
        .map(|entry| {
            let id = entry["agent"]["id"]
                .as_str()
                .ok_or(ErrorCode::AgentIo)?
                .to_owned();
            Ok((id, entry))
        })
        .collect::<Result<_, ErrorCode>>()?;
    Ok((prepared["response"].take(), rows))
}

impl Observation {
    fn update(&mut self, prepared: Value) -> Result<Vec<ServerMessage>, ErrorCode> {
        let (response, next) = unpack(prepared)?;
        let mut updates = Vec::new();
        if response.get("sync").is_some() {
            if response["sync"]["mode"] != "changes" {
                return Err(ErrorCode::AgentIo);
            }
            let entries = response["entries"].as_array().ok_or(ErrorCode::AgentIo)?;
            let removals = response["sync"]["removals"]
                .as_array()
                .ok_or(ErrorCode::AgentIo)?;
            for entry in entries {
                let mut update = upsert(entry);
                update["generation"] = response["sync"]["generation"].clone();
                update["seq"] = entry["syncSeq"].clone();
                updates.push(update);
            }
            for removal in removals {
                updates.push(json!({"kind":"remove","agentId":removal["id"],
                    "generation":response["sync"]["generation"],"seq":removal["seq"]}));
            }
            self.params["sync"] = checkpoint(&response);
            updates.sort_unstable_by_key(|update| update["seq"].as_u64().unwrap_or_default());
        } else {
            for (id, entry) in &next {
                if self.previous.get(id) != Some(entry) {
                    updates.push(upsert(entry));
                }
            }
            for id in self.previous.keys() {
                if !next.contains_key(id) {
                    updates.push(json!({"kind":"remove","agentId":id}));
                }
            }
        }
        self.previous = next;
        Ok(updates
            .into_iter()
            .map(|mut params| {
                params["subscriptionId"] = json!(self.id);
                ServerMessage::Event {
                    method: "agent.update".to_owned(),
                    params,
                }
            })
            .collect())
    }
}

fn upsert(entry: &Value) -> Value {
    json!({"kind":"upsert","agent":entry["agent"],"project":entry["project"]})
}

fn checkpoint(response: &Value) -> Value {
    json!({"generation":response["sync"]["generation"],"afterSeq":response["sync"]["headSeq"]})
}

#[cfg(test)]
mod tests;
