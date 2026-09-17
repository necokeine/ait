//! Bounded tools whose execution needs the current Agent or a member response.

use crate::host::{MAX_BYTES, parameters};
use ait_domain::{
    DomainError, DomainMetadata, ErrorCode, Message, MessageId, MessageKind, MessageOrigin,
    MessageRole, ProjectId, ProjectedMessage, RunId, RunUsage, SubMessage, TimestampMs,
    ToolExecution, ToolExecutionId, ToolResult, ToolResultStatus, ToolUse,
};
use ait_ports::{
    AgentInvocation, RunAgent, RunTool, RunToolInteraction, ToolInvocation, ToolOutcome,
    ToolRecovery,
};
use async_trait::async_trait;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::sync::Arc;

const MAX_CHILD_ROUNDS: u32 = 8;
const MAX_CHILD_TOOL_CALLS: u32 = 16;

fn add_usage(total: &mut RunUsage, delta: &RunUsage) {
    total.input_tokens = total.input_tokens.saturating_add(delta.input_tokens);
    total.cached_input_tokens = total
        .cached_input_tokens
        .saturating_add(delta.cached_input_tokens);
    total.output_tokens = total.output_tokens.saturating_add(delta.output_tokens);
    total.tool_executions = total.tool_executions.saturating_add(delta.tool_executions);
    total.cost = match (total.cost, delta.cost) {
        (None, None) => None,
        (left, right) => Some(ait_domain::CostMicros(
            left.map_or(0, ait_domain::CostMicros::get)
                .saturating_add(right.map_or(0, ait_domain::CostMicros::get)),
        )),
    };
}

fn failed(message: &'static str) -> DomainError {
    DomainError::invariant(ErrorCode::ToolExecutionFailed, message)
}

fn message_id(seed: &str, sequence: u32) -> MessageId {
    let digest = Sha256::digest(format!("{seed}:{sequence}"));
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    let value = u128::from_be_bytes(bytes).max(1);
    MessageId::from_u128(value)
}

fn timestamp() -> TimestampMs {
    let millis = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    TimestampMs(i64::try_from(millis).unwrap_or(i64::MAX))
}

fn project(path: &[ProjectedMessage]) -> ProjectId {
    path.iter()
        .rev()
        .find_map(|entry| match entry {
            ProjectedMessage::Visible(message) => Some(message.project_id.clone()),
            ProjectedMessage::Redacted { .. } => None,
        })
        .unwrap_or_else(|| ProjectId::new("subagent"))
}

fn parent(path: &[ProjectedMessage]) -> Option<MessageId> {
    path.iter().rev().find_map(|entry| match entry {
        ProjectedMessage::Visible(message) => Some(message.id),
        ProjectedMessage::Redacted { .. } => None,
    })
}

fn child_path(request: &mut ToolInvocation) -> Vec<ProjectedMessage> {
    if request.tool_name != "subagent_fork" {
        return Vec::new();
    }
    let mut inherited = std::mem::take(&mut request.message_path);
    if inherited.last().is_some_and(|entry| {
        matches!(entry, ProjectedMessage::Visible(message) if message.role == MessageRole::Assistant)
    }) {
        inherited.pop();
    }
    inherited
}

fn child_prompt(request: &ToolInvocation) -> Result<String, DomainError> {
    if request
        .arguments
        .get("run_in_background")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(failed(
            "background subagents are not supported by this host",
        ));
    }
    request
        .arguments
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= MAX_BYTES)
        .map(str::to_owned)
        .ok_or_else(|| failed("subagent prompt is invalid"))
}

fn user_message(
    seed: &str,
    sequence: u32,
    project_id: ProjectId,
    parent_message_id: Option<MessageId>,
    run_id: &RunId,
    text: String,
) -> Message {
    Message {
        id: message_id(seed, sequence),
        project_id,
        parent_message_id,
        role: MessageRole::User,
        kind: MessageKind::Standard,
        origin: MessageOrigin::System,
        sub_messages: vec![SubMessage::Text { text }],
        created_by_session_id: None,
        run_id: Some(run_id.clone()),
        run_seq: Some(sequence.into()),
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: timestamp(),
    }
}

fn assistant_message(
    seed: &str,
    sequence: u32,
    project_id: ProjectId,
    parent_message_id: Option<MessageId>,
    run_id: &RunId,
    sub_messages: Vec<SubMessage>,
) -> Message {
    Message {
        id: message_id(seed, sequence),
        project_id,
        parent_message_id,
        role: MessageRole::Assistant,
        kind: MessageKind::Standard,
        origin: MessageOrigin::Agent,
        sub_messages,
        created_by_session_id: None,
        run_id: Some(run_id.clone()),
        run_seq: Some(sequence.into()),
        tool_result: None,
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: timestamp(),
    }
}

