//! WF-13: real API HTTP fixtures through the public Session/Run path and SQLite.
#![allow(clippy::pedantic)]
#[path = "../../../crates/application/tests/support.rs"]
mod support;
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
struct Gateway;
#[async_trait]
impl AgentProviderGateway for Gateway {
    async fn credential_grant(&self, _: &str) -> Result<String, DomainError> {
        Ok("offline-fixture-key".into())
    }
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
        _: &AgentConfiguration,
        _: AgentInvocation,
        _: Vec<String>,
    ) -> Result<AgentResponse, DomainError> {
        panic!("provider execution must be in ait-worker")
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
    workdir: std::path::PathBuf,
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
        let directory = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let store = Arc::new(SqliteControlStore::open(directory.path().join("ait.db")).unwrap());
        let service = LocalControlService::new(store.clone())
            .with_provider_gateway(Arc::new(Gateway))
            .with_run_dispatcher(Arc::new(ait_ipc::supervisor::WorkerSupervisor::new(
                env!("CARGO_BIN_EXE_ait-worker").into(),
            )));
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
        let workdir =
            std::path::PathBuf::from(&support::workspace(&service).await.sessions[0].workdir);
        Self {
            directory,
            project,
            workdir,
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
async fn subprocess_openai_and_deepseek_keep_tool_result_order_and_sqlite_receipts() {
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
        assert_eq!(run.status, "completed", "{run:?}");
        assert!(run.execution.as_ref().unwrap().worker_receipts.len() > 10);
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
            std::fs::read_to_string(f.workdir.join("hello.py")).unwrap(),
            "print('hello')\n"
        );
        let git = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(&f.workdir)
            .output()
            .unwrap();
        assert!(git.status.success());
        assert_eq!(String::from_utf8(git.stdout).unwrap().trim(), "?? hello.py");
        let python = std::process::Command::new("python3")
            .arg(f.workdir.join("hello.py"))
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
        assert!(
            !f.project.path().join("hello.py").exists(),
            "worker wrote into Project main checkout"
        );
        let primary = std::process::Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(f.project.path())
            .output()
            .unwrap();
        assert!(primary.status.success());
        assert!(primary.stdout.is_empty(), "Project main checkout is dirty");
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

#[cfg(unix)]
struct KillOnce {
    method: &'static str,
    boundary: ait_ipc::supervisor::CommitBoundary,
    fired: std::sync::atomic::AtomicBool,
}
#[cfg(unix)]
impl ait_ipc::supervisor::WorkerObserver for KillOnce {
    fn checkpoint(
        &self,
        pid: u32,
        method: &str,
        boundary: ait_ipc::supervisor::CommitBoundary,
    ) -> Result<(), ait_contracts::worker::ProtocolError> {
        if method == self.method
            && boundary == self.boundary
            && !self.fired.swap(true, std::sync::atomic::Ordering::SeqCst)
        {
            assert!(
                std::process::Command::new("kill")
                    .args(["-KILL", &pid.to_string()])
                    .status()
                    .unwrap()
                    .success()
            );
            return Err(ait_contracts::worker::ProtocolError::UnexpectedEof);
        }
        Ok(())
    }
}
#[cfg(unix)]
struct FaultDispatcher(Arc<KillOnce>);
#[cfg(unix)]
#[async_trait]
impl ait_ports::RunDispatcher for FaultDispatcher {
    async fn dispatch(&self, request: ait_ports::ApiRunDispatch) -> Result<(), DomainError> {
        use ait_contracts::worker::{Bootstrap, Executor, Lease, Limits};
        use ait_ipc::{mapping::Wire, supervisor::WorkerSupervisor};
        let supervisor = WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
            .with_observer(self.0.clone());
        for _ in 0..3 {
            let lease = request
                .store
                .claim_worker(&uuid::Uuid::new_v4().to_string())
                .await
                .unwrap();
            let bootstrap = Bootstrap {
                lease: Lease {
                    run_id: request.run_id.as_str().into(),
                    worker_instance_id: lease.instance_id,
                    lease_epoch: lease.epoch,
                },
                limits: Limits::default(),
                workdir: request.workdir.to_string_lossy().into_owned(),
                permission: request.permission.to_wire(),
                maximum_sandbox: request.maximum_sandbox.to_wire(),
                executor: Executor::Scripted {
                    replies: vec![
                        vec![
                            ait_domain::SubMessage::ToolUse(ait_domain::ToolUse {
                                call_id: "write_once".into(),
                                tool_name: "write".into(),
                                arguments: json!({"file_path":"effect.txt","content":"one effect"})
                                    .to_string(),
                                provider_metadata: None,
                            })
                            .to_wire(),
                        ],
                        vec![
                            ait_domain::SubMessage::Text {
                                text: "finished".into(),
                            }
                            .to_wire(),
                        ],
                    ],
                },
            };
            let _ = supervisor
                .execute(
                    bootstrap,
                    request.store.clone(),
                    request.cancellation.clone(),
                )
                .await;
            if request
                .store
                .load_run(&request.run_id)
                .await
                .unwrap()
                .status
                .is_terminal()
            {
                return Ok(());
            }
        }
        panic!("bounded recovery did not settle")
    }
}
#[cfg(unix)]
#[tokio::test]
async fn kill_matrix_preserves_acknowledged_messages_results_and_side_effect_intents() {
    use ait_ipc::supervisor::CommitBoundary::{AfterAck, BeforeAck, BeforeCommit};
    for method in [
        "append_message",
        "tool_intent",
        "tool_running",
        "tool_outcome",
        "tool_result",
        "terminal",
    ] {
        for boundary in [BeforeCommit, BeforeAck, AfterAck] {
            let mut f = Fixture::new(ProviderKind::OpenAI, vec![], "workspace_write").await;
            let fault = Arc::new(KillOnce {
                method,
                boundary,
                fired: std::sync::atomic::AtomicBool::new(false),
            });
            f.service = f
                .service
                .clone()
                .with_run_dispatcher(Arc::new(FaultDispatcher(fault.clone())));
            let run = f.run().await;
            assert!(
                fault.fired.load(std::sync::atomic::Ordering::SeqCst),
                "{method} {boundary:?}"
            );
            assert!(
                matches!(run.status.as_str(), "completed" | "failed"),
                "{method} {boundary:?}: {run:?}"
            );
            let execution = run.execution.as_ref().unwrap();
            assert!(
                execution.run.usage.tool_executions <= 1,
                "effect was dispatched twice"
            );
            assert!(execution.tools.len() <= 1);
            assert!(
                execution
                    .tools
                    .iter()
                    .all(|tool| tool.status.is_terminal() && tool.tool_result_message_id.is_some())
            );
            let state = support::workspace(&f.service).await;
            assert!(state.sessions[0].active_run_id.is_none());
            let generated = state
                .messages
                .iter()
                .filter_map(|m| m.data.as_ref().and_then(|d| d.get("native_message")))
                .collect::<Vec<_>>();
            let results = generated
                .iter()
                .filter(|m| m["kind"] == "tool_result")
                .count();
            assert_eq!(results, 1, "{method} {boundary:?}");
            let calls = generated
                .iter()
                .flat_map(|m| m["sub_messages"].as_array().into_iter().flatten())
                .filter(|s| s["type"] == "tool_use")
                .count();
            assert_eq!(calls, 1, "duplicated assistant tool proposal");
            let mut sequences = generated
                .iter()
                .map(|m| m["run_seq"].as_u64().unwrap())
                .collect::<Vec<_>>();
            sequences.sort_unstable();
            sequences.dedup();
            assert_eq!(sequences.len(), generated.len());
            if method == "tool_result"
                || method == "terminal"
                || method == "tool_outcome" && boundary != BeforeCommit
            {
                assert_eq!(
                    run.status, "completed",
                    "lost an acknowledged result at {method} {boundary:?}"
                );
                assert_eq!(
                    std::fs::read_to_string(f.workdir.join("effect.txt")).unwrap(),
                    "one effect"
                );
            }
            f.finish().await;
        }
    }
}

struct ProbeDispatcher {
    old: Mutex<Option<ait_ports::WorkerLease>>,
    store: Mutex<Option<Arc<dyn ait_ports::RunStore>>>,
}
#[async_trait]
impl ait_ports::RunDispatcher for ProbeDispatcher {
    async fn dispatch(&self, request: ait_ports::ApiRunDispatch) -> Result<(), DomainError> {
        use ait_ports::RunMutation;
        let lease = request.store.claim_worker("obsolete-worker").await.unwrap();
        let original = request.store.load_run(&request.run_id).await.unwrap();
        let mut forged = original.clone();
        forged.budget.max_steps += 1;
        assert!(
            request
                .store
                .commit_worker(&lease, "enlarge-budget", RunMutation::SaveRun(forged))
                .await
                .is_err()
        );
        let mut forged = original.clone();
        forged.id = ait_domain::RunId::new("another-run");
        assert!(
            request
                .store
                .commit_worker(&lease, "wrong-run", RunMutation::SaveRun(forged))
                .await
                .is_err()
        );
        assert_eq!(
            request.store.load_run(&request.run_id).await.unwrap(),
            original
        );
        *self.old.lock().unwrap() = Some(lease);
        *self.store.lock().unwrap() = Some(request.store.clone());
        ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
            .dispatch(request)
            .await
    }
}
#[tokio::test]
async fn receipts_survive_sqlite_commit_and_reject_stale_or_changed_replays() {
    let mut f = Fixture::new(
        ProviderKind::OpenAI,
        vec![response(ProviderKind::OpenAI, &[])],
        "read_only",
    )
    .await;
    let probe = Arc::new(ProbeDispatcher {
        old: Mutex::new(None),
        store: Mutex::new(None),
    });
    f.service = f.service.clone().with_run_dispatcher(probe.clone());
    let view = f.run().await;
    assert_eq!(view.status, "completed");
    let execution = view.execution.as_ref().unwrap();
    let (operation, receipt) = execution
        .worker_receipts
        .iter()
        .find(|(_, r)| r.completed == Some(true))
        .unwrap();
    let lease = ait_ports::WorkerLease {
        run_id: ait_domain::RunId::new(&view.id),
        instance_id: execution.worker_instance_id.clone().unwrap(),
        epoch: view.lease_epoch,
    };
    let store = probe.store.lock().unwrap().clone().unwrap();
    let mutation = ait_ports::RunMutation::Complete(receipt.run.clone(), receipt.run.queue_version);
    let first = store
        .commit_worker(&lease, operation, mutation.clone())
        .await
        .unwrap();
    let second = store
        .commit_worker(&lease, operation, mutation.clone())
        .await
        .unwrap();
    assert_eq!(first, second);
    assert_eq!(first.run, receipt.run);
    let old = probe.old.lock().unwrap().clone().unwrap();
    assert!(
        store
            .commit_worker(&old, operation, mutation)
            .await
            .is_err()
    );
    assert!(
        store
            .commit_worker(
                &lease,
                operation,
                ait_ports::RunMutation::SaveRun(receipt.run.clone())
            )
            .await
            .is_err()
    );
    let reopened = LocalControlService::new(Arc::new(
        SqliteControlStore::open(f.directory.path().join("ait.db")).unwrap(),
    ));
    assert_eq!(support::workspace(&reopened).await.runs[0], view);
    for record in [
        serde_json::to_vec(&view).unwrap(),
        std::fs::read(f.directory.path().join("ait.db")).unwrap(),
    ] {
        assert!(
            !record
                .windows(b"offline-fixture-key".len())
                .any(|part| part == b"offline-fixture-key")
        );
    }
    f.finish().await;
}

#[tokio::test]
async fn credential_echo_never_reaches_database_checkpoint_events_or_export() {
    let secret = "offline-fixture-key";
    let reply = response(
        ProviderKind::OpenAI,
        &[(
            "leak",
            "write",
            json!({"file_path":"leak.txt","content":secret}),
        )],
    );
    let f = Fixture::new(ProviderKind::OpenAI, vec![reply; 3], "workspace_write").await;
    let run = f.run().await;
    assert!(matches!(run.status.as_str(), "failed" | "interrupted"));
    assert!(!f.workdir.join("leak.txt").exists());
    let view = support::workspace(&f.service).await;
    assert!(view.sessions[0].active_run_id.is_none());
    assert!(!format!("{view:?}").contains(secret));
    let archive = ok(
        &f.service,
        Command::ExportProject {
            project_id: "p".into(),
        },
    )
    .await;
    assert!(!serde_json::to_string(&archive).unwrap().contains(secret));
    assert!(
        !serde_json::to_string(&f.service.replay_events(0, 1000).await.unwrap())
            .unwrap()
            .contains(secret)
    );
    assert!(
        !serde_json::to_string(&f.service.progress_checkpoints("p").await.unwrap())
            .unwrap()
            .contains(secret)
    );
    for name in ["ait.db", "ait.db-wal"] {
        if let Ok(bytes) = std::fs::read(f.directory.path().join(name)) {
            assert!(
                !bytes
                    .windows(secret.len())
                    .any(|window| window == secret.as_bytes())
            );
        }
    }
    f.finish().await;
}

#[tokio::test]
async fn strict_cost_ceiling_rejects_unpriced_providers_before_network_use() {
    for kind in [ProviderKind::OpenAI, ProviderKind::DeepSeek] {
        let mut f = Fixture::new(kind, vec![], "workspace_write").await;
        f.service = f.service.clone().with_run_dispatcher(Arc::new(
            ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
                .with_cost_ceiling(Some(10_000_000)),
        ));
        let run = f.run().await;
        assert_eq!(run.status, "failed");
        assert_eq!(
            run.error.unwrap().code,
            ait_domain::ErrorCode::RunLimitExceeded
        );
        assert!(
            f.requests.lock().unwrap().is_empty(),
            "an unpriced Provider was charged"
        );
        assert!(
            support::workspace(&f.service).await.sessions[0]
                .active_run_id
                .is_none()
        );
        f.finish().await;
    }
}

#[cfg(unix)]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shutdown_drains_a_real_tool_run_and_rejects_new_admission() {
    struct Started(tokio::sync::Notify);
    impl ait_ipc::supervisor::WorkerObserver for Started {
        fn checkpoint(
            &self,
            _: u32,
            method: &str,
            boundary: ait_ipc::supervisor::CommitBoundary,
        ) -> Result<(), ait_contracts::worker::ProtocolError> {
            if method == "tool_running" && boundary == ait_ipc::supervisor::CommitBoundary::AfterAck
            {
                self.0.notify_one();
            }
            Ok(())
        }
    }
    let mut f = Fixture::new(
        ProviderKind::OpenAI,
        vec![response(
            ProviderKind::OpenAI,
            &[("sleep", "bash", json!({"command":"sleep 30"}))],
        )],
        "workspace_write",
    )
    .await;
    let started = Arc::new(Started(tokio::sync::Notify::new()));
    let supervisor = Arc::new(
        ait_ipc::supervisor::WorkerSupervisor::new(env!("CARGO_BIN_EXE_ait-worker").into())
            .with_observer(started.clone()),
    );
    f.service = f.service.clone().with_run_dispatcher(supervisor.clone());
    let service = Arc::new(f.service.clone());
    let submitted = service
        .submit(Command::SendMessage {
            session_id: "session".into(),
            text: "Run the controlled sleep command".into(),
        })
        .await;
    assert!(submitted.ok);
    tokio::time::timeout(std::time::Duration::from_secs(5), started.0.notified())
        .await
        .unwrap();
    service.begin_shutdown().await.unwrap();
    supervisor.drain();
    let denied = service
        .submit(Command::SendMessage {
            session_id: "session".into(),
            text: "must not start".into(),
        })
        .await;
    assert!(!denied.ok);
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        while !service.runs_drained() {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let view = support::workspace(&service).await;
    assert_eq!(view.runs.len(), 1);
    assert_eq!(view.runs[0].status, "cancelled");
    assert!(view.sessions[0].active_run_id.is_none());
    assert!(
        view.runs[0]
            .execution
            .as_ref()
            .unwrap()
            .tools
            .iter()
            .all(|tool| tool.status.is_terminal() && tool.tool_result_message_id.is_some())
    );
    f.finish().await;
}
