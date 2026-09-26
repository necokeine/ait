use std::future::{Future, poll_fn};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Duration;

use crate::ErrorCode;
use crate::tests::runtime;

#[tokio::test]
async fn saturated_checkout_polling_does_not_consume_foreground_admission() {
    let runtime = runtime();
    let held = runtime
        .checkout_poll_jobs
        .clone()
        .acquire_owned()
        .await
        .unwrap();
    assert!(
        runtime
            .checkout_poll_jobs
            .clone()
            .try_acquire_owned()
            .is_err()
    );
    let mut queued = Box::pin(runtime.checkout_poll_jobs.clone().acquire_owned());
    poll_fn(|context| {
        assert!(queued.as_mut().poll(context).is_pending());
        Poll::Ready(())
    })
    .await;

    let service = Arc::new(Mutex::new(0_u8));
    let result = tokio::time::timeout(
        Duration::from_secs(5),
        runtime.run(Some(service.clone()), ErrorCode::RegistryIo, |value| {
            *value += 1;
            Ok(*value)
        }),
    )
    .await
    .expect("unrelated foreground service proceeds while all poll permits are occupied");
    assert_eq!(result, Ok(1));
    assert_eq!(*service.lock().unwrap(), 1);
    assert_eq!(runtime.checkout_poll_jobs.available_permits(), 0);
    assert_eq!(runtime.jobs.available_permits(), 1);

    drop(held);
    drop(queued.await.unwrap());
    assert_eq!(runtime.checkout_poll_jobs.available_permits(), 1);
}
