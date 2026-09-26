//! Session-scoped task announcements and explicitly attributed sidechain streams.

use std::collections::BTreeMap;

use serde_json::{Value, json};

use crate::local::claude::streaming;
use crate::local::images::ImageStore;
use crate::ports::agent_session::{AgentSessionError, AgentTurnEvent};
use crate::ports::controls::{NativeSubagent, SubagentEvent};

#[derive(Debug)]
struct Child {
    descriptor: NativeSubagent,
    stream: streaming::Stream,
    background: bool,
    announced: bool,
    workflow: bool,
}

#[derive(Debug)]
pub(in crate::local::claude) struct Live {
    images: ImageStore,
    children: BTreeMap<String, Child>,
    tasks: BTreeMap<String, String>,
    aliases: BTreeMap<String, String>,
    declarations: BTreeMap<String, (String, Value)>,
    announces_tasks: bool,
}

impl Live {
    pub(in crate::local::claude) fn restore(
        &mut self,
        client: &super::ClaudeClient,
        root: &str,
        cwd: &str,
    ) -> Result<(), AgentSessionError> {
        let directory = super::history::project_dir(client, cwd)?;
        if !directory.join(root).exists() {
            return Ok(());
        }
        let children = super::tree(client, cwd, root)?;
        for record in super::history::read_active_records(&directory.join(format!("{root}.jsonl")))
            .map_err(|_| AgentSessionError::Failed)?
        {
            if record["isSidechain"] != true {
                self.declare(&record, root)?;
            }
        }
        for (descriptor, _) in children {
            if let Some(run) = descriptor
                .persistence
                .as_ref()
                .and_then(|handle| handle.metadata.as_ref())
                .and_then(|metadata| metadata.get(super::LOCATOR))
                .and_then(|locator| locator["workflowRun"].as_str())
            {
                self.tasks.insert(run.to_owned(), descriptor.id.clone());
                self.aliases
                    .insert(descriptor.id.clone(), descriptor.id.clone());
                self.children.insert(
                    descriptor.id.clone(),
                    Child {
                        descriptor,
                        stream: streaming::Stream::new(self.images.clone()),
                        background: false,
                        announced: true,
                        workflow: true,
                    },
                );
                continue;
            }
            let native = descriptor
                .persistence
                .as_ref()
                .and_then(|handle| handle.metadata.as_ref())
                .and_then(|metadata| metadata.get(super::LOCATOR))
                .and_then(|locator| locator["nativeId"].as_str())
                .ok_or(AgentSessionError::Failed)?;
            self.tasks.insert(native.to_owned(), descriptor.id.clone());
            self.aliases
                .insert(descriptor.id.clone(), descriptor.id.clone());
            let mut stream = streaming::Stream::new(self.images.clone());
            for mut record in super::history::read_active_records(
                &directory
                    .join(root)
                    .join("subagents")
                    .join(format!("agent-{native}.jsonl")),
            )
            .map_err(|_| AgentSessionError::Failed)?
            {
                record["isSidechain"] = json!(false);
                record["parent_tool_use_id"] = Value::Null;
                if record["type"] == "assistant" && record["message"]["id"].is_null() {
                    record["message"]["id"] = record["uuid"].clone();
                }
                self.declare(&record, &descriptor.id)?;
                stream.record(&record)?;
                stream.events.clear();
            }
            self.children.insert(
                descriptor.id.clone(),
                Child {
                    descriptor,
                    stream,
                    background: false,
                    announced: true,
                    workflow: false,
                },
            );
        }
        Ok(())
    }

    pub(in crate::local::claude) fn new(images: ImageStore) -> Self {
        Self {
            images,
            children: BTreeMap::new(),
            tasks: BTreeMap::new(),
            aliases: BTreeMap::new(),
            declarations: BTreeMap::new(),
            announces_tasks: false,
        }
    }

    pub(in crate::local::claude) fn children(&self) -> Vec<NativeSubagent> {
        self.children
            .values()
            .map(|child| child.descriptor.clone())
            .collect()
    }

