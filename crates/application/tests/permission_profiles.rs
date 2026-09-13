//! Permission profiles regression coverage.
#![allow(clippy::pedantic)]

mod fixtures;
mod support;

use crate::fixtures::control_fixtures::{
    config, git_head, git_index_tree, ok, save_permission_settings, send, setup, view,
    wait_for_signal,
};
use crate::fixtures::pausing_store::PausingStore;
use crate::fixtures::provider_fixtures::Gateway;
use crate::fixtures::workspace_agents::CapturingWorkspaceAgent;
use crate::support::ControlStoreTestExt;
use ait_agent_adapters::codex::CodexWorkspaceAgent;
use ait_agent_adapters::{
    AdapterError, AgentAdapter, AgentCapabilities, AgentEvent, AgentRunRequest, AgentRunStatus,
    AgentStream,
};
use ait_application::{LocalControlService, PermissionPolicyLimits};
use ait_contracts::{
    AgentConfiguration, AgentMode, AgentProvider, Command, CommandResult, ProviderModel,
    ProviderSecret, default_settings,
};
use ait_domain::{
    ApprovalMode, ErrorCode, NativeApprovalKind, NativeApprovalTarget, RunPermissionProfile,
    SandboxAccess,
};
use ait_ports::{ControlStore, WorkspaceApprovalRequest};
use ait_storage_sqlite::SqliteControlStore;
use async_trait::async_trait;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Semaphore;

#[derive(Debug)]
struct ReadOnlyViolatingAdapter;

#[async_trait]
impl AgentAdapter for ReadOnlyViolatingAdapter {
    fn driver(&self) -> &'static str {
        "read_only_violation_test"
    }

    fn capabilities(&self) -> AgentCapabilities {
        AgentCapabilities {
            streaming: true,
            thread_resume: false,
            approvals: false,
            command_execution: true,
            file_changes: true,
            usage: false,
        }
    }

    async fn run(&self, request: AgentRunRequest) -> Result<AgentStream, AdapterError> {
        std::fs::write(request.cwd.join("unauthorized.txt"), "must not escape\n").unwrap();
        Ok(Box::pin(futures_util::stream::iter([
            Ok(AgentEvent::ItemCompleted {
                item: serde_json::json!({
                    "type": "agentMessage",
                    "id": "final",
                    "phase": "final_answer",
                    "text": "Wrote a file."
                }),
            }),
            Ok(AgentEvent::Completed {
                turn_id: "turn-read-only".into(),
                status: AgentRunStatus::Completed,
                error: None,
            }),
        ])))
    }
}

fn api_provider(kind: AgentMode) -> AgentProvider {
    let id = match kind {
        AgentMode::OpenAI => "api-openai",
        AgentMode::DeepSeek => "api-deepseek",
        _ => panic!("expected API provider"),
    };
    AgentProvider {
        id: id.into(),
        name: id.into(),
        kind,
        url: None,
        models: vec![ProviderModel {
            id: "chat".into(),
            name: "Chat".into(),
            reasoning_efforts: Vec::new(),
        }],
    }
}

async fn setup_api_provider(service: &LocalControlService, kind: AgentMode) -> tempfile::TempDir {
    let provider = api_provider(kind);
    let provider_id = provider.id.clone();
    ok(
        service,
        Command::SaveAgentProvider {
            provider,
            secret: Some(ProviderSecret("fixture-secret".into())),
        },
    )
    .await;
    setup(
        service,
        AgentConfiguration {
            provider_id,
            model: "chat".into(),
            reasoning_effort: None,
        },
    )
    .await
}

#[derive(Clone, Copy, Debug)]
enum ApiSessionBranch {
    Fork,
    DeriveReuse,
    DeriveFork,
}

