//! Workspace admission regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{
    config, git_head, git_index_tree, ok, send, send_text, setup, view,
};
use crate::fixtures::pausing_store::PausingStore;
use crate::fixtures::workspace_agents::BlockingAgent;
use ait_application::LocalControlService;
use ait_contracts::{Command, CommandResult};
use ait_domain::{DomainError, ErrorCode};
use ait_ports::{WorkspaceAgent, WorkspaceAgentInvocation, WorkspaceAgentResponse};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::Semaphore;

struct CommittingAgent {
    entered: Semaphore,
    release: Semaphore,
    commits: Mutex<Vec<(String, String, String)>>,
}

impl CommittingAgent {
    fn new() -> Self {
        Self {
            entered: Semaphore::new(0),
            release: Semaphore::new(0),
            commits: Mutex::default(),
        }
    }

    async fn started(&self) {
        self.entered.acquire().await.unwrap().forget();
    }
}

#[async_trait]
impl WorkspaceAgent for CommittingAgent {
    async fn invoke(
        &self,
        request: WorkspaceAgentInvocation,
    ) -> Result<WorkspaceAgentResponse, DomainError> {
        assert_eq!(git_head(&request.cwd), request.baseline_commit);
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        let file = if request.commit_subject == "first change" {
            "first.txt"
        } else {
            "second.txt"
        };
        std::fs::write(request.cwd.join(file), format!("{file}\n")).unwrap();
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args(["add", "--", file])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            std::process::Command::new("git")
                .arg("-C")
                .arg(&request.cwd)
                .args([
                    "-c",
                    "user.name=Test Agent",
                    "-c",
                    "user.email=test-agent@example.invalid",
                    "commit",
                    "-m",
                    &request.commit_subject,
                ])
                .status()
                .unwrap()
                .success()
        );
        let commit = git_head(&request.cwd);
        self.commits
            .lock()
            .unwrap()
            .push((request.baseline_commit, commit.clone(), file.into()));
        Ok(WorkspaceAgentResponse {
            assistant_text: format!("created {file}"),
            commit_id: Some(commit),
            operations: Vec::new(),
            output_items: Vec::new(),
        })
    }
}

#[tokio::test]
async fn active_session_rejects_competitors_and_same_project_writers_are_serialized() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let directory = setup(&service, config("high")).await;
    let running = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;
    let before = view(&service).await;
    // Busy admission precedes even the clean-Git prerequisite.
    std::fs::write(directory.path().join("dirty"), "temporary").unwrap();
    for command in [
        send("one"),
        Command::SetSessionConfig {
            session_id: "one".into(),
            config: config("low"),
        },
        Command::SetSessionAgent {
            session_id: "one".into(),
            agent_id: "preset".into(),
        },
    ] {
        let response = tokio::time::timeout(Duration::from_millis(500), service.execute(command))
            .await
            .unwrap();
        assert_eq!(response.error.unwrap().code, ErrorCode::SessionBusy);
    }
    assert_eq!(view(&service).await, before);
    std::fs::remove_file(directory.path().join("dirty")).unwrap();
    // Changing a shared preset cannot alter the already pinned Run.
    ok(
        &service,
        Command::UpdateAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: config("low"),
        },
    )
    .await;
    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err(),
        "the second Session must wait for the same Project write lease"
    );
    agent.release.add_permits(1);
    let CommandResult::Run(first_run) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(first_run.status, "completed");
    agent.started().await;
    assert_eq!(view(&service).await.runs.len(), 2);
    assert_eq!(
        *agent.requests.lock().unwrap(),
        vec![
            ("gpt-5.6-sol".into(), Some("high".into())),
            ("gpt-5.6-sol".into(), Some("low".into()))
        ]
    );
    agent.release.add_permits(1);
    let CommandResult::Run(second_run) = second.await.unwrap() else {
        panic!()
    };
    assert_eq!(second_run.status, "completed");
    let finished = view(&service).await;
    assert!(
        finished
            .sessions
            .iter()
            .all(|session| session.active_run_id.is_none())
    );
    assert_eq!(
        finished
            .runs
            .iter()
            .find(|run| run.id == first_run.id)
            .unwrap()
            .config,
        config("high")
    );
    assert_eq!(
        finished
            .runs
            .iter()
            .find(|run| run.id == second_run.id)
            .unwrap()
            .config,
        config("low")
    );
    assert_eq!(
        finished
            .messages
            .iter()
            .filter(|m| m.role == "user")
            .count(),
        2
    );
}

