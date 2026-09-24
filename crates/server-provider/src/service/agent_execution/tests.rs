use super::*;
use crate::ports::agent_runtime::AgentRuntimeRegistry;
use crate::storage::agent_runtime::FileBackedAgentRuntimeRegistry;
use crate::test_support::Fixture;
use server_metadata::ports::registry::{ProjectRegistry, WorkspaceRegistry};
use server_metadata::storage::registry::{FileBackedProjectRegistry, FileBackedWorkspaceRegistry};

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
    let mut manager = AgentManager::new(Box::new(registry.clone()));
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
            .execute(
                "agent.message.send.request",
                json!({"agentId":id,"text":"busy"})
            )
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
        json!({"config":{"provider":"codex","cwd":fixture.cwd,"modeId":"auto"}}),
        json!({"config":{"provider":"codex","cwd":fixture.cwd},"initialPrompt":"not silently lost"}),
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
        json!({"agentId":id,"text":"x","messageId":"dedupe"}),
        json!({"agentId":id,"text":"x","attachments":[]}),
        json!({"agentId":id,"text":"x","activeTurnBehavior":"queue"}),
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