impl ApiSessionBranch {
    async fn command(self, service: &LocalControlService) -> Command {
        let state = view(service).await;
        let agent_id = if matches!(self, Self::DeriveFork) {
            // Changing the requested Agent forces a real fork without a setup Run.
            let config = state
                .agents
                .iter()
                .find(|agent| agent.id == "preset")
                .unwrap()
                .config
                .clone();
            ok(
                service,
                Command::RegisterAgent {
                    id: "branch-agent".into(),
                    name: "Branch Agent".into(),
                    config,
                },
            )
            .await;
            "branch-agent"
        } else {
            "preset"
        };
        let at_message_id = state.projects[0].root_message_id.clone();
        match self {
            Self::Fork => Command::ForkSession {
                id: "branch".into(),
                project_id: "p".into(),
                agent_id: agent_id.into(),
                at_message_id,
                text: "branch input".into(),
            },
            Self::DeriveReuse | Self::DeriveFork => Command::DeriveSession {
                id: "branch".into(),
                project_id: "p".into(),
                source_session_id: "one".into(),
                agent_id: agent_id.into(),
                at_message_id,
                text: "branch input".into(),
            },
        }
    }
}

#[tokio::test]
async fn api_provider_branch_permission_settings_are_snapshotted_into_each_run() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        for branch in [
            ApiSessionBranch::Fork,
            ApiSessionBranch::DeriveReuse,
            ApiSessionBranch::DeriveFork,
        ] {
            for (setting, expected) in [
                ("workspace_write", SandboxAccess::WorkspaceWrite),
                ("full_access", SandboxAccess::FullAccess),
                ("read_only", SandboxAccess::ReadOnly),
                ("strict", SandboxAccess::ReadOnly),
            ] {
                let store = Arc::new(SqliteControlStore::in_memory().unwrap());
                let gateway = Arc::new(Gateway::default());
                let service =
                    LocalControlService::new(store).with_provider_gateway(gateway.clone());
                let _directory = setup_api_provider(&service, kind).await;
                let command = branch.command(&service).await;
                save_permission_settings(&service, setting, "untrusted_only").await;

                let CommandResult::Run(run) = ok(&service, command).await else {
                    panic!("expected Run")
                };
                let expected_profile = RunPermissionProfile {
                    sandbox: expected,
                    approval: ApprovalMode::OnRequest,
                };
                assert_eq!(
                    run.permission_profile, expected_profile,
                    "{kind:?}/{branch:?}/{setting}"
                );
                assert_eq!(run.status, "completed");
                assert_eq!(gateway.calls.lock().unwrap().len(), 1);
                let (session_id, session_count) = if matches!(branch, ApiSessionBranch::DeriveReuse)
                {
                    ("one", 2)
                } else {
                    ("branch", 3)
                };
                assert_eq!(run.session_id.as_deref(), Some(session_id));
                assert_eq!(view(&service).await.sessions.len(), session_count);

                ok(
                    &service,
                    Command::SaveSettings {
                        expected_revision: 2,
                        values: default_settings(),
                    },
                )
                .await;
                let CommandResult::Run(persisted) =
                    ok(&service, Command::GetRun { run_id: run.id }).await
                else {
                    panic!("expected persisted Run")
                };
                assert_eq!(persisted.permission_profile, expected_profile);
            }
        }
    }
}

#[tokio::test]
async fn api_provider_branch_invalid_permissions_have_no_side_effects() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        for branch in [
            ApiSessionBranch::Fork,
            ApiSessionBranch::DeriveReuse,
            ApiSessionBranch::DeriveFork,
        ] {
            for sandbox in [
                Some("workspace_write"),
                Some("full_access"),
                Some("unknown-policy"),
                None,
            ] {
                for asynchronous in [false, true] {
                    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
                    let gateway = Arc::new(Gateway::default());
                    let service = Arc::new(
                        LocalControlService::new(store.clone())
                            .with_provider_gateway(gateway.clone())
                            .with_permission_limits(PermissionPolicyLimits {
                                max_sandbox: SandboxAccess::ReadOnly,
                                allow_session_approvals: true,
                            }),
                    );
                    let _directory = setup_api_provider(&service, kind).await;
                    let command = branch.command(&service).await;
                    save_rejected_sandbox(&service, store.as_ref(), sandbox).await;
                    let before = view(&service).await;

                    let response = if asynchronous {
                        service.submit(command).await
                    } else {
                        service.execute(command).await
                    };
                    assert_eq!(
                        response.error.as_ref().map(|error| error.code),
                        Some(ErrorCode::InvalidConfiguration),
                        "{kind:?}/{branch:?}/{sandbox:?}/async={asynchronous}"
                    );
                    // Public reads prove neither new records nor source Session mutation escaped.
                    assert_eq!(view(&service).await, before);
                    assert!(gateway.calls.lock().unwrap().is_empty());
                }
            }
        }
    }
}

