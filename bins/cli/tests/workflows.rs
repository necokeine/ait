//! User-facing acceptance flows indexed in workflows/README.md.

mod support;

use serde_json::{Value, json};
use support::{Workspace, entity, events, failure, success};

#[tokio::test]
async fn wf11_stdin_commands_keep_credentials_out_of_diagnostics() {
    let mut workspace = Workspace::new().await;
    let input = json!({"type": "register_agent", "id": "stdin-agent", "name": "中文\nAgent",
        "config": {"provider_id": "builtin-tool", "model": "default", "reasoning_effort": null}});
    let agent = success(
        &workspace
            .cli_stdin(&serde_json::to_string_pretty(&input).unwrap())
            .await,
    );
    assert_eq!(agent["name"], "中文\nAgent");
    let before = workspace.snapshot().await;
    for input in [
        "",
        "{",
        "{\"type\":\"sk-must-stay-private\"}",
        "{\"type\":\"get_run\",\"run_id\":\"unused\",\"sk-must-stay-private\":true}",
    ] {
        let output = workspace.cli_stdin(input).await;
        assert_eq!(output.status.code(), Some(1));
        assert!(output.stdout.is_empty());
        let diagnostic = String::from_utf8(output.stderr).unwrap();
        assert!(diagnostic.contains("invalid command JSON"));
        assert!(!diagnostic.contains("sk-must-stay-private"));
    }
    let request = json!({"type": "save_agent_provider", "secret": "sk-must-stay-private",
        "provider": {"id": "deepseek", "name": "DeepSeek", "kind": "deepseek",
            "url": "https://api.deepseek.com", "models": [{"id": "deepseek-v4-flash", "name": "Flash"}]}});
    let output = workspace.cli_stdin(&request.to_string()).await;
    // This service has no credential gateway; failure must not echo the write-only secret.
    failure(&output, "INVALID_AGENT_CONFIGURATION");
    assert!(
        !String::from_utf8(output.stdout)
            .unwrap()
            .contains("sk-must-stay-private")
    );
    assert_eq!(workspace.snapshot().await, before);
    workspace.stop().await;
}

// WF-01: Register a real directory and choose an Agent before opening a Session.
#[tokio::test]
async fn wf01_register_project_and_agent() {
    let mut workspace = Workspace::new().await;
    assert_eq!(workspace.snapshot().await["projects"], json!([]));
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
    let before = workspace.snapshot().await;
    workspace
        .reject(
            json!({
                "type": "register_project", "id": "duplicate", "name": "Duplicate",
                "workdir": workspace.path("project with spaces/.")
            }),
            "PROJECT_PATH_ALREADY_REGISTERED",
        )
        .await;
    workspace
        .reject(
            json!({
                "type": "register_project", "id": "missing", "name": "Missing",
                "workdir": workspace.path("missing")
            }),
            "PROJECT_PATH_NOT_FOUND",
        )
        .await;
    workspace
        .reject(
            json!({
                "type": "register_agent", "id": "bad", "name": "", "config": { "provider_id": "builtin-tool", "model": "default" }
            }),
            "INVALID_AGENT_CONFIGURATION",
        )
        .await;
    assert_eq!(workspace.snapshot().await, before);

    let agent = workspace.agent("primary", "tool").await;
    let default = workspace
        .command(json!({
            "type": "set_project_default_agent", "project_id": project["id"], "agent_id": "primary"
        }))
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
    let snapshot = workspace.snapshot().await;
    assert_eq!(snapshot["messages"].as_array().unwrap().len(), 1);
    let root = entity(&snapshot, "messages", &project["root_message_id"]);
    assert_eq!(root["role"], "system");
    assert!(root["parent_message_id"].is_null());
    assert_eq!(
        workspace.command(json!({"type": "snapshot"})).await,
        snapshot
    );
    workspace.stop().await;
}

