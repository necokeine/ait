//! User-facing acceptance flows indexed in workflows/README.md.

mod support;

use serde_json::{Value, json};
use support::{Workspace, entity, events, failure, success};

// WF-01: Register a real directory and choose an Agent before opening a Session.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One complete acceptance scenario with explicit CLI arguments.
async fn wf01_register_project_and_agent() {
    let mut workspace = Workspace::new().await;
    assert_eq!(workspace.view().await["projects"], json!([]));
    let project = workspace.project("project with spaces").await;
    assert_eq!(
        project["workdir"],
        workspace
            .path("project with spaces")
            .canonicalize()
            .unwrap()
            .to_str()
            .unwrap()
    );
    assert!(workspace.path("project with spaces/.git").is_dir());
    let head = tokio::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace.path("project with spaces"))
        .output()
        .await
        .unwrap();
    assert!(head.status.success());
    assert_eq!(
        project["base_commit"],
        String::from_utf8(head.stdout).unwrap().trim()
    );
    let before = workspace.view().await;
    workspace
        .reject(
            &[
                "project",
                "register",
                "--id",
                "duplicate",
                "--name",
                "Duplicate",
                "--workdir",
                workspace.path("project with spaces/.").to_str().unwrap(),
            ],
            "PROJECT_PATH_ALREADY_REGISTERED",
        )
        .await;
    workspace
        .reject(
            &[
                "project",
                "register",
                "--id",
                "missing",
                "--name",
                "Missing",
                "--workdir",
                workspace.path("missing").to_str().unwrap(),
            ],
            "PROJECT_PATH_NOT_FOUND",
        )
        .await;
    workspace
        .reject(
            &[
                "agent",
                "create",
                "--id",
                "bad",
                "--name",
                "",
                "--provider-id",
                "builtin-codex",
                "--model",
                "gpt-5.6-sol",
            ],
            "INVALID_AGENT_CONFIGURATION",
        )
        .await;
    assert_eq!(workspace.view().await, before);

    let agent = workspace.agent("primary").await;
    let default = workspace
        .call(&[
            "project",
            "set-default-agent",
            "--project-id",
            project["id"].as_str().unwrap(),
            "--agent-id",
            "primary",
        ])
        .await;
    assert_eq!(default["default_agent_id"], "primary");
    assert_eq!(
        default["revision"],
        project["revision"].as_u64().unwrap() + 1
    );
    let session = workspace
        .session("main", project["id"].as_str().unwrap(), "primary")
        .await;
    assert_eq!(session["current_message_id"], project["root_message_id"]);
    assert_eq!(session["agent_id"], agent["id"]);
    assert_eq!(session["version"], 1);
    assert_eq!(session["name"], "");
    assert_eq!(
        success(
            &workspace
                .cli(&[
                    "session",
                    "list",
                    "--project-id",
                    project["id"].as_str().unwrap(),
                ])
                .await,
        ),
        json!([session])
    );
    let snapshot = workspace.view().await;
    assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
    let root = entity(&snapshot, "messages", &project["root_message_id"]);
    assert_eq!(root["role"], "system");
    assert!(root["parent_message_id"].is_null());
    workspace.stop().await;
}

// WF-02: Reject unsafe input atomically, then observe the completed Agent reply.
#[tokio::test]
async fn wf02_send_message_and_inspect_agent_reply() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    let agent = workspace.agent("codex").await;
    workspace.session("main", "project", "codex").await;
    let before = workspace.view().await;
    workspace
        .reject(
            &[
                "session",
                "set-config",
                "--session-id",
                "main",
                "--provider-id",
                "builtin-codex",
                "--model",
                "gpt-5.6-sol",
                "--reasoning-effort",
                "unsupported",
            ],
            "INVALID_AGENT_CONFIGURATION",
        )
        .await;
    assert_eq!(workspace.view().await, before);
    let dirty = workspace.path("project/untracked.txt");
    std::fs::write(&dirty, "unsaved work").unwrap();
    workspace
        .reject(
            &[
                "session",
                "send",
                "--session-id",
                "main",
                "--text",
                "must not append",
            ],
            "PROJECT_GIT_DIRTY",
        )
        .await;
    assert_eq!(workspace.view().await, before);
    std::fs::remove_file(dirty).unwrap();

    let text = "检查工具输出：\"hello\"\n第二行";
    let run = workspace.send("main", 1, text).await;
    assert_eq!(run["status"], "completed");
    assert_eq!(run["agent_revision"], agent["revision"]);
    assert_eq!(run["session_id"], "main");
    assert!(run["error"].is_null());
    assert_eq!(
        workspace
            .call(&["run", "get", "--run-id", run["id"].as_str().unwrap()])
            .await,
        run
    );
    let snapshot = workspace.view().await;
    let messages = snapshot["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 3);
    let user = entity(&snapshot, "messages", &run["base_message_id"]);
    assert_eq!(user["text"], text);
    assert_eq!(user["git_commit"], project["base_commit"]);
    assert_eq!(user["parent_message_id"], project["root_message_id"]);
    let final_message = entity(&snapshot, "messages", &run["last_message_id"]);
    assert_eq!(final_message["parent_message_id"], user["id"]);
    assert_eq!(final_message["role"], "assistant");
    assert_eq!(final_message["text"], format!("Completed: {text}"));
    let session = entity(&snapshot, "sessions", &json!("main"));
    assert_eq!(session["current_message_id"], final_message["id"]);
    assert!(session["version"].as_u64().unwrap() > 1);
    assert!(session["active_run_id"].is_null());
    workspace.stop().await;
}

