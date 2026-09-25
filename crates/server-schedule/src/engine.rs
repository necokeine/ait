use crate::{
    cadence,
    ports::{Error, Outcome, Store},
    protocol::{Cadence, Create, Run, RunStatus, Schedule, Status, Target},
};
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) struct Engine {
    store: Box<dyn Store>,
    records: Vec<Schedule>,
}
impl Engine {
    pub fn open(store: Box<dyn Store>, now: DateTime<Utc>) -> Result<Self, Error> {
        let mut records = store.load()?;
        if records.len() > 1024 {
            return Err(Error::Storage);
        }
        let mut ids = std::collections::BTreeSet::new();
        let mut dirty = false;
        for schedule in &mut records {
            if !ids.insert(schedule.id.clone())
                || Uuid::parse_str(&schedule.id).is_err()
                || schedule.runs.len() > 4096
            {
                return Err(Error::Storage);
            }
            validate(&schedule.prompt, &schedule.target, schedule.max_runs)?;
            cadence::next(&schedule.cadence, now)?;
            for run in &mut schedule.runs {
                if run.status == RunStatus::Running {
                    run.status = RunStatus::Failed;
                    run.ended_at = Some(now);
                    run.error = Some("Server restarted before the scheduled run completed".into());
                    schedule.updated_at = now;
                    dirty = true;
                }
            }
            if schedule.status == Status::Active && schedule.next_run_at.is_some_and(|at| at <= now)
            {
                schedule.next_run_at = Some(advance(
                    &schedule.cadence,
                    schedule.next_run_at.unwrap_or(now),
                    now,
                )?);
                schedule.updated_at = now;
                dirty = true;
            }
        }
        let mut result = Self { store, records };
        if dirty {
            result.store.save(&result.records)?;
        }
        Ok(result)
    }
    fn commit(&mut self, records: Vec<Schedule>) -> Result<(), Error> {
        self.store.save(&records)?;
        self.records = records;
        Ok(())
    }
    pub fn inspect(&self, id: &str) -> Result<Schedule, Error> {
        self.records
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or(Error::NotFound)
    }
    pub fn request(
        &mut self,
        method: &str,
        params: Value,
        now: DateTime<Utc>,
    ) -> Result<Value, Error> {
        if method == "schedule.create.request" {
            let request: Create = serde_json::from_value(params).map_err(|_| Error::Invalid)?;
            validate(&request.prompt, &request.target, request.max_runs)?;
            let next = cadence::next(&request.cadence, now)?;
            if self.records.len() >= 1024 {
                return Err(Error::Conflict);
            }
            let immediate = request
                .run_on_create
                .unwrap_or(matches!(request.cadence, Cadence::Every { .. }));
            let record = Schedule {
                id: Uuid::new_v4().to_string(),
                name: normalize_name(request.name),
                prompt: request.prompt.trim().to_owned(),
                cadence: request.cadence,
                target: request.target,
                status: Status::Active,
                created_at: now,
                updated_at: now,
                next_run_at: Some(if immediate { now } else { next }),
                last_run_at: None,
                paused_at: None,
                expires_at: request.expires_at,
                max_runs: request.max_runs,
                runs: vec![],
            };
            let value = summary(&record)?;
            let mut records = self.records.clone();
            records.push(record);
            self.commit(records)?;
            return Ok(json!({"schedule":value,"error":null}));
        }
        if method == "schedule.list.request" {
            only(&params, &[])?;
            let mut records = self.records.clone();
            records.sort_by_key(|s| std::cmp::Reverse(s.created_at));
            return Ok(
                json!({"schedules":records.iter().map(summary).collect::<Result<Vec<_>,_>>()?,"error":null}),
            );
        }
        if method == "schedule.update.request" {
            return self.update(&params, now);
        }
        only(&params, &["scheduleId"])?;
        let id = params["scheduleId"].as_str().ok_or(Error::Invalid)?;
        if method == "schedule.delete.request" {
            let mut records = self.records.clone();
            records.retain(|s| s.id != id);
            self.commit(records)?;
            return Ok(json!({"scheduleId":id,"error":null}));
        }
        let mut record = self.inspect(id)?;
        match method {
            "schedule.inspect.request" => return Ok(json!({"schedule":record,"error":null})),
            "schedule.logs.request" => return Ok(json!({"runs":record.runs,"error":null})),
            "schedule.pause.request" | "schedule.resume.request" => {
                if record.status == Status::Completed {
                    return Err(Error::Conflict);
                }
                let desired = if method == "schedule.pause.request" {
                    Status::Paused
                } else {
                    Status::Active
                };
                if record.status != desired {
                    record.status = desired;
                    record.updated_at = now;
                    record.paused_at = if desired == Status::Paused {
                        Some(now)
                    } else {
                        None
                    };
                    record.next_run_at = if desired == Status::Paused {
                        None
                    } else {
                        Some(cadence::next(&record.cadence, now)?)
                    };
                    self.replace(record.clone())?;
                }
            }
            _ => return Err(Error::Invalid),
        }
        Ok(json!({"schedule":summary(&record)?,"error":null}))
    }
    fn replace(&mut self, record: Schedule) -> Result<(), Error> {
        let mut records = self.records.clone();
        let index = records
            .iter()
            .position(|s| s.id == record.id)
            .ok_or(Error::NotFound)?;
        records[index] = record;
        self.commit(records)
    }
    fn update(&mut self, params: &Value, now: DateTime<Utc>) -> Result<Value, Error> {
        only(
            params,
            &[
                "scheduleId",
                "name",
                "prompt",
                "cadence",
                "newAgentConfig",
                "maxRuns",
                "expiresAt",
            ],
        )?;
        let mut record = self.inspect(params["scheduleId"].as_str().ok_or(Error::Invalid)?)?;
        if let Some(value) = params.get("name") {
            record.name =
                normalize_name(serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?);
        }
        if let Some(value) = params.get("prompt") {
            value
                .as_str()
                .ok_or(Error::Invalid)?
                .trim()
                .clone_into(&mut record.prompt);
        }
        if let Some(value) = params.get("cadence") {
            let mut next: Cadence =
                serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
            if let (Cadence::Cron { timezone: old, .. }, Cadence::Cron { timezone: next, .. }) =
                (&record.cadence, &mut next)
                && next.is_none()
            {
                next.clone_from(old);
            }
            let at = cadence::next(&next, now)?;
            record.cadence = next;
            record.next_run_at = (record.status == Status::Active).then_some(at);
        }
        if let Some(value) = params.get("newAgentConfig") {
            only(
                value,
                &[
                    "provider",
                    "cwd",
                    "model",
                    "modeId",
                    "thinkingOptionId",
                    "archiveOnFinish",
                    "isolation",
                ],
            )?;
            let Target::NewAgent { config } = &mut record.target else {
                return Err(Error::Invalid);
            };
            for (key, value) in value.as_object().ok_or(Error::Invalid)? {
                if ["model", "modeId", "thinkingOptionId"].contains(&key.as_str())
                    && (value.is_null() || value.as_str().is_some_and(|s| s.trim().is_empty()))
                {
                    config.as_object_mut().ok_or(Error::Invalid)?.remove(key);
                } else {
                    config[key] = value.clone();
                }
            }
        }
        if let Some(value) = params.get("maxRuns") {
            record.max_runs = serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        }
        if let Some(value) = params.get("expiresAt") {
            record.expires_at =
                serde_json::from_value(value.clone()).map_err(|_| Error::Invalid)?;
        }
        validate(&record.prompt, &record.target, record.max_runs)?;
        record.updated_at = now;
        self.replace(record.clone())?;
        Ok(json!({"schedule":record,"error":null}))
    }
    pub fn due(&mut self, now: DateTime<Utc>) -> Result<Vec<String>, Error> {
        let mut records = self.records.clone();
        let mut dirty = false;
        let mut due = Vec::new();
        for schedule in &mut records {
            if schedule.status != Status::Active
                || schedule.next_run_at.is_none()
                || schedule.runs.iter().any(|r| r.status == RunStatus::Running)
            {
                continue;
            }
            if complete_due(schedule, now) {
                complete(schedule, now);
                dirty = true;
            } else if schedule.next_run_at.is_some_and(|at| at <= now) {
                due.push(schedule.id.clone());
            }
        }
        if dirty {
            self.commit(records)?;
        }
        Ok(due)
    }
    pub fn begin(
        &mut self,
        id: &str,
        manual: bool,
        now: DateTime<Utc>,
    ) -> Result<(Schedule, String), Error> {
        let mut schedule = self.inspect(id)?;
        if schedule.status == Status::Completed
            || (!manual && schedule.status != Status::Active)
            || schedule.runs.len() >= 4096
            || schedule.runs.iter().any(|r| r.status == RunStatus::Running)
        {
            return Err(Error::Conflict);
        }
        let run_id = Uuid::new_v4().to_string();
        schedule.runs.push(Run {
            id: run_id.clone(),
            scheduled_for: if manual {
                now
            } else {
                schedule.next_run_at.unwrap_or(now)
            },
            started_at: now,
            ended_at: None,
            status: RunStatus::Running,
            agent_id: None,
            workspace_id: None,
            output: None,
            error: None,
        });
        schedule.updated_at = now;
        self.replace(schedule.clone())?;
        Ok((schedule, run_id))
    }
    pub fn checkpoint(&mut self, update: &crate::ports::Checkpoint) -> Result<(), Error> {
        let mut schedule = self.inspect(&update.schedule_id)?;
        let run = schedule
            .runs
            .iter_mut()
            .find(|r| r.id == update.run_id && r.status == RunStatus::Running)
            .ok_or(Error::NotFound)?;
        run.agent_id.clone_from(&update.agent_id);
        run.workspace_id.clone_from(&update.workspace_id);
        self.replace(schedule)
    }
    pub fn finish(
        &mut self,
        id: &str,
        run_id: &str,
        manual: bool,
        outcome: Outcome,
        now: DateTime<Utc>,
    ) -> Result<(), Error> {
        let mut schedule = match self.inspect(id) {
            Ok(s) => s,
            Err(Error::NotFound) => return Ok(()),
            Err(e) => return Err(e),
        };
        let run = schedule
            .runs
            .iter_mut()
            .find(|r| r.id == run_id)
            .ok_or(Error::NotFound)?;
        run.status = if outcome.error.is_some() {
            RunStatus::Failed
        } else {
            RunStatus::Succeeded
        };
        run.ended_at = Some(now);
        if outcome.agent_id.is_some() {
            run.agent_id = outcome.agent_id;
        }
        if outcome.workspace_id.is_some() {
            run.workspace_id = outcome.workspace_id;
        }
        run.output = outcome.output.map(|s| limit_text(&s));
        run.error = outcome.error.map(|s| limit_text(&s));
        schedule.last_run_at = Some(now);
        schedule.updated_at = now;
        if outcome.target_gone
            || schedule.runs.len() >= 4096
            || (!manual && complete_due(&schedule, now))
        {
            complete(&mut schedule, now);
        } else if !manual && schedule.status == Status::Active {
            schedule.next_run_at = Some(advance(
                &schedule.cadence,
                schedule.next_run_at.unwrap_or(now),
                now,
            )?);
        }
        self.replace(schedule)
    }
}
fn advance(
    cadence: &Cadence,
    anchor: DateTime<Utc>,
    now: DateTime<Utc>,
) -> Result<DateTime<Utc>, Error> {
    if let Cadence::Every { every_ms } = cadence {
        let delta = now.signed_duration_since(anchor).num_milliseconds().max(0);
        let steps = delta
            .checked_div(*every_ms)
            .and_then(|n| n.checked_add(1))
            .ok_or(Error::Invalid)?;
        return anchor
            .checked_add_signed(chrono::Duration::milliseconds(
                steps.checked_mul(*every_ms).ok_or(Error::Invalid)?,
            ))
            .ok_or(Error::Invalid);
    }
    cadence::next(cadence, now)
}
fn summary(schedule: &Schedule) -> Result<Value, Error> {
    let mut value = serde_json::to_value(schedule).map_err(|_| Error::Storage)?;
    value.as_object_mut().ok_or(Error::Storage)?.remove("runs");
    Ok(value)
}
fn normalize_name(name: Option<String>) -> Option<String> {
    name.map(|s| s.trim().to_owned()).filter(|s| !s.is_empty())
}
fn complete_due(schedule: &Schedule, now: DateTime<Utc>) -> bool {
    schedule.expires_at.is_some_and(|at| at <= now)
        || schedule.max_runs.is_some_and(|max| {
            schedule
                .runs
                .iter()
                .filter(|r| r.status != RunStatus::Running)
                .count() as u64
                >= max
        })
}
fn complete(schedule: &mut Schedule, now: DateTime<Utc>) {
    schedule.status = Status::Completed;
    schedule.next_run_at = None;
    schedule.paused_at = None;
    schedule.updated_at = now;
}
pub(crate) fn only(value: &Value, fields: &[&str]) -> Result<(), Error> {
    if !value
        .as_object()
        .is_some_and(|map| map.keys().all(|key| fields.contains(&key.as_str())))
    {
        return Err(Error::Invalid);
    }
    Ok(())
}
fn validate(prompt: &str, target: &Target, max_runs: Option<u64>) -> Result<(), Error> {
    if prompt.trim().is_empty() || prompt.len() > 64 * 1024 || max_runs == Some(0) {
        return Err(Error::Invalid);
    }
    match target {
        Target::Agent { agent_id } => {
            Uuid::parse_str(agent_id).map_err(|_| Error::Invalid)?;
        }
        Target::NewAgent { config } => {
            only(
                config,
                &[
                    "provider",
                    "cwd",
                    "modeId",
                    "model",
                    "thinkingOptionId",
                    "archiveOnFinish",
                    "isolation",
                    "title",
                    "providerOptions",
                    "featureValues",
                    "systemPrompt",
                    "mcpServers",
                ],
            )?;
            for key in ["provider", "cwd"] {
                if config[key].as_str().is_none_or(|s| s.trim().is_empty()) {
                    return Err(Error::Invalid);
                }
            }
            for key in ["model", "modeId", "thinkingOptionId", "isolation"] {
                if let Some(value) = config.get(key)
                    && value.as_str().is_none_or(|s| s.trim().is_empty())
                {
                    return Err(Error::Invalid);
                }
            }
            if config
                .get("isolation")
                .is_some_and(|v| v != "local" && v != "worktree")
                || config
                    .get("archiveOnFinish")
                    .is_some_and(|v| !v.is_boolean())
            {
                return Err(Error::Invalid);
            }
        }
    }
    Ok(())
}
fn limit_text(text: &str) -> String {
    let end = text.floor_char_boundary((64 * 1024).min(text.len()));
    text[..end].to_owned()
}
#[cfg(test)]
pub(crate) mod tests;
