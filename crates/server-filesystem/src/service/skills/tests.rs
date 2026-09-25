use super::*;
use crate::protocol::skills::{Selection, Snapshot};

#[derive(Debug)]
struct FailingStore;
impl SkillStore for FailingStore {
    fn recover(&mut self) -> Result<(), ErrorCode> {
        Ok(())
    }
    fn selection(&self) -> Result<Option<Selection>, ErrorCode> {
        Ok(None)
    }
    fn import(&mut self, _: &Selection) -> Result<(), ErrorCode> {
        Err(ErrorCode::RegistryIo)
    }
    fn scan(&self, selection: &Selection) -> Result<Snapshot, ErrorCode> {
        Ok(Snapshot {
            state: "not-installed".into(),
            ops: vec![],
            available: vec![],
            installed: vec![],
            selection: selection.clone(),
        })
    }
    fn apply(&mut self, _: &Selection, _: &[Operation], _: ApplyMode) -> Result<(), ErrorCode> {
        Err(ErrorCode::RegistryIo)
    }
}

#[test]
fn requests_are_strict_and_io_failures_propagate() {
    let mut service = Skills::new(Box::new(FailingStore));
    for method in [skills::GET_STATUS, skills::RECONCILE, skills::UNINSTALL] {
        for params in [Value::Null, json!({"unexpected":true})] {
            assert_eq!(
                service.execute(method, params),
                Err(ErrorCode::InvalidMessage)
            );
        }
    }
    for params in [
        json!({"selection":{"mode":"all","skills":[]}}),
        json!({"selection":{"mode":"custom"}}),
        json!({"selection":{"mode":"all"},"unknown":1}),
        json!({"selection":{"mode":"all"},"confirmedRemovals":7}),
    ] {
        assert_eq!(
            service.execute(skills::SAVE_SELECTION, params),
            Err(ErrorCode::InvalidMessage)
        );
    }
    assert_eq!(
        service.execute("unknown", json!({})),
        Err(ErrorCode::MethodNotFound)
    );
    assert_eq!(
        service.execute(
            skills::IMPORT_LEGACY_SELECTION,
            json!({"selection":{"mode":"all"}})
        ),
        Err(ErrorCode::RegistryIo)
    );
    assert_eq!(
        service.execute(skills::SAVE_SELECTION, json!({"selection":{"mode":"all"}})),
        Err(ErrorCode::RegistryIo)
    );
    assert_eq!(
        service.execute(skills::RECONCILE, json!({})),
        Err(ErrorCode::RegistryIo)
    );
}