// WF-02: Reject unsafe/stale input atomically, then observe the complete tool chain.
#[tokio::test]
async fn wf02_send_message_and_inspect_tool_history() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    let agent = workspace.agent("tool", "tool").await;
    workspace.session("main", "project", "tool").await;
    let before = workspace.snapshot().await;
    workspace.reject(json!({
        "type": "set_session_config", "session_id": "main",
        "config": {"provider_id": "builtin-tool", "model": "default", "reasoning_effort": "high"}
    }), "INVALID_AGENT_CONFIGURATION").await;
    assert_eq!(workspace.snapshot().await, before);
    let dirty = workspace.path("project/untracked.txt");
    std::fs::write(&dirty, "unsaved work").unwrap();
    workspace
        .reject(
            json!({
                "type": "send_message", "session_id": "main", "text": "must not append",
            }),
            "PROJECT_GIT_DIRTY",
        )
        .await;
    assert_eq!(workspace.snapshot().await, before);
    std::fs::remove_file(dirty).unwrap();

    let text = "检查工具输出：\"hello\"\n第二行";
    let run = workspace.send("main", 1, text).await;
    assert_eq!(run["status"], "completed");
    assert_eq!(run["agent_revision"], agent["revision"]);
    assert_eq!(run["session_id"], "main");
    assert!(run["error"].is_null());
    assert_eq!(
        workspace
            .command(json!({"type": "get_run", "run_id": run["id"]}))
            .await,
        run
    );
    let snapshot = workspace.snapshot().await;
    let messages = snapshot["messages"].as_array().unwrap();
    assert_eq!(messages.len(), 5);
    let user = entity(&snapshot, "messages", &run["base_message_id"]);
    assert_eq!(user["text"], text);
    assert_eq!(user["git_commit"], project["base_commit"]);
    assert_eq!(user["parent_message_id"], project["root_message_id"]);
    let tool_use = messages
        .iter()
        .find(|message| message["data"]["tool_use"].is_object())
        .unwrap();
    let tool_result = messages
        .iter()
        .find(|message| message["kind"] == "tool_result")
        .unwrap();
    assert_eq!(tool_use["role"], "assistant");
    assert_eq!(tool_use["parent_message_id"], user["id"]);
    assert_eq!(tool_result["role"], "user");
    assert_eq!(tool_result["parent_message_id"], tool_use["id"]);
    assert_eq!(
        tool_result["data"]["tool_result"]["call_id"],
        tool_use["data"]["tool_use"]["call_id"]
    );
    let final_message = entity(&snapshot, "messages", &run["last_message_id"]);
    assert_eq!(final_message["parent_message_id"], tool_result["id"]);
    assert_eq!(final_message["role"], "assistant");
    let session = entity(&snapshot, "sessions", &json!("main"));
    assert_eq!(session["current_message_id"], final_message["id"]);
    assert!(session["version"].as_u64().unwrap() > 1);
    assert!(session["active_run_id"].is_null());
    workspace.stop().await;
}

// WF-03: Branches share immutable history; names and Agent bindings have distinct effects.
#[tokio::test]
async fn wf03_branch_rename_and_rebind_session() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    workspace.agent("primary", "tool").await;
    workspace.agent("tool", "tool").await;
    workspace.session("main", "project", "primary").await;
    let run = workspace.send("main", 1, "original").await;
    let before = workspace.snapshot().await;
    let original = entity(&before, "sessions", &json!("main")).clone();
    let branch = workspace
        .command(json!({
            "type": "create_session", "id": "branch", "project_id": "project",
            "agent_id": "primary", "at_message_id": run["last_message_id"]
        }))
        .await;
    assert_eq!(branch["current_message_id"], run["last_message_id"]);
    assert_eq!(workspace.snapshot().await["messages"], before["messages"]);
    let titled = workspace
        .command(json!({"type": "set_session_title", "session_id": "branch", "title": "临时标题"}))
        .await;
    let renamed = workspace
        .command(
            json!({"type": "rename_session", "session_id": "branch", "name": "  我的   分支  "}),
        )
        .await;
    assert_eq!(renamed["name"], "我的 分支");
    assert_eq!(renamed["title"], titled["title"]);
    assert_eq!(renamed["version"], branch["version"]);
    let rebound = workspace
        .command(json!({
            "type": "set_session_agent", "session_id": "branch", "agent_id": "tool",
        }))
        .await;
    assert_eq!(rebound["version"], 2);
    assert_eq!(rebound["current_message_id"], branch["current_message_id"]);
    let continued = workspace.send("branch", 2, "continue independently").await;
    assert_eq!(continued["agent_id"], "tool");

    let fork = workspace
        .command(json!({
            "type": "fork_session", "id": "fork", "project_id": "project", "agent_id": "primary",
            "at_message_id": project["root_message_id"], "text": "new direction"
        }))
        .await;
    assert_eq!(fork["status"], "completed");
    let after = workspace.snapshot().await;
    assert_eq!(entity(&after, "sessions", &json!("main")), &original);
    assert_eq!(
        entity(&after, "messages", &fork["base_message_id"])["parent_message_id"],
        project["root_message_id"]
    );
    for old_message in before["messages"].as_array().unwrap() {
        assert_eq!(entity(&after, "messages", &old_message["id"]), old_message);
    }
    let other = workspace.project("other").await;
    let before_rejection = workspace.snapshot().await;
    workspace.reject(json!({
        "type": "fork_session", "id": "invalid-fork", "project_id": "project", "agent_id": "primary",
        "at_message_id": other["root_message_id"], "text": "cross-project input"
    }), "SESSION_MESSAGE_PROJECT_MISMATCH").await;
    assert_eq!(workspace.snapshot().await, before_rejection);
    workspace.stop().await;
}

