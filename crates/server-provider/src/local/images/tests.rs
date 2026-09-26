use super::*;

#[test]
fn windows_drive_and_network_paths_are_rendered_without_treating_them_as_base64() {
    let images = ImageStore::default();
    assert_eq!(
        images
            .render(&json!({"path":r"C:\images\result (1).png"}))
            .unwrap(),
        "![Image](C:/images/result%20%281%29.png)"
    );
    assert_eq!(
        images
            .render(&json!({"path":r"\\server\share\result.png"}))
            .unwrap(),
        "![Image](//server/share/result.png)"
    );
    assert!(images.render(&json!({"path":"C:\\bad\n.png"})).is_err());
}

#[test]
fn content_addressed_artifacts_survive_store_recreation_and_remove_raw_payload() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("images");
    let store = ImageStore::new(directory.clone());
    let block = json!({"type":"image","mimeType":"image/png","data":"aGVsbG8="});
    let (sanitized, images) = store
        .split(&json!({"content":[block,{"type":"text","text":"caption"}]}))
        .unwrap();
    assert_eq!(sanitized["content"][0]["text"], "[Image]");
    assert!(!sanitized.to_string().contains("aGVsbG8="));
    assert_eq!(
        images,
        vec![ImageStore::new(directory.clone()).render(&block).unwrap()]
    );
    let files: Vec<_> = std::fs::read_dir(directory).unwrap().collect();
    assert_eq!(files.len(), 1);
    assert_eq!(
        std::fs::read(files[0].as_ref().unwrap().path()).unwrap(),
        b"hello"
    );
}

#[test]
fn paths_and_urls_escape_markdown_and_malformed_images_fail_closed() {
    let root = tempfile::tempdir().unwrap();
    let store = ImageStore::new(root.path().join("images"));
    assert_eq!(
        store.render(&json!({"path":"/tmp/a (1).png"})).unwrap(),
        "![Image](/tmp/a%20%281%29.png)"
    );
    assert_eq!(
        store.render(&json!("https://example.test/a.png")).unwrap(),
        "![Image](https://example.test/a.png)"
    );
    assert!(store.render(&json!({"data":"?"})).is_err());
    assert!(
        store
            .render(&json!({"data":"aGVsbG8=","mimeType":"text/html"}))
            .is_err()
    );
    assert!(store.render(&json!("/tmp/line\nbreak")).is_err());
    assert!(
        store
            .render(&json!({"data":"x".repeat(2 * 1024 * 1024 + 1)}))
            .is_err()
    );
    assert_eq!(
        store
            .render(&json!("data:image/png;base64,aGVsbG8="))
            .unwrap(),
        store.render(&json!({"data":"aGVsbG8="})).unwrap()
    );
}

#[test]
fn damaged_existing_artifacts_are_never_overwritten() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("images");
    let store = ImageStore::new(directory.clone());
    let block = json!({"data":"aGVsbG8="});
    store.render(&block).unwrap();
    let path = std::fs::read_dir(directory)
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    std::fs::write(&path, b"other").unwrap();
    assert!(store.render(&block).is_err());
    assert_eq!(std::fs::read(path).unwrap(), b"other");
}

#[cfg(unix)]
#[test]
fn symlink_and_public_artifact_directories_are_rejected() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("public");
    std::fs::create_dir(&directory).unwrap();
    std::fs::set_permissions(&directory, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        ImageStore::new(directory.clone())
            .render(&json!({"data":"aGVsbG8="}))
            .is_err()
    );
    let link = root.path().join("link");
    symlink(&directory, &link).unwrap();
    assert!(
        ImageStore::new(link)
            .render(&json!({"data":"aGVsbG8="}))
            .is_err()
    );
}