fn tool_result_message(
    seed: &str,
    sequence: u32,
    project_id: ProjectId,
    parent_message_id: Option<MessageId>,
    run_id: &RunId,
    call_id: String,
    result: Result<ToolOutcome, DomainError>,
) -> Message {
    let (status, output, error) = match result {
        Ok(outcome) => (
            ToolResultStatus::Succeeded,
            serde_json::to_string(&outcome.output).ok(),
            None,
        ),
        Err(failure) => (ToolResultStatus::Failed, None, Some(failure.message)),
    };
    Message {
        id: message_id(seed, sequence),
        project_id,
        parent_message_id,
        role: MessageRole::User,
        kind: MessageKind::ToolResult,
        origin: MessageOrigin::Tool,
        sub_messages: Vec::new(),
        created_by_session_id: None,
        run_id: Some(run_id.clone()),
        run_seq: Some(sequence.into()),
        tool_result: Some(ToolResult {
            call_id,
            status,
            output,
            error,
        }),
        git_commit: None,
        metadata: DomainMetadata::default(),
        created_at: timestamp(),
    }
}

/// Implements member interaction and one-level foreground child delegation.
pub struct AgentTools {
    child_agent: Arc<dyn RunAgent>,
    child_tools: Arc<dyn RunTool>,
    interactions: Arc<dyn RunToolInteraction>,
}

struct ChildState {
    run: RunId,
    project: ProjectId,
    sequence: u32,
    path: Vec<ProjectedMessage>,
    usage: RunUsage,
    tool_calls: u32,
}

impl AgentTools {
    /// Creates the extension. `child_agent` should advertise only
    /// `child_tools`; this deliberately prevents recursive delegation.
    #[must_use]
    pub fn new(
        child_agent: Arc<dyn RunAgent>,
        child_tools: Arc<dyn RunTool>,
        interactions: Arc<dyn RunToolInteraction>,
    ) -> Self {
        Self {
            child_agent,
            child_tools,
            interactions,
        }
    }

    async fn execute_child_call(
        &self,
        request: &ToolInvocation,
        state: &mut ChildState,
        call: ToolUse,
    ) -> Result<(), DomainError> {
        state.sequence = state.sequence.saturating_add(1);
        let arguments = serde_json::from_str(&call.arguments)
            .map_err(|_| failed("subagent returned invalid tool arguments"))?;
        let result = if self
            .child_tools
            .executable_tools()
            .contains(&call.tool_name)
        {
            self.child_tools
                .execute(ToolInvocation {
                    run_id: state.run.clone(),
                    call_id: call.call_id.clone(),
                    execution_id: ToolExecutionId::new(format!(
                        "{}:tool:{}",
                        request.execution_id.as_str(),
                        state.sequence
                    )),
                    tool_name: call.tool_name.clone(),
                    arguments,
                    message_path: state.path.clone(),
                    cancellation: request.cancellation.clone(),
                })
                .await
        } else {
            Err(failed("subagent requested an unavailable tool"))
        };
        state.usage.tool_executions = state.usage.tool_executions.saturating_add(1);
        if let Ok(outcome) = &result {
            add_usage(&mut state.usage, &outcome.usage);
        }
        let message = tool_result_message(
            state.run.as_str(),
            state.sequence,
            state.project.clone(),
            parent(&state.path),
            &state.run,
            call.call_id,
            result,
        );
        state.path.push(ProjectedMessage::Visible(message));
        Ok(())
    }

