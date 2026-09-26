//! Paseo file-upload chunking, replacement, expiry, and disposal invariants.

use std::fs;

use super::*;
use crate::local::files::LocalFiles;
use crate::protocol::file_transfer::FileBegin;

fn fixture() -> (tempfile::TempDir, Files) {
    let root = tempfile::tempdir().unwrap();
    let files = Files::new(Box::new(LocalFiles::new(
        root.path().to_owned(),
        root.path(),
    )));
    (root, files)
}

fn begin_frame(size: u64) -> FileFrame {
    FileFrame::Begin(FileBegin {
        mime: "application/octet-stream".to_owned(),
        size,
        encoding: "binary".to_owned(),
        modified_at: "now".to_owned(),
        revision: None,
        file_name: None,
    })
}

fn pending(upload: Upload, frame: FileFrame, files: &Files) -> Upload {
    match upload.apply(frame, files).unwrap() {
        UploadStep::Pending(upload) => upload,
        UploadStep::Complete(_) => panic!("unexpected complete upload"),
    }
}

fn start(uploads: &mut Uploads, id: &str, size: u64, files: &Files) {
    uploads.begin(id, upload_params(size)).unwrap();
    let upload = pending(uploads.take(id).unwrap(), begin_frame(size), files);
    uploads.resume(id.to_owned(), upload);
}

fn count(root: &std::path::Path) -> usize {
    match fs::read_dir(root.join("uploads")) {
        Ok(entries) => entries.count(),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => 0,
        Err(error) => panic!("cannot inspect upload fixture: {error}"),
    }
}

#[test]
fn sequential_binary_chunks_are_concatenated_exactly_and_persist_only_on_end() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "binary", 6, &files);
    for chunk in [vec![0, 1], vec![255, 2, 3], vec![4]] {
        let upload = pending(
            uploads.take("binary").unwrap(),
            FileFrame::Chunk(chunk),
            &files,
        );
        uploads.resume("binary".to_owned(), upload);
    }
    let UploadStep::Complete(file) = uploads
        .take("binary")
        .unwrap()
        .apply(FileFrame::End, &files)
        .unwrap()
    else {
        panic!("expected completion")
    };
    assert_eq!(file.size, 6);
    assert_eq!(fs::read(&file.path).unwrap(), [0, 1, 255, 2, 3, 4]);
    assert_eq!(count(root.path()), 1);
    assert!(uploads.take("binary").is_none());
}

#[test]
fn oversized_chunk_aborts_and_removes_the_partial_file() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "overflow", 3, &files);
    let upload = pending(
        uploads.take("overflow").unwrap(),
        FileFrame::Chunk(vec![1, 2]),
        &files,
    );
    assert!(upload.apply(FileFrame::Chunk(vec![3, 4]), &files).is_err());
    assert_eq!(count(root.path()), 0);
    assert!(uploads.take("overflow").is_none());
}

#[test]
fn early_end_aborts_and_removes_an_incomplete_file() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "short", 3, &files);
    let upload = pending(
        uploads.take("short").unwrap(),
        FileFrame::Chunk(vec![1]),
        &files,
    );
    assert!(upload.apply(FileFrame::End, &files).is_err());
    assert_eq!(count(root.path()), 0);
}

#[test]
fn duplicate_binary_begin_aborts_the_old_writer() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "duplicate", 3, &files);
    assert!(
        uploads
            .take("duplicate")
            .unwrap()
            .apply(begin_frame(3), &files)
            .is_err()
    );
    assert_eq!(count(root.path()), 0);
}

#[test]
fn end_before_binary_begin_has_no_filesystem_side_effects() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    uploads.begin("early", upload_params(1)).unwrap();
    assert!(
        uploads
            .take("early")
            .unwrap()
            .apply(FileFrame::End, &files)
            .is_err()
    );
    assert!(!root.path().join("uploads").exists());
}

#[test]
fn repeated_request_id_replaces_and_cleans_the_previous_partial_upload() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "same", 3, &files);
    let old_identity = uploads.uploads["same"].metadata.id.clone();
    assert_eq!(count(root.path()), 1);
    uploads.begin("same", upload_params(1)).unwrap();
    assert_eq!(count(root.path()), 0);
    assert_ne!(uploads.uploads["same"].metadata.id, old_identity);
    let upload = pending(uploads.take("same").unwrap(), begin_frame(1), &files);
    let upload = pending(upload, FileFrame::Chunk(vec![7]), &files);
    let UploadStep::Complete(file) = upload.apply(FileFrame::End, &files).unwrap() else {
        panic!("expected completion")
    };
    assert_eq!(fs::read(file.path).unwrap(), [7]);
}

#[test]
fn invalid_duplicate_request_does_not_destroy_the_admitted_upload() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "same", 1, &files);
    let identity = uploads.uploads["same"].metadata.id.clone();
    assert!(
        uploads
            .begin(
                "same",
                json!({"fileName":"","mimeType":"text/plain","size":1,"modifiedAt":"now"})
            )
            .is_err()
    );
    assert_eq!(uploads.uploads["same"].metadata.id, identity);
    assert_eq!(count(root.path()), 1);
    let upload = pending(
        uploads.take("same").unwrap(),
        FileFrame::Chunk(vec![9]),
        &files,
    );
    assert!(matches!(
        upload.apply(FileFrame::End, &files).unwrap(),
        UploadStep::Complete(_)
    ));
}

#[test]
fn processing_a_chunk_refreshes_an_upload_near_its_idle_deadline() {
    let (_root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "active", 2, &files);
    let old = Instant::now()
        .checked_sub(Duration::from_secs(599))
        .unwrap();
    uploads.uploads.get_mut("active").unwrap().touched = old;
    let upload = pending(
        uploads.take("active").unwrap(),
        FileFrame::Chunk(vec![1]),
        &files,
    );
    uploads.resume("active".to_owned(), upload);
    uploads.prune();
    assert!(uploads.uploads["active"].touched > old);
    assert!(uploads.take("active").is_some());
}

#[test]
fn expiring_a_partial_upload_removes_its_directory_but_keeps_active_uploads() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "expired", 1, &files);
    start(&mut uploads, "active", 1, &files);
    uploads.uploads.get_mut("expired").unwrap().touched = Instant::now()
        .checked_sub(Duration::from_secs(601))
        .unwrap();
    uploads.prune();
    assert!(uploads.take("expired").is_none());
    assert_eq!(count(root.path()), 1);
    assert!(uploads.take("active").is_some());
}

#[test]
fn zero_byte_upload_finishes_without_a_chunk() {
    let (root, files) = fixture();
    let mut uploads = Uploads::default();
    start(&mut uploads, "empty", 0, &files);
    let UploadStep::Complete(file) = uploads
        .take("empty")
        .unwrap()
        .apply(FileFrame::End, &files)
        .unwrap()
    else {
        panic!("expected completion")
    };
    assert_eq!(file.size, 0);
    assert!(fs::read(file.path).unwrap().is_empty());
    assert_eq!(count(root.path()), 1);
}
