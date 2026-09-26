use super::*;
use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::storage::agent_runtime::FileBackedAgentRuntimeRegistry;
use crate::test_support::Fixture;
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_metadata::storage::registry::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry};

#[tokio::test]
async fn configuration_commits_atomically_applies_next_turn_and_survives_restart() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let configured = execution
        .execute(
            "agent.config.apply.request",
            json!({"agentId":id,
        "config":{"modelId":"changed-model","thinkingOptionId":"high"}}),
        )
        .await
        .unwrap();
    assert_eq!(configured["accepted"], true);
    assert_invalid_configurations(&execution, &registry, id).await;
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"hang"}),
        )
        .await
        .unwrap();
    let first = fixture
        .requests()
        .into_iter()
        .rev()
        .find(|request| request["method"] == "turn/start")
        .unwrap();
    assert_eq!(first["params"]["model"], "changed-model");
    assert_eq!(first["params"]["effort"], "high");
    let changed = execution
        .execute(
            "agent.model.set.request",
            json!({"agentId":id,"modelId":"next-model"}),
        )
        .await
        .unwrap();
    assert_eq!(changed["accepted"], true);
    assert_eq!(
        changed["notice"]["message"],
        "Configuration applies next turn"
    );
    execution
        .execute(
            "agent.thinking.set.request",
            json!({"agentId":id,"thinkingOptionId":null}),
        )
        .await
        .unwrap();
    assert_eq!(
        execution
            .execute(
                "agent.finish.wait.request",
                json!({"agentId":id,"timeoutMs":1})
            )
            .await
            .unwrap()["status"],
        "timeout"
    );
    execution
        .execute("agent.cancel.request", json!({"agentId":id}))
        .await
        .unwrap();
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    execution.shutdown().await.unwrap();
    assert_configuration_after_restart(&fixture, id).await;
}

async fn assert_invalid_configurations(
    execution: &AgentExecution,
    registry: &FileBackedAgentRuntimeRegistry,
    id: &str,
) {
    let original = registry.get(id).unwrap().unwrap();
    for config in [
        json!({"modelId":"","thinkingOptionId":"low"}),
        json!({"modelId":"a\nb"}),
        json!({"modelId":"x".repeat(257)}),
        json!({"modelId":"other","thinkingOptionId":"imaginary"}),
    ] {
        let result = execution
            .execute(
                "agent.config.apply.request",
                json!({"agentId":id,"config":config}),
            )
            .await
            .unwrap();
        assert_eq!(result["accepted"], false);
        assert_eq!(registry.get(id).unwrap().unwrap(), original);
    }
    assert_eq!(
        execution
            .execute(
                "agent.config.apply.request",
                json!({"agentId":id,
        "config":{"unsupported":true}})
            )
            .await,
        Err(ErrorCode::UnsupportedCapability)
    );
    assert_eq!(
        execution
            .execute("agent.model.set.request", json!({"agentId":id}))
            .await,
        Err(ErrorCode::InvalidMessage)
    );
    assert_eq!(
        execution
            .execute(
                "agent.model.set.request",
                json!({"agentId":id,"modelId":true})
            )
            .await,
        Err(ErrorCode::InvalidMessage)
    );
}

async fn assert_configuration_after_restart(fixture: &Fixture, id: &str) {
    let (restarted, reopened) = worker(fixture);
    restarted
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"next"}),
        )
        .await
        .unwrap();
    restarted
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let second = fixture
        .requests()
        .into_iter()
        .rev()
        .find(|request| request["method"] == "turn/start")
        .unwrap();
    assert_eq!(second["params"]["model"], "next-model");
    assert_eq!(second["params"]["effort"], Value::Null);
    restarted
        .execute(
            "agent.model.set.request",
            json!({"agentId":id,"modelId":null}),
        )
        .await
        .unwrap();
    assert!(
        reopened
            .get(id)
            .unwrap()
            .unwrap()
            .config
            .unwrap()
            .model
            .is_none()
    );
    restarted
        .execute("agent.archive.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(
        restarted
            .execute(
                "agent.thinking.set.request",
                json!({"agentId":id,"thinkingOptionId":"low"})
            )
            .await
            .unwrap()["accepted"],
        false
    );
    restarted.shutdown().await.unwrap();
}

