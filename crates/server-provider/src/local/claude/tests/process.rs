use super::*;
use crate::ports::agent_session::AgentTurnEvent;

mod parity;

#[tokio::test]
async fn steering_drains_prior_result_and_preserves_the_active_turn_and_input_identity() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let turn = session.start_turn("hold", &spec.config).await.unwrap();
    assert_eq!(
        session.steer_turn("wrong", "second").await,
        Err(AgentSessionError::Rejected)
    );
    assert_eq!(
        session.steer_turn(&turn, "/clear").await,
        Err(AgentSessionError::Rejected)
    );
    let prompt = crate::protocol::prompt::AgentPrompt {
        client_message_id: Some(uuid::Uuid::new_v4().to_string()),
        ..crate::protocol::prompt::AgentPrompt::text("second")
    };
    session.steer_input(&turn, &prompt).await.unwrap();
    let mut submitted = Vec::new();
    loop {
        match next(session.as_mut()).await.unwrap() {
            AgentTurnEvent::Timeline(entry) if entry.item["type"] == "user_message" => {
                submitted.push(entry.item);
            }
            AgentTurnEvent::Completed(text) => {
                assert_eq!(text.as_deref(), Some("Claude: second"));
                break;
            }
            AgentTurnEvent::Failed => panic!("steered turn failed"),
            _ => {}
        }
    }
    assert_eq!(submitted.len(), 2);
    assert_eq!(submitted[1]["messageId"], prompt.client_message_id.unwrap());
    assert_eq!(
        session.steer_turn(&turn, "stale").await,
        Err(AgentSessionError::Rejected)
    );
    let next_turn = session.start_turn("hold", &spec.config).await.unwrap();
    session.steer_turn(&next_turn, "hold").await.unwrap();
    session.cancel_turn(&next_turn).await.unwrap();
    while next(session.as_mut()).await.unwrap() != AgentTurnEvent::Cancelled {}
    session.start_turn("fresh", &spec.config).await.unwrap();
    assert_eq!(finish(session.as_mut()).await, "Claude: fresh");
    session.close().await.unwrap();
}

async fn next(session: &mut dyn AgentSession) -> Result<AgentTurnEvent, AgentSessionError> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(event) = session.poll_turn()? {
                return Ok(event);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

async fn finish(session: &mut dyn AgentSession) -> String {
    loop {
        match next(session).await.unwrap() {
            AgentTurnEvent::Completed(text) => return text.unwrap(),
            AgentTurnEvent::Failed => panic!("native turn failed"),
            _ => {}
        }
    }
}

#[tokio::test]
async fn native_rotation_updates_the_handle_and_missing_accepted_history_is_not_recreated() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let initial = session.persistence().unwrap();
    session
        .start_turn("rotate-session", &spec.config)
        .await
        .unwrap();
    assert_eq!(finish(session.as_mut()).await, "Claude: rotated");
    let current = session.persistence().unwrap();
    assert_ne!(current.session_id, initial.session_id);
    assert_eq!(
        session.runtime_info().await.unwrap().session_id.as_deref(),
        Some(current.session_id.as_str())
    );
    session.close().await.unwrap();
    let path = crate::local::claude::history::project_dir(&client, &spec.cwd)
        .unwrap()
        .join(format!("{}.jsonl", current.session_id));
    std::fs::remove_file(&path).unwrap();
    assert!(
        client
            .resume_session(&current, &spec, AgentResumePurpose::Interactive)
            .await
            .is_err()
    );
    assert!(!path.exists());
}

