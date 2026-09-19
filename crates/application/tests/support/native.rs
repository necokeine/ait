//! Stateful native Thread double; every continuation receives only its new input.
#![allow(dead_code, clippy::pedantic)]
use ait_domain::DomainError;
use ait_ports::{
    CodexPreparedThread, CodexThreadConnection, CodexThreadInvocation, CodexThreadSnapshot,
    CodexThreadWriter, WorkspaceOperation, WorkspaceOutputItem, WorkspaceProgressReporter,
};
use async_trait::async_trait;
use serde_json::json;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};

#[derive(Clone, Default)]
pub struct NativeReply {
    pub assistant_text: String,
    pub operations: Vec<WorkspaceOperation>,
    pub output_items: Vec<WorkspaceOutputItem>,
}

#[async_trait]
pub trait NativeHandler: Send + Sync {
    async fn invoke(&self, request: CodexThreadInvocation) -> Result<NativeReply, DomainError>;
    async fn invoke_with_progress(
        &self,
        request: CodexThreadInvocation,
        _progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<NativeReply, DomainError> {
        self.invoke(request).await
    }
}

pub fn native_service(
    workspace: Arc<dyn ait_workspace::ProjectWorkspace>,
    store: Arc<dyn ait_ports::ControlStore>,
    handler: Arc<dyn NativeHandler>,
) -> ait_application::LocalControlService {
    ait_application::LocalControlService::new(workspace, store).with_codex_thread_writer(Arc::new(
        NativeFixture {
            handler,
            histories: Arc::default(),
        },
    ))
}

pub struct NativeFixture {
    pub handler: Arc<dyn NativeHandler>,
    pub histories: Arc<Mutex<HashMap<String, CodexThreadSnapshot>>>,
}
struct Connection {
    request: CodexThreadInvocation,
    prepared: CodexPreparedThread,
    handler: Arc<dyn NativeHandler>,
    histories: Arc<Mutex<HashMap<String, CodexThreadSnapshot>>>,
}
#[async_trait]
impl CodexThreadWriter for NativeFixture {
    async fn open(
        &self,
        request: CodexThreadInvocation,
    ) -> Result<Box<dyn CodexThreadConnection>, DomainError> {
        let id = request
            .thread_id
            .clone()
            .unwrap_or_else(|| format!("native-{}", request.request_id));
        let mut histories = self.histories.lock().unwrap();
        let history = histories.entry(id.clone()).or_insert_with(|| serde_json::from_value(json!({
            "id":id,"sessionId":id,"cwd":request.cwd,"source":"appServer","preview":"",
            "historyMode":"paginated","status":{"type":"idle"},"createdAt":1,"updatedAt":1,"turns":[]
        })).unwrap());
        history.writer_confirmed = true;
        Ok(Box::new(Connection {
            prepared: CodexPreparedThread {
                history: history.clone(),
                model: request.model.clone(),
                reasoning_effort: request.reasoning_effort.clone(),
                model_provider: "openai".into(),
            },
            request,
            handler: self.handler.clone(),
            histories: self.histories.clone(),
        }))
    }
}
#[async_trait]
impl CodexThreadConnection for Connection {
    fn prepared(&self) -> &CodexPreparedThread {
        &self.prepared
    }
    async fn start(
        &mut self,
        progress: Arc<dyn WorkspaceProgressReporter>,
    ) -> Result<CodexThreadSnapshot, DomainError> {
        let result = self
            .handler
            .invoke_with_progress(self.request.clone(), progress)
            .await;
        if result
            .as_ref()
            .is_err_and(|error| error.code == ait_domain::ErrorCode::CodexInputNotAccepted)
        {
            return result.map(|_| unreachable!());
        }
        let (status, output) = match result {
            Ok(output) => ("completed", output),
            Err(error) if error.code == ait_domain::ErrorCode::RunCancelled => {
                ("interrupted", NativeReply::default())
            }
            Err(_) => ("failed", NativeReply::default()),
        };
        let mut histories = self.histories.lock().unwrap();
        let history = histories.get_mut(&self.prepared.history.id).unwrap();
        let turn_id = format!("turn-{}", self.request.request_id);
        let mut items = vec![json!({"id":format!("{turn_id}-user"),"type":"userMessage",
            "clientId":self.request.request_id,"content":[{"type":"text","text":self.request.prompt}]})];
        for operation in output.operations {
            items.push(
                json!({"id": operation.id,"type":"commandExecution","status":operation.status,
                "command":operation.title,"aggregatedOutput":operation.detail}),
            );
        }
        for item in output.output_items {
            if let WorkspaceOutputItem::Message { id, phase, text } = item {
                items.push(json!({"id":id,"type":"agentMessage","phase":phase,"text":text}));
            }
        }
        if !output.assistant_text.is_empty()
            && !items.iter().any(|item| {
                item.get("text").and_then(serde_json::Value::as_str) == Some(&output.assistant_text)
            })
        {
            items.push(json!({"id":format!("{turn_id}-answer"),"type":"agentMessage","text":output.assistant_text}));
        }
        history.turns.push(
            serde_json::from_value(json!({"id":turn_id,"status":status,
            "items":items,"itemsView":"full","startedAt":1,"completedAt":2}))
            .unwrap(),
        );
        Ok(history.clone())
    }
    async fn read(&mut self) -> Result<CodexThreadSnapshot, DomainError> {
        Ok(self.histories.lock().unwrap()[&self.prepared.history.id].clone())
    }
    async fn close(&mut self) {}
}