fn worker(fixture: &Fixture) -> (AgentExecution, FileBackedAgentRuntimeRegistry) {
    let registry = FileBackedAgentRuntimeRegistry::new(fixture.root.path().join("agents.json"));
    registry.initialize().unwrap();
    let projects = FileBackedProjectRegistry::new(fixture.root.path().join("projects.json"));
    projects.initialize().unwrap();
    let workspaces = FileBackedWorkspaceRegistry::new(fixture.root.path().join("workspaces.json"));
    workspaces.initialize().unwrap();
    projects.upsert(&serde_json::from_value(json!({
        "projectId":"project-1","rootPath":fixture.cwd,"kind":"non_git","displayName":"project",
        "createdAt":"2026-09-24T00:00:00Z","updatedAt":"2026-09-24T00:00:00Z","archivedAt":null
    })).unwrap()).unwrap();
    workspaces
        .upsert(
            &serde_json::from_value(json!({
                "workspaceId":"wks_0123456789abcdef","projectId":"project-1","cwd":fixture.cwd,
                "kind":"local_checkout","displayName":"main","createdAt":"2026-09-24T00:00:00Z",
                "updatedAt":"2026-09-24T00:00:00Z","archivedAt":null
            }))
            .unwrap(),
            server_metadata::ports::registry::WorkspaceMutationContext::default(),
        )
        .unwrap();
    let mut manager = AgentManager::new(Box::new(registry.clone())).with_timeline(
        crate::storage::timeline::Timeline::open(&fixture.root.path().join("timeline.sqlite"))
            .unwrap(),
    );
    manager.register_client(Box::new(fixture.client())).unwrap();
    let worker = AgentExecution::spawn(ExecutionDependencies {
        manager,
        directory: AgentRuntimeDirectory::new(
            Box::new(registry.clone()),
            Box::new(workspaces.clone()),
            Box::new(projects.clone()),
        ),
        registry: Box::new(registry.clone()),
        workspaces: Box::new(workspaces),
        lifetime: Arc::new(()),
        import_directory: None,
        projects: Box::new(projects),
    })
    .unwrap();
    (worker, registry)
}

async fn create(worker: &AgentExecution, fixture: &Fixture) -> Value {
    worker
        .execute(
            "agent.create.request",
            json!({"config":{
                "provider":"codex","cwd":fixture.cwd,"title":"Native test"
            }}),
        )
        .await
        .unwrap()
}

#[tokio::test]
async fn worker_runs_cancels_waits_and_restores_durable_native_identity() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    let handle = created["agent"]["persistence"].clone();
    assert_eq!(created["agent"]["providerUnavailable"], false);
    assert_eq!(
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"hello"})
            )
            .await
            .unwrap()["accepted"],
        true
    );
    let finished = execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["status"], "idle");
    assert_eq!(finished["lastMessage"], "Echo: hello");
    assert!(registry.get(id).unwrap().unwrap().requires_attention);
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"hang"}),
        )
        .await
        .unwrap();
    assert_eq!(
        execution
            .execute("internal.voice.send", json!({"agentId":id,"text":"busy"}))
            .await
            .unwrap()["accepted"],
        false
    );
    assert_eq!(
        execution
            .execute(
                "agent.finish.wait.request",
                json!({"agentId":id,"timeoutMs":10})
            )
            .await
            .unwrap()["status"],
        "timeout"
    );
    let waiter = execution.clone();
    let wait_id = id.to_owned();
    let task = tokio::spawn(async move {
        waiter
            .execute("agent.finish.wait.request", json!({"agentId":wait_id}))
            .await
    });
    execution
        .execute("agent.cancel.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(task.await.unwrap().unwrap()["status"], "idle");
    assert!(!registry.get(id).unwrap().unwrap().requires_attention);
    execution
        .execute(
            "agent.update.request",
            json!({"agentId":id,"name":"Renamed","labels":{"latest":"yes"}}),
        )
        .await
        .unwrap();
    execution.shutdown().await.unwrap();
    assert_eq!(
        registry.get(id).unwrap().unwrap().title.as_deref(),
        Some("Renamed")
    );
    let (restarted, _) = worker(&fixture);
    let resumed = restarted
        .execute("agent.resume.request", json!({"handle":handle}))
        .await
        .unwrap();
    assert_eq!(resumed["agentId"], id);
    assert_eq!(resumed["agent"]["title"], "Renamed");
    assert_eq!(resumed["agent"]["labels"]["latest"], "yes");
    restarted
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"after restart"}),
        )
        .await
        .unwrap();
    assert_eq!(
        restarted
            .execute("agent.finish.wait.request", json!({"agentId":id}))
            .await
            .unwrap()["lastMessage"],
        "Echo: after restart"
    );
    restarted.shutdown().await.unwrap();
    assert_eq!(registry.list().unwrap().len(), 1);
}