// WF-03: Branches share immutable history; names and Agent bindings have distinct effects.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One complete acceptance scenario with explicit CLI arguments.
async fn wf03_branch_rename_and_rebind_session() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    workspace.agent("primary").await;
    workspace.agent("alternate").await;
    workspace.session("main", "project", "primary").await;
    let run = workspace.send("main", 1, "original").await;
    let before = workspace.view().await;
    let original = entity(&before, "sessions", &json!("main")).clone();
    let branch = workspace
        .call(&[
            "session",
            "create",
            "--id",
            "branch",
            "--project-id",
            "project",
            "--agent-id",
            "primary",
            "--at-message-id",
            run["last_message_id"].as_str().unwrap(),
        ])
        .await;
    assert_eq!(branch["current_message_id"], run["last_message_id"]);
    assert_eq!(workspace.view().await["messages"], before["messages"]);
    let titled = workspace
        .call(&[
            "session",
            "set-title",
            "--session-id",
            "branch",
            "--title",
            "临时标题",
        ])
        .await;
    let renamed = workspace
        .call(&[
            "session",
            "rename",
            "--session-id",
            "branch",
            "--name",
            "  我的   分支  ",
        ])
        .await;
    assert_eq!(renamed["name"], "我的 分支");
    assert_eq!(renamed["title"], titled["title"]);
    assert_eq!(renamed["version"], branch["version"]);
    let rebound = workspace
        .call(&[
            "session",
            "set-agent",
            "--session-id",
            "branch",
            "--agent-id",
            "alternate",
        ])
        .await;
    assert_eq!(rebound["version"], 2);
    assert_eq!(rebound["current_message_id"], branch["current_message_id"]);
    let continued = workspace.send("branch", 2, "continue independently").await;
    assert_eq!(continued["agent_id"], "alternate");

    let fork = workspace
        .call(&[
            "session",
            "fork",
            "--id",
            "fork",
            "--project-id",
            "project",
            "--agent-id",
            "primary",
            "--at-message-id",
            project["root_message_id"].as_str().unwrap(),
            "--text",
            "new direction",
        ])
        .await;
    assert_eq!(fork["status"], "completed");
    let after = workspace.view().await;
    assert_eq!(entity(&after, "sessions", &json!("main")), &original);
    assert_eq!(
        entity(&after, "messages", &fork["base_message_id"])["parent_message_id"],
        project["root_message_id"]
    );
    for old_message in before["messages"].as_array().unwrap() {
        assert_eq!(entity(&after, "messages", &old_message["id"]), old_message);
    }
    let other = workspace.project("other").await;
    let before_rejection = workspace.view().await;
    workspace
        .reject(
            &[
                "session",
                "fork",
                "--id",
                "invalid-fork",
                "--project-id",
                "project",
                "--agent-id",
                "primary",
                "--at-message-id",
                other["root_message_id"].as_str().unwrap(),
                "--text",
                "cross-project input",
            ],
            "SESSION_MESSAGE_PROJECT_MISMATCH",
        )
        .await;
    assert_eq!(workspace.view().await, before_rejection);
    workspace.stop().await;
}

