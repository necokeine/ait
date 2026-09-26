//! Paseo schedule/service.test.ts update, timing and history cases.
use super::*;

fn new_agent(engine: &mut Engine) -> String {
    let mut params = input();
    params["target"] = json!({"type":"new-agent","config":{
        "provider":"codex","cwd":"/not/created/yet","model":"first","modeId":"read-only",
        "thinkingOptionId":"low","archiveOnFinish":true,"isolation":"local"}});
    engine
        .request("schedule.create.request", params, now())
        .unwrap()["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}

#[test]
fn prompt_only_update_preserves_next_slot_last_run_and_history() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    engine
        .finish(
            &id,
            &run,
            false,
            Outcome {
                output: Some("first result".into()),
                ..Outcome::default()
            },
            now() + chrono::Duration::seconds(5),
        )
        .unwrap();
    let before = engine.inspect(&id).unwrap();
    let after = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"prompt":" updated "}),
            now() + chrono::Duration::seconds(30),
        )
        .unwrap()["schedule"]
        .clone();
    assert_eq!(after["prompt"], "updated");
    assert_eq!(after["nextRunAt"], json!(before.next_run_at));
    assert_eq!(after["lastRunAt"], json!(before.last_run_at));
    assert_eq!(after["runs"], json!(before.runs));
    assert_eq!(after, json!(store.0.lock().unwrap().0[0]));
}

#[test]
fn changing_cadence_switches_every_to_cron_and_back_from_update_time() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let later = now() + chrono::Duration::seconds(30);
    let cron = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "cadence":{"type":"cron","expression":"15 9 * * *"}}),
            later,
        )
        .unwrap();
    assert_eq!(cron["schedule"]["nextRunAt"], "2026-01-01T09:15:00Z");
    let every = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "cadence":{"type":"every","everyMs":120_000}}),
            later,
        )
        .unwrap();
    assert_eq!(every["schedule"]["nextRunAt"], "2026-01-01T00:02:30Z");
    assert_eq!(every["schedule"]["createdAt"], json!(now()));
}

#[test]
fn cadence_update_on_paused_schedule_does_not_resume_it() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "cadence":{"type":"cron","expression":"0 9 * * *"}}),
            now(),
        )
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Paused);
    assert_eq!(record.next_run_at, None);
    assert_eq!(record.paused_at, Some(now()));
}

#[test]
fn name_can_be_cleared_by_empty_or_whitespace_and_then_renamed() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    for name in [json!(""), json!(" \t\n "), Value::Null] {
        let record = engine
            .request(
                "schedule.update.request",
                json!({"scheduleId":id,"name":name}),
                now(),
            )
            .unwrap();
        assert_eq!(record["schedule"]["name"], Value::Null);
        let renamed = engine
            .request(
                "schedule.update.request",
                json!({"scheduleId":id,"name":" new name "}),
                now(),
            )
            .unwrap();
        assert_eq!(renamed["schedule"]["name"], "new name");
    }
}

#[test]
fn new_agent_fields_update_independently_and_nullable_options_clear() {
    let (mut engine, _) = fixture();
    let id = new_agent(&mut engine);
    let updated = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "newAgentConfig":{"modeId":"full-access","archiveOnFinish":false}}),
            now(),
        )
        .unwrap();
    let config = &updated["schedule"]["target"]["config"];
    assert_eq!(config["provider"], "codex");
    assert_eq!(config["cwd"], "/not/created/yet");
    assert_eq!(config["model"], "first");
    assert_eq!(config["thinkingOptionId"], "low");
    assert_eq!(config["modeId"], "full-access");
    assert_eq!(config["archiveOnFinish"], false);
    let cleared = engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,
        "newAgentConfig":{"model":null,"modeId":" ","thinkingOptionId":""}}),
            now(),
        )
        .unwrap();
    let config = &cleared["schedule"]["target"]["config"];
    for key in ["model", "modeId", "thinkingOptionId"] {
        assert!(config.get(key).is_none());
    }
    assert_eq!(config["archiveOnFinish"], false);
}

#[test]
fn new_agent_fields_on_existing_agent_are_rejected_atomically() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let before = json!(engine.inspect(&id).unwrap());
    assert_eq!(
        engine
            .request(
                "schedule.update.request",
                json!({"scheduleId":id,
        "prompt":"changed","newAgentConfig":{"model":"new"}}),
                now()
            )
            .unwrap_err(),
        Error::Invalid
    );
    assert_eq!(json!(engine.inspect(&id).unwrap()), before);
    assert_eq!(json!(store.0.lock().unwrap().0[0]), before);
}

#[test]
fn invalid_new_agent_edits_reject_the_entire_mutation_atomically() {
    let (mut engine, store) = fixture();
    let id = new_agent(&mut engine);
    let before = json!(engine.inspect(&id).unwrap());
    for patch in [
        json!({"provider":""}),
        json!({"cwd":null}),
        json!({"isolation":"container"}),
        json!({"archiveOnFinish":"false"}),
        json!({"surprise":true}),
        json!({"model":123}),
    ] {
        assert_eq!(
            engine
                .request(
                    "schedule.update.request",
                    json!({"scheduleId":id,
            "name":"not committed","newAgentConfig":patch}),
                    now()
                )
                .unwrap_err(),
            Error::Invalid
        );
        assert_eq!(json!(engine.inspect(&id).unwrap()), before);
        assert_eq!(json!(store.0.lock().unwrap().0[0]), before);
    }
}

#[test]
fn repeated_pause_and_resume_are_idempotent_without_storage_writes() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    store.0.lock().unwrap().1 = true;
    engine
        .request(
            "schedule.pause.request",
            json!({"scheduleId":id}),
            now() + chrono::Duration::seconds(10),
        )
        .unwrap();
    assert_eq!(engine.inspect(&id).unwrap().paused_at, Some(now()));
    store.0.lock().unwrap().1 = false;
    engine
        .request("schedule.resume.request", json!({"scheduleId":id}), now())
        .unwrap();
    store.0.lock().unwrap().1 = true;
    engine
        .request(
            "schedule.resume.request",
            json!({"scheduleId":id}),
            now() + chrono::Duration::seconds(20),
        )
        .unwrap();
    assert_eq!(
        engine.inspect(&id).unwrap().next_run_at,
        Some(now() + chrono::Duration::minutes(1))
    );
}

#[test]
fn list_sorts_newest_first_without_leaking_occurrence_history() {
    let (mut engine, _) = fixture();
    let first = create(&mut engine);
    let second = engine
        .request(
            "schedule.create.request",
            input(),
            now() + chrono::Duration::seconds(1),
        )
        .unwrap()["schedule"]["id"]
        .clone();
    engine.begin(&first, true, now()).unwrap();
    let list = engine
        .request("schedule.list.request", json!({}), now())
        .unwrap();
    assert_eq!(list["schedules"][0]["id"], second);
    assert_eq!(list["schedules"][1]["id"], first);
    assert!(
        list["schedules"]
            .as_array()
            .unwrap()
            .iter()
            .all(|value| value.get("runs").is_none())
    );
}