#[tokio::test]
async fn codex_writers_for_unrelated_projects_enter_concurrently() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    for (project_id, directory) in [("p1", &first), ("p2", &second)] {
        ok(
            &service,
            Command::RegisterProject {
                id: project_id.into(),
                name: project_id.into(),
                workdir: Some(directory.path().display().to_string()),
                repo_url: None,
            },
        )
        .await;
    }
    ok(
        &service,
        Command::RegisterAgent {
            id: "preset".into(),
            name: "Shared".into(),
            config: config("high"),
        },
    )
    .await;
    for (session_id, project_id) in [("one", "p1"), ("two", "p2")] {
        ok(
            &service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: project_id.into(),
                agent_id: "preset".into(),
                at_message_id: None,
            },
        )
        .await;
    }

    let one = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    let two = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("two")).await })
    };
    agent.started().await;
    agent.started().await;
    assert_eq!(agent.requests.lock().unwrap().len(), 2);
    agent.release.add_permits(2);
    for task in [one, two] {
        let CommandResult::Run(run) = task.await.unwrap() else {
            panic!()
        };
        assert_eq!(run.status, "completed");
    }
}

#[tokio::test]
async fn a_second_service_cannot_bypass_the_process_wide_workspace_lease() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(BlockingAgent::new());
    let first_service = Arc::new(LocalControlService::with_workspace_agent(
        store.clone(),
        agent.clone(),
    ));
    let second_service = LocalControlService::with_workspace_agent(store, agent.clone());
    let _directory = setup(&first_service, config("high")).await;
    let running = {
        let service = first_service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;

    let rejected = second_service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::ProjectWorkspaceBusy
    );
    assert_eq!(view(&first_service).await.runs.len(), 1);

    agent.release.add_permits(1);
    let CommandResult::Run(finished) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(finished.status, "completed");
    let retried = {
        let service = Arc::new(second_service);
        let task_service = service.clone();
        let task = tokio::spawn(async move { ok(&task_service, send("two")).await });
        agent.started().await;
        agent.release.add_permits(1);
        task.await.unwrap()
    };
    let CommandResult::Run(retried) = retried else {
        panic!()
    };
    assert_eq!(retried.status, "completed");
}

#[cfg(unix)]
#[tokio::test]
async fn canonical_path_aliases_share_the_same_process_wide_lease() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("project");
    let alias = root.path().join("project-alias");
    std::fs::create_dir(&project).unwrap();
    std::os::unix::fs::symlink(&project, &alias).unwrap();
    let agent = Arc::new(BlockingAgent::new());
    let first_service = Arc::new(LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        agent.clone(),
    ));
    let second_service = LocalControlService::with_workspace_agent(
        Arc::new(SqliteControlStore::in_memory().unwrap()),
        agent.clone(),
    );
    for (service, workdir, session_id) in [
        (first_service.as_ref(), project.as_path(), "one"),
        (&second_service, alias.as_path(), "two"),
    ] {
        ok(
            service,
            Command::RegisterProject {
                id: "p".into(),
                name: "Project".into(),
                workdir: Some(workdir.display().to_string()),
                repo_url: None,
            },
        )
        .await;
        ok(
            service,
            Command::RegisterAgent {
                id: "preset".into(),
                name: "Shared".into(),
                config: config("high"),
            },
        )
        .await;
        ok(
            service,
            Command::CreateSession {
                id: session_id.into(),
                project_id: "p".into(),
                agent_id: "preset".into(),
                at_message_id: None,
            },
        )
        .await;
    }
    let running = {
        let service = first_service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    agent.started().await;

    let rejected = second_service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::ProjectWorkspaceBusy
    );
    agent.release.add_permits(1);
    let CommandResult::Run(finished) = running.await.unwrap() else {
        panic!()
    };
    assert_eq!(finished.status, "completed");
}