// WF-04: Command success and Run success are separate contracts.
#[tokio::test]
async fn wf04_observe_injected_provider_failure_and_continue() {
    let mut workspace = Workspace::new().await;
    workspace.project("project").await;
    workspace.agent("codex").await;
    workspace.session("main", "project", "codex").await;
    let failed = workspace.send("main", 1, "simulate provider failure").await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"]["code"], "PROVIDER_FAILED");
    assert_eq!(failed["error"]["retryable"], true);
    let snapshot = workspace.view().await;
    assert!(entity(&snapshot, "sessions", &json!("main"))["active_run_id"].is_null());
    assert_eq!(
        workspace
            .call(&["run", "get", "--run-id", failed["id"].as_str().unwrap()])
            .await,
        failed
    );
    let completed = workspace.send("main", 2, "continue").await;
    assert_eq!(completed["status"], "completed");
    workspace.stop().await;
}

// WF-05: An occurrence is idempotent and never moves an interactive Session.
#[tokio::test]
async fn wf05_cron_occurrence_is_idempotent_and_independent() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    workspace.agent("primary").await;
    workspace.session("main", "project", "primary").await;
    let sessions = workspace.view().await["sessions"].clone();
    workspace
        .call(&[
            "cron",
            "create",
            "--id",
            "daily",
            "--name",
            "Daily summary",
            "--project-id",
            "project",
            "--base-message-id",
            project["root_message_id"].as_str().unwrap(),
            "--agent-id",
            "primary",
            "--schedule",
            "0 9 * * *",
            "--timezone",
            "Asia/Shanghai",
        ])
        .await;
    workspace
        .call(&["cron", "disable", "--cron-id", "daily"])
        .await;
    let trigger = &[
        "cron",
        "trigger",
        "--cron-id",
        "daily",
        "--scheduled-at",
        "1788480000000",
    ];
    let disabled = workspace.view().await;
    workspace.reject(trigger, "INVALID_CRON").await;
    assert_eq!(workspace.view().await, disabled);
    workspace
        .call(&["cron", "enable", "--cron-id", "daily"])
        .await;
    let run = workspace.call(trigger).await;
    assert_eq!(run["base_message_id"], project["root_message_id"]);
    assert_eq!(run["trigger"], "cron");
    assert_eq!(run["status"], "completed");
    assert!(run["session_id"].is_null());
    let once = workspace.view().await;
    let event_count = events(&workspace.cli(&["event", "list"]).await).len();
    assert_eq!(workspace.call(trigger).await, run);
    assert_eq!(workspace.view().await, once);
    assert_eq!(
        events(&workspace.cli(&["event", "list"]).await).len(),
        event_count
    );
    let second = workspace
        .call(&[
            "cron",
            "trigger",
            "--cron-id",
            "daily",
            "--scheduled-at",
            "1788566400000",
        ])
        .await;
    assert_ne!(second["id"], run["id"]);
    let after = workspace.view().await;
    assert_eq!(after["sessions"], sessions);
    assert_eq!(after["runs"].as_array().unwrap().len(), 2);
    assert_eq!(after["messages"].as_array().unwrap().len(), 3);
    workspace.stop().await;
}

