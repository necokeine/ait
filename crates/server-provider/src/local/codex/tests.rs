use super::*;
use crate::test_support::Fixture;

mod configuration;
mod installed;
mod progress;

fn process_is_running(pid: u32) -> bool {
    let output = std::process::Command::new("ps")
        .args(["-o", "stat=", "-p", &pid.to_string()])
        .output()
        .unwrap();
    assert!(output.status.success() || output.status.code() == Some(1));
    let state = String::from_utf8(output.stdout).unwrap();
    // An orphan can remain a zombie until its new parent reaps it on Linux.
    !state.trim().is_empty() && !state.trim().starts_with('Z')
}

async fn terminal(session: &mut dyn AgentSession) -> Result<AgentTurnEvent, AgentSessionError> {
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if let Some(event) = session.poll_turn()?
                && !matches!(event, AgentTurnEvent::Timeline(_))
            {
                return Ok(event);
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap()
}

#[tokio::test]
async fn native_create_text_turn_cancel_resume_and_history_are_distinct() {
    let fixture = Fixture::new();
    let client = fixture.client();
    assert!(client.is_available().await.unwrap());
    let mut spec = fixture.spec();
    spec.config.model = Some("test-model".to_owned());
    spec.config.thinking_option_id = Some("high".to_owned());
    spec.config.system_prompt = Some("test instructions".to_owned());
    let mut session = client.create_session(&spec).await.unwrap();
    let handle = session.persistence().unwrap();
    assert_eq!(session.provider(), "codex");
    assert_eq!(
        session.runtime_info().await.unwrap().model.as_deref(),
        Some("offline-model")
    );
    session
        .start_turn("hello", &fixture.spec().config)
        .await
        .unwrap();
    assert!(
        session
            .start_turn("busy", &fixture.spec().config)
            .await
            .is_err()
    );
    assert_eq!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(Some("Echo: hello".to_owned()))
    );
    let turn = session
        .start_turn("hang", &fixture.spec().config)
        .await
        .unwrap();
    assert!(session.cancel_turn("wrong").await.is_err());
    session.cancel_turn(&turn).await.unwrap();
    assert_eq!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Cancelled
    );
    session.close().await.unwrap();
    session.close().await.unwrap();
    let mut resumed = client
        .resume_session(&handle, &spec, AgentResumePurpose::Interactive)
        .await
        .unwrap();
    assert_eq!(resumed.persistence(), Some(handle.clone()));
    resumed
        .start_turn("again", &fixture.spec().config)
        .await
        .unwrap();
    assert!(matches!(
        terminal(resumed.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(_)
    ));
    resumed.close().await.unwrap();
    let mut history = client
        .resume_session(&handle, &spec, AgentResumePurpose::History)
        .await
        .unwrap();
    assert!(
        history
            .start_turn("forbidden", &fixture.spec().config)
            .await
            .is_err()
    );
    assert_eq!(history.poll_turn().unwrap(), None);
    history.close().await.unwrap();
    let requests = fixture.requests();
    let start = requests
        .iter()
        .find(|request| request["method"] == "thread/start")
        .unwrap();
    assert_eq!(start["params"]["sandbox"], "read-only");
    assert_eq!(start["params"]["approvalPolicy"], "never");
    assert_eq!(
        start["params"]["developerInstructions"],
        "test instructions"
    );
    assert!(
        requests
            .iter()
            .any(|request| request["method"] == "thread/resume")
    );
    assert_eq!(requests.last().unwrap()["method"], "thread/read");
}

#[tokio::test]
async fn malformed_timeout_error_and_missing_identity_close_without_exposing_native_errors() {
    for mode in [
        "malformed",
        "oversize",
        "wrong-id",
        "error",
        "timeout",
        "missing-thread",
    ] {
        let fixture = Fixture::new();
        fixture.mode(mode);
        let mut client = fixture.client();
        if mode == "timeout" {
            client.deadline = Duration::from_millis(300);
        }
        let error = client.create_session(&fixture.spec()).await.unwrap_err();
        assert_eq!(
            error,
            if mode == "error" {
                AgentSessionError::Rejected
            } else {
                AgentSessionError::Failed
            },
            "{mode}"
        );
        assert!(!error.to_string().contains("sensitive"));
    }
}

