use super::*;

#[tokio::test]
async fn native_plan_progress_is_a_durable_todo_snapshot_or_a_reviewable_plan() {
    let fixture = Fixture::new();
    fixture.mode("workflows");
    let client = fixture.client();
    let mut spec = fixture.spec();
    for planning in [false, true] {
        spec.config.feature_values = Some(std::collections::BTreeMap::from([(
            "plan_mode".to_owned(),
            json!(planning),
        )]));
        let mut session = client.create_session(&spec).await.unwrap();
        session
            .start_turn("plan-progress", &spec.config)
            .await
            .unwrap();
        let mut todos = Vec::new();
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                match session.poll_turn().unwrap() {
                    Some(AgentTurnEvent::Timeline(entry)) if entry.item["type"] == "todo" => {
                        todos.push(entry);
                    }
                    Some(AgentTurnEvent::Completed(_)) => break,
                    Some(AgentTurnEvent::Failed) => panic!("plan progress failed"),
                    _ => tokio::time::sleep(Duration::from_millis(5)).await,
                }
            }
        })
        .await
        .unwrap();
        let handle = session.persistence().unwrap();
        if planning {
            assert!(todos.is_empty());
            assert!(
                session.pending_permissions()[0]["input"]["plan"]
                    .as_str()
                    .unwrap()
                    .contains("Second step")
            );
        } else {
            assert_eq!(todos[0].item["items"][1]["status"], "in_progress");
            assert_eq!(todos[0].item["items"][0]["completed"], true);
        }
        session.close().await.unwrap();
        if !planning {
            assert!(
                client
                    .history(&handle, &spec.cwd)
                    .await
                    .unwrap()
                    .iter()
                    .any(|entry| entry.item["type"] == "todo")
            );
        }
    }
}