// WF-06: Resume SSE strictly after a cursor and reopen the durable database.
#[tokio::test]
async fn wf06_replay_events_and_reopen_workspace() {
    let mut workspace = Workspace::new().await;
    assert!(events(&workspace.cli(&["event", "list"]).await).is_empty());
    workspace.project("project").await;
    workspace.agent("primary").await;
    let first = events(&workspace.cli(&["event", "list"]).await);
    assert!(
        first
            .iter()
            .any(|(_, event)| event["kind"] == "project.registered")
    );
    let cursor = first.last().unwrap().0;
    workspace.session("main", "project", "primary").await;
    let run = workspace.send("main", 1, "persist this").await;
    let all = events(&workspace.cli(&["event", "list", "--after", "0"]).await);
    assert!(all.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let rest = events(
        &workspace
            .cli(&["event", "list", "--after", &cursor.to_string()])
            .await,
    );
    assert!(!rest.is_empty());
    assert!(rest.iter().all(|(next, _)| *next > cursor));
    assert_eq!([first, rest.clone()].concat(), all);
    let snapshot = workspace.view().await;
    workspace.restart().await;
    assert_eq!(workspace.view().await, snapshot);
    assert_eq!(
        workspace
            .call(&["run", "get", "--run-id", run["id"].as_str().unwrap()])
            .await,
        run
    );
    assert_eq!(
        events(
            &workspace
                .cli(&["event", "list", "--after", &cursor.to_string()])
                .await
        ),
        rest
    );
    assert!(
        events(
            &workspace
                .cli(&[
                    "event",
                    "list",
                    "--after",
                    &all.last().unwrap().0.to_string()
                ])
                .await
        )
        .is_empty()
    );
    workspace.stop().await;
}

// WF-07: Export a branch forest, import to a fresh workspace, and reject conflicts.
#[tokio::test]
#[allow(clippy::too_many_lines)] // One complete acceptance scenario with explicit CLI arguments.
async fn wf07_export_and_import_project_archive() {
    let mut source = Workspace::new().await;
    let project = source.project("project").await;
    source.agent("portable").await;
    source
        .call(&[
            "project",
            "set-default-agent",
            "--project-id",
            "project",
            "--agent-id",
            "portable",
        ])
        .await;
    source.session("main", "project", "portable").await;
    source.send("main", 1, "keep portable history").await;
    source
        .call(&[
            "session",
            "create",
            "--id",
            "branch",
            "--project-id",
            "project",
            "--agent-id",
            "portable",
            "--at-message-id",
            project["root_message_id"].as_str().unwrap(),
        ])
        .await;
    let output = source
        .cli(&[
            "project",
            "export",
            "--project-id",
            "project",
            "--output",
            "archive with spaces.json",
        ])
        .await;
    assert_eq!(output.status.code(), Some(0), "{output:?}");
    assert!(output.stdout.is_empty() && output.stderr.is_empty());
    let path = source.path("archive with spaces.json");
    let archive: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(archive["format_version"], 3);
    assert_eq!(archive["sessions"].as_array().unwrap().len(), 2);
    assert!(
        archive["sessions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|session| session["active_run_id"].is_null())
    );
    assert!(archive.get("runs").is_none() && archive.get("crons").is_none());
    let before = source.view().await;
    assert!(entity(&before, "sessions", &json!("main"))["active_run_id"].is_null());
    let contents = std::fs::read(&path).unwrap();
    failure(
        &source
            .cli(&[
                "project",
                "export",
                "--project-id",
                "missing",
                "--output",
                "archive with spaces.json",
            ])
            .await,
        "INVALID_PROJECT",
    );
    assert_eq!(std::fs::read(&path).unwrap(), contents);

    let mut target = Workspace::new().await;
    let imported_dir = target.path("imported project");
    std::fs::create_dir(&imported_dir).unwrap();
    let arguments = [
        "project",
        "import",
        "--input",
        path.to_str().unwrap(),
        "--workdir",
        imported_dir.to_str().unwrap(),
    ];
    let imported = success(&target.cli(&arguments).await);
    assert_eq!(
        imported["workdir"],
        imported_dir.canonicalize().unwrap().to_str().unwrap()
    );
    assert_eq!(imported["default_agent_id"], "portable");
    let restored = target.view().await;
    assert_eq!(restored["messages"], archive["messages"]);
    assert_eq!(restored["sessions"], archive["sessions"]);
    assert_eq!(restored["agents"], archive["agents"]);
    assert_eq!(restored["runs"], json!([]));
    assert_eq!(restored["crons"], json!([]));
    failure(&target.cli(&arguments).await, "INVALID_PROJECT");
    assert_eq!(target.view().await, restored);

    let mut invalid = archive;
    invalid["format_version"] = json!(999);
    std::fs::write(target.path("invalid.json"), invalid.to_string()).unwrap();
    failure(
        &target
            .cli(&[
                "project",
                "import",
                "--input",
                "invalid.json",
                "--workdir",
                imported_dir.to_str().unwrap(),
            ])
            .await,
        "INVALID_PROJECT",
    );
    assert_eq!(target.view().await, restored);
    assert_eq!(source.view().await, before);
    target.stop().await;
    source.stop().await;
}

// WF-08: Replace settings using the observed revision; stale/invalid writes do not win.
#[tokio::test]
async fn wf08_save_reset_and_recover_settings() {
    let mut workspace = Workspace::new().await;
    let initial = workspace.call(&["settings", "get"]).await;
    assert_eq!(initial["values"]["permissions.sandbox"], "read_only");
    assert_eq!(initial["values"]["permissions.approval"], "on_request");
    let mut values = initial["values"].clone();
    values["interface.theme"] = json!("dark");
    values["permissions.sandbox"] = json!("workspace_write");
    let revision = initial["revision"].to_string();
    let args = [
        "settings",
        "set",
        "--expected-revision",
        &revision,
        "--input",
        "-",
    ];
    let saved = success(&workspace.cli_stdin(&args, &values.to_string()).await);
    assert_eq!(saved["values"]["interface.theme"], "dark");
    assert_eq!(saved["values"]["permissions.sandbox"], "workspace_write");
    assert_eq!(saved["revision"], initial["revision"].as_u64().unwrap() + 1);
    failure(
        &workspace.cli_stdin(&args, &values.to_string()).await,
        "INVALID_CONFIGURATION",
    );
    for invalid_values in [json!({"interface.theme": "light"}), {
        let mut invalid = values;
        invalid["interface.theme"] = json!("unknown-theme");
        invalid
    }] {
        failure(
            &workspace
                .cli_stdin(
                    &[
                        "settings",
                        "set",
                        "--expected-revision",
                        &saved["revision"].to_string(),
                        "--input",
                        "-",
                    ],
                    &invalid_values.to_string(),
                )
                .await,
            "INVALID_CONFIGURATION",
        );
    }
    assert_eq!(workspace.call(&["settings", "get"]).await, saved);
    workspace.restart().await;
    assert_eq!(workspace.call(&["settings", "get"]).await, saved);
    let reset = workspace.call(&["settings", "reset"]).await;
    assert_eq!(reset["values"], initial["values"]);
    assert_eq!(reset["revision"], saved["revision"].as_u64().unwrap() + 1);
    workspace.restart().await;
    assert_eq!(workspace.call(&["settings", "get"]).await, reset);
    workspace.stop().await;
}

// WF-09: Scripts can distinguish usage, input/file/transport, and domain failures.
#[tokio::test]
async fn wf09_cli_diagnostics_do_not_mutate_workspace() {
    let mut workspace = Workspace::new().await;
    let initial = workspace.view().await;
    let help = workspace.cli(&["--help"]).await;
    assert_eq!(help.status.code(), Some(0));
    let help_text = String::from_utf8(help.stdout).unwrap();
    for command in [
        "project",
        "agent",
        "agent-provider",
        "session",
        "message",
        "run",
        "cron",
        "settings",
        "event",
        "export",
        "project",
        "import",
        "--endpoint",
    ] {
        assert!(
            help_text.contains(command),
            "missing {command}: {help_text}"
        );
    }
    for arguments in [
        vec![],
        vec!["unknown"],
        vec!["project"],
        vec!["message", "list"],
        vec!["event", "list", "--after", "invalid"],
        vec!["project", "export"],
        vec!["command"],
    ] {
        let output = workspace.cli(&arguments).await;
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty() && !output.stderr.is_empty());
    }
    std::fs::write(workspace.path("malformed.json"), "{").unwrap();
    for arguments in [
        vec![
            "settings",
            "set",
            "--expected-revision",
            "1",
            "--input",
            "malformed.json",
        ],
        vec![
            "project",
            "import",
            "--input",
            "missing.json",
            "--workdir",
            ".",
        ],
        vec![
            "project",
            "import",
            "--input",
            "malformed.json",
            "--workdir",
            ".",
        ],
    ] {
        let output = workspace.cli(&arguments).await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty() && !output.stderr.is_empty());
    }
    workspace
        .reject(&["run", "get", "--run-id", "missing"], "INVALID_RUN")
        .await;
    assert_eq!(workspace.view().await, initial);
    workspace.stop().await;
    let unavailable = workspace.cli(&["project", "list"]).await;
    assert_eq!(unavailable.status.code(), Some(1), "{unavailable:?}");
    assert!(unavailable.stdout.is_empty() && !unavailable.stderr.is_empty());
}

