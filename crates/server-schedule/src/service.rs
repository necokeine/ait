//! Bounded schedule actor: disk operations run on a dedicated thread, not the WebSocket reactor.
use crate::{
    engine::{Engine, only},
    ports::{Checkpoint, Error, Outcome, Progress, Runner, Store},
};
use chrono::Utc;
use serde_json::{Value, json};
use std::{
    fmt,
    sync::{Arc, Mutex},
    thread::JoinHandle,
    time::Duration,
};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinSet,
};
use tokio_util::sync::CancellationToken;

type Reply = oneshot::Sender<Result<Value, Error>>;
enum Command {
    Request(String, Value, Reply),
}
struct Worker {
    sender: mpsc::Sender<Command>,
    cancel: CancellationToken,
    thread: Mutex<Option<JoinHandle<()>>>,
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}
/// Cloneable, bounded owner of persistent schedules and running occurrences.
#[derive(Clone)]
pub struct Schedules(Arc<Worker>);
impl fmt::Debug for Schedules {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Schedules").finish_non_exhaustive()
    }
}
impl Schedules {
    /// Open state, recover interrupted runs, and start the one-second scheduler.
    /// # Errors
    /// Returns storage or worker initialization failures.
    pub fn spawn(store: Box<dyn Store>, runner: Arc<dyn Runner>) -> Result<Self, Error> {
        let engine = Engine::open(store, Utc::now())?;
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|_| Error::Storage)?;
        let (sender, receiver) = mpsc::channel(64);
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        let thread = std::thread::Builder::new()
            .name("server-schedule".into())
            .spawn(move || runtime.block_on(serve(engine, runner, receiver, stop)))
            .map_err(|_| Error::Storage)?;
        Ok(Self(Arc::new(Worker {
            sender,
            cancel,
            thread: Mutex::new(Some(thread)),
        })))
    }
    /// Execute one canonical schedule request without holding the scheduler lane during a run.
    /// # Errors
    /// Returns admission, validation, storage, or missing-schedule errors.
    pub async fn execute(&self, method: &str, params: Value) -> Result<Value, Error> {
        if self.0.cancel.is_cancelled() {
            return Err(Error::Conflict);
        }
        let (reply, receive) = oneshot::channel();
        self.0
            .sender
            .try_send(Command::Request(method.into(), params, reply))
            .map_err(|_| Error::Conflict)?;
        receive.await.map_err(|_| Error::Storage)?
    }
    /// Stop admission and cancel accepted runs. Joining remains asynchronous.
    pub fn stop(&self) {
        self.0.cancel.cancel();
    }
    /// Cancel accepted runs, settle their records and join the worker.
    /// # Errors
    /// Returns an error if the worker panicked or failed to acknowledge shutdown.
    pub async fn shutdown(&self) -> Result<(), Error> {
        self.stop();
        let thread = self.0.thread.lock().map_err(|_| Error::Storage)?.take();
        if let Some(thread) = thread {
            tokio::task::spawn_blocking(move || thread.join())
                .await
                .map_err(|_| Error::Storage)?
                .map_err(|_| Error::Storage)?;
        }
        Ok(())
    }
}
struct Finished {
    id: String,
    run_id: String,
    manual: bool,
    outcome: Outcome,
    reply: Option<Reply>,
}
fn launch(
    engine: &mut Engine,
    jobs: &mut JoinSet<Finished>,
    runner: &Arc<dyn Runner>,
    cancel: &CancellationToken,
    progress: &mpsc::Sender<Checkpoint>,
    input: (String, bool, Option<Reply>),
) {
    let (id, manual, reply) = input;
    if jobs.len() >= 16 {
        if let Some(reply) = reply {
            let _ = reply.send(Err(Error::Conflict));
        }
        return;
    }
    match engine.begin(&id, manual, Utc::now()) {
        Ok((schedule, run_id)) => {
            let runner = runner.clone();
            let cancel = cancel.child_token();
            let progress = Progress {
                sender: progress.clone(),
                schedule_id: id.clone(),
                run_id: run_id.clone(),
            };
            jobs.spawn(async move {
                // The inner task converts a panicking host runner into a durable failed occurrence.
                let key = run_id.clone();
                let stop = cancel.clone();
                let job =
                    tokio::spawn(async move { runner.run(schedule, key, progress, stop).await });
                let outcome = match job.await {
                    Ok(result) => result,
                    Err(_) => Outcome {
                        error: Some("Schedule runner failed".into()),
                        ..Outcome::default()
                    },
                };
                Finished {
                    id,
                    run_id,
                    manual,
                    outcome,
                    reply,
                }
            });
        }
        Err(error) => {
            if let Some(reply) = reply {
                let _ = reply.send(Err(error));
            }
        }
    }
}
fn settle(engine: &mut Engine, mut finished: Finished) -> Result<(), Box<Finished>> {
    if engine
        .finish(
            &finished.id,
            &finished.run_id,
            finished.manual,
            finished.outcome.clone(),
            Utc::now(),
        )
        .is_err()
    {
        return Err(Box::new(finished));
    }
    if let Some(reply) = finished.reply.take() {
        let _ = reply.send(
            engine
                .inspect(&finished.id)
                .map(|schedule| json!({"schedule":schedule,"error":null})),
        );
    }
    Ok(())
}
async fn serve(
    mut engine: Engine,
    runner: Arc<dyn Runner>,
    mut receiver: mpsc::Receiver<Command>,
    cancel: CancellationToken,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(1));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let (progress, mut updates) = mpsc::channel::<Checkpoint>(64);
    let mut jobs = JoinSet::new();
    let mut pending = Vec::new();
    loop {
        tokio::select! {
            ()=cancel.cancelled()=>break,
            Some(update)=updates.recv()=>{let result=engine.checkpoint(&update);let _=update.reply.send(result);},
            command=receiver.recv()=>match command {
                Some(Command::Request(method,params,reply))=>{
                    if method=="schedule.run_once.request" {
                        if !pending.is_empty() { let _ = reply.send(Err(Error::Storage)); continue; }
                        let id=only(&params,&["scheduleId"]).and_then(|()|params["scheduleId"].as_str().map(str::to_owned).ok_or(Error::Invalid));
                        match id {Ok(id)=>launch(&mut engine,&mut jobs,&runner,&cancel,&progress,(id,true,Some(reply))),Err(error)=>{let _=reply.send(Err(error));}}
                    } else {let _=reply.send(engine.request(&method,params,Utc::now()));}
                },
                None=>break,
            },
            Some(result)=jobs.join_next(),if !jobs.is_empty()=>{if let Ok(finished)=result&& let Err(finished)=settle(&mut engine,finished){pending.push(*finished);}},
            _=tick.tick()=>{
                pending=pending.into_iter().filter_map(|finished|settle(&mut engine,finished).err().map(|entry|*entry)).collect();
                if pending.is_empty()&& let Ok(due)=engine.due(Utc::now()){for id in due {launch(&mut engine,&mut jobs,&runner,&cancel,&progress,(id,false,None));}}
            }
        }
    }
    cancel.cancel();
    let drained = async {
        while !jobs.is_empty() {
            tokio::select! {
                Some(update)=updates.recv()=>{let result=engine.checkpoint(&update);let _=update.reply.send(result);},
                Some(result)=jobs.join_next()=>{if let Ok(finished)=result&& let Err(finished)=settle(&mut engine,finished){pending.push(*finished);}}
            }
        }
    };
    let _ = tokio::time::timeout(Duration::from_secs(5), drained).await;
    jobs.abort_all();
    for finished in pending {
        let _ = settle(&mut engine, finished);
    }
}
#[cfg(test)]
mod tests;