    pub(in crate::local::claude) fn observe(
        &mut self,
        record: &Value,
        root: &str,
        cwd: &str,
    ) -> Result<Vec<AgentTurnEvent>, AgentSessionError> {
        let mut events = Vec::new();
        if record["type"] == "system" {
            self.task(record, root, cwd, &mut events)?;
        }
        let parent = record["parent_tool_use_id"].as_str();
        let owner = parent.and_then(|parent| self.aliases.get(parent).cloned());
        if record["type"] == "assistant" && (parent.is_none() || owner.is_some()) {
            self.declare(record, owner.as_deref().unwrap_or(root))?;
        }
        if let Some(parent) = parent {
            if owner.is_none() && !self.announces_tasks && self.declarations.contains_key(parent) {
                self.start(
                    &json!({"tool_use_id":parent,"task_id":parent}),
                    (root, cwd, false),
                    &mut events,
                )?;
            }
            if let Some(id) = self.aliases.get(parent).cloned() {
                if record["type"] == "assistant" && owner.is_none() {
                    self.declare(record, &id)?;
                }
                self.sidechain(&id, record, &mut events)?;
            }
        }
        if record["type"] == "user" {
            for block in record["message"]["content"]
                .as_array()
                .into_iter()
                .flatten()
            {
                if block["type"] != "tool_result" {
                    continue;
                }
                let Some(id) = block["tool_use_id"]
                    .as_str()
                    .and_then(|id| self.aliases.get(id))
                    .cloned()
                else {
                    continue;
                };
                if let Some(child) = self.children.get_mut(&id)
                    && !child.announced
                {
                    set_status(
                        child,
                        if block["is_error"] == true {
                            "failed"
                        } else {
                            "completed"
                        },
                        &mut events,
                    );
                }
            }
        }
        Ok(events)
    }

    fn declare(&mut self, record: &Value, owner: &str) -> Result<(), AgentSessionError> {
        for block in record["message"]["content"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if block["type"] != "tool_use"
                || !matches!(block["name"].as_str(), Some("Task" | "Agent" | "Workflow"))
            {
                continue;
            }
            let id = text(block, "id")?;
            if self.declarations.len() >= 2048 && !self.declarations.contains_key(id) {
                return Err(AgentSessionError::Failed);
            }
            if let Some((previous, input)) = self.declarations.get(id)
                && (previous != owner || input != &block["input"])
            {
                return Err(AgentSessionError::Failed);
            }
            self.declarations
                .insert(id.to_owned(), (owner.to_owned(), block["input"].clone()));
        }
        Ok(())
    }

    fn task(
        &mut self,
        record: &Value,
        root: &str,
        cwd: &str,
        events: &mut Vec<AgentTurnEvent>,
    ) -> Result<(), AgentSessionError> {
        if record["subtype"] == "task_started" {
            self.announces_tasks = true;
            if record["skip_transcript"] == true
                || !(matches!(
                    record["task_type"].as_str(),
                    Some("local_agent" | "local_workflow")
                ) || record["task_type"].is_null() && record["subagent_type"].is_string())
            {
                return Ok(());
            }
            return self.start(record, (root, cwd, true), events);
        }
        if !matches!(
            record["subtype"].as_str(),
            Some("task_updated" | "task_notification" | "task_progress")
        ) {
            return Ok(());
        }
        let Some(id) = record["task_id"].as_str().and_then(|id| self.tasks.get(id)) else {
            return Ok(());
        };
        let child = self.children.get_mut(id).ok_or(AgentSessionError::Failed)?;
        if let Some(background) = record["patch"]["is_backgrounded"].as_bool() {
            child.background = background;
        }
        if let Some(status) = status(
            record["patch"]["status"]
                .as_str()
                .or_else(|| record["status"].as_str()),
        ) {
            set_status(child, status, events);
        }
        if let Some(tokens) = record["usage"]["total_tokens"].as_u64() {
            let subtitle = format!("{tokens} tokens");
            if child.descriptor.descriptor["subtitle"] != subtitle {
                child.descriptor.descriptor["subtitle"] = json!(subtitle);
                events.push(upsert(child));
            }
        }
        if child.workflow
            && record["subtype"] == "task_notification"
            && let Some(text) = record["output_file"]
                .as_str()
                .and_then(super::workflow::read)
        {
            use sha2::{Digest, Sha256};
            let id = format!("workflow-result:{:x}", Sha256::digest(text.as_bytes()));
            let result = json!({"type":"assistant","uuid":id,"message":{"id":id,"content":[{"type":"text","text":text}]}});
            child.stream.record(&result)?;
            drain(&child.descriptor.id, &mut child.stream, events);
        }
        Ok(())
    }

