use super::*;

#[test]
fn defaults_cover_every_setting_once() {
    let schema = settings_schema();
    let defaults = default_settings();
    assert_eq!(schema.definitions.len(), defaults.0.len());
    assert!(
        schema
            .definitions
            .iter()
            .all(|item| defaults.0.contains_key(&item.id))
    );
}

#[test]
fn provider_settings_have_a_single_authoritative_catalog() {
    assert!(
        settings_schema()
            .definitions
            .iter()
            .all(|item| !item.id.starts_with("models."))
    );
}