// WF-04: Command success and Run success are separate contracts.
#[tokio::test]
async fn wf04_observe_failure_and_cancel_active_run() {
    let mut workspace = Workspace::new().await;
    workspace.project("project").await;
    workspace.agent("primary", "tool").await;
    for (mode, status, code) in [
        ("provider_failure", "failed", "PROVIDER_FAILED"),
        (
            "approval_required",
            "waiting_approval",
            "TOOL_APPROVAL_REQUIRED",
        ),
        ("manual", "queued", ""),
    ] {
        workspace.agent(mode, mode).await;
        workspace.session(mode, "project", mode).await;
        let run = workspace.send(mode, 1, "start").await;
        assert_eq!(run["status"], status);
        if !code.is_empty() {
            assert_eq!(run["error"]["code"], code);
        }
        let snapshot = workspace.snapshot().await;
        let session = entity(&snapshot, "sessions", &json!(mode));
        if status == "failed" {
            assert!(session["active_run_id"].is_null());
            assert_eq!(run["error"]["retryable"], true);
            continue;
        }
        assert_eq!(session["active_run_id"], run["id"]);
        workspace
            .reject(
                json!({
                    "type": "set_session_agent", "session_id": mode, "agent_id": "primary",
                }),
                "SESSION_BUSY",
            )
            .await;
        workspace
            .reject(
                json!({
                    "type": "send_message", "session_id": mode, "text": "second input",
                }),
                "SESSION_BUSY",
            )
            .await;
        assert_eq!(workspace.snapshot().await, snapshot);
        let cancel = json!({"type": "cancel_run", "run_id": run["id"]});
        let cancelled = workspace.command(cancel.clone()).await;
        assert_eq!(cancelled["status"], "cancelled");
        assert_eq!(cancelled["error"]["code"], "RUN_CANCELLED");
        assert_eq!(
            workspace
                .command(json!({"type": "get_run", "run_id": run["id"]}))
                .await,
            cancelled
        );
        let after = workspace.snapshot().await;
        assert!(entity(&after, "sessions", &json!(mode))["active_run_id"].is_null());
        assert_eq!(after["messages"], snapshot["messages"]);
        workspace.reject(cancel, "RUN_ALREADY_TERMINAL").await;
        assert_eq!(workspace.snapshot().await, after);
        let rebound = workspace
            .command(json!({
                "type": "set_session_agent", "session_id": mode, "agent_id": "primary",
            }))
            .await;
        assert_eq!(
            workspace
                .send(mode, rebound["version"].as_u64().unwrap(), "continue")
                .await["status"],
            "completed"
        );
    }
    workspace.stop().await;
}

