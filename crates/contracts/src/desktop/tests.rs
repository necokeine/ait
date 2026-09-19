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

#[test]
fn settings_schema_keeps_agent_roles_with_agents_and_execution_limits_with_runtime() {
    let schema = settings_schema();
    let category = |id: &str| {
        schema
            .definitions
            .iter()
            .find(|definition| definition.id == id)
            .map(|definition| definition.category)
    };

    assert_eq!(
        category("agents.default_agent"),
        Some(SettingCategory::Agents)
    );
    assert_eq!(
        category("agents.small_agent"),
        Some(SettingCategory::Agents)
    );
    assert_eq!(category("agents.max_steps"), Some(SettingCategory::Runtime));
    assert_eq!(
        category("agents.parallel_tools"),
        Some(SettingCategory::Runtime)
    );
    assert!(
        schema
            .definitions
            .iter()
            .all(|definition| definition.category != SettingCategory::Network)
    );
    assert!(!default_settings().0.contains_key("network.proxy"));
}
