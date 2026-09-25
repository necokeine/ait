//! Host selection, tab affinity, bounded pending requests, cancellation and reply ownership.
use crate::protocol::{self, Register};
use serde_json::{Value, json};
use server_model::{ErrorCode, ServerMessage, outbound::Outbound};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::oneshot;

#[derive(Debug, Clone)]
struct Host {
    order: u64,
    commands: BTreeSet<String>,
    outbound: Outbound,
}
#[derive(Debug)]
struct Pending {
    host: String,
    command: String,
    browser_id: Option<String>,
    remember: bool,
    reply: oneshot::Sender<Value>,
}
#[derive(Debug, Default)]
struct State {
    sequence: u64,
    hosts: BTreeMap<String, Host>,
    pending: BTreeMap<String, Pending>,
    affinity: BTreeMap<String, String>,
    stranded: BTreeMap<String, String>,
}
/// Shared broker; host lifetimes are owned by physical WebSocket registrations.
#[derive(Debug, Clone, Default)]
pub struct Broker(Arc<Mutex<State>>);
/// Registration lease. Dropping it removes the host and settles its outstanding requests.
#[derive(Debug)]
pub struct Registration {
    broker: Broker,
    id: String,
}
impl Registration {
    /// Connection-owned subscription identity returned to the client.
    #[must_use]
    pub fn id(&self) -> &str {
        &self.id
    }
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.broker.unregister(&self.id);
    }
}
struct PendingGuard {
    broker: Broker,
    id: String,
}
impl Drop for PendingGuard {
    fn drop(&mut self) {
        if let Ok(mut state) = self.broker.0.lock() {
            state.pending.remove(&self.id);
        }
    }
}
impl Broker {
    /// Register the host's actual supported commands and allocate a releaseable lease.
    /// # Errors
    /// Rejects malformed declarations or exhausted host capacity.
    pub fn register(
        &self,
        request: Register,
        outbound: Outbound,
    ) -> Result<Registration, ErrorCode> {
        if request.host_kind.trim().is_empty()
            || request.host_kind.len() > 128
            || request.supported_commands.is_empty()
            || request.supported_commands.len() > protocol::COMMANDS.len()
            || request
                .supported_commands
                .iter()
                .any(|s| !protocol::COMMANDS.contains(&s.as_str()))
        {
            return Err(ErrorCode::InvalidMessage);
        }
        let mut state = self.0.lock().map_err(|_| ErrorCode::RegistryIo)?;
        if state.hosts.len() >= 128 {
            return Err(ErrorCode::ResourceExhausted);
        }
        state.sequence = state
            .sequence
            .checked_add(1)
            .ok_or(ErrorCode::ResourceExhausted)?;
        let order = state.sequence;
        let id = uuid::Uuid::new_v4().to_string();
        state.hosts.insert(
            id.clone(),
            Host {
                order,
                commands: request.supported_commands.into_iter().collect(),
                outbound,
            },
        );
        Ok(Registration {
            broker: self.clone(),
            id,
        })
    }
    fn unregister(&self, id: &str) {
        let Ok(mut state) = self.0.lock() else {
            return;
        };
        if state.hosts.remove(id).is_none() {
            return;
        }
        let tabs: Vec<_> = state
            .affinity
            .iter()
            .filter(|(_, owner)| *owner == id)
            .map(|(tab, _)| tab.clone())
            .collect();
        for tab in tabs {
            state.affinity.remove(&tab);
            if state.stranded.len() < 4096 {
                state.stranded.insert(tab, id.to_owned());
            }
        }
        let ids: Vec<_> = state
            .pending
            .iter()
            .filter(|(_, p)| p.host == id)
            .map(|(key, _)| key.clone())
            .collect();
        for key in ids {
            if let Some(pending) = state.pending.remove(&key) {
                let _ = pending.reply.send(failure(&key, "browser_no_host", true));
            }
        }
    }
    /// Execute an automation command with the same host routing and list aggregation as Paseo.
    /// The host context may contain agentId, cwd and workspaceId; it cannot select a connection.
    pub async fn execute(&self, command: Value, context: Value, timeout: Duration) -> Value {
        let id = format!("browser_{}", uuid::Uuid::new_v4());
        let Ok(command) = protocol::command(command) else {
            return failure(&id, "browser_unknown_error", false);
        };
        if !context.as_object().is_some_and(|map| {
            map.iter().all(|(key, value)| {
                ["agentId", "cwd", "workspaceId"].contains(&key.as_str())
                    && value.as_str().is_some_and(|s| !s.is_empty())
            })
        }) || timeout.is_zero()
            || timeout > Duration::from_secs(120)
        {
            return failure(&id, "browser_unknown_error", false);
        }
        let hosts = match self.select(&command) {
            Ok(hosts) => hosts,
            Err(code) => return failure(&id, code, code == "browser_no_host"),
        };
        if hosts.iter().any(|(_, host)| {
            !host
                .commands
                .contains(command["command"].as_str().unwrap_or_default())
        }) {
            return failure(&id, "browser_unsupported", false);
        }
        if hosts.len() == 1 {
            return self
                .send(&hosts[0], &id, &command, &context, timeout, true)
                .await;
        }
        let futures = hosts.iter().enumerate().map(|(index, host)| {
            let child_id = format!("{id}:{index}");
            let command = &command;
            let context = &context;
            async move {
                self.send(host, &child_id, command, context, timeout, false)
                    .await
            }
        });
        let replies = futures_util::future::join_all(futures).await;
        if let Some(failed) = replies.iter().find(|r| r["ok"] != true) {
            let mut reply = failed.clone();
            reply["requestId"] = json!(id);
            return reply;
        }
        if replies
            .iter()
            .filter_map(|reply| reply["result"]["tabs"].as_array())
            .map(Vec::len)
            .sum::<usize>()
            > 4096
        {
            return failure(&id, "browser_unknown_error", false);
        }
        let mut tabs = Vec::new();
        if let Ok(mut state) = self.0.lock() {
            for ((host, _), reply) in hosts.iter().zip(&replies) {
                remember(&mut state, host, reply);
                if let Some(values) = reply["result"]["tabs"].as_array() {
                    tabs.extend(values.iter().cloned());
                }
            }
        }
        json!({"requestId":id,"ok":true,"result":{"command":"list_tabs","tabs":tabs}})
    }
    fn select(&self, command: &Value) -> Result<Vec<(String, Host)>, &'static str> {
        let state = self.0.lock().map_err(|_| "browser_unknown_error")?;
        if state.hosts.is_empty() {
            return Err("browser_no_host");
        }
        if command["command"] == "list_tabs" {
            let mut hosts: Vec<_> = state
                .hosts
                .iter()
                .map(|(id, h)| (id.clone(), h.clone()))
                .collect();
            hosts.sort_by_key(|(_, h)| h.order);
            return Ok(hosts);
        }
        if let Some(tab) = command["args"]["browserId"].as_str() {
            if let Some(owner) = state.affinity.get(tab) {
                return state
                    .hosts
                    .get(owner)
                    .map(|h| vec![(owner.clone(), h.clone())])
                    .ok_or("browser_no_host");
            }
            if state.stranded.contains_key(tab) {
                return Err("browser_no_host");
            }
            if state.hosts.len() != 1 {
                return Err("browser_tab_not_found");
            }
        }
        state
            .hosts
            .iter()
            .max_by_key(|(_, host)| host.order)
            .map(|(id, h)| vec![(id.clone(), h.clone())])
            .ok_or("browser_no_host")
    }
    async fn send(
        &self,
        host: &(String, Host),
        id: &str,
        command: &Value,
        context: &Value,
        timeout: Duration,
        remember: bool,
    ) -> Value {
        let (reply, receive) = oneshot::channel();
        {
            let Ok(mut state) = self.0.lock() else {
                return failure(id, "browser_unknown_error", false);
            };
            if !state.hosts.contains_key(&host.0) {
                return failure(id, "browser_no_host", true);
            }
            if state.pending.len() >= 128 {
                return failure(id, "browser_unknown_error", true);
            }
            state.pending.insert(
                id.to_owned(),
                Pending {
                    host: host.0.clone(),
                    command: command["command"].as_str().unwrap_or_default().to_owned(),
                    browser_id: command["args"]["browserId"].as_str().map(str::to_owned),
                    remember,
                    reply,
                },
            );
        }
        let _guard = PendingGuard {
            broker: self.clone(),
            id: id.to_owned(),
        };
        let mut params = context.clone();
        params["requestId"] = json!(id);
        params["subscriptionId"] = json!(host.0);
        params["command"] = command.clone();
        if host
            .1
            .outbound
            .send(&ServerMessage::Event {
                method: "browser.automation.execute.request".into(),
                params,
            })
            .is_err()
        {
            return failure(id, "browser_unknown_error", true);
        }
        match tokio::time::timeout(timeout, receive).await {
            Ok(Ok(reply)) => reply,
            Ok(Err(_)) => failure(id, "browser_no_host", true),
            Err(_) => failure(id, "browser_timeout", true),
        }
    }
    /// Accept a callback only from a registration owned by this physical connection.
    /// Malformed matching responses settle the pending call as a safe browser error.
    #[must_use]
    pub fn receive(&self, id: &str, payload: Value, owners: &BTreeSet<&str>) -> bool {
        let Ok(mut state) = self.0.lock() else {
            return false;
        };
        let Some(pending) = state.pending.get(id) else {
            return false;
        };
        if !owners.contains(pending.host.as_str()) {
            return false;
        }
        let valid = protocol::response(&payload, &pending.command)
            && payload.get("requestId").is_none_or(|value| value == id)
            && (payload["ok"] != true
                || pending
                    .browser_id
                    .as_deref()
                    .is_none_or(|tab| payload["result"]["browserId"] == tab));
        let Some(pending) = state.pending.remove(id) else {
            return false;
        };
        let mut reply = if valid {
            payload
        } else {
            failure(id, "browser_unknown_error", false)
        };
        protocol::defaults(&mut reply);
        reply["requestId"] = json!(id);
        if pending.remember {
            remember(&mut state, &pending.host, &reply);
        }
        let _ = pending.reply.send(reply);
        true
    }
}
fn remember(state: &mut State, host: &str, reply: &Value) {
    if reply["ok"] != true {
        return;
    }
    let result = &reply["result"];
    if result["command"] == "close_tab" {
        if let Some(tab) = result["browserId"].as_str() {
            state.affinity.remove(tab);
            state.stranded.remove(tab);
        }
        return;
    }
    let tabs: Vec<_> = if result["command"] == "list_tabs" {
        result["tabs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|tab| tab["browserId"].as_str())
            .collect()
    } else {
        result["browserId"].as_str().into_iter().collect()
    };
    for tab in tabs {
        if state.affinity.len() < 4096 || state.affinity.contains_key(tab) {
            state.affinity.insert(tab.to_owned(), host.to_owned());
            state.stranded.remove(tab);
        }
    }
}
fn failure(id: &str, code: &str, retryable: bool) -> Value {
    json!({"requestId":id,"ok":false,"error":{"code":code,"message":match code{"browser_no_host"=>"No connected browser host owns this request","browser_tab_not_found"=>"List tabs to select a connected browser tab","browser_unsupported"=>"Browser host does not support this command","browser_timeout"=>"Browser automation timed out",_=>"Browser automation failed"},"retryable":retryable}})
}
#[cfg(test)]
mod tests;
