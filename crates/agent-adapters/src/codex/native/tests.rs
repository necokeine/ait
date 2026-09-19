use super::pre_send_error;
use ait_domain::ErrorCode;

#[cfg(unix)]
mod process;

#[test]
fn actual_writer_busy_rejection_is_not_an_unknown_submission() {
    let failure = pre_send_error(crate::AdapterError::protocol(
        "thread test already has an active writer",
    ));
    assert_eq!(failure.code, ErrorCode::CodexThreadWriterBusy);
}
