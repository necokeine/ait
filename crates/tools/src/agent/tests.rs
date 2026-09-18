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

#[derive(Default)]
struct ChildTools(Mutex<Vec<String>>);

#[async_trait]
impl RunTool for ChildTools {
    fn executable_tools(&self) -> Vec<String> {
        vec!["read".into()]
    }

    fn requires_approval(&self, _: &str, _: &Value) -> bool {
        false
    }

    async fn execute(&self, request: ToolInvocation) -> Result<ToolOutcome, DomainError> {
        self.0.lock().unwrap().push(request.call_id);
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
        Arc::new(ChildTools::default()),
        Arc::new(Interactions::default()),
    );
    let (request, usage) = call(
        "task",
        json!({"description":"Inspect input","prompt":"Read input and report."}),
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
    let tools = AgentTools::new(
        agent.clone(),
        Arc::new(ChildTools::default()),
        interactions.clone(),
    );
    let (request, _) = call(
        "task",
        json!({"description":"Continue task","prompt":"Finish it."}),
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
                    && matches!(
                        message.sub_messages.as_slice(),
                        [SubMessage::Text { text }] if text == "Finish it."
                    )
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
        Arc::new(ChildTools::default()),
        Arc::new(Interactions::default()),
    );
    let (request, usage) = call(
        "task",
        json!({"description":"Inspect input","prompt":"Read input and report."}),
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
    let response = |index| {
        Ok(AgentResponse {
            sub_messages: vec![SubMessage::ToolUse(ToolUse {
                call_id: format!("child-read-{index}"),
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
        responses: Mutex::new((0..MAX_CHILD_ROUNDS).map(response).collect()),
        paths: Mutex::new(Vec::new()),
    });
    let tools = AgentTools::new(
        agent,
        Arc::new(ChildTools::default()),
        Arc::new(Interactions::default()),
    );
    let (request, usage) = call(
        "task",
        json!({"description":"Inspect input","prompt":"Keep inspecting."}),
    );

    assert!(tools.execute(request).await.is_err());
    assert_eq!(usage.snapshot().input_tokens, u64::from(MAX_CHILD_ROUNDS));
    assert_eq!(usage.snapshot().output_tokens, u64::from(MAX_CHILD_ROUNDS));
    assert_eq!(
        usage.snapshot().tool_executions,
        u64::from(MAX_CHILD_ROUNDS)
    );
}

#[tokio::test]
async fn task_rejects_same_turn_duplicate_call_ids_before_any_tool_executes() {
    let duplicate = || {
        SubMessage::ToolUse(ToolUse {
            call_id: "duplicate".into(),
            tool_name: "read".into(),
            arguments: json!({"file_path":"input"}).to_string(),
            provider_metadata: None,
        })
    };
    let agent = Arc::new(ScriptedAgent {
        responses: Mutex::new(VecDeque::from([Ok(AgentResponse {
            sub_messages: vec![duplicate(), duplicate()],
            usage: RunUsage::default(),
        })])),
        paths: Mutex::new(Vec::new()),
    });
    let child_tools = Arc::new(ChildTools::default());
    let tools = AgentTools::new(
        agent,
        child_tools.clone(),
        Arc::new(Interactions::default()),
    );
    let (request, _) = call(
        "task",
        json!({"description":"Inspect input","prompt":"Read input."}),
    );

    let error = tools.execute(request).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::InvalidSubmessageKind);
    assert!(child_tools.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn task_rejects_cross_round_duplicate_call_ids_without_replaying_the_tool() {
    let response = || {
        Ok(AgentResponse {
            sub_messages: vec![SubMessage::ToolUse(ToolUse {
                call_id: "duplicate".into(),
                tool_name: "read".into(),
                arguments: json!({"file_path":"input"}).to_string(),
                provider_metadata: None,
            })],
            usage: RunUsage::default(),
        })
    };
    let agent = Arc::new(ScriptedAgent {
        responses: Mutex::new(VecDeque::from([response(), response()])),
        paths: Mutex::new(Vec::new()),
    });
    let child_tools = Arc::new(ChildTools::default());
    let tools = AgentTools::new(
        agent,
        child_tools.clone(),
        Arc::new(Interactions::default()),
    );
    let (request, _) = call(
        "task",
        json!({"description":"Inspect input","prompt":"Read input twice."}),
    );

    let error = tools.execute(request).await.unwrap_err();
    assert_eq!(error.code, ErrorCode::ToolCallDuplicate);
    assert_eq!(*child_tools.0.lock().unwrap(), ["duplicate"]);
}
