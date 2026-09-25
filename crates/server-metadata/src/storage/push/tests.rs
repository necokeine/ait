use super::*;
use crate::service::push::PushTokens;
use serde_json::json;

#[test]
fn private_atomic_file_reloads_and_migrates_legacy_tokens() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("push-tokens.json");
    let store = FileTokenStore::new(path.clone());
    assert_eq!(store.load().unwrap(), json!({}));
    store.save(&json!({"tokens":["legacy"]})).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            fs::metadata(root.path()).unwrap().permissions().mode() & 0o777,
            0o700
        );
        fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        // Loading alone repairs permissions; migration must not mask a missing repair.
        store.load().unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let mut service = PushTokens::open(Box::new(store), 0).unwrap();
    assert_eq!(service.active(0), ["legacy"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    service.revoke("legacy").unwrap();
    assert_eq!(
        FileTokenStore::new(path).load().unwrap(),
        json!({"subscriptions":[]})
    );
}

#[test]
fn invalid_documents_and_nonfiles_are_rejected_without_overwrite() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tokens");
    fs::write(&path, "corrupt").unwrap();
    assert_eq!(
        FileTokenStore::new(path.clone()).load(),
        Err(PushError::Invalid)
    );
    let store = FileTokenStore::new(root.path().to_path_buf());
    assert_eq!(store.load(), Err(PushError::Invalid));
    assert_eq!(store.save(&json!({})), Err(PushError::Invalid));
    #[cfg(unix)]
    {
        let link = root.path().join("link");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        let store = FileTokenStore::new(link);
        assert_eq!(store.load(), Err(PushError::Invalid));
        assert_eq!(store.save(&json!({})), Err(PushError::Invalid));
        assert_eq!(fs::read_to_string(path).unwrap(), "corrupt");
    }
}