#[tokio::test]
async fn conversation_and_file_rewinds_keep_original_history_and_resume_the_fork() {
    let (root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let first = session.start_turn("first", &spec.config).await.unwrap();
    finish(session.as_mut()).await;
    let second = session.start_turn("second", &spec.config).await.unwrap();
    finish(session.as_mut()).await;
    let handle = session.persistence().unwrap();
    session.close().await.unwrap();
    let original = client.history(&handle, &spec.cwd).await.unwrap();
    assert!(
        client
            .rewind(&handle, &spec, &uuid::Uuid::new_v4().to_string())
            .await
            .is_err()
    );
    let fork = client.rewind(&handle, &spec, &second).await.unwrap();
    assert_eq!(fork.entries.len(), 2);
    assert_ne!(fork.descriptor.provider_handle_id, handle.session_id);
    assert_eq!(client.history(&handle, &spec.cwd).await.unwrap(), original);
    let fork_handle = AgentPersistenceHandle {
        session_id: fork.descriptor.provider_handle_id,
        metadata: None,
        ..handle.clone()
    };
    let mut resumed = client
        .resume_session(&fork_handle, &spec, AgentResumePurpose::Interactive)
        .await
        .unwrap();
    resumed.start_turn("branch", &spec.config).await.unwrap();
    assert_eq!(finish(resumed.as_mut()).await, "Claude: branch");
    resumed.close().await.unwrap();
    std::fs::write(root.path().join("tracked.txt"), "changed").unwrap();
    client.rewind_files(&handle, &spec, &second).await.unwrap();
    assert_eq!(
        std::fs::read_to_string(root.path().join("tracked.txt")).unwrap(),
        "checkpoint restored"
    );
    assert_eq!(client.history(&handle, &spec.cwd).await.unwrap(), original);
    let fresh = client.rewind(&handle, &spec, &first).await.unwrap();
    assert!(fresh.entries.is_empty());
    let fresh_handle = AgentPersistenceHandle {
        session_id: fresh.descriptor.provider_handle_id,
        metadata: Some(fresh.resume_metadata),
        ..handle
    };
    assert!(
        client
            .history(&fresh_handle, &spec.cwd)
            .await
            .unwrap()
            .is_empty()
    );
    let mut fresh = client
        .resume_session(&fresh_handle, &spec, AgentResumePurpose::Interactive)
        .await
        .unwrap();
    fresh
        .start_turn("fresh branch", &spec.config)
        .await
        .unwrap();
    assert_eq!(finish(fresh.as_mut()).await, "Claude: fresh branch");
    fresh.close().await.unwrap();
}

#[tokio::test]
async fn discovers_creates_streams_resumes_changes_settings_and_reads_history() {
    let (root, client, mut spec) = fixture();
    assert!(client.is_available().await.unwrap());
    assert!(client.diagnostic().await.unwrap().contains("found"));
    assert_eq!(
        client.discover(&spec.cwd).await.unwrap().models[0]["id"],
        "sonnet"
    );
    assert_eq!(client.commands(&spec).await.unwrap()[0]["name"], "review");
    let mut session = client.create_session(&spec).await.unwrap();
    assert_eq!(session.provider(), "claude");
    let handle = session.persistence().unwrap();
    session.start_turn("hello", &spec.config).await.unwrap();
    assert_eq!(finish(session.as_mut()).await, "Claude: hello");
    spec.config.model = Some("custom-model".into());
    spec.config.thinking_option_id = Some("high".into());
    spec.config.mode_id = Some("plan".into());
    spec.config.system_prompt = Some("Project instructions".into());
    session.start_turn("second", &spec.config).await.unwrap();
    assert_eq!(finish(session.as_mut()).await, "Claude: second");
    assert_eq!(
        session.runtime_info().await.unwrap().model.as_deref(),
        Some("custom-model")
    );
    let args = std::fs::read_to_string(root.path().join("claude-args.json")).unwrap();
    for expected in [
        "--resume=",
        "--model=custom-model",
        "--effort=high",
        "--permission-mode=plan",
        "--append-system-prompt=Project instructions",
    ] {
        assert!(args.contains(expected), "{args}");
    }
    session.close().await.unwrap();
    assert!(session.start_turn("closed", &spec.config).await.is_err());
    let mut restored = client
        .resume_session(&handle, &spec, AgentResumePurpose::Interactive)
        .await
        .unwrap();
    restored.start_turn("third", &spec.config).await.unwrap();
    assert_eq!(finish(restored.as_mut()).await, "Claude: third");
    restored.close().await.unwrap();
    let history = client.inspect_session(&handle, &spec.cwd).await.unwrap();
    assert_eq!(history.entries.len(), 6);
    assert!(!history.active);
    assert_eq!(
        history.descriptor.first_prompt_preview.as_deref(),
        Some("hello")
    );
    assert_eq!(client.history(&handle, &spec.cwd).await.unwrap().len(), 6);
    assert_eq!(
        client
            .list_sessions(&ListOptions {
                cwd: Some(spec.cwd.clone()),
                scan_limit: 10
            })
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        client
            .list_sessions(&ListOptions {
                cwd: None,
                scan_limit: 10
            })
            .await
            .unwrap()
            .len(),
        1
    );
    let mut history_only = client
        .resume_session(&handle, &spec, AgentResumePurpose::History)
        .await
        .unwrap();
    assert!(
        history_only
            .start_turn("forbidden", &spec.config)
            .await
            .is_err()
    );
    history_only.close().await.unwrap();
}

#[tokio::test]
async fn permission_and_question_roundtrips_are_scoped_and_stale_responses_rejected() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    for (prompt, response, expected) in [
        ("permission", json!({"behavior":"deny"}), "Denied"),
        ("permission", json!({"behavior":"allow"}), "Allowed"),
        (
            "question",
            json!({"behavior":"allow","updatedInput":{"answers":{"Color":"Blue"}}}),
            "Allowed",
        ),
    ] {
        session.start_turn(prompt, &spec.config).await.unwrap();
        let request = loop {
            if let AgentTurnEvent::PermissionRequested(request) =
                next(session.as_mut()).await.unwrap()
            {
                break request;
            }
        };
        assert_eq!(session.pending_permissions().len(), 1);
        let id = request["id"].as_str().unwrap();
        assert!(
            session
                .respond_permission(id, &json!({"behavior":"allow","updatedPermissions":[{}]}))
                .await
                .is_err()
        );
        session.respond_permission(id, &response).await.unwrap();
        assert!(session.pending_permissions().is_empty());
        assert!(session.respond_permission(id, &response).await.is_err());
        assert_eq!(finish(session.as_mut()).await, expected);
    }
    session.close().await.unwrap();
}