#[tokio::test]
async fn archive_delete_and_history_resume_never_leave_a_writer_or_resurrect_records() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"hang"}),
        )
        .await
        .unwrap();
    execution
        .execute("agent.archive.request", json!({"agentId":id}))
        .await
        .unwrap();
    let archived = registry.get(id).unwrap().unwrap();
    assert!(archived.archived_at.is_some());
    execution
        .execute(
            "agent.resume.request",
            json!({"handle":created["agent"]["persistence"]}),
        )
        .await
        .unwrap();
    let rejected = execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"no"}),
        )
        .await
        .unwrap();
    assert_eq!(rejected["accepted"], false);
    assert_eq!(fixture.requests().last().unwrap()["method"], "thread/read");
    execution
        .execute("agent.delete.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert!(registry.get(id).unwrap().is_none());
    execution.shutdown().await.unwrap();
    assert!(registry.get(id).unwrap().is_none());
}

#[tokio::test]
async fn failures_are_durable_and_a_new_turn_can_resume_after_native_exit() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    for prompt in ["fail", "approval", "exit"] {
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":prompt}),
            )
            .await
            .unwrap();
        let result = execution
            .execute("agent.finish.wait.request", json!({"agentId":id}))
            .await
            .unwrap();
        assert_eq!(result["status"], "error", "{prompt}");
        assert!(registry.get(id).unwrap().unwrap().last_error.is_some());
    }
    execution
        .execute(
            "agent.message.send.request",
            json!({"agentId":id,"text":"recovered"}),
        )
        .await
        .unwrap();
    assert_eq!(
        execution
            .execute("agent.finish.wait.request", json!({"agentId":id}))
            .await
            .unwrap()["lastMessage"],
        "Echo: recovered"
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn request_validation_rejects_unimplemented_creation_and_message_semantics_before_launch() {
    let fixture = Fixture::new();
    let (execution, registry) = worker(&fixture);
    for params in [
        json!({"config":{"provider":"other","cwd":fixture.cwd}}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd,"modeId":"invalid"}}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd},"initialPrompt":" "}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd},"workspaceId":"missing"}),
        json!({"config":{"provider":"codex","cwd":"relative"}}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd,"title":"x".repeat(201)}}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd,"title":"😀".repeat(101)}}),
        json!({"agentId":"invalid","config":{"provider":"codex","cwd":fixture.cwd}}),
    ] {
        assert!(
            execution
                .execute("agent.create.request", params)
                .await
                .is_err()
        );
    }
    assert!(registry.list().unwrap().is_empty());
    assert!(!fixture.cwd.join("native-requests.jsonl").exists());
    let created = create(&execution, &fixture).await;
    let id = created["agentId"].as_str().unwrap();
    for params in [
        json!({"agentId":id,"text":"x","unknownMessageField":"dedupe"}),
        json!({"agentId":id,"text":"x","unknownAttachments":[]}),
    ] {
        assert_eq!(
            execution
                .execute("agent.message.send.request", params)
                .await,
            Err(ErrorCode::UnsupportedCapability)
        );
    }
    assert_eq!(
        execution
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":" "})
            )
            .await
            .unwrap()["accepted"],
        false
    );
    assert!(
        execution
            .execute(
                "agent.finish.wait.request",
                json!({"agentId":id,"timeoutMs":0})
            )
            .await
            .is_err()
    );
    assert!(
        execution
            .execute(
                "agent.resume.request",
                json!({"handle":{"provider":"codex","sessionId":"missing"}})
            )
            .await
            .is_err()
    );
    assert_eq!(
        execution
            .execute("agent.cancel.request", json!({"agentId":id}))
            .await
            .unwrap()["error"],
        Value::Null
    );
    let unicode_title = format!("  {}  ", "😀".repeat(100));
    let unicode = execution
        .execute(
            "agent.create.request",
            json!({"config":{
                "provider":"codex","cwd":fixture.cwd,"title":unicode_title
            }}),
        )
        .await
        .unwrap();
    assert_eq!(unicode["agent"]["title"], "😀".repeat(100));
    let listed = execution
        .execute("agent.list.request", json!({}))
        .await
        .unwrap();
    assert_eq!(listed["entries"][0]["agent"]["providerUnavailable"], false);
    execution.shutdown().await.unwrap();
    execution.shutdown().await.unwrap();
    assert!(
        execution
            .execute("agent.get.request", json!({"agentId":id}))
            .await
            .is_err()
    );
}

mod async_questions;
mod controls;
mod delivery;
mod fork_context;
mod streaming;
mod subscriptions;
mod voice;

mod rich_input;
