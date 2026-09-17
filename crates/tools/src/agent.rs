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

fn parent(path: &[ProjectedMessage]) -> Option<MessageId> {
    path.iter().rev().find_map(|entry| match entry {
        ProjectedMessage::Visible(message) => Some(message.id),
        ProjectedMessage::Redacted { .. } => None,
    })
}

fn child_prompt(request: &ToolInvocation) -> Result<String, DomainError> {
    if request
        .arguments
        .get("run_in_background")
        .is_some_and(|value| value != &Value::Bool(false))
    {
        return Err(failed("background tasks are not supported by this host"));
    }
    request
        .arguments
        .get("prompt")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty() && value.len() <= MAX_BYTES)
        .map(str::to_owned)
        .ok_or_else(|| failed("task prompt is invalid"))
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
            .map_err(|_| failed("task returned invalid tool arguments"))?;
        let child_usage = ait_ports::ToolUsageRecorder::default();
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
                    usage: child_usage.clone(),
                    cancellation: request.cancellation.clone(),
                })
                .await
        } else {
            Err(failed("task requested an unavailable tool"))
        };
        request.usage.record(&child_usage.snapshot());
        request.usage.record(&RunUsage {
            tool_executions: 1,
            ..RunUsage::default()
        });
        if let Ok(outcome) = &result {
            request.usage.record(&outcome.usage);
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

    async fn task(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        let prompt = child_prompt(&request)?;
        let child_run = RunId::new(format!(
            "{}:task:{}",
            request.run_id.as_str(),
            request.execution_id.as_str()
        ));
        let mut path = Vec::new();
        let project_id = ProjectId::new("task");
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
            tool_calls: 0,
        };
        for round in 1..=MAX_CHILD_ROUNDS {
            if request.cancellation.is_cancelled() {
                return Err(DomainError::invariant(
                    ErrorCode::RunCancelled,
                    "task cancelled",
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
            request.usage.record(&response.usage);
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
                    return Err(failed("task returned an invalid final response"));
                }
                return Ok(ToolOutcome {
                    output: json!({"content": final_text, "rounds": round}),
                    usage: RunUsage::default(),
                });
            }
            state.tool_calls = state
                .tool_calls
                .saturating_add(u32::try_from(calls.len()).unwrap_or(u32::MAX));
            if state.tool_calls > MAX_CHILD_TOOL_CALLS {
                return Err(failed("task exceeded its tool-call limit"));
            }
            for call in calls {
                self.execute_child_call(&request, &mut state, call).await?;
            }
        }
        Err(failed("task exceeded its round limit"))
    }
}