#[tokio::test]
async fn cancellation_reaps_query_and_next_turn_resumes_same_session() {
    let (root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let turn = session.start_turn("hold", &spec.config).await.unwrap();
    assert!(session.start_turn("busy", &spec.config).await.is_err());
    assert!(
        session
            .steer_turn(&turn, "/unsupported-steer")
            .await
            .is_err()
    );
    assert!(session.cancel_turn("stale").await.is_err());
    session.cancel_turn(&turn).await.unwrap();
    let pid = std::fs::read_to_string(root.path().join("claude-pid")).unwrap();
    assert!(
        !std::process::Command::new("/bin/kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    loop {
        if next(session.as_mut()).await.unwrap() == AgentTurnEvent::Cancelled {
            break;
        }
    }
    let handle = session.persistence();
    session
        .start_turn("after cancel", &spec.config)
        .await
        .unwrap();
    assert_eq!(finish(session.as_mut()).await, "Claude: after cancel");
    assert_eq!(session.persistence(), handle);
    session.close().await.unwrap();
    session.close().await.unwrap();
}

#[tokio::test]
async fn malformed_output_exit_and_unknown_interactions_fail_closed() {
    let (root, client, spec) = fixture();
    for prompt in [
        "exit",
        "malformed",
        "unknown-control",
        "oversized",
        "wrong-session",
    ] {
        let mut session = client.create_session(&spec).await.unwrap();
        session.start_turn(prompt, &spec.config).await.unwrap();
        loop {
            if next(session.as_mut()).await.is_err() {
                break;
            }
        }
        session.close().await.unwrap();
    }
    std::fs::write(root.path().join("fail-init"), "").unwrap();
    assert!(client.create_session(&spec).await.is_err());
}

#[tokio::test]
async fn native_result_errors_and_initialize_deadlines_are_not_successes() {
    let (root, mut client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    session
        .start_turn("error-result", &spec.config)
        .await
        .unwrap();
    loop {
        match next(session.as_mut()).await.unwrap() {
            AgentTurnEvent::Failed => break,
            AgentTurnEvent::Completed(_) => panic!("error result became success"),
            _ => {}
        }
    }
    session.close().await.unwrap();
    client.deadline = Duration::from_millis(100);
    std::fs::write(root.path().join("timeout-init"), "").unwrap();
    assert!(client.create_session(&spec).await.is_err());
}

#[tokio::test]
#[ignore = "requires a locally installed Claude Code CLI; sends no model request"]
async fn installed_claude_control_protocol_discovers_models_without_inference() {
    let root = tempfile::tempdir().unwrap();
    let mut client = ClaudeClient::new(
        std::env::var_os("AIT_SERVER_CLAUDE_BIN").map_or_else(|| "claude".into(), Into::into),
    );
    client.config_dir = Some(root.path().join("config"));
    let models = client
        .discover(root.path().to_str().unwrap())
        .await
        .unwrap();
    assert!(!models.models.is_empty());
    assert!(
        models
            .models
            .iter()
            .all(|model| model["provider"] == "claude")
    );
}

#[tokio::test]
async fn advanced_configuration_reaches_native_cli_and_can_be_cleared_next_turn() {
    let (root, client, mut spec) = fixture();
    spec.config = serde_json::from_value(json!({"model":"claude-opus-5",
        "thinkingOptionId":"ultracode","featureValues":{"fast_mode":true},
        "providerOptions":{"allowedTools":["Read"],"additionalDirectories":["/tmp/fixture"],
            "sandbox":{"enabled":true}},
        "mcpServers":{"docs":{"type":"stdio","command":"node","args":["server.js"]}},
        "toolPolicy":{"preapproved":[{"kind":"mcp","server":"docs","tool":"search"}]}}))
    .unwrap();
    let mut session = client.create_session(&spec).await.unwrap();
    session
        .start_turn("configured", &spec.config)
        .await
        .unwrap();
    finish(session.as_mut()).await;
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.path().join("claude-args.json")).unwrap())
            .unwrap();
    assert!(args.contains(&"--allowedTools=Read,mcp__docs__search".to_owned()));
    assert!(args.contains(&"--add-dir=/tmp/fixture".to_owned()));
    assert!(args.contains(&"--effort=xhigh".to_owned()));
    assert!(args.contains(&"--thinking=adaptive".to_owned()));
    let settings: Value = serde_json::from_str(
        args.iter()
            .find_map(|arg| arg.strip_prefix("--settings="))
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        settings,
        json!({"sandbox":{"enabled":true},"fastMode":true,"ultracode":true})
    );
    assert!(args.iter().any(|arg| arg.starts_with("--mcp-config=")));
    let handle = session.persistence();
    session
        .start_turn("cleared", &StoredAgentConfig::default())
        .await
        .unwrap();
    finish(session.as_mut()).await;
    assert_eq!(session.persistence(), handle);
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.path().join("claude-args.json")).unwrap())
            .unwrap();
    assert!(args.iter().all(|arg| !arg.starts_with("--settings=")
        && !arg.starts_with("--mcp-config=")
        && !arg.starts_with("--allowedTools=")));
    session.close().await.unwrap();
}

