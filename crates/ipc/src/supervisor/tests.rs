//! Virtual time exercises the real framed supervision loop without model or process side effects.
use super::*;
use ait_contracts::worker::{StoreRequest, StoreResponse, codex::Operation};

struct Finished;

#[async_trait]
impl Handler for Finished {
    async fn request(
        &self,
        _lease: &Lease,
        _operation_id: &str,
        _request: StoreRequest,
    ) -> Result<StoreResponse, ProtocolError> {
        Err(ProtocolError::InvalidFrame)
    }

    async fn finished(&self) -> Result<(), ProtocolError> {
        Ok(())
    }
}

fn native_writer() -> Executor {
    Executor::Codex {
        binary: "unused-codex".into(),
        operation: Box::new(Operation::Open {
            request_id: "run".into(),
            thread_id: None,
            prompt: "offline fixture".into(),
            model: "fixture".into(),
            reasoning_effort: None,
            developer_instructions: None,
        }),
    }
}

fn connection(executor: Executor) -> (Bootstrap, Connection, Connection) {
    let bootstrap = Bootstrap {
        lease: Lease {
            project_owner: None,
            scope_id: "run".into(),
            worker_instance_id: "worker".into(),
            lease_epoch: 1,
        },
        workdir: "/fixture".into(),
        permission: ait_domain::RunPermissionProfile::default().to_wire(),
        maximum_sandbox: ait_domain::SandboxAccess::ReadOnly.to_wire(),
        limits: Limits::default(),
        executor,
    };
    let (daemon, worker) = tokio::io::duplex(MAX_FRAME_BYTES as usize);
    let (daemon_read, daemon_write) = tokio::io::split(daemon);
    let (worker_read, worker_write) = tokio::io::split(worker);
    let daemon = Connection::start(
        Reader::new(daemon_read, MAX_FRAME_BYTES),
        Writer::new(daemon_write, MAX_FRAME_BYTES),
        bootstrap.lease.clone(),
        None,
    );
    let worker = Connection::start(
        Reader::new(worker_read, MAX_FRAME_BYTES),
        Writer::new(worker_write, MAX_FRAME_BYTES),
        bootstrap.lease.clone(),
        None,
    );
    (bootstrap, daemon, worker)
}

async fn respond_until_exit(worker: &mut Connection, duration: Duration) -> bool {
    let finish = tokio::time::sleep(duration);
    tokio::pin!(finish);
    loop {
        tokio::select! {
            frame = worker.receiver.recv() => {
                match frame.unwrap().unwrap().payload {
                    Payload::Heartbeat => worker.send(Payload::Heartbeat).await.unwrap(),
                    Payload::Cancel => {
                        worker.send(Payload::ExitReport).await.unwrap();
                        return true;
                    }
                    payload => panic!("unexpected supervisor frame: {payload:?}"),
                }
            }
            () = &mut finish => {
                worker.send(Payload::ExitReport).await.unwrap();
                return false;
            }
        }
    }
}

#[tokio::test(start_paused = true)]
async fn native_writer_remains_alive_past_the_old_five_minute_ceiling() {
    let (bootstrap, mut daemon, mut worker) = connection(native_writer());
    let supervisor = WorkerSupervisor::new(PathBuf::from("unused-worker"));
    let duration = Duration::from_hours(24);
    let started = Instant::now();
    let (result, cancelled) = tokio::join!(
        supervisor.serve(
            &mut daemon,
            &Finished,
            &bootstrap,
            CancellationToken::new(),
            1
        ),
        respond_until_exit(&mut worker, duration),
    );
    assert_eq!(result, Ok(()));
    assert!(!cancelled);
    assert!(started.elapsed() >= duration);
    daemon.close().await;
    worker.close().await;
}

#[tokio::test(start_paused = true)]
async fn api_and_auxiliary_codex_operations_keep_their_deadline() {
    for executor in [
        Executor::Scripted { replies: vec![] },
        Executor::Codex {
            binary: "unused-codex".into(),
            operation: Box::new(Operation::Read {
                thread_id: "thread".into(),
            }),
        },
    ] {
        let (bootstrap, mut daemon, mut worker) = connection(executor);
        let supervisor = WorkerSupervisor::new(PathBuf::from("unused-worker"));
        let (result, cancelled) = tokio::join!(
            supervisor.serve(
                &mut daemon,
                &Finished,
                &bootstrap,
                CancellationToken::new(),
                1
            ),
            respond_until_exit(&mut worker, Duration::from_mins(10)),
        );
        assert_eq!(result, Ok(()));
        assert!(cancelled);
        daemon.close().await;
        worker.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn native_writer_still_honors_user_cancellation_and_daemon_shutdown() {
    for shutdown in [false, true] {
        let (bootstrap, mut daemon, mut worker) = connection(native_writer());
        let supervisor = WorkerSupervisor::new(PathBuf::from("unused-worker"));
        let cancel = CancellationToken::new();
        let stop = async {
            tokio::time::sleep(Duration::from_mins(10)).await;
            if shutdown {
                supervisor.drain();
            } else {
                cancel.cancel();
            }
        };
        let (result, cancelled, ()) = tokio::join!(
            supervisor.serve(&mut daemon, &Finished, &bootstrap, cancel.clone(), 1),
            respond_until_exit(&mut worker, Duration::from_mins(20)),
            stop,
        );
        assert_eq!(result, Ok(()));
        assert!(cancelled);
        daemon.close().await;
        worker.close().await;
    }
}

#[tokio::test(start_paused = true)]
async fn an_unresponsive_native_writer_still_hits_the_heartbeat_timeout() {
    let (bootstrap, mut daemon, worker) = connection(native_writer());
    let supervisor = WorkerSupervisor::new(PathBuf::from("unused-worker"));
    assert_eq!(
        supervisor
            .serve(
                &mut daemon,
                &Finished,
                &bootstrap,
                CancellationToken::new(),
                1
            )
            .await,
        Err(ProtocolError::HeartbeatTimeout),
    );
    daemon.close().await;
    worker.close().await;
}

#[tokio::test(start_paused = true)]
async fn native_cancellation_cannot_wait_forever_for_a_worker_to_exit() {
    let (bootstrap, mut daemon, mut worker) = connection(native_writer());
    let supervisor = WorkerSupervisor::new(PathBuf::from("unused-worker"));
    let cancel = CancellationToken::new();
    cancel.cancel();
    let result = {
        let ignore_cancellation = async {
            while let Some(Ok(frame)) = worker.receiver.recv().await {
                if matches!(frame.payload, Payload::Heartbeat) {
                    worker.send(Payload::Heartbeat).await.unwrap();
                }
            }
        };
        tokio::pin!(ignore_cancellation);
        tokio::select! {
            result = supervisor.serve(&mut daemon, &Finished, &bootstrap, cancel, 1) => result,
            () = &mut ignore_cancellation => panic!("worker pipe closed before drain deadline"),
        }
    };
    assert_eq!(result, Err(ProtocolError::WorkerExited));
    daemon.close().await;
    worker.close().await;
}
