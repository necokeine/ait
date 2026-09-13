//! Settings validation, revision checks and reset reducers.
use crate::control::errors::error;
use crate::control::events::pending;
use crate::control::state::WorkingSet;
use ait_contracts::{
    ApiError, CommandResult, SettingKind, SettingsDocument, SettingsView, default_settings,
    settings_schema,
};
use ait_domain::ErrorCode;
use ait_ports::PendingEvent;
use std::collections::HashSet;

pub(in crate::control) fn settings_view(state: &WorkingSet) -> SettingsView {
    SettingsView {
        schema: settings_schema(),
        values: state.settings.clone(),
        revision: state.settings_revision,
    }
}

pub(in crate::control) fn save_settings(
    state: &mut WorkingSet,
    expected_revision: u64,
    values: SettingsDocument,
) -> Result<(CommandResult, Vec<PendingEvent>), ApiError> {
    if state.settings_revision != expected_revision {
        return Err(error(
            ErrorCode::InvalidConfiguration,
            "settings changed in another client; reload and try again",
            false,
        ));
    }
    validate_settings(&values)?;
    state.settings = values;
    state.settings_revision = state.settings_revision.saturating_add(1);
    let view = settings_view(state);
    Ok((
        CommandResult::Settings(view.clone()),
        vec![pending("settings.updated", None, &view)],
    ))
}

pub(in crate::control) fn reset_settings(
    state: &mut WorkingSet,
) -> (CommandResult, Vec<PendingEvent>) {
    state.settings = default_settings();
    state.settings_revision = state.settings_revision.saturating_add(1);
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
            SettingKind::Text | SettingKind::Path | SettingKind::CredentialReference => {
                value.is_string()
            }
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