    fn start(
        &mut self,
        record: &Value,
        context: (&str, &str, bool),
        events: &mut Vec<AgentTurnEvent>,
    ) -> Result<(), AgentSessionError> {
        let (root, cwd, announced) = context;
        let call = text(record, "tool_use_id")?;
        let task = text(record, "task_id")?;
        let id = self
            .tasks
            .get(task)
            .cloned()
            .unwrap_or_else(|| call.to_owned());
        if self.tasks.len() >= 1024 || self.aliases.len() >= 2048 {
            return Err(AgentSessionError::Failed);
        }
        self.tasks.insert(task.to_owned(), id.clone());
        self.aliases.insert(call.to_owned(), id.clone());
        if let Some(child) = self.children.get_mut(&id) {
            child.announced |= announced;
            set_status(child, "running", events);
            return Ok(());
        }
        if self.children.len() >= 256 {
            return Err(AgentSessionError::Failed);
        }
        let (parent, input) = self
            .declarations
            .get(call)
            .map_or((root, &Value::Null), |(parent, input)| {
                (parent.as_str(), input)
            });
        let now = chrono::Utc::now().to_rfc3339();
        let title = input["name"]
            .as_str()
            .or_else(|| record["subagent_type"].as_str())
            .or_else(|| input["subagent_type"].as_str())
            .or_else(|| (record["task_type"] == "local_workflow").then_some("Workflow"));
        let description = record["description"]
            .as_str()
            .or_else(|| input["description"].as_str());
        let handle = server_domain::agent_runtime::AgentPersistenceHandle {
            provider: "claude".to_owned(),
            session_id: id.clone(),
            native_handle: None,
            metadata: Some(BTreeMap::from([(
                super::LOCATOR.to_owned(),
                json!({"root":root,"cwd":cwd}),
            )])),
        };
        let mut child = Child {
            descriptor: NativeSubagent {
                persistence: Some(handle),
                id: id.clone(),
                parent_id: parent.to_owned(),
                cwd: cwd.to_owned(),
                descriptor: json!({"id":id,"provider":"claude","title":title,"description":description,"status":"running",
                "createdAt":now,"updatedAt":now,"toolCallId":id,"cwd":cwd,"subtitle":title}),
            },
            stream: streaming::Stream::new(self.images.clone()),
            background: false,
            announced,
            workflow: record["task_type"] == "local_workflow",
        };
        events.push(upsert(&child));
        let prompt = if record["task_type"] == "local_workflow" {
            description
        } else {
            record["prompt"]
                .as_str()
                .or_else(|| input["prompt"].as_str())
        };
        if let Some(prompt) = prompt {
            child.stream.record(&json!({"type":"user","uuid":format!("task:{task}:prompt"),"message":{"content":prompt}}))?;
            drain(&id, &mut child.stream, events);
        }
        self.children.insert(id, child);
        Ok(())
    }

    fn sidechain(
        &mut self,
        id: &str,
        record: &Value,
        events: &mut Vec<AgentTurnEvent>,
    ) -> Result<(), AgentSessionError> {
        let child = self.children.get_mut(id).ok_or(AgentSessionError::Failed)?;
        let mut record = record.clone();
        record["parent_tool_use_id"] = Value::Null;
        record["isSidechain"] = json!(false);
        if record["type"] == "assistant" {
            if record["message"]["id"].is_null() {
                record["message"]["id"] = record["uuid"].clone();
            }
            if let Some(model) = record["message"]["model"].as_str()
                && child.descriptor.descriptor["subtitle"] != model
            {
                child.descriptor.descriptor["subtitle"] = json!(model);
                events.push(upsert(child));
            }
        }
        child.stream.record(&record)?;
        drain(id, &mut child.stream, events);
        Ok(())
    }

    pub(in crate::local::claude) fn stopped(
        &mut self,
        failed: bool,
        all: bool,
    ) -> Vec<AgentTurnEvent> {
        let mut events = Vec::new();
        for child in self.children.values_mut() {
            if child.descriptor.descriptor["status"] == "running" && (all || !child.background) {
                set_status(
                    child,
                    if failed { "failed" } else { "canceled" },
                    &mut events,
                );
            }
        }
        events
    }
}

fn drain(id: &str, stream: &mut streaming::Stream, events: &mut Vec<AgentTurnEvent>) {
    while let Some(event) = stream.events.pop_front() {
        let child = match event {
            AgentTurnEvent::Progress { observation, entry } => SubagentEvent::Progress {
                id: id.to_owned(),
                observation,
                entry,
            },
            AgentTurnEvent::Timeline(entry) => SubagentEvent::Timeline {
                id: id.to_owned(),
                entry,
            },
            _ => continue,
        };
        events.push(AgentTurnEvent::Subagent(child));
    }
}

fn upsert(child: &Child) -> AgentTurnEvent {
    AgentTurnEvent::Subagent(SubagentEvent::Upsert(child.descriptor.clone()))
}

fn set_status(child: &mut Child, status: &str, events: &mut Vec<AgentTurnEvent>) {
    if child.descriptor.descriptor["status"] == status {
        return;
    }
    child.descriptor.descriptor["status"] = json!(status);
    child.descriptor.descriptor["updatedAt"] = json!(chrono::Utc::now().to_rfc3339());
    events.push(upsert(child));
}

fn status(status: Option<&str>) -> Option<&'static str> {
    match status {
        Some("pending" | "running" | "paused") => Some("running"),
        Some("completed") => Some("completed"),
        Some("failed") => Some("failed"),
        Some("killed" | "stopped") => Some("canceled"),
        _ => None,
    }
}

fn text<'a>(value: &'a Value, key: &str) -> Result<&'a str, AgentSessionError> {
    value[key]
        .as_str()
        .filter(|value| !value.is_empty() && value.len() <= 256 && !value.contains('\0'))
        .ok_or(AgentSessionError::Failed)
}

#[cfg(test)]
mod tests;