struct CredentialGateway {
    stored: std::sync::Mutex<std::collections::HashMap<String, String>>,
}

#[async_trait::async_trait]
impl ait_ports::AgentProviderGateway for CredentialGateway {
    async fn store_secret(
        &self,
        reference: &str,
        secret: &str,
    ) -> Result<(), ait_domain::DomainError> {
        self.stored
            .lock()
            .unwrap()
            .insert(reference.into(), secret.into());
        Ok(())
    }
    async fn delete_secret(&self, reference: &str) -> Result<(), ait_domain::DomainError> {
        self.stored.lock().unwrap().remove(reference);
        Ok(())
    }
    async fn list_models(
        &self,
        _provider: &ait_domain::AgentProvider,
        reference: &str,
    ) -> Result<Vec<ait_domain::ProviderModel>, ait_domain::DomainError> {
        assert_eq!(
            self.stored.lock().unwrap()[reference],
            "sk-must-stay-private"
        );
        Ok(vec![ait_domain::ProviderModel {
            id: "m".into(),
            name: "Model".into(),
            reasoning_efforts: vec!["high".into()],
        }])
    }
    async fn list_models_with_secret(
        &self,
        _provider: &ait_domain::AgentProvider,
        secret: &str,
    ) -> Result<Vec<ait_domain::ProviderModel>, ait_domain::DomainError> {
        // Exercise defense in depth against an adapter error echoing the credential.
        Err(ait_domain::DomainError::invariant(
            ait_domain::ErrorCode::ProviderFailed,
            format!("remote echoed: {secret}"),
        ))
    }
    async fn complete(
        &self,
        _provider: &ait_domain::AgentProvider,
        _reference: &str,
        _config: &ait_domain::AgentConfiguration,
        _messages: Vec<ait_ports::ProviderMessage>,
    ) -> Result<String, ait_domain::DomainError> {
        Ok("API fixture reply".into())
    }
}