#[tokio::test]
async fn native_stream_emits_usage_before_terminal_completion() {
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    session.start_turn("usage", &spec.config).await.unwrap();
    let mut usages = Vec::new();
    loop {
        match next(session.as_mut()).await.unwrap() {
            AgentTurnEvent::Usage(usage) => usages.push(usage),
            AgentTurnEvent::Completed(_) => break,
            AgentTurnEvent::Failed => panic!("usage turn failed"),
            _ => {}
        }
    }
    assert_eq!(usages[0].context_window_used_tokens, Some(30));
    assert_eq!(usages.last().unwrap().total_cost_usd, Some(0.02));
    assert_eq!(
        usages.last().unwrap().context_window_max_tokens,
        Some(200_000)
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn rich_input_preserves_images_attachments_identity_and_output_schema_at_native_boundary() {
    let (root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let identity = uuid::Uuid::new_v4().to_string();
    let prompt = serde_json::from_value(json!({"text":"Inspect","clientMessageId":identity,
        "images":[{"data":"aGVsbG8=","mimeType":"image/png"}],
        "attachments":[{"type":"text","mimeType":"text/plain","text":"Context"}],
        "outputSchema":{"type":"object","properties":{"answer":{"type":"string"}}}}))
    .unwrap();
    assert_eq!(
        session.start_input(&prompt, &spec.config).await.unwrap(),
        identity
    );
    assert_eq!(
        finish(session.as_mut()).await,
        "Claude: Inspect [Image attachment] Context"
    );
    let args: Vec<String> =
        serde_json::from_slice(&std::fs::read(root.path().join("claude-args.json")).unwrap())
            .unwrap();
    assert!(args.iter().any(|arg| arg.starts_with("--json-schema=")));
    let history = client
        .history(&session.persistence().unwrap(), &spec.cwd)
        .await
        .unwrap();
    assert!(history.iter().any(|entry| {
        entry.item["messageId"] == identity
            && entry.item["text"]
                .as_str()
                .unwrap()
                .contains("[Image attachment]")
    }));
    session.close().await.unwrap();
}

#[tokio::test]
async fn background_child_streams_after_root_completion_without_changing_root_history() {
    use crate::ports::controls::SubagentEvent;
    let (_root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    session
        .start_turn("live-subagent", &spec.config)
        .await
        .unwrap();
    let mut parent_finished = false;
    let mut child_answer = false;
    loop {
        match next(session.as_mut()).await.unwrap() {
            AgentTurnEvent::Completed(text) => {
                assert_eq!(text.as_deref(), Some("Parent finished"));
                parent_finished = true;
            }
            AgentTurnEvent::Timeline(entry) => {
                assert_ne!(entry.item["text"], "Background child answer");
            }
            AgentTurnEvent::Subagent(SubagentEvent::Timeline { id, entry })
                if entry.item["text"] == "Background child answer" =>
            {
                assert!(parent_finished);
                assert_eq!(id, "child-call");
                child_answer = true;
            }
            AgentTurnEvent::Subagent(SubagentEvent::Upsert(child))
                if child.descriptor["status"] == "completed" =>
            {
                assert!(child_answer);
                break;
            }
            AgentTurnEvent::Failed => panic!("child stream failed"),
            _ => {}
        }
    }
    assert_eq!(session.subagents().len(), 1);
    session.close().await.unwrap();
}

#[tokio::test]
async fn rewind_slash_command_uses_native_file_checkpoints_without_adding_an_inference_prompt() {
    let (root, client, spec) = fixture();
    let mut session = client.create_session(&spec).await.unwrap();
    let message = session
        .start_turn("checkpoint", &spec.config)
        .await
        .unwrap();
    finish(session.as_mut()).await;
    let handle = session.persistence().unwrap();
    let path = super::super::history::project_dir(&client, &spec.cwd)
        .unwrap()
        .join(format!("{}.jsonl", handle.session_id));
    let before = std::fs::read(&path).unwrap();
    session.start_turn("/rewind", &spec.config).await.unwrap();
    assert_eq!(
        finish(session.as_mut()).await,
        format!("Rewound tracked files to message {message}.")
    );
    assert_eq!(
        std::fs::read_to_string(root.path().join("tracked.txt")).unwrap(),
        "checkpoint restored"
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(
        session
            .start_turn("/rewind unknown", &spec.config)
            .await
            .is_err()
    );
    let handle = session.persistence().unwrap();
    session.close().await.unwrap();
    let history = client.history(&handle, &spec.cwd).await.unwrap();
    assert!(
        history
            .iter()
            .any(|entry| entry.item["text"]
                == format!("Rewound tracked files to message {message}."))
    );
}
