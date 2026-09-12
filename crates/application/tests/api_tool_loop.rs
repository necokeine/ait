//! WF-13: real API HTTP fixtures through the public Session/Run path and SQLite.
#![allow(clippy::pedantic)]
mod support;
use ait_agent_adapters::{LLMClient, LLMClientConfig, LLMProvider, provider_turn};
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult, ProviderSecret, default_settings};
use ait_domain::{AgentConfiguration, AgentProvider, DomainError, ProviderKind, ProviderModel};
use ait_ports::{AgentInvocation, AgentProviderGateway, AgentResponse, ProviderMessage};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use axum::{Json, Router, routing::post};
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};
struct Gateway(LLMClient);
#[async_trait]
impl AgentProviderGateway for Gateway {
    async fn store_secret(&self, _: &str, _: &str) -> Result<(), DomainError> {
        Ok(())
    }
    async fn delete_secret(&self, _: &str) -> Result<(), DomainError> {
        Ok(())
    }
    async fn list_models(
        &self,
        p: &AgentProvider,
        _: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        Ok(p.models.clone())
    }
    async fn list_models_with_secret(
        &self,
        p: &AgentProvider,
        _: &str,
    ) -> Result<Vec<ProviderModel>, DomainError> {
        Ok(p.models.clone())
    }
    async fn complete(
        &self,
        _: &AgentProvider,
        _: &str,
        _: &AgentConfiguration,
        _: Vec<ProviderMessage>,
    ) -> Result<String, DomainError> {
        panic!("text shortcut must not run")
    }
    async fn complete_turn(
        &self,
        _: &AgentProvider,
        _: &str,
        c: &AgentConfiguration,
        r: AgentInvocation,
        n: Vec<String>,
    ) -> Result<AgentResponse, DomainError> {
        provider_turn::complete_turn(&self.0, c, r, &n).await
    }
}
fn response(kind: ProviderKind, calls: &[(&str, &str, Value)]) -> Value {
    if kind == ProviderKind::OpenAI {
        let output = if calls.is_empty() {
            vec![
                json!({"type":"message","id":"msg_final","status":"completed","role":"assistant","content":[{"type":"output_text","annotations":[],"text":"Verified hello.py"}]}),
            ]
        } else {
            calls.iter().map(|(id,name,args)|json!({"type":"function_call","id":format!("fc_{id}"),"call_id":id,"name":name,"arguments":args.to_string(),"status":"completed"})).collect()
        };
        json!({"id":"resp_fixture","object":"response","created_at":0,"status":"completed","model":"fixture-model","tools":[],"output":output,"usage":{"input_tokens":3,"output_tokens":2,"total_tokens":5}})
    } else {
        let message = if calls.is_empty() {
            json!({"role":"assistant","content":"Verified hello.py"})
        } else {
            json!({"role":"assistant","content":null,"reasoning_content":"Use the host tools.","tool_calls":calls.iter().map(|(id,name,args)|json!({"id":id,"type":"function","function":{"name":name,"arguments":args.to_string()}})).collect::<Vec<_>>()})
        };
        json!({"id":"chatcmpl_fixture","object":"chat.completion","created":0,"model":"fixture-model","choices":[{"index":0,"message":message,"finish_reason":if calls.is_empty(){"stop"}else{"tool_calls"}}],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}})
    }
}
async fn ok(service: &LocalControlService, command: Command) -> CommandResult {
    let r = service.execute(command).await;
    assert!(r.ok, "{:?}", r.error);
    r.result.unwrap()
}
struct Fixture {
    directory: tempfile::TempDir,
    project: tempfile::TempDir,
    service: LocalControlService,
    requests: Arc<Mutex<Vec<Value>>>,
    server: tokio::task::JoinHandle<()>,
}
impl Fixture {
    async fn new(kind: ProviderKind, responses: Vec<Value>, sandbox: &str) -> Self {
        let responses = Arc::new(Mutex::new(VecDeque::from(responses)));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let app = Router::new().fallback(post(move |Json(body): Json<Value>| {
            let captured = captured.clone();
            let responses = responses.clone();
            async move {
                captured.lock().unwrap().push(body);
                Json(
                    responses
                        .lock()
                        .unwrap()
                        .pop_front()
                        .expect("unexpected provider request"),
                )
            }
        }));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        let mut config = LLMClientConfig::new(
            if kind == ProviderKind::OpenAI {
                LLMProvider::OpenAI
            } else {
                LLMProvider::DeepSeek
            },
            "offline-fixture-key",
        );
        config.base_url = Some(url.clone());
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let service = LocalControlService::new(Arc::new(
            SqliteControlStore::open(directory.path().join("ait.db")).unwrap(),
        ))
        .with_provider_gateway(Arc::new(Gateway(LLMClient::new(config).unwrap())))
        .with_api_tools(Arc::new(ait_tools::host::HostToolFactory));
        let mut settings = default_settings();
        settings
            .0
            .insert("permissions.sandbox".into(), json!(sandbox));
        ok(
            &service,
            Command::SaveSettings {
                expected_revision: 1,
                values: settings,
            },
        )
        .await;
        ok(
            &service,
            Command::SaveAgentProvider {
                provider: AgentProvider {
                    id: "api".into(),
                    name: "API".into(),
                    kind,
                    url: Some(url),
                    models: vec![ProviderModel {
                        id: "fixture-model".into(),
                        name: "Fixture".into(),
                        reasoning_efforts: vec![],
                    }],
                },
                secret: Some(ProviderSecret("offline-fixture-key".into())),
            },
        )
        .await;
        ok(
            &service,
            Command::RegisterProject {
                id: "p".into(),
                name: "Project".into(),
                workdir: Some(project.path().display().to_string()),
                repo_url: None,
            },
        )
        .await;
        ok(
            &service,
            Command::RegisterAgent {
                id: "agent".into(),
                name: "Agent".into(),
                config: AgentConfiguration {
                    provider_id: "api".into(),
                    model: "fixture-model".into(),
                    reasoning_effort: None,
                },
            },
        )
        .await;
        ok(
            &service,
            Command::CreateSession {
                id: "session".into(),
                project_id: "p".into(),
                agent_id: "agent".into(),
                at_message_id: None,
            },
        )
        .await;
        Self {
            directory,
            project,
            service,
            requests,
            server,
        }
    }
    async fn run(&self) -> ait_contracts::RunView {
        let CommandResult::Run(run) = ok(
            &self.service,
            Command::SendMessage {
                session_id: "session".into(),
                text: "Create hello.py using tools, read and search it, then report.".into(),
            },
        )
        .await
        else {
            panic!()
        };
        run
    }
    async fn finish(self) {
        self.server.abort();
        let _ = self.server.await;
    }
}
#[tokio::test]
async fn wf13_openai_and_deepseek_create_and_verify_files_through_persisted_tool_rounds() {
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        let f = Fixture::new(
            kind,
            vec![
                response(
                    kind,
                    &[(
                        "write_1",
                        "write",
                        json!({"file_path":"hello.py","content":"print('hello')\n"}),
                    )],
                ),
                response(
                    kind,
                    &[
                        ("read_2", "read", json!({"file_path":"hello.py"})),
                        ("search_2", "grep", json!({"pattern":"hello"})),
                    ],
                ),
                response(kind, &[]),
            ],
            "workspace_write",
        )
        .await;
        let run = f.run().await;
        assert_eq!(run.status, "completed");
        assert!(run.native_approvals.is_empty());
        let execution = run.execution.as_ref().unwrap();
        assert_eq!(execution.run.attempt_count, 1);
        assert_eq!(execution.attempts.len(), 1);
        assert_eq!(execution.tools.len(), 3);
        assert_eq!(execution.run.usage.input_tokens, 9);
        assert_eq!(execution.run.usage.output_tokens, 6);
        assert_eq!(execution.run.usage.tool_executions, 3);
        assert_eq!(execution.run.agent_snapshot.revision, run.agent_revision);
        assert_eq!(
            std::fs::read_to_string(f.project.path().join("hello.py")).unwrap(),
            "print('hello')\n"
        );
        let git = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(f.project.path())
            .output()
            .unwrap();
        assert!(git.status.success());
        assert_eq!(String::from_utf8(git.stdout).unwrap().trim(), "?? hello.py");
        let python = std::process::Command::new("python3")
            .arg(f.project.path().join("hello.py"))
            .output()
            .unwrap();
        assert!(python.status.success());
        assert_eq!(python.stdout, b"hello\n");
        let reopened = LocalControlService::new(Arc::new(
            SqliteControlStore::open(f.directory.path().join("ait.db")).unwrap(),
        ));
        let after = support::workspace(&reopened).await;
        assert_eq!(after.runs[0], run);
        assert_eq!(
            after.sessions[0].current_message_id,
            run.last_message_id.clone().unwrap()
        );
        assert!(after.sessions[0].active_run_id.is_none());
        let mut generated = after
            .messages
            .iter()
            .filter_map(|m| m.data.as_ref().and_then(|d| d.get("native_message")))
            .collect::<Vec<_>>();
        generated.sort_by_key(|m| m["run_seq"].as_u64());
        assert_eq!(generated.len(), 6);
        let mut parent = run.base_message_id.clone();
        for (index, message) in generated.iter().enumerate() {
            assert_eq!(message["run_id"], run.id);
            assert_eq!(message["run_seq"], index + 1);
            assert_eq!(message["parent_message_id"], parent);
            parent = message["id"].as_str().unwrap().into();
        }
        assert_eq!(generated[1]["tool_result"]["call_id"], "write_1");
        assert_eq!(generated[3]["tool_result"]["call_id"], "read_2");
        assert_eq!(generated[4]["tool_result"]["call_id"], "search_2");
        let requests = f.requests.lock().unwrap().clone();
        assert_eq!(requests.len(), 3);
        let wire = requests[2].to_string();
        for id in ["write_1", "read_2", "search_2"] {
            assert!(wire.contains(id));
        }
        assert!(wire.contains("1: print('hello')"));
        let first = &requests[0];
        let names = first["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| {
                if kind == ProviderKind::OpenAI {
                    t["name"].as_str().unwrap()
                } else {
                    t["function"]["name"].as_str().unwrap()
                }
            })
            .collect::<Vec<_>>();
        let expected = if cfg!(unix) {
            vec!["bash", "edit", "grep", "read", "write"]
        } else {
            vec!["edit", "grep", "read", "write"]
        };
        assert_eq!(names, expected);
        let result_ids = if kind == ProviderKind::OpenAI {
            requests[2]["input"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["type"] == "function_call_output")
                .map(|m| m["call_id"].as_str().unwrap())
                .collect::<Vec<_>>()
        } else {
            requests[2]["messages"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|m| m["role"] == "tool")
                .map(|m| m["tool_call_id"].as_str().unwrap())
                .collect::<Vec<_>>()
        };
        assert_eq!(result_ids, ["write_1", "read_2", "search_2"]);
        assert!(
            after
                .messages
                .iter()
                .filter(|m| m
                    .data
                    .as_ref()
                    .is_some_and(|d| d.get("native_message").is_some()))
                .all(|m| m.data.as_ref().unwrap()["agent_revision"] == run.agent_revision)
        );
        assert!(
            !serde_json::to_string(&f.service.replay_events(0, 1000).await.unwrap())
                .unwrap()
                .contains("offline-fixture-key")
        );
        f.finish().await;
    }
}
#[tokio::test]
async fn denied_invalid_unknown_failed_and_approval_results_continue_without_side_effects() {
    let kind = ProviderKind::DeepSeek;
    let f=Fixture::new(kind,vec![response(kind,&[
        ("bad_path","write",json!({"file_path":"../escape","content":"bad"})),
        ("bad_args","read",json!({"file_path":10})),
        ("unknown","web_search",json!({})),
        ("missing","read",json!({"file_path":"missing"})),
        ("approval","write",json!({"file_path":"denied","content":"bad","sandbox_permissions":"danger-full-access","justification":"escape"})),
    ]),response(kind,&[])],"workspace_write").await;
    let run = f.run().await;
    assert_eq!(run.status, "completed");
    let tools = &run.execution.unwrap().tools;
    assert_eq!(tools.len(), 5);
    assert!(
        tools
            .iter()
            .all(|t| t.status != ait_domain::ToolExecutionStatus::Succeeded)
    );
    assert_eq!(tools[4].status, ait_domain::ToolExecutionStatus::Denied);
    assert!(!f.project.path().join("denied").exists());
    f.finish().await;
}
#[tokio::test]
async fn duplicate_calls_fail_before_dispatch_and_read_only_never_advertises_writes() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[
                ("same", "write", json!({"file_path":"bad","content":"bad"})),
                ("same", "write", json!({"file_path":"bad","content":"bad"})),
            ],
        )],
        "read_only",
    )
    .await;
    let run = f.run().await;
    assert_eq!(run.status, "failed");
    assert_eq!(
        run.error.unwrap().code,
        ait_domain::ErrorCode::ToolCallDuplicate
    );
    assert!(!f.project.path().join("bad").exists());
    let wire = f.requests.lock().unwrap()[0]["tools"].to_string();
    assert!(!wire.contains("\"write\""));
    f.finish().await;
}

