use super::*;

#[tokio::test]
async fn abandoned_voice_admission_is_skipped_or_interrupted_without_a_receipt_reader() {
    let fixture = Fixture::new();
    fixture.mode("delayed-voice-admission");
    let (execution, _) = worker(&fixture);
    let agent = create(&execution, &fixture).await;
    let id = agent["agentId"].as_str().unwrap().to_owned();
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    assert_eq!(
        execution.send_voice(&id, "hang", cancelled).await,
        Err(ErrorCode::AgentIo)
    );
    assert!(
        fixture
            .requests()
            .iter()
            .all(|request| request["method"] != "turn/start")
    );
    let cancel = CancellationToken::new();
    let sending = {
        let execution = execution.clone();
        let cancel = cancel.clone();
        let id = id.clone();
        tokio::spawn(async move { execution.send_voice(&id, "hang", cancel).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .requests()
            .iter()
            .all(|request| request["method"] != "turn/start")
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    sending.abort();
    assert!(sending.await.unwrap_err().is_cancelled());
    std::fs::write(fixture.cwd.join("release-voice-admission"), "ready").unwrap();
    let finished = execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    assert_eq!(finished["status"], "idle");
    assert!(
        fixture
            .requests()
            .iter()
            .any(|request| request["method"] == "turn/interrupt")
    );
    execution.shutdown().await.unwrap();
}

#[tokio::test]
async fn voice_turn_receipts_fence_cancellation_and_results_against_later_turns() {
    let fixture = Fixture::new();
    let (execution, _) = worker(&fixture);
    let agent = create(&execution, &fixture).await;
    let id = agent["agentId"].as_str().unwrap();
    let sent = execution
        .execute("internal.voice.send", json!({"agentId":id,"text":"hang"}))
        .await
        .unwrap();
    assert_eq!(sent["accepted"], true);
    let turn = sent["turnId"].as_str().unwrap();
    let rejected = execution
        .execute(
            "internal.voice.cancel",
            json!({"agentId":id,"turnId":"different-turn"}),
        )
        .await
        .unwrap();
    assert_eq!(rejected["cancelled"], false);
    assert_eq!(
        execution
            .execute("internal.voice.status", json!({"agentId":id,"turnId":turn}))
            .await
            .unwrap()["status"],
        "running"
    );
    assert_eq!(
        execution
            .execute("internal.voice.cancel", json!({"agentId":id,"turnId":turn}))
            .await
            .unwrap()["cancelled"],
        true
    );
    execution
        .execute("agent.finish.wait.request", json!({"agentId":id}))
        .await
        .unwrap();
    let later = execution
        .execute("internal.voice.send", json!({"agentId":id,"text":"hang"}))
        .await
        .unwrap();
    let later_turn = later["turnId"].as_str().unwrap();
    assert_ne!(later_turn, turn);
    assert_eq!(
        execution
            .execute("internal.voice.cancel", json!({"agentId":id,"turnId":turn}))
            .await
            .unwrap()["cancelled"],
        false
    );
    assert_eq!(
        execution
            .execute("internal.voice.status", json!({"agentId":id,"turnId":turn}))
            .await,
        Err(ErrorCode::IdempotencyConflict)
    );
    assert_eq!(
        execution
            .execute(
                "internal.voice.status",
                json!({"agentId":id,"turnId":later_turn})
            )
            .await
            .unwrap()["status"],
        "running"
    );
    execution.shutdown().await.unwrap();
}