    async fn subagent(&self, mut request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        let prompt = child_prompt(&request)?;
        let child_run = RunId::new(format!(
            "{}:subagent:{}",
            request.run_id.as_str(),
            request.execution_id.as_str()
        ));
        let mut path = child_path(&mut request);
        let project_id = project(&path);
        let initial = user_message(
            child_run.as_str(),
            1,
            project_id.clone(),
            parent(&path),
            &child_run,
            prompt,
        );
        path.push(ProjectedMessage::Visible(initial));
        let mut state = ChildState {
            run: child_run,
            project: project_id,
            sequence: 1,
            path,
            usage: RunUsage::default(),
            tool_calls: 0,
        };
        for round in 1..=MAX_CHILD_ROUNDS {
            if request.cancellation.is_cancelled() {
                return Err(DomainError::invariant(
                    ErrorCode::RunCancelled,
                    "subagent cancelled",
                ));
            }
            let response = self
                .child_agent
                .invoke(AgentInvocation {
                    attempt_id: ait_domain::RunAttemptId::new(format!(
                        "{}:attempt:{round}",
                        state.run.as_str()
                    )),
                    run_id: state.run.clone(),
                    agent_revision: 1,
                    message_path: state.path.clone(),
                    cancellation: request.cancellation.clone(),
                })
                .await?;
            add_usage(&mut state.usage, &response.usage);
            state.sequence = state.sequence.saturating_add(1);
            let assistant = assistant_message(
                state.run.as_str(),
                state.sequence,
                state.project.clone(),
                parent(&state.path),
                &state.run,
                response.sub_messages,
            );
            let calls: Vec<_> = assistant
                .sub_messages
                .iter()
                .filter_map(|part| match part {
                    SubMessage::ToolUse(call) => Some(call.clone()),
                    _ => None,
                })
                .collect();
            let final_text: String = assistant
                .sub_messages
                .iter()
                .filter_map(|part| match part {
                    SubMessage::Text { text } => Some(text.as_str()),
                    _ => None,
                })
                .collect::<Vec<_>>()
                .join("");
            state.path.push(ProjectedMessage::Visible(assistant));
            if calls.is_empty() {
                if final_text.trim().is_empty() || final_text.len() > MAX_BYTES {
                    return Err(failed("subagent returned an invalid final response"));
                }
                return Ok(ToolOutcome {
                    output: json!({"content": final_text, "rounds": round}),
                    usage: state.usage,
                });
            }
            state.tool_calls = state
                .tool_calls
                .saturating_add(u32::try_from(calls.len()).unwrap_or(u32::MAX));
            if state.tool_calls > MAX_CHILD_TOOL_CALLS {
                return Err(failed("subagent exceeded its tool-call limit"));
            }
            for call in calls {
                self.execute_child_call(&request, &mut state, call).await?;
            }
        }
        Err(failed("subagent exceeded its round limit"))
    }
}

#[async_trait]
impl RunTool for AgentTools {
    fn executable_tools(&self) -> Vec<String> {
        [
            "ask_user_question",
            "exit_plan_mode",
            "subagent",
            "subagent_fork",
        ]
        .into_iter()
        .map(str::to_owned)
        .collect()
    }