// WF-05: An occurrence is idempotent and never moves an interactive Session.
#[tokio::test]
async fn wf05_cron_occurrence_is_idempotent_and_independent() {
    let mut workspace = Workspace::new().await;
    let project = workspace.project("project").await;
    workspace.agent("primary", "tool").await;
    workspace.session("main", "project", "primary").await;
    let sessions = workspace.snapshot().await["sessions"].clone();
    workspace
        .command(json!({
            "type": "create_cron", "id": "daily", "name": "Daily summary", "project_id": "project",
            "base_message_id": project["root_message_id"], "agent_id": "primary",
            "schedule": "0 9 * * *", "timezone": "Asia/Shanghai"
        }))
        .await;
    workspace
        .command(json!({"type": "set_cron_enabled", "cron_id": "daily", "enabled": false}))
        .await;
    let trigger =
        json!({"type": "trigger_cron", "cron_id": "daily", "scheduled_at": 1_788_480_000_000_i64});
    let disabled = workspace.snapshot().await;
    workspace.reject(trigger.clone(), "INVALID_CRON").await;
    assert_eq!(workspace.snapshot().await, disabled);
    workspace
        .command(json!({"type": "set_cron_enabled", "cron_id": "daily", "enabled": true}))
        .await;
    let run = workspace.command(trigger.clone()).await;
    assert_eq!(run["base_message_id"], project["root_message_id"]);
    assert_eq!(run["trigger"], "cron");
    assert_eq!(run["status"], "completed");
    assert!(run["session_id"].is_null());
    let once = workspace.snapshot().await;
    let event_count = events(&workspace.cli(&["events"]).await).len();
    assert_eq!(workspace.command(trigger).await, run);
    assert_eq!(workspace.snapshot().await, once);
    assert_eq!(events(&workspace.cli(&["events"]).await).len(), event_count);
    let second = workspace
        .command(json!({
            "type": "trigger_cron", "cron_id": "daily", "scheduled_at": 1_788_566_400_000_i64
        }))
        .await;
    assert_ne!(second["id"], run["id"]);
    let after = workspace.snapshot().await;
    assert_eq!(after["sessions"], sessions);
    assert_eq!(after["runs"].as_array().unwrap().len(), 2);
    assert_eq!(after["messages"].as_array().unwrap().len(), 7);
    workspace.stop().await;
}

// WF-06: Resume SSE strictly after a cursor and reopen the durable database.
#[tokio::test]
async fn wf06_replay_events_and_reopen_workspace() {
    let mut workspace = Workspace::new().await;
    assert!(events(&workspace.cli(&["events"]).await).is_empty());
    workspace.project("project").await;
    workspace.agent("primary", "tool").await;
    let first = events(&workspace.cli(&["events"]).await);
    assert!(
        first
            .iter()
            .any(|(_, event)| event["kind"] == "project.registered")
    );
    let cursor = first.last().unwrap().0;
    workspace.session("main", "project", "primary").await;
    let run = workspace.send("main", 1, "persist this").await;
    let all = events(&workspace.cli(&["events", "--after", "0"]).await);
    assert!(all.windows(2).all(|pair| pair[0].0 < pair[1].0));
    let rest = events(
        &workspace
            .cli(&["events", "--after", &cursor.to_string()])
            .await,
    );
    assert!(!rest.is_empty());
    assert!(rest.iter().all(|(next, _)| *next > cursor));
    assert_eq!([first, rest.clone()].concat(), all);
    let snapshot = workspace.snapshot().await;
    workspace.restart().await;
    assert_eq!(workspace.snapshot().await, snapshot);
    assert_eq!(
        workspace
            .command(json!({"type": "get_run", "run_id": run["id"]}))
            .await,
        run
    );
    assert_eq!(
        events(
            &workspace
                .cli(&["events", "--after", &cursor.to_string()])
                .await
        ),
        rest
    );
    assert!(
        events(
            &workspace
                .cli(&["events", "--after", &all.last().unwrap().0.to_string()])
                .await
        )
        .is_empty()
    );
    workspace.stop().await;
}