#[cfg(unix)]
#[tokio::test]
async fn public_cancellation_waits_for_one_cancelled_tool_result_and_releases_session() {
    let kind = ProviderKind::DeepSeek;
    let f = Fixture::new(
        kind,
        vec![response(
            kind,
            &[(
                "sleep",
                "bash",
                json!({"command":"sleep 30","description":"Wait"}),
            )],
        )],
        "read_only",
    )
    .await;
    let service = Arc::new(f.service.clone());
    let accepted = service
        .submit(Command::SendMessage {
            session_id: "session".into(),
            text: "Wait using the tool".into(),
        })
        .await;
    assert!(accepted.ok, "{:?}", accepted.error);
    let CommandResult::Run(initial) = accepted.result.unwrap() else {
        panic!()
    };
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let CommandResult::Run(run) = ok(
                &service,
                Command::GetRun {
                    run_id: initial.id.clone(),
                },
            )
            .await
            else {
                panic!()
            };
            if run.execution.as_ref().is_some_and(|e| {
                e.tools
                    .iter()
                    .any(|t| t.status == ait_domain::ToolExecutionStatus::Running)
            }) {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let CommandResult::Run(cancelling) = ok(
        &service,
        Command::CancelRun {
            run_id: initial.id.clone(),
        },
    )
    .await
    else {
        panic!()
    };
    assert_eq!(cancelling.status, "cancelling");
    let run = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            let CommandResult::Run(run) = ok(
                &service,
                Command::GetRun {
                    run_id: initial.id.clone(),
                },
            )
            .await
            else {
                panic!()
            };
            if run.status == "cancelled" {
                break run;
            }
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let execution = run.execution.unwrap();
    assert_eq!(execution.tools.len(), 1);
    assert_eq!(
        execution.tools[0].status,
        ait_domain::ToolExecutionStatus::Cancelled
    );
    assert!(execution.tools[0].tool_result_message_id.is_some());
    assert!(
        support::workspace(&service).await.sessions[0]
            .active_run_id
            .is_none()
    );
    assert_eq!(f.requests.lock().unwrap().len(), 1);
    f.finish().await;
}

#[tokio::test]
async fn archive_and_events_omit_tool_payloads_while_queries_retain_them() {
    let kind = ProviderKind::OpenAI;
    let f = Fixture::new(
        kind,
        vec![
            response(
                kind,
                &[(
                    "write",
                    "write",
                    json!({"file_path":"hello.py","content":"private-payload-marker"}),
                )],
            ),
            response(kind, &[]),
        ],
        "workspace_write",
    )
    .await;
    let run = f.run().await;
    assert_eq!(run.status, "completed");
    assert!(
        serde_json::to_string(&run)
            .unwrap()
            .contains("private-payload-marker")
    );
    let events = f.service.replay_events(0, 1000).await.unwrap();
    assert!(
        !serde_json::to_string(&events)
            .unwrap()
            .contains("private-payload-marker")
    );
    let archive = ok(
        &f.service,
        Command::ExportProject {
            project_id: "p".into(),
        },
    )
    .await;
    assert!(
        !serde_json::to_string(&archive)
            .unwrap()
            .contains("private-payload-marker")
    );
    f.finish().await;
}
