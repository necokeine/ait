use super::*;
use crate::tests::runtime;

mod polling;

#[tokio::test]
async fn admission_rejects_missing_exhausted_and_draining_work() {
    let runtime = runtime();
    let service = Arc::new(Mutex::new(()));
    assert_eq!(
        runtime
            .run::<(), ()>(None, ErrorCode::RegistryIo, |()| Ok(()))
            .await,
        Err(ErrorCode::UnsupportedCapability)
    );
    let permit = runtime.jobs.clone().acquire_owned().await.unwrap();
    assert_eq!(
        runtime
            .run(Some(service.clone()), ErrorCode::RegistryIo, |()| Ok(()))
            .await,
        Err(ErrorCode::ResourceExhausted)
    );
    drop(permit);
    runtime.cancellation.cancel();
    assert_eq!(runtime.info().lifecycle, Lifecycle::Draining);
    assert_eq!(
        runtime
            .run(Some(service), ErrorCode::RegistryIo, |()| Ok(()))
            .await,
        Err(ErrorCode::ServerDraining)
    );
}

#[tokio::test]
async fn dropped_response_keeps_the_started_job_tracked_until_it_finishes() {
    let runtime = runtime();
    let service = Arc::new(Mutex::new(0_u8));
    let (started, start) = tokio::sync::oneshot::channel();
    let (release, released) = std::sync::mpsc::channel();
    let task_runtime = runtime.clone();
    let task_service = service.clone();
    let task = tokio::spawn(async move {
        task_runtime
            .run(Some(task_service), ErrorCode::RegistryIo, move |value| {
                started.send(()).unwrap();
                released.recv().unwrap();
                *value = 1;
                Ok(())
            })
            .await
    });
    start.await.unwrap();
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    assert_eq!(runtime.jobs.available_permits(), 0);
    assert_eq!(runtime.tasks.len(), 1);
    runtime.tasks.close();
    release.send(()).unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(5), runtime.tasks.wait())
        .await
        .unwrap();
    assert_eq!(runtime.jobs.available_permits(), 1);
    assert_eq!(*service.lock().unwrap(), 1);
}