    fn requires_approval(&self, _: &str, _: &Value) -> bool {
        false
    }

    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        let schema = parameters(&request.tool_name).ok_or_else(|| failed("tool is unavailable"))?;
        if request.arguments.to_string().len() > MAX_BYTES
            || !jsonschema::validator_for(&schema)
                .map_err(|_| failed("tool schema is invalid"))?
                .is_valid(&request.arguments)
        {
            return Err(failed("tool arguments are invalid"));
        }
        if request.tool_name == "ask_user_question" {
            let mut ids = std::collections::HashSet::new();
            if !request.arguments["questions"]
                .as_array()
                .is_some_and(|questions| {
                    questions.iter().all(|question| {
                        let unique_id = question["id"]
                            .as_str()
                            .is_some_and(|id| ids.insert(id.to_owned()));
                        let options = question.get("options").and_then(Value::as_array);
                        let mut labels = std::collections::HashSet::new();
                        let valid_options = options.is_none_or(|options| {
                            !options.is_empty()
                                && options.len() <= 20
                                && options.iter().all(|option| {
                                    option["label"].as_str().is_some_and(|label| {
                                        !label.trim().is_empty()
                                            && label.len() <= 1024
                                            && labels.insert(label.to_owned())
                                    })
                                })
                        });
                        unique_id
                            && valid_options
                            && (!question["multi_select"].as_bool().unwrap_or(false)
                                || options.is_some())
                    })
                })
            {
                return Err(failed("question ids must be unique"));
            }
        }
        if request.tool_name == "exit_plan_mode"
            && !request.arguments["plan"]
                .as_str()
                .is_some_and(|plan| plan.trim_start().starts_with("# "))
        {
            return Err(failed("plan must start with a markdown heading"));
        }
        match request.tool_name.as_str() {
            "ask_user_question" | "exit_plan_mode" => self.interactions.request(request).await,
            "subagent" | "subagent_fork" => self.subagent(request).await,
            _ => Err(failed("tool is unavailable")),
        }
    }

    async fn cancel_and_drain(&self) {
        self.interactions.cancel_and_drain().await;
    }

    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        if matches!(
            execution.tool_name.as_str(),
            "ask_user_question" | "exit_plan_mode"
        ) {
            self.interactions.reconcile(execution).await
        } else {
            Ok(ToolRecovery::Unknown)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ait_domain::{RunUsage, ToolUse};
    use ait_ports::AgentResponse;
    use std::{collections::VecDeque, sync::Mutex};

    struct ScriptedAgent {
        responses: Mutex<VecDeque<AgentResponse>>,
        paths: Mutex<Vec<Vec<ProjectedMessage>>>,
    }

    #[async_trait]
    impl RunAgent for ScriptedAgent {
        async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
            self.paths.lock().unwrap().push(request.message_path);
            Ok(self.responses.lock().unwrap().pop_front().unwrap())
        }
    }

    struct ChildTools;

    #[async_trait]
    impl RunTool for ChildTools {
        fn executable_tools(&self) -> Vec<String> {
            vec!["read".into()]
        }

        fn requires_approval(&self, _: &str, _: &Value) -> bool {
            false
        }

        async fn execute(&self, _: ToolInvocation) -> Result<ToolOutcome, DomainError> {
            Ok(ToolOutcome {
                output: json!({"text":"child input"}),
                usage: RunUsage::default(),
            })
        }

        async fn reconcile(&self, _: &ToolExecution) -> Result<ToolRecovery, DomainError> {
            Ok(ToolRecovery::Unknown)
        }
    }

    #[derive(Default)]
    struct Interactions(Mutex<Vec<String>>);

    #[async_trait]
    impl RunToolInteraction for Interactions {
        async fn request(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
            self.0.lock().unwrap().push(request.tool_name);
            Ok(ToolOutcome {
                output: json!({"answers":{"choice":"Safe"}}),
                usage: RunUsage::default(),
            })
        }

        async fn reconcile(&self, _: &ToolExecution) -> Result<ToolRecovery, DomainError> {
            Ok(ToolRecovery::RetrySafe)
        }
    }

    fn call(name: &str, arguments: Value, message_path: Vec<ProjectedMessage>) -> ToolInvocation {
        ToolInvocation {
            run_id: RunId::new("parent"),
            call_id: "call".into(),
            execution_id: ToolExecutionId::new("execution"),
            tool_name: name.into(),
            arguments,
            message_path,
            cancellation: tokio_util::sync::CancellationToken::new(),
        }
    }

    #[tokio::test]
    async fn foreground_subagent_runs_tools_and_charges_nested_usage() {
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new(VecDeque::from([
                AgentResponse {
                    sub_messages: vec![SubMessage::ToolUse(ToolUse {
                        call_id: "child-read".into(),
                        tool_name: "read".into(),
                        arguments: json!({"file_path":"input"}).to_string(),
                        provider_metadata: None,
                    })],
                    usage: RunUsage {
                        input_tokens: 3,
                        output_tokens: 2,
                        ..RunUsage::default()
                    },
                },
                AgentResponse {
                    sub_messages: vec![SubMessage::Text {
                        text: "done".into(),
                    }],
                    usage: RunUsage {
                        input_tokens: 5,
                        output_tokens: 1,
                        ..RunUsage::default()
                    },
                },
            ])),
            paths: Mutex::new(Vec::new()),
        });
        let tools = AgentTools::new(
            agent.clone(),
            Arc::new(ChildTools),
            Arc::new(Interactions::default()),
        );
        let outcome = tools
            .execute(call(
                "subagent",
                json!({"description":"Inspect input","prompt":"Read input and report.","run_in_background":false}),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(outcome.output["content"], "done");
        assert_eq!(outcome.usage.input_tokens, 8);
        assert_eq!(outcome.usage.output_tokens, 3);
        assert_eq!(outcome.usage.tool_executions, 1);
        let paths = agent.paths.lock().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(matches!(
            paths[1].last(),
            Some(ProjectedMessage::Visible(message)) if message.kind == MessageKind::ToolResult
        ));
    }

    #[tokio::test]
    async fn fork_inherits_context_and_questions_use_the_interaction_port() {
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new(VecDeque::from([AgentResponse {
                sub_messages: vec![SubMessage::Text {
                    text: "forked".into(),
                }],
                usage: RunUsage::default(),
            }])),
            paths: Mutex::new(Vec::new()),
        });
        let interactions = Arc::new(Interactions::default());
        let tools = AgentTools::new(agent.clone(), Arc::new(ChildTools), interactions.clone());
        let inherited = user_message(
            "inherited",
            1,
            ProjectId::new("project"),
            None,
            &RunId::new("parent"),
            "prior context".into(),
        );
        let outcome = tools
            .execute(call(
                "subagent_fork",
                json!({"description":"Continue task","prompt":"Finish it.","run_in_background":false}),
                vec![ProjectedMessage::Visible(inherited)],
            ))
            .await
            .unwrap();
        assert_eq!(outcome.output["content"], "forked");
        assert_eq!(agent.paths.lock().unwrap()[0].len(), 2);

        let answer = tools
            .execute(call(
                "ask_user_question",
                json!({"questions":[{"id":"choice","question":"Choose?"}]}),
                Vec::new(),
            ))
            .await
            .unwrap();
        assert_eq!(answer.output["answers"]["choice"], "Safe");
        assert_eq!(*interactions.0.lock().unwrap(), ["ask_user_question"]);
        assert!(
            tools
                .execute(call(
                    "subagent",
                    json!({"description":"Background","prompt":"Wait.","run_in_background":true}),
                    Vec::new(),
                ))
                .await
                .is_err()
        );
    }
}