// WF-07: Export a branch forest, import to a fresh workspace, and reject conflicts.
#[tokio::test]
async fn wf07_export_and_import_project_archive() {
    let mut source = Workspace::new().await;
    let project = source.project("project").await;
    source.agent("manual", "manual").await;
    source.command(json!({"type": "set_project_default_agent", "project_id": "project", "agent_id": "manual"})).await;
    source.session("main", "project", "manual").await;
    source.send("main", 1, "keep active history").await;
    source
        .command(json!({
            "type": "create_session", "id": "branch", "project_id": "project", "agent_id": "manual",
            "at_message_id": project["root_message_id"]
        }))
        .await;
    let output = source
        .cli(&[
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
    let before = source.snapshot().await;
    assert!(!entity(&before, "sessions", &json!("main"))["active_run_id"].is_null());
    assert_eq!(
        source
            .command(json!({"type": "export_project", "project_id": "project"}))
            .await,
        archive
    );
    let contents = std::fs::read(&path).unwrap();
    failure(
        &source
            .cli(&[
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
    assert_eq!(imported["default_agent_id"], "manual");
    let restored = target.snapshot().await;
    assert_eq!(restored["messages"], archive["messages"]);
    assert_eq!(restored["sessions"], archive["sessions"]);
    assert_eq!(restored["agents"], archive["agents"]);
    assert_eq!(restored["runs"], json!([]));
    assert_eq!(restored["crons"], json!([]));
    failure(&target.cli(&arguments).await, "INVALID_PROJECT");
    assert_eq!(target.snapshot().await, restored);

    let mut invalid = archive;
    invalid["format_version"] = json!(999);
    std::fs::write(target.path("invalid.json"), invalid.to_string()).unwrap();
    failure(
        &target
            .cli(&[
                "import",
                "--input",
                "invalid.json",
                "--workdir",
                imported_dir.to_str().unwrap(),
            ])
            .await,
        "INVALID_PROJECT",
    );
    assert_eq!(target.snapshot().await, restored);
    assert_eq!(source.snapshot().await, before);
    target.stop().await;
    source.stop().await;
}

// WF-08: Replace settings using the observed revision; stale/invalid writes do not win.
#[tokio::test]
async fn wf08_save_reset_and_recover_settings() {
    let mut workspace = Workspace::new().await;
    let get = json!({"type": "get_settings"});
    let initial = workspace.command(get.clone()).await;
    let mut values = initial["values"].clone();
    values["interface.theme"] = json!("dark");
    let save = json!({"type": "save_settings", "expected_revision": initial["revision"], "values": values});
    let saved = workspace.command(save.clone()).await;
    assert_eq!(saved["values"]["interface.theme"], "dark");
    assert_eq!(saved["revision"], initial["revision"].as_u64().unwrap() + 1);
    workspace.reject(save, "INVALID_CONFIGURATION").await;
    for invalid_values in [json!({"interface.theme": "light"}), {
        let mut invalid = values;
        invalid["interface.theme"] = json!("unknown-theme");
        invalid
    }] {
        workspace.reject(json!({
            "type": "save_settings", "expected_revision": saved["revision"], "values": invalid_values
        }), "INVALID_CONFIGURATION").await;
    }
    assert_eq!(workspace.command(get.clone()).await, saved);
    workspace.restart().await;
    assert_eq!(workspace.command(get.clone()).await, saved);
    let reset = workspace.command(json!({"type": "reset_settings"})).await;
    assert_eq!(reset["values"], initial["values"]);
    assert_eq!(reset["revision"], saved["revision"].as_u64().unwrap() + 1);
    workspace.restart().await;
    assert_eq!(workspace.command(get).await, reset);
    workspace.stop().await;
}

// WF-09: Scripts can distinguish usage, input/file/transport, and domain failures.
#[tokio::test]
async fn wf09_cli_diagnostics_do_not_mutate_workspace() {
    let mut workspace = Workspace::new().await;
    let initial = workspace.snapshot().await;
    let help = workspace.cli(&["--help"]).await;
    assert_eq!(help.status.code(), Some(0));
    let help_text = String::from_utf8(help.stdout).unwrap();
    for command in [
        "command",
        "snapshot",
        "events",
        "export",
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
        vec!["events", "--after", "invalid"],
        vec!["export"],
    ] {
        let output = workspace.cli(&arguments).await;
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(output.stdout.is_empty() && !output.stderr.is_empty());
    }
    std::fs::write(workspace.path("malformed.json"), "{").unwrap();
    for arguments in [
        vec!["command", "{"],
        vec!["command", "{\"type\":\"unknown\"}"],
        vec!["import", "--input", "missing.json", "--workdir", "."],
        vec!["import", "--input", "malformed.json", "--workdir", "."],
    ] {
        let output = workspace.cli(&arguments).await;
        assert_eq!(output.status.code(), Some(1), "{output:?}");
        assert!(output.stdout.is_empty() && !output.stderr.is_empty());
    }
    workspace
        .reject(
            json!({"type": "get_run", "run_id": "missing"}),
            "INVALID_RUN",
        )
        .await;
    assert_eq!(workspace.snapshot().await, initial);
    workspace.stop().await;
    let unavailable = workspace.cli(&["snapshot"]).await;
    assert_eq!(unavailable.status.code(), Some(1), "{unavailable:?}");
    assert!(unavailable.stdout.is_empty() && !unavailable.stderr.is_empty());
}