async fn save_rejected_sandbox(
    service: &LocalControlService,
    store: &dyn ControlStore,
    sandbox: Option<&str>,
) {
    if let Some(sandbox @ ("workspace_write" | "full_access")) = sandbox {
        save_permission_settings(service, sandbox, "on_request").await;
        return;
    }
    // Invalid values cannot be saved through SaveSettings; simulate persisted corruption.
    let snapshot = store.load().await.unwrap();
    let mut corrupted = snapshot.value;
    if let Some(sandbox) = sandbox {
        corrupted["settings"]["permissions.sandbox"] = serde_json::json!(sandbox);
    } else {
        corrupted["settings"]
            .as_object_mut()
            .unwrap()
            .remove("permissions.sandbox");
    }
    store
        .replace_state(snapshot.revision, corrupted, Vec::new())
        .await
        .unwrap();
}

#[tokio::test]
async fn api_provider_branch_rechecks_permissions_after_a_commit_conflict() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        for branch in [
            ApiSessionBranch::Fork,
            ApiSessionBranch::DeriveReuse,
            ApiSessionBranch::DeriveFork,
        ] {
            for sandbox in [
                Some("workspace_write"),
                Some("full_access"),
                Some("unknown-policy"),
                None,
            ] {
                let store = Arc::new(PausingStore {
                    inner: SqliteControlStore::in_memory().unwrap(),
                    entered: Semaphore::new(0),
                    release: Semaphore::new(0),
                });
                let gateway = Arc::new(Gateway::default());
                let service = Arc::new(
                    LocalControlService::new(store.clone())
                        .with_provider_gateway(gateway.clone())
                        .with_permission_limits(PermissionPolicyLimits {
                            max_sandbox: SandboxAccess::ReadOnly,
                            allow_session_approvals: true,
                        }),
                );
                let _directory = setup_api_provider(&service, kind).await;
                let command = branch.command(&service).await;
                let before = view(&service).await;
                let pending = {
                    let service = service.clone();
                    tokio::spawn(async move { service.execute(command).await })
                };
                wait_for_signal(&store.entered).await;
                // Admission used valid read_only, but Settings change before the CAS commit.
                save_rejected_sandbox(&service, store.as_ref(), sandbox).await;
                assert_eq!(view(&service).await, before);
                assert!(gateway.calls.lock().unwrap().is_empty());
                store.release.add_permits(1);
                let response = tokio::time::timeout(Duration::from_secs(3), pending)
                    .await
                    .unwrap()
                    .unwrap();

                assert_eq!(
                    response.error.as_ref().map(|error| error.code),
                    Some(ErrorCode::InvalidConfiguration),
                    "{kind:?}/{branch:?}/{sandbox:?}"
                );
                assert_eq!(view(&service).await, before);
                assert!(gateway.calls.lock().unwrap().is_empty());
            }
        }
    }
}