#[async_trait]
impl RunTool for AgentTools {
    fn executable_tools(&self) -> Vec<String> {
        ["plan_exit", "question", "task"]
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
        if request.tool_name == "question" {
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
        if request.tool_name == "plan_exit"
            && !request.arguments["plan"]
                .as_str()
                .is_some_and(|plan| plan.trim_start().starts_with("# "))
        {
            return Err(failed("plan must start with a markdown heading"));
        }
        match request.tool_name.as_str() {
            "plan_exit" | "question" => self.interactions.request(request).await,
            "task" => self.task(request).await,
            _ => Err(failed("tool is unavailable")),
        }
    }

    async fn cancel_and_drain(&self) {
        self.interactions.cancel_and_drain().await;
    }

    async fn reconcile(&self, execution: &ToolExecution) -> Result<ToolRecovery, DomainError> {
        if matches!(execution.tool_name.as_str(), "plan_exit" | "question") {
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
        responses: Mutex<VecDeque<Result<AgentResponse, DomainError>>>,
        paths: Mutex<Vec<Vec<ProjectedMessage>>>,
    }

    #[async_trait]
    impl RunAgent for ScriptedAgent {
        async fn invoke(&self, request: AgentInvocation) -> Result<AgentResponse, DomainError> {
            self.paths.lock().unwrap().push(request.message_path);
            self.responses.lock().unwrap().pop_front().unwrap()
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

    fn call(name: &str, arguments: Value) -> (ToolInvocation, ait_ports::ToolUsageRecorder) {
        let usage = ait_ports::ToolUsageRecorder::default();
        (
            ToolInvocation {
                run_id: RunId::new("parent"),
                call_id: "call".into(),
                execution_id: ToolExecutionId::new("execution"),
                tool_name: name.into(),
                arguments,
                usage: usage.clone(),
                cancellation: tokio_util::sync::CancellationToken::new(),
            },
            usage,
        )
    }

    #[tokio::test]
    async fn foreground_task_runs_tools_and_charges_nested_usage() {
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new(VecDeque::from([
                Ok(AgentResponse {
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
                }),
                Ok(AgentResponse {
                    sub_messages: vec![SubMessage::Text {
                        text: "done".into(),
                    }],
                    usage: RunUsage {
                        input_tokens: 5,
                        output_tokens: 1,
                        ..RunUsage::default()
                    },
                }),
            ])),
            paths: Mutex::new(Vec::new()),
        });
        let tools = AgentTools::new(
            agent.clone(),
            Arc::new(ChildTools),
            Arc::new(Interactions::default()),
        );
        let (request, usage) = call(
            "task",
            json!({"description":"Inspect input","prompt":"Read input and report.","run_in_background":false}),
        );
        let outcome = tools.execute(request).await.unwrap();
        assert_eq!(outcome.output["content"], "done");
        assert_eq!(outcome.usage, RunUsage::default());
        assert_eq!(usage.snapshot().input_tokens, 8);
        assert_eq!(usage.snapshot().output_tokens, 3);
        assert_eq!(usage.snapshot().tool_executions, 1);
        let paths = agent.paths.lock().unwrap();
        assert_eq!(paths.len(), 2);
        assert!(matches!(
            paths[1].last(),
            Some(ProjectedMessage::Visible(message)) if message.kind == MessageKind::ToolResult
        ));
    }

    #[tokio::test]
    async fn task_is_self_contained_and_questions_use_the_interaction_port() {
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new(VecDeque::from([Ok(AgentResponse {
                sub_messages: vec![SubMessage::Text {
                    text: "completed".into(),
                }],
                usage: RunUsage::default(),
            })])),
            paths: Mutex::new(Vec::new()),
        });
        let interactions = Arc::new(Interactions::default());
        let tools = AgentTools::new(agent.clone(), Arc::new(ChildTools), interactions.clone());
        let (request, _) = call(
            "task",
            json!({"description":"Continue task","prompt":"Finish it.","run_in_background":false}),
        );
        let outcome = tools.execute(request).await.unwrap();
        assert_eq!(outcome.output["content"], "completed");
        {
            let paths = agent.paths.lock().unwrap();
            assert_eq!(paths[0].len(), 1);
            assert!(matches!(
                &paths[0][0],
                ProjectedMessage::Visible(message)
                    if message.role == MessageRole::User
                        && matches!(message.sub_messages.as_slice(), [SubMessage::Text { text }] if text == "Finish it.")
            ));
        }

        let (request, _) = call(
            "question",
            json!({"questions":[{"id":"choice","question":"Choose?"}]}),
        );
        let answer = tools.execute(request).await.unwrap();
        assert_eq!(answer.output["answers"]["choice"], "Safe");
        assert_eq!(*interactions.0.lock().unwrap(), ["question"]);
        let (request, _) = call(
            "task",
            json!({"description":"Background","prompt":"Wait.","run_in_background":true}),
        );
        assert!(tools.execute(request).await.is_err());
    }

    #[tokio::test]
    async fn task_preserves_known_usage_when_a_later_agent_turn_fails() {
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new(VecDeque::from([
                Ok(AgentResponse {
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
                }),
                Err(failed("provider failed")),
            ])),
            paths: Mutex::new(Vec::new()),
        });
        let tools = AgentTools::new(
            agent,
            Arc::new(ChildTools),
            Arc::new(Interactions::default()),
        );
        let (request, usage) = call(
            "task",
            json!({"description":"Inspect input","prompt":"Read input and report.","run_in_background":false}),
        );

        assert!(tools.execute(request).await.is_err());
        assert_eq!(
            usage.snapshot(),
            RunUsage {
                input_tokens: 3,
                output_tokens: 2,
                tool_executions: 1,
                ..RunUsage::default()
            }
        );
    }

    #[tokio::test]
    async fn task_preserves_usage_when_it_exceeds_the_round_limit() {
        let response = || {
            Ok(AgentResponse {
                sub_messages: vec![SubMessage::ToolUse(ToolUse {
                    call_id: "child-read".into(),
                    tool_name: "read".into(),
                    arguments: json!({"file_path":"input"}).to_string(),
                    provider_metadata: None,
                })],
                usage: RunUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    ..RunUsage::default()
                },
            })
        };
        let agent = Arc::new(ScriptedAgent {
            responses: Mutex::new((0..MAX_CHILD_ROUNDS).map(|_| response()).collect()),
            paths: Mutex::new(Vec::new()),
        });
        let tools = AgentTools::new(
            agent,
            Arc::new(ChildTools),
            Arc::new(Interactions::default()),
        );
        let (request, usage) = call(
            "task",
            json!({"description":"Inspect input","prompt":"Keep inspecting.","run_in_background":false}),
        );

        assert!(tools.execute(request).await.is_err());
        assert_eq!(usage.snapshot().input_tokens, u64::from(MAX_CHILD_ROUNDS));
        assert_eq!(usage.snapshot().output_tokens, u64::from(MAX_CHILD_ROUNDS));
        assert_eq!(
            usage.snapshot().tool_executions,
            u64::from(MAX_CHILD_ROUNDS)
        );
    }
}
