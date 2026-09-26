//! Deterministic runner barriers shared by schedule actor tests.
use super::*;

pub(super) struct Started {
    pub schedule: Schedule,
    pub progress: Progress,
    pub complete: oneshot::Sender<Outcome>,
}

#[derive(Debug)]
pub(super) struct Controlled {
    started: mpsc::UnboundedSender<Started>,
    pub calls: AtomicUsize,
}

impl Controlled {
    pub fn new() -> (Arc<Self>, mpsc::UnboundedReceiver<Started>) {
        let (started, receive) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                started,
                calls: AtomicUsize::new(0),
            }),
            receive,
        )
    }
}

impl Runner for Controlled {
    fn run(
        &self,
        schedule: Schedule,
        _: String,
        progress: Progress,
        cancel: CancellationToken,
    ) -> Pin<Box<dyn Future<Output = Outcome> + Send + '_>> {
        Box::pin(async move {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let (complete, receive) = oneshot::channel();
            self.started
                .send(Started {
                    schedule,
                    progress,
                    complete,
                })
                .unwrap_or_else(|_| panic!("controlled runner receiver must stay alive"));
            tokio::select! {
                result = receive => result.expect("test must complete accepted occurrence"),
                () = cancel.cancelled() => Outcome { error: Some("cancelled".into()), ..Outcome::default() },
            }
        })
    }
}

pub(super) async fn create(service: &Schedules) -> Value {
    let mut params = input();
    params["runOnCreate"] = json!(false);
    service
        .execute("schedule.create.request", params)
        .await
        .unwrap()["schedule"]["id"]
        .clone()
}

pub(super) fn run(
    service: &Schedules,
    id: &Value,
) -> tokio::task::JoinHandle<Result<Value, Error>> {
    let service = service.clone();
    let id = id.clone();
    tokio::spawn(async move {
        service
            .execute("schedule.run_once.request", json!({"scheduleId":id}))
            .await
    })
}

pub(super) async fn started(receive: &mut mpsc::UnboundedReceiver<Started>) -> Started {
    tokio::time::timeout(Duration::from_secs(5), receive.recv())
        .await
        .unwrap()
        .unwrap()
}

pub(super) async fn settled(
    run: tokio::task::JoinHandle<Result<Value, Error>>,
) -> Result<Value, Error> {
    tokio::time::timeout(Duration::from_secs(5), run)
        .await
        .unwrap()
        .unwrap()
}
