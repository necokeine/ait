use std::sync::{Arc, Mutex};

use super::*;

#[derive(Default)]
struct Memory {
    value: Value,
    fail: bool,
    writes: usize,
}

#[derive(Clone, Default)]
struct Store(Arc<Mutex<Memory>>);

impl fmt::Debug for Store {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Store")
    }
}

impl TokenStore for Store {
    fn load(&self) -> Result<Value, PushError> {
        Ok(self.0.lock().unwrap().value.clone())
    }
    fn save(&self, document: &Value) -> Result<(), PushError> {
        let mut state = self.0.lock().unwrap();
        if state.fail {
            return Err(PushError::Io);
        }
        state.value = document.clone();
        state.writes += 1;
        Ok(())
    }
}

fn fixture(value: Value) -> (Store, PushTokens) {
    let store = Store::default();
    store.0.lock().unwrap().value = value;
    let service = PushTokens::open(Box::new(store.clone()), 0).unwrap();
    (store, service)
}

#[test]
fn trims_and_renews_only_at_half_lease_and_survives_reload() {
    let (store, mut service) = fixture(json!({}));
    service.renew("  secret  ", 0).unwrap();
    service.renew("secret", LEASE_MS / 2 - 1).unwrap();
    assert_eq!(store.0.lock().unwrap().writes, 1);
    service.renew("secret", LEASE_MS / 2).unwrap();
    assert_eq!(store.0.lock().unwrap().writes, 2);
    assert!(!format!("{service:?}").contains("secret"));
    let mut reloaded = PushTokens::open(Box::new(store.clone()), LEASE_MS).unwrap();
    assert_eq!(reloaded.active(LEASE_MS), ["secret"]);
    assert!(reloaded.active(LEASE_MS + LEASE_MS / 2).is_empty());
    assert_eq!(store.0.lock().unwrap().value, json!({"subscriptions":[]}));
}

#[test]
fn failed_writes_do_not_change_live_leases_and_pruning_excludes_expired_tokens() {
    let (store, mut service) = fixture(json!({}));
    service.renew("token", 0).unwrap();
    store.0.lock().unwrap().fail = true;
    assert_eq!(service.revoke("token"), Err(PushError::Io));
    assert_eq!(service.renew("new", 0), Err(PushError::Io));
    assert_eq!(service.renew("token", LEASE_MS), Err(PushError::Io));
    assert_eq!(service.active(0), ["token"]);
    assert!(service.active(LEASE_MS).is_empty());
    assert_eq!(store.0.lock().unwrap().writes, 1);
    store.0.lock().unwrap().fail = false;
    service.revoke(" token ").unwrap();
    service.revoke("token").unwrap();
    assert_eq!(store.0.lock().unwrap().writes, 2);
    assert!(service.active(0).is_empty());
}

#[test]
fn migrates_legacy_tokens_and_ignores_invalid_subscription_entries() {
    let (store, mut service) = fixture(
        json!({"tokens":[" a ","",5],"subscriptions":[null,{"token":"bad","expiresAt":"bad"},{"token":"b","expiresAt":"1970-01-02T00:00:00Z"}]}),
    );
    assert_eq!(service.active(0), ["a", "b"]);
    assert!(store.0.lock().unwrap().value.get("tokens").is_none());
    let store = Store::default();
    store.0.lock().unwrap().value = json!({"tokens":["secret"]});
    store.0.lock().unwrap().fail = true;
    assert!(matches!(
        PushTokens::open(Box::new(store), 0),
        Err(PushError::Io)
    ));
}

#[test]
fn blank_tokens_are_noops_and_capacity_is_bounded() {
    let (store, mut service) = fixture(json!({}));
    service.renew(" ", 0).unwrap();
    service.revoke("").unwrap();
    assert_eq!(store.0.lock().unwrap().writes, 0);
    assert_eq!(service.renew(&"x".repeat(4097), 0), Err(PushError::Invalid));
    service.subscriptions = (0..MAX_TOKENS).map(|i| (i.to_string(), LEASE_MS)).collect();
    assert_eq!(service.renew("overflow", 0), Err(PushError::Capacity));
    service.renew("overflow", LEASE_MS).unwrap();
    assert_eq!(service.active(LEASE_MS), ["overflow"]);
    assert!(matches!(
        PushTokens::open(Box::new(Store::default()), 0),
        Err(PushError::Invalid)
    ));
}