#[tokio::test]
async fn api_provider_permission_settings_are_snapshotted_into_each_run() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        for (setting, expected) in [
            ("read_only", SandboxAccess::ReadOnly),
            ("strict", SandboxAccess::ReadOnly),
            ("workspace_write", SandboxAccess::WorkspaceWrite),
            ("full_access", SandboxAccess::FullAccess),
        ] {
            let store = Arc::new(SqliteControlStore::in_memory().unwrap());
            let gateway = Arc::new(Gateway::default());
            let service = LocalControlService::new(store).with_provider_gateway(gateway.clone());
            let _directory = setup_api_provider(&service, kind).await;
            save_permission_settings(&service, setting, "untrusted_only").await;

            let CommandResult::Run(run) = ok(&service, send("one")).await else {
                panic!("expected Run")
            };
            assert_eq!(run.permission_profile.sandbox, expected);
            assert_eq!(run.permission_profile.approval, ApprovalMode::OnRequest);
            assert_eq!(gateway.calls.lock().unwrap().len(), 1);

            let mut changed = default_settings();
            changed
                .0
                .insert("permissions.sandbox".into(), serde_json::json!("read_only"));
            let _ = ok(
                &service,
                Command::SaveSettings {
                    expected_revision: 2,
                    values: changed,
                },
            )
            .await;
            let persisted = view(&service)
                .await
                .runs
                .into_iter()
                .find(|candidate| candidate.id == run.id)
                .unwrap();
            assert_eq!(persisted.permission_profile.sandbox, expected);
        }
    }
}