#[tokio::test]
async fn wf11_stdin_commands_keep_credentials_out_of_diagnostics() {
    let gateway = std::sync::Arc::new(CredentialGateway {
        stored: std::sync::Mutex::new(std::collections::HashMap::new()),
    });
    let mut workspace = Workspace::with_gateway(Some(gateway.clone())).await;
    let secret = "sk-must-stay-private";
    let args = [
        "agent-provider",
        "save",
        "--id",
        "api",
        "--name",
        "API 中文\nProvider",
        "--kind",
        "deepseek",
        "--url",
        "https://api.deepseek.com",
        "--input",
        "models with spaces.json",
        "--secret-stdin",
    ];
    std::fs::write(
        workspace.path("models with spaces.json"),
        r#"[{"id":"m","name":"Model","reasoning_efforts":["high"]}]"#,
    )
    .unwrap();
    let output = workspace.cli_stdin(&args, &format!("{secret}\r\n")).await;
    assert_eq!(success(&output)["has_secret"], true);
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    assert_eq!(
        gateway.stored.lock().unwrap().values().next().unwrap(),
        secret
    );
    let refreshed = workspace
        .call(&["agent-provider", "refresh-models", "--provider-id", "api"])
        .await;
    assert_eq!(refreshed["models"][0]["id"], "m");
    let mut discover = args;
    discover[1] = "discover-models";
    let output = workspace.cli_stdin(&discover, secret).await;
    failure(&output, "PROVIDER_FAILED");
    assert!(!String::from_utf8_lossy(&output.stdout).contains(secret));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));

    let before = workspace.view().await;
    let input_args = [
        "agent-provider",
        "save",
        "--id",
        "p",
        "--name",
        "P",
        "--kind",
        "deepseek",
        "--input",
        "-",
    ];
    for input in [
        "",
        "{",
        r#"[{"id":"m","name":"M","sk-must-stay-private":true}]"#,
        r#"[{"id":"m","name":"M","reasoning_efforts":"sk-must-stay-private"}]"#,
    ] {
        let output = workspace.cli_stdin(&input_args, input).await;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("invalid entity JSON"));
        assert!(!diagnostic.contains(secret));
    }
    let mut conflicting = input_args.to_vec();
    conflicting.push("--secret-stdin");
    let output = workspace.cli_stdin(&conflicting, secret).await;
    assert_eq!(output.status.code(), Some(1));
    assert!(!String::from_utf8_lossy(&output.stderr).contains(secret));
    let output = workspace.cli_stdin(&args, "").await;
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(workspace.view().await, before);
    let listed = workspace.cli(&["agent-provider", "list"]).await;
    assert!(!String::from_utf8_lossy(&listed.stdout).contains(secret));
    let replay = workspace.cli(&["event", "list"]).await;
    assert!(!String::from_utf8_lossy(&replay.stdout).contains(secret));
    workspace.stop().await;
    for entry in std::fs::read_dir(workspace.directory.path()).unwrap() {
        let path = entry.unwrap().path();
        if path.is_file() {
            assert!(
                !std::fs::read(path)
                    .unwrap()
                    .windows(secret.len())
                    .any(|bytes| bytes == secret.as_bytes())
            );
        }
    }
}

