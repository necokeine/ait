use serde_json::json;

use super::*;

fn upload_params(size: u64) -> Value {
    json!({"fileName":"a.txt","mimeType":"text/plain","size":size,"modifiedAt":"now"})
}

#[test]
fn upload_admission_limits_and_same_id_replacement_are_connection_local() {
    let mut connection = Uploads::default();
    for index in 0..8 {
        connection
            .begin(&index.to_string(), upload_params(1))
            .unwrap();
    }
    assert!(matches!(
        connection.begin("overflow", upload_params(1)),
        Err(ErrorCode::ResourceExhausted)
    ));
    let old = connection.uploads["0"].metadata.id.clone();
    connection.begin("0", upload_params(2)).unwrap();
    assert_ne!(connection.uploads["0"].metadata.id, old);
    assert_eq!(connection.uploads.len(), 8);
    assert!(Uploads::default().uploads.is_empty());
}

#[test]
fn upload_expiration_and_invalid_sizes_do_not_retain_pending_state() {
    let mut connection = Uploads::default();
    connection.begin("old", upload_params(1)).unwrap();
    connection.uploads.get_mut("old").unwrap().touched = Instant::now()
        .checked_sub(Duration::from_secs(601))
        .unwrap();
    connection.prune();
    assert!(connection.uploads.is_empty());
    assert!(matches!(
        connection.begin("huge", upload_params(64 * 1024 * 1024 + 1)),
        Err(ErrorCode::ResourceExhausted)
    ));
    assert!(matches!(
        connection.begin(
            "bad",
            json!({"fileName":"","mimeType":"x","size":0,"modifiedAt":"now"})
        ),
        Err(ErrorCode::InvalidMessage)
    ));
}

#[test]
fn frames_are_ordered_connection_owned_and_partial_writers_are_removed_on_drop() {
    use crate::local::files::LocalFiles;
    use crate::protocol::file_transfer::FileBegin;
    let temp = tempfile::tempdir().unwrap();
    let files = Files::new(Box::new(LocalFiles::new(
        temp.path().to_path_buf(),
        temp.path(),
    )));
    let mut first = Uploads::default();
    let mut second = Uploads::default();
    first.begin("one", upload_params(3)).unwrap();
    assert!(second.take("one").is_none());
    let upload = first.take("one").unwrap();
    assert!(upload.apply(FileFrame::Chunk(vec![1]), &files).is_err());
    first.begin("one", upload_params(3)).unwrap();
    let begin = FileFrame::Begin(FileBegin {
        mime: "text/plain".to_owned(),
        size: 3,
        encoding: "utf-8".to_owned(),
        modified_at: "now".to_owned(),
        revision: None,
        file_name: None,
    });
    let UploadStep::Pending(upload) = first.take("one").unwrap().apply(begin, &files).unwrap()
    else {
        panic!("expected pending upload")
    };
    let UploadStep::Pending(upload) = upload
        .apply(FileFrame::Chunk(b"abc".to_vec()), &files)
        .unwrap()
    else {
        panic!("expected pending upload")
    };
    first.resume("one".to_owned(), upload);
    let upload = first.take("one").unwrap();
    let UploadStep::Complete(file) = upload.apply(FileFrame::End, &files).unwrap() else {
        panic!("expected completed upload")
    };
    assert_eq!(std::fs::read(&file.path).unwrap(), b"abc");
    first.begin("partial", upload_params(3)).unwrap();
    let upload = first.take("partial").unwrap();
    let begin = FileFrame::Begin(FileBegin {
        mime: "text/plain".to_owned(),
        size: 3,
        encoding: "utf-8".to_owned(),
        modified_at: "now".to_owned(),
        revision: None,
        file_name: None,
    });
    let UploadStep::Pending(upload) = upload.apply(begin, &files).unwrap() else {
        panic!("expected pending upload")
    };
    let count_with_partial = std::fs::read_dir(temp.path().join("uploads"))
        .unwrap()
        .count();
    first.resume("partial".to_owned(), upload);
    drop(first);
    assert!(
        std::fs::read_dir(temp.path().join("uploads"))
            .unwrap()
            .count()
            < count_with_partial
    );
    assert_eq!(std::fs::read(&file.path).unwrap(), b"abc");
}