#[tokio::test]
async fn api_provider_permission_ceiling_fails_before_message_or_remote_call() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let gateway = Arc::new(Gateway::default());
        let service = LocalControlService::new(store)
            .with_provider_gateway(gateway.clone())
            .with_permission_limits(PermissionPolicyLimits {
                max_sandbox: SandboxAccess::ReadOnly,
                allow_session_approvals: true,
            });
        let _directory = setup_api_provider(&service, kind).await;
        save_permission_settings(&service, "workspace_write", "on_request").await;

        let rejected = service.execute(send("one")).await;
        assert_eq!(
            rejected.error.unwrap().code,
            ErrorCode::InvalidConfiguration
        );
        assert_eq!(view(&service).await.messages.len(), 1);
        assert!(gateway.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn api_provider_invalid_permission_setting_fails_before_message_or_remote_call() {
    for kind in [AgentMode::OpenAI, AgentMode::DeepSeek] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let gateway = Arc::new(Gateway::default());
        let service =
            LocalControlService::new(store.clone()).with_provider_gateway(gateway.clone());
        let _directory = setup_api_provider(&service, kind).await;

        let snapshot = store.load().await.unwrap();
        let mut corrupted = snapshot.value;
        corrupted["settings"]["permissions.sandbox"] = serde_json::json!("unknown-policy");
        store
            .replace_state(snapshot.revision, corrupted, Vec::new())
            .await
            .unwrap();
        let rejected = service.execute(send("one")).await;
        assert_eq!(
            rejected.error.unwrap().code,
            ErrorCode::InvalidConfiguration
        );

        let snapshot = store.load().await.unwrap();
        let mut corrupted = snapshot.value;
        corrupted["settings"]
            .as_object_mut()
            .unwrap()
            .remove("permissions.sandbox");
        store
            .replace_state(snapshot.revision, corrupted, Vec::new())
            .await
            .unwrap();
        let rejected = service.execute(send("one")).await;
        assert_eq!(
            rejected.error.unwrap().code,
            ErrorCode::InvalidConfiguration
        );
        assert_eq!(view(&service).await.messages.len(), 1);
        assert!(gateway.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn codex_permission_settings_are_snapshotted_into_each_run_and_native_invocation() {
    for (setting, expected) in [
        ("read_only", SandboxAccess::ReadOnly),
        ("strict", SandboxAccess::ReadOnly),
        ("workspace_write", SandboxAccess::WorkspaceWrite),
        ("full_access", SandboxAccess::FullAccess),
    ] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let native = Arc::new(CapturingWorkspaceAgent::default());
        let service = LocalControlService::with_workspace_agent(store, native.clone());
        let _directory = setup(&service, config("high")).await;
        save_permission_settings(&service, setting, "untrusted_only").await;
        let CommandResult::Run(run) = ok(&service, send("one")).await else {
            panic!("expected Run")
        };
        let expected_profile = RunPermissionProfile {
            sandbox: expected,
            approval: ApprovalMode::UntrustedOnly,
        };
        assert_eq!(run.permission_profile, expected_profile);
        assert_eq!(
            native.0.lock().unwrap()[0].permission_profile,
            expected_profile
        );
        let session_workdir = view(&service).await.sessions[0].workdir.clone();
        assert_eq!(
            native.0.lock().unwrap()[0].cwd,
            PathBuf::from(session_workdir)
        );

        let mut changed = default_settings();
        changed
            .0
            .insert("permissions.sandbox".into(), serde_json::json!("read_only"));
        let _ = ok(
            &service,
            Command::SaveSettings {
                expected_revision: 2,
                values: changed,
            },
        )
        .await;
        let persisted = view(&service)
            .await
            .runs
            .into_iter()
            .find(|candidate| candidate.id == run.id)
            .unwrap();
        assert_eq!(persisted.permission_profile, expected_profile);
    }
}

#[tokio::test]
async fn fresh_and_reset_settings_are_read_only_while_explicit_write_survives_restart() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let native = Arc::new(CapturingWorkspaceAgent::default());
    let service = LocalControlService::with_workspace_agent(store.clone(), native.clone());
    let _directory = setup(&service, config("high")).await;
    let CommandResult::Run(fresh) = ok(&service, send("one")).await else {
        panic!("expected Run")
    };
    assert_eq!(fresh.permission_profile.sandbox, SandboxAccess::ReadOnly);
    save_permission_settings(&service, "workspace_write", "on_request").await;
    drop(service);

    let restarted = LocalControlService::with_workspace_agent(store, native.clone());
    let CommandResult::Run(explicit) = ok(&restarted, send("two")).await else {
        panic!("expected Run")
    };
    assert_eq!(
        explicit.permission_profile.sandbox,
        SandboxAccess::WorkspaceWrite
    );
    let _ = ok(&restarted, Command::ResetSettings).await;
    let CommandResult::Run(reset) = ok(&restarted, send("one")).await else {
        panic!("expected Run")
    };
    assert_eq!(reset.permission_profile.sandbox, SandboxAccess::ReadOnly);
    assert_eq!(
        native
            .0
            .lock()
            .unwrap()
            .iter()
            .map(|call| call.permission_profile.sandbox)
            .collect::<Vec<_>>(),
        vec![
            SandboxAccess::ReadOnly,
            SandboxAccess::WorkspaceWrite,
            SandboxAccess::ReadOnly,
        ]
    );
}

#[tokio::test]
async fn read_only_protocol_write_attempt_fails_run_without_project_or_message_side_effects() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let workspace_agent = Arc::new(CodexWorkspaceAgent::new(Arc::new(ReadOnlyViolatingAdapter)));
    let service = LocalControlService::with_workspace_agent(store, workspace_agent);
    let directory = setup(&service, config("high")).await;
    let baseline = git_head(directory.path());
    let baseline_index = git_index_tree(directory.path());
    let CommandResult::Run(run) = ok(&service, send("one")).await else {
        panic!("expected Run")
    };

    assert_eq!(run.permission_profile.sandbox, SandboxAccess::ReadOnly);
    assert_eq!(run.status, "failed");
    assert_eq!(run.error.unwrap().code, ErrorCode::ProjectGitDirty);
    assert_eq!(git_head(directory.path()), baseline);
    assert_eq!(git_index_tree(directory.path()), baseline_index);
    assert!(!directory.path().join("unauthorized.txt").exists());
    let snapshot = view(&service).await;
    assert!(
        snapshot
            .messages
            .iter()
            .all(|message| message.role != "assistant")
    );
    assert_eq!(
        snapshot
            .messages
            .iter()
            .filter(|message| message.role == "user")
            .count(),
        1
    );
}

