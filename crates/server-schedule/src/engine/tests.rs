use super::*;
use std::sync::{Arc, Mutex};
#[derive(Debug, Clone, Default)]
pub(crate) struct Memory(pub Arc<Mutex<(Vec<Schedule>, bool)>>);
impl Store for Memory {
    fn load(&self) -> Result<Vec<Schedule>, Error> {
        Ok(self.0.lock().unwrap().0.clone())
    }
    fn save(&mut self, records: &[Schedule]) -> Result<(), Error> {
        let mut state = self.0.lock().unwrap();
        if state.1 {
            return Err(Error::Storage);
        }
        state.0 = records.to_vec();
        Ok(())
    }
}
fn now() -> DateTime<Utc> {
    "2026-01-01T00:00:00Z".parse().unwrap()
}
fn fixture() -> (Engine, Memory) {
    let store = Memory::default();
    (Engine::open(Box::new(store.clone()), now()).unwrap(), store)
}
pub(crate) fn input() -> Value {
    json!({"name":" test ","prompt":" hello ","cadence":{"type":"every","everyMs":60_000},"target":{"type":"self","agentId":"11111111-1111-4111-8111-111111111111"}})
}
fn create(engine: &mut Engine) -> String {
    engine
        .request("schedule.create.request", input(), now())
        .unwrap()["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_owned()
}
#[test]
fn crud_summary_logs_pause_resume_and_delete() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let listed = engine
        .request("schedule.list.request", json!({}), now())
        .unwrap();
    assert_eq!(listed["schedules"][0]["name"], "test");
    assert!(listed["schedules"][0].get("runs").is_none());
    assert_eq!(listed["schedules"][0]["target"]["type"], "agent");
    let inspect = engine
        .request("schedule.inspect.request", json!({"scheduleId":id}), now())
        .unwrap();
    assert_eq!(inspect["schedule"]["runs"], json!([]));
    assert_eq!(
        engine
            .request("schedule.logs.request", json!({"scheduleId":id}), now())
            .unwrap()["runs"],
        json!([])
    );
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    assert!(engine.due(now()).unwrap().is_empty());
    engine
        .request("schedule.resume.request", json!({"scheduleId":id}), now())
        .unwrap();
    assert_eq!(
        engine.inspect(&id).unwrap().next_run_at,
        Some(now() + chrono::Duration::minutes(1))
    );
    engine
        .request("schedule.delete.request", json!({"scheduleId":id}), now())
        .unwrap();
    assert!(matches!(engine.inspect(&id), Err(Error::NotFound)));
    engine
        .request("schedule.delete.request", json!({"scheduleId":id}), now())
        .unwrap();
}
#[test]
fn failed_writes_do_not_publish_mutation() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    store.0.lock().unwrap().1 = true;
    assert_eq!(
        engine
            .request(
                "schedule.update.request",
                json!({"scheduleId":id,"prompt":"changed"}),
                now()
            )
            .unwrap_err(),
        Error::Storage
    );
    assert_eq!(engine.inspect(&id).unwrap().prompt, "hello");
    assert!(engine.begin(&id, true, now()).is_err());
    assert!(engine.inspect(&id).unwrap().runs.is_empty());
}
#[test]
fn manual_paused_execution_preserves_cadence_and_ignores_limits() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"maxRuns":1,"expiresAt":now()}),
            now(),
        )
        .unwrap();
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    assert_eq!(engine.begin(&id, true, now()).unwrap_err(), Error::Conflict);
    engine
        .finish(&id, &run, true, Outcome::default(), now())
        .unwrap();
    let record = engine.inspect(&id).unwrap();
    assert_eq!(record.status, Status::Paused);
    assert!(record.next_run_at.is_none());
    assert!(engine.begin(&id, true, now()).is_ok());
}
#[test]
fn failures_count_towards_max_runs_and_target_gone_completes() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"maxRuns":1}),
            now(),
        )
        .unwrap();
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    engine
        .finish(
            &id,
            &run,
            false,
            Outcome {
                error: Some("failed".into()),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Completed);
    assert!(engine.begin(&id, true, now()).is_err());
    assert!(
        engine
            .request("schedule.resume.request", json!({"scheduleId":id}), now())
            .is_err()
    );
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    engine
        .finish(
            &id,
            &run,
            true,
            Outcome {
                target_gone: true,
                error: Some("gone".into()),
                ..Outcome::default()
            },
            now(),
        )
        .unwrap();
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Completed);
}
#[test]
fn expiry_and_cron_defaults() {
    let (mut engine, _) = fixture();
    let mut params = input();
    params["expiresAt"] = json!(now());
    engine
        .request("schedule.create.request", params, now())
        .unwrap();
    assert!(engine.due(now()).unwrap().is_empty());
    assert_eq!(engine.records[0].status, Status::Completed);
    let mut params = input();
    params["cadence"] = json!({"type":"cron","expression":"* * * * *"});
    let created = engine
        .request("schedule.create.request", params.clone(), now())
        .unwrap();
    assert_eq!(
        created["schedule"]["nextRunAt"],
        json!(now() + chrono::Duration::minutes(1))
    );
    params["runOnCreate"] = json!(true);
    let created = engine
        .request("schedule.create.request", params, now())
        .unwrap();
    assert_eq!(created["schedule"]["nextRunAt"], json!(now()));
}
#[test]
fn in_flight_edits_pause_and_deletion_survive_completion() {
    let (mut engine, _) = fixture();
    let id = create(&mut engine);
    let (_, run) = engine.begin(&id, false, now()).unwrap();
    engine
        .request("schedule.pause.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine
        .request(
            "schedule.update.request",
            json!({"scheduleId":id,"name":"new"}),
            now(),
        )
        .unwrap();
    engine
        .finish(&id, &run, false, Outcome::default(), now())
        .unwrap();
    assert_eq!(engine.inspect(&id).unwrap().status, Status::Paused);
    assert_eq!(engine.inspect(&id).unwrap().name.as_deref(), Some("new"));
    let (_, run) = engine.begin(&id, true, now()).unwrap();
    engine
        .request("schedule.delete.request", json!({"scheduleId":id}), now())
        .unwrap();
    engine
        .finish(&id, &run, true, Outcome::default(), now())
        .unwrap();
    assert!(engine.records.is_empty());
}
#[test]
fn restart_marks_interrupted_failed_and_skips_missed_slots() {
    let (mut engine, store) = fixture();
    let id = create(&mut engine);
    let (_, run_id) = engine.begin(&id, false, now()).unwrap();
    let (reply, _) = tokio::sync::oneshot::channel();
    engine
        .checkpoint(&crate::ports::Checkpoint {
            schedule_id: id.clone(),
            run_id,
            agent_id: Some("agent".into()),
            workspace_id: Some("workspace".into()),
            reply,
        })
        .unwrap();
    let recovered = Engine::open(Box::new(store), now() + chrono::Duration::minutes(3))
        .unwrap()
        .inspect(&id)
        .unwrap();
    assert_eq!(recovered.runs[0].status, RunStatus::Failed);
    assert_eq!(recovered.runs[0].workspace_id.as_deref(), Some("workspace"));
    assert_eq!(
        recovered.next_run_at,
        Some(now() + chrono::Duration::minutes(4))
    );
}
#[test]
fn update_inherits_cron_zone_and_clears_nullable_fields() {
    let (mut engine, _) = fixture();
    let mut params = input();
    params["target"] =
        json!({"type":"new-agent","config":{"provider":"codex","cwd":"/tmp","model":"old"}});
    params["cadence"] = json!({"type":"cron","expression":"0 9 * * *","timezone":"Asia/Shanghai"});
    let id = engine
        .request("schedule.create.request", params, now())
        .unwrap()["schedule"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let updated=engine.request("schedule.update.request",json!({"scheduleId":id,"name":null,"maxRuns":null,"expiresAt":null,"cadence":{"type":"cron","expression":"0 10 * * *"},"newAgentConfig":{"model":null,"modeId":"plan"}}),now()).unwrap();
    assert_eq!(updated["schedule"]["cadence"]["timezone"], "Asia/Shanghai");
    assert!(
        updated["schedule"]["target"]["config"]
            .get("model")
            .is_none()
    );
    assert_eq!(updated["schedule"]["name"], Value::Null);
}
#[test]
fn invalid_inputs_never_write() {
    let (mut engine, _) = fixture();
    for patch in [
        json!({"prompt":" "}),
        json!({"maxRuns":0}),
        json!({"cadence":{"type":"every","everyMs":0}}),
        json!({"target":{"type":"agent","agentId":"bad"}}),
        json!({"surprise":true}),
    ] {
        let mut params = input();
        params
            .as_object_mut()
            .unwrap()
            .extend(patch.as_object().unwrap().clone());
        assert!(
            engine
                .request("schedule.create.request", params, now())
                .is_err()
        );
    }
    assert!(engine.records.is_empty());
    assert!(
        engine
            .request("schedule.list.request", json!({"extra":true}), now())
            .is_err()
    );
}

mod lifecycle;
mod mutation;
mod recovery;