#[tokio::test]
async fn failed_turn_exit_and_permission_requests_have_explicit_terminal_outcomes() {
    for text in ["fail", "exit", "approval"] {
        let fixture = Fixture::new();
        let client = fixture.client();
        let mut session = client.create_session(&fixture.spec()).await.unwrap();
        session
            .start_turn(text, &fixture.spec().config)
            .await
            .unwrap();
        let outcome = terminal(session.as_mut()).await;
        if text == "fail" {
            assert_eq!(outcome.unwrap(), AgentTurnEvent::Failed);
        } else {
            assert!(outcome.is_err());
        }
        session.close().await.unwrap();
    }
}

#[tokio::test]
async fn rejects_unsupported_options_missing_executable_and_mismatched_resume_identity() {
    let fixture = Fixture::new();
    let missing = CodexClient::new(fixture.root.path().join("absent"));
    assert!(!missing.is_available().await.unwrap());
    assert_eq!(
        missing.create_session(&fixture.spec()).await.unwrap_err(),
        AgentSessionError::Unavailable
    );
    let client = fixture.client();
    let mut spec = fixture.spec();
    spec.config.mode_id = Some("bypassPermissions".to_owned());
    assert_eq!(
        client.create_session(&spec).await.unwrap_err(),
        AgentSessionError::Unavailable
    );
    fixture.mode("wrong-thread");
    let handle = AgentPersistenceHandle {
        provider: "codex".to_owned(),
        session_id: "expected".to_owned(),
        native_handle: None,
        metadata: None,
    };
    assert!(
        client
            .resume_session(&handle, &fixture.spec(), AgentResumePurpose::Interactive)
            .await
            .is_err()
    );
    let wrong_provider = AgentPersistenceHandle {
        provider: "other".to_owned(),
        ..handle
    };
    assert!(
        client
            .resume_session(
                &wrong_provider,
                &fixture.spec(),
                AgentResumePurpose::History
            )
            .await
            .is_err()
    );
}

#[tokio::test]
async fn completion_racing_interruption_keeps_the_native_connection_usable() {
    let fixture = Fixture::new();
    fixture.mode("interrupt-completed");
    let client = fixture.client();
    let mut session = client.create_session(&fixture.spec()).await.unwrap();
    let turn = session
        .start_turn("hang", &fixture.spec().config)
        .await
        .unwrap();
    assert_eq!(
        session.cancel_turn(&turn).await,
        Err(AgentSessionError::Rejected)
    );
    assert_eq!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(Some("Echo: raced".to_owned()))
    );
    session
        .start_turn("next", &fixture.spec().config)
        .await
        .unwrap();
    assert_eq!(
        terminal(session.as_mut()).await.unwrap(),
        AgentTurnEvent::Completed(Some("Echo: next".to_owned()))
    );
    session.close().await.unwrap();
}

#[tokio::test]
async fn closing_or_dropping_a_session_terminates_its_tool_process_group() {
    for close in [true, false] {
        let fixture = Fixture::new();
        let mut session = fixture
            .client()
            .create_session(&fixture.spec())
            .await
            .unwrap();
        session
            .start_turn("child", &fixture.spec().config)
            .await
            .unwrap();
        let pid = tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if let Ok(pid) = std::fs::read_to_string(fixture.cwd.join("child.pid"))
                    && let Ok(pid) = pid.trim().parse::<u32>()
                {
                    break pid;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap();
        assert!(process_is_running(pid));
        if close {
            session.close().await.unwrap();
        }
        drop(session);
        tokio::time::timeout(Duration::from_secs(3), async {
            while process_is_running(pid) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("tool process is still running after session close");
    }
}