#[tokio::test]
async fn unsupported_or_administrator_conflicting_policies_fail_before_messages_or_agents() {
    let store = Arc::new(SqliteControlStore::in_memory().unwrap());
    let native = Arc::new(CapturingWorkspaceAgent::default());
    let service = LocalControlService::with_workspace_agent(store.clone(), native.clone())
        .with_permission_limits(PermissionPolicyLimits {
            max_sandbox: SandboxAccess::ReadOnly,
            allow_session_approvals: true,
        });
    let _directory = setup(&service, config("high")).await;
    save_permission_settings(&service, "full_access", "on_request").await;
    let rejected = service.execute(send("one")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());

    let mut always = default_settings();
    always
        .0
        .insert("permissions.sandbox".into(), serde_json::json!("read_only"));
    always
        .0
        .insert("permissions.approval".into(), serde_json::json!("always"));
    let _ = ok(
        &service,
        Command::SaveSettings {
            expected_revision: 2,
            values: always,
        },
    )
    .await;
    let rejected = service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());

    let snapshot = store.load().await.unwrap();
    let mut corrupted = snapshot.value;
    corrupted["settings"]["permissions.sandbox"] = serde_json::json!("unknown-policy");
    store
        .replace_state(snapshot.revision, corrupted, Vec::new())
        .await
        .unwrap();
    let rejected = service.execute(send("two")).await;
    assert_eq!(
        rejected.error.unwrap().code,
        ErrorCode::InvalidConfiguration
    );
    assert_eq!(view(&service).await.messages.len(), 1);
    assert!(native.0.lock().unwrap().is_empty());
}

#[tokio::test]
async fn invalid_permission_profiles_never_echo_input_in_errors() {
    use ait_ports::WorkspaceApproval as _;
    let service = LocalControlService::new(Arc::new(SqliteControlStore::in_memory().unwrap()));
    for permissions in [
        serde_json::json!({"network": {"enabled": "fixture-secret"}}),
        serde_json::json!({"fixture-secret": true}),
        serde_json::json!({"fileSystem": {"entries": [{"path": {"type": "path", "path": "/workspace"}, "access": "fixture-secret"}]}}),
    ] {
        let failure = service
            .decide(WorkspaceApprovalRequest {
                run_id: "run".into(),
                protocol_request_id: serde_json::json!(73),
                method: "item/permissions/requestApproval".into(),
                kind: NativeApprovalKind::Permissions,
                thread_id: "thread".into(),
                turn_id: "turn".into(),
                item_id: "item".into(),
                target: NativeApprovalTarget::Permissions {
                    cwd: "/workspace".into(),
                },
                requested_permissions: Some(permissions),
            })
            .await
            .unwrap_err();
        assert_eq!(failure.code, ErrorCode::ToolApprovalRequired);
        assert!(!format!("{failure:?}").contains("fixture-secret"));
    }
}

#[tokio::test]
async fn corrupt_permission_settings_fail_closed_without_echoing_values() {
    for key in ["permissions.sandbox", "permissions.approval"] {
        let store = Arc::new(SqliteControlStore::in_memory().unwrap());
        let native = Arc::new(CapturingWorkspaceAgent::default());
        let service = LocalControlService::with_workspace_agent(store.clone(), native.clone());
        let _directory = setup(&service, config("high")).await;
        let snapshot = store.load().await.unwrap();
        let mut corrupted = snapshot.value;
        corrupted["settings"][key] = serde_json::json!("fixture-secret");
        store
            .replace_state(snapshot.revision, corrupted, Vec::new())
            .await
            .unwrap();
        let rejected = service.execute(send("one")).await;
        assert_eq!(
            rejected.error.as_ref().unwrap().code,
            ErrorCode::InvalidConfiguration
        );
        assert!(!format!("{rejected:?}").contains("fixture-secret"));
        assert_eq!(view(&service).await.messages.len(), 1);
        assert!(view(&service).await.runs.is_empty());
        assert!(native.0.lock().unwrap().is_empty());
    }
}