#[tokio::test]
#[allow(clippy::too_many_lines)] // One complete acceptance scenario with explicit CLI arguments.
async fn typed_writes_stdin_and_files_reach_the_production_router() {
    let mut workspace = Workspace::new().await;
    workspace.project("project").await;
    workspace.agent("agent").await;
    workspace
        .call(&[
            "agent",
            "update",
            "--id",
            "agent",
            "--name",
            "更新 Agent",
            "--provider-id",
            "builtin-codex",
            "--model",
            "gpt-5.6-sol",
            "--reasoning-effort",
            "medium",
        ])
        .await;
    workspace.session("main", "project", "agent").await;
    workspace
        .call(&[
            "session",
            "set-config",
            "--session-id",
            "main",
            "--provider-id",
            "builtin-codex",
            "--model",
            "gpt-5.6-sol",
            "--reasoning-effort",
            "high",
        ])
        .await;
    let text = "中文第一行\n第二行 C:\\path\n";
    let run = success(
        &workspace
            .cli_stdin(
                &["session", "send", "--session-id", "main", "--text-stdin"],
                text,
            )
            .await,
    );
    let snapshot = workspace.view().await;
    assert_eq!(
        entity(&snapshot, "messages", &run["base_message_id"])["text"],
        text
    );
    let text_file = workspace.path("中文 input with spaces.txt");
    std::fs::write(&text_file, text).unwrap();
    let derived = workspace
        .call(&[
            "session",
            "derive",
            "--id",
            "derived",
            "--project-id",
            "project",
            "--source-session-id",
            "main",
            "--agent-id",
            "agent",
            "--at-message-id",
            run["base_message_id"].as_str().unwrap(),
            "--text-file",
            text_file.to_str().unwrap(),
        ])
        .await;
    let snapshot = workspace.view().await;
    // Derive preserves the Run response and appends the exact file contents.
    assert_eq!(derived["status"], "completed");
    assert!(derived["error"].is_null());
    assert!(
        snapshot["messages"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|message| message["text"] == text)
            .count()
            >= 2
    );
    workspace
        .reject(
            &[
                "run",
                "approval",
                "approve",
                "--run-id",
                "missing",
                "--approval-id",
                "missing",
                "--scope",
                "one-shot",
            ],
            "INVALID_RUN",
        )
        .await;
    for verb in ["deny", "cancel"] {
        workspace
            .reject(
                &[
                    "run",
                    "approval",
                    verb,
                    "--run-id",
                    "missing",
                    "--approval-id",
                    "missing",
                ],
                "INVALID_RUN",
            )
            .await;
    }
    let before = workspace.view().await;
    for args in [
        vec![
            "agent-provider",
            "save",
            "--id",
            "p",
            "--name",
            "P",
            "--kind",
            "invalid",
        ],
        vec![
            "cron",
            "trigger",
            "--cron-id",
            "c",
            "--scheduled-at",
            "2026-99-99",
        ],
        vec!["session", "send", "--session-id", "main"],
    ] {
        assert_eq!(workspace.cli(&args).await.status.code(), Some(2));
    }
    assert_eq!(workspace.view().await, before);
    workspace.stop().await;
}

#[tokio::test]
async fn provider_secret_is_absent_from_malformed_response_diagnostics() {
    use axum::{Json, Router, routing::post};
    use tokio::net::TcpListener;
    let mut workspace = Workspace::new().await;
    // Put a reflected stdin secret into a field whose serde error would quote it.
    let router = Router::new().route("/v1/agent-provider/save", post(|Json(body): Json<Value>| async move {
        Json(json!({"api_version": 1, "ok": false, "error": {"code": body["secret"], "message": "bad response", "retryable": false}}))
    }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    workspace.endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, router).await.unwrap();
    });
    let secret = "sk-malformed-response-private";
    let output = workspace
        .cli_stdin(
            &[
                "agent-provider",
                "save",
                "--id",
                "p",
                "--name",
                "P",
                "--kind",
                "deepseek",
                "--secret-stdin",
            ],
            secret,
        )
        .await;
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert_eq!(output.status.code(), Some(1));
    assert!(output.stdout.is_empty());
    let diagnostic = String::from_utf8(output.stderr).unwrap();
    assert!(!diagnostic.contains(secret));
    assert!(diagnostic.contains("agent-provider request failed"));
    workspace.stop().await;
}
