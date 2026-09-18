//! Settings validation, revision checks and reset reducers.
use crate::control::catalog::require_named_agent;
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::persistence::{HasAgents, HasProjects, HasSettings, HasSettingsRevision};
use ait_contracts::{
    ApiError, CommandResult, SettingKind, SettingsDocument, SettingsView, default_settings,
    settings_schema,
};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use std::collections::HashSet;

pub(in crate::control) const DEFAULT_AGENT_SETTING_ID: &str = "agents.default_agent";
pub(in crate::control) const SMALL_AGENT_SETTING_ID: &str = "agents.small_agent";

pub(in crate::control) fn configured_agent_id<'a>(
    settings: &'a SettingsDocument,
    key: &str,
) -> Option<&'a str> {
    settings
        .0
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|id| !id.is_empty())
}

pub(in crate::control) fn resolve_project_agent_id(
    state: &(impl HasProjects + HasSettings),
    project_id: &str,
    requested_agent_id: &str,
) -> Result<String, ApiError> {
    if !requested_agent_id.trim().is_empty() {
        return Ok(requested_agent_id.to_owned());
    }
    let project = state
        .projects()
        .iter()
        .find(|project| project.id == project_id)
        .ok_or_else(|| error(ErrorCode::InvalidProject, "project not found", false))?;
    project
        .default_agent_id()
        .or_else(|| configured_agent_id(state.settings(), DEFAULT_AGENT_SETTING_ID))
        .map(str::to_owned)
        .ok_or_else(|| {
            error(
                ErrorCode::InvalidAgentConfiguration,
                "no Agent selected and global Default Agent is not configured",
                false,
            )
        })
}

pub(in crate::control) fn settings_view(
    state: &(impl HasSettings + HasSettingsRevision),
) -> SettingsView {
    SettingsView {
        schema: settings_schema(),
        values: state.settings().clone(),
        revision: *state.settings_revision(),
    }
}

pub(in crate::control) fn save_settings(
    state: &mut (impl HasAgents + HasSettings + HasSettingsRevision),
    expected_revision: u64,
    values: SettingsDocument,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if *state.settings_revision() != expected_revision {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings changed in another client; reload and try again",
            false,
        ));
    }
    validate_settings(&values)?;
    for key in [DEFAULT_AGENT_SETTING_ID, SMALL_AGENT_SETTING_ID] {
        if let Some(agent_id) = configured_agent_id(&values, key) {
            require_named_agent(state, agent_id)?;
        }
    }
    *state.settings_mut() = values;
    *state.settings_revision_mut() = state.settings_revision().saturating_add(1);
    let view = settings_view(state);
    Ok((
        CommandResult::Settings(view.clone()),
        vec![pending("settings.updated", None, &view)],
    ))
}

pub(in crate::control) fn reset_settings(
    state: &mut (impl HasSettings + HasSettingsRevision),
) -> (CommandResult, Vec<PendingEvent>) {
    *state.settings_mut() = default_settings();
    *state.settings_revision_mut() = state.settings_revision().saturating_add(1);
    let view = settings_view(state);
    (
        CommandResult::Settings(view.clone()),
        vec![pending("settings.reset", None, &view)],
    )
}

fn validate_settings(values: &SettingsDocument) -> Result<(), ApiError> {
    let schema = settings_schema();
    let expected = schema
        .definitions
        .iter()
        .map(|definition| definition.id.as_str())
        .collect::<HashSet<_>>();
    if values.0.keys().any(|key| !expected.contains(key.as_str())) {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings contain an unknown key",
            false,
        ));
    }
    for definition in schema.definitions {
        let Some(value) = values.0.get(&definition.id) else {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("missing setting {}", definition.id),
                false,
            ));
        };
        let valid = match &definition.kind {
            SettingKind::Text
            | SettingKind::Path
            | SettingKind::CredentialReference
            | SettingKind::AgentReference => value.is_string(),
            SettingKind::Boolean => value.is_boolean(),
            SettingKind::Number { min, max } => value
                .as_i64()
                .is_some_and(|number| number >= *min && number <= *max),
            SettingKind::Select { options } => value
                .as_str()
                .is_some_and(|choice| options.iter().any(|option| option == choice)),
        };
        if !valid {
            return Err(error(
                ErrorCode::InvalidConfiguration,
                format!("invalid value for setting {}", definition.id),
                false,
            ));
        }
    }
    Ok(())
}
mod context;
pub(in crate::control) use context::SettingsContext;
