use super::*;

fn upload_params(size: u64) -> Value {
    json!({"fileName":"a.txt","mimeType":"text/plain","size":size,"modifiedAt":"now"})
}

#[test]
fn upload_admission_limits_and_same_id_replacement_are_connection_local() {
    let mut connection = FileConnection::default();
    for index in 0..8 {
        connection
            .begin_upload(&index.to_string(), upload_params(1))
            .unwrap();
    }
    assert!(matches!(
        connection.begin_upload("overflow", upload_params(1)),
        Err(ErrorCode::ResourceExhausted)
    ));
    let old = connection.uploads["0"].metadata.id.clone();
    connection.begin_upload("0", upload_params(2)).unwrap();
    assert_ne!(connection.uploads["0"].metadata.id, old);
    assert_eq!(connection.uploads.len(), 8);
    assert!(FileConnection::default().uploads.is_empty());
}

#[test]
fn upload_expiration_and_invalid_sizes_do_not_retain_pending_state() {
    let mut connection = FileConnection::default();
    connection.begin_upload("old", upload_params(1)).unwrap();
    connection.uploads.get_mut("old").unwrap().touched = Instant::now()
        .checked_sub(Duration::from_secs(601))
        .unwrap();
    connection.prune_uploads();
    assert!(connection.uploads.is_empty());
    assert!(matches!(
        connection.begin_upload("huge", upload_params(64 * 1024 * 1024 + 1)),
        Err(ErrorCode::ResourceExhausted)
    ));
    assert!(matches!(
        connection.begin_upload(
            "bad",
            json!({"fileName":"","mimeType":"x","size":0,"modifiedAt":"now"})
        ),
        Err(ErrorCode::InvalidMessage)
    ));
}
