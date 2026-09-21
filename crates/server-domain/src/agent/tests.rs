use super::*;

#[test]
fn configurations_are_bounded_and_credentials_are_references_only() {
    let reference: CredentialRef = "env:AIT_SERVER_CREDENTIAL_TEST_1".parse().unwrap();
    assert_eq!(reference.to_string(), "env:AIT_SERVER_CREDENTIAL_TEST_1");
    for invalid in [
        "",
        "raw-secret-value",
        "env:HOME",
        "env:AIT_SERVER_TOKEN",
        "env:AIT_SERVER_CREDENTIAL_",
        "env:AIT_SERVER_CREDENTIAL_1TEST",
        "env:AIT_SERVER_CREDENTIAL_lower",
        "env:AIT_SERVER_CREDENTIAL_A=value",
    ] {
        assert!(invalid.parse::<CredentialRef>().is_err());
    }
    assert!(
        format!("env:AIT_SERVER_CREDENTIAL_{}", "A".repeat(65))
            .parse::<CredentialRef>()
            .is_err()
    );
    let config = AgentConfig::new(
        "Example".to_owned(),
        "codex".parse().unwrap(),
        "model/test-v1".to_owned(),
        Some(reference.clone()),
        true,
    )
    .unwrap();
    assert_eq!(config.name(), "Example");
    assert_eq!(config.model(), "model/test-v1");
    assert_eq!(config.driver().to_string(), "codex");
    assert_eq!(config.credential_ref(), Some(&reference));
    assert!(config.enabled());
    assert!("fake".parse::<Driver>().is_err());
    for name in ["", " \t", "bad\nname", &"a".repeat(256)] {
        assert!(
            AgentConfig::new(name.to_owned(), Driver::Codex, "m".to_owned(), None, true).is_err()
        );
    }
    for model in ["", "model name", "m?key=secret", &"a".repeat(129)] {
        assert!(
            AgentConfig::new("n".to_owned(), Driver::Codex, model.to_owned(), None, true).is_err()
        );
    }
}

#[test]
fn revision_identity_and_immutable_snapshot_bounds() {
    let id = AgentId::generate();
    assert_eq!(id.to_string().parse::<AgentId>().unwrap(), id);
    assert!(
        "00000000-0000-0000-0000-000000000000"
            .parse::<AgentId>()
            .is_err()
    );
    assert!(Revision::new(0).is_err());
    assert!(Revision::new(u64::MAX).is_err());
    assert!(Revision::new(i64::MAX as u64).unwrap().next().is_err());
    let revision = Revision::new(1).unwrap().next().unwrap();
    assert_eq!(revision.value(), 2);
    let config =
        AgentConfig::new("n".to_owned(), Driver::Codex, "m".to_owned(), None, false).unwrap();
    let snapshot = AgentSnapshot::new(id, revision, config.clone(), 42).unwrap();
    assert_eq!(snapshot.id(), id);
    assert_eq!(snapshot.revision(), revision);
    assert_eq!(snapshot.config(), &config);
    assert_eq!(snapshot.recorded_at(), 42);
    assert!(AgentSnapshot::new(id, revision, config, u64::MAX).is_err());
}