#[tokio::test]
async fn serialized_session_worktrees_keep_independent_baselines_and_commits() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let agent = Arc::new(CommittingAgent::new());
    let service = Arc::new(LocalControlService::with_workspace_agent(
        store,
        agent.clone(),
    ));
    let directory = setup(&service, config("high")).await;
    let initial = git_head(directory.path());
    let initial_tree = git_index_tree(directory.path());

    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send_text("one", "first change")).await })
    };
    agent.started().await;
    let second = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send_text("two", "second change")).await })
    };
    assert!(
        tokio::time::timeout(Duration::from_millis(100), agent.entered.acquire())
            .await
            .is_err()
    );
    agent.release.add_permits(1);
    let CommandResult::Run(first_run) = first.await.unwrap() else {
        panic!()
    };
    agent.started().await;
    agent.release.add_permits(1);
    let CommandResult::Run(second_run) = second.await.unwrap() else {
        panic!()
    };

    let commits = agent.commits.lock().unwrap().clone();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0].0, initial);
    assert_eq!(commits[1].0, initial);
    assert_eq!(first_run.status, "completed");
    assert_eq!(second_run.status, "completed");
    assert_eq!(
        first_run.workspace_base_commit.as_deref(),
        Some(initial.as_str())
    );
    assert_eq!(
        first_run.workspace_base_index_tree.as_deref(),
        Some(initial_tree.as_str())
    );
    assert_eq!(
        second_run.workspace_base_commit.as_deref(),
        Some(initial.as_str())
    );
    assert_eq!(
        second_run.workspace_base_index_tree.as_deref(),
        Some(initial_tree.as_str())
    );
    assert_ne!(first_run.id, second_run.id);
    for (baseline, commit, file) in &commits {
        assert_ne!(baseline, commit);
        let output = std::process::Command::new("git")
            .arg("-C")
            .arg(directory.path())
            .args(["show", "--pretty=format:", "--name-only", commit])
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap().trim(), file);
    }
    let workspace = view(&service).await;
    let message_commit = |id: &str| {
        workspace
            .messages
            .iter()
            .find(|message| message.id == id)
            .unwrap()
            .git_commit
            .clone()
            .unwrap()
    };
    assert_eq!(message_commit(&first_run.base_message_id), initial);
    assert_eq!(message_commit(&second_run.base_message_id), initial);
    assert_eq!(git_head(directory.path()), initial);
    let session = |id: &str| {
        workspace
            .sessions
            .iter()
            .find(|session| session.id == id)
            .unwrap()
    };
    assert_eq!(git_head(Path::new(&session("one").workdir)), commits[0].1);
    assert_eq!(git_head(Path::new(&session("two").workdir)), commits[1].1);
    assert!(
        Path::new(&session("one").workdir)
            .join("first.txt")
            .exists()
    );
    assert!(
        !Path::new(&session("one").workdir)
            .join("second.txt")
            .exists()
    );
    assert!(
        Path::new(&session("two").workdir)
            .join("second.txt")
            .exists()
    );
    assert!(
        !Path::new(&session("two").workdir)
            .join("first.txt")
            .exists()
    );
}

#[tokio::test]
async fn pessimistic_admission_rejects_a_competing_send_before_the_first_run_is_committed() {
    let store = Arc::new(PausingStore {
        inner: SqliteControlStore::in_memory().unwrap(),
        entered: Semaphore::new(0),
        release: Semaphore::new(0),
    });
    let service = Arc::new(LocalControlService::new(store.clone()));
    let _directory = setup(&service, config("high")).await;
    let first = {
        let service = service.clone();
        tokio::spawn(async move { ok(&service, send("one")).await })
    };
    tokio::time::timeout(Duration::from_secs(3), store.entered.acquire())
        .await
        .unwrap()
        .unwrap()
        .forget();
    assert!(view(&service).await.runs.is_empty());
    let rejected = tokio::time::timeout(Duration::from_millis(500), service.execute(send("one")))
        .await
        .unwrap();
    assert_eq!(rejected.error.unwrap().code, ErrorCode::SessionBusy);
    assert_eq!(view(&service).await.messages.len(), 1);
    store.release.add_permits(1);
    first.await.unwrap();
    assert_eq!(view(&service).await.runs.len(), 1);
}
