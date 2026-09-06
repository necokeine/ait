//! Portable v2 archive upgrade. New exports always use the current format.
use super::{
    AgentConfiguration, AgentMode, AgentProvider, AgentView, Deserialize, MessageView,
    PROJECT_EXPORT_VERSION, ProjectExport, ProjectView, ProviderModel, SessionView, Value,
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct ArchiveInput {
    format_version: u16,
    source_revision: u64,
    project: ProjectView,
    agents: Vec<Value>,
    #[serde(default)]
    providers: Vec<AgentProvider>,
    sessions: Vec<SessionView>,
    messages: Vec<MessageView>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyAgent {
    id: String,
    name: String,
    model: String,
    mode: AgentMode,
    revision: u64,
    enabled: bool,
}

impl TryFrom<ArchiveInput> for ProjectExport {
    type Error = String;
    fn try_from(input: ArchiveInput) -> Result<Self, Self::Error> {
        let mut providers = input.providers;
        let mut agents = Vec::new();
        if input.format_version == 2 && !providers.is_empty() {
            return Err("v2 archives cannot contain providers".into());
        }
        for value in input.agents {
            if input.format_version != 2 {
                agents.push(serde_json::from_value(value).map_err(|error| error.to_string())?);
                continue;
            }
            let legacy: LegacyAgent =
                serde_json::from_value(value).map_err(|error| error.to_string())?;
            let kind = serde_json::to_value(legacy.mode).map_err(|error| error.to_string())?;
            // Imported connection identities cannot silently inherit a local credential.
            let id = format!(
                "archive-{}-{}",
                input.project.id,
                kind.as_str().unwrap_or_default()
            );
            if !providers.iter().any(|provider| provider.id == id) {
                providers.push(AgentProvider {
                    id: id.clone(),
                    name: kind.as_str().unwrap_or_default().into(),
                    kind: legacy.mode,
                    url: None,
                    models: Vec::new(),
                });
            }
            let provider = providers
                .iter_mut()
                .find(|provider| provider.id == id)
                .expect("inserted provider");
            if !provider.models.iter().any(|model| model.id == legacy.model) {
                let reasoning_efforts =
                    if legacy.mode == AgentMode::Codex && legacy.model == "gpt-5.6-sol" {
                        ["low", "medium", "high", "xhigh", "max", "ultra"]
                            .map(str::to_owned)
                            .to_vec()
                    } else {
                        Vec::new()
                    };
                provider.models.push(ProviderModel {
                    id: legacy.model.clone(),
                    name: legacy.model.clone(),
                    reasoning_efforts,
                });
            }
            agents.push(AgentView {
                id: legacy.id,
                name: legacy.name,
                config: AgentConfiguration {
                    provider_id: id,
                    model: legacy.model,
                    reasoning_effort: None,
                },
                owner_session_id: None,
                revision: legacy.revision,
                enabled: legacy.enabled,
            });
        }
        Ok(Self {
            format_version: if input.format_version == 2 {
                PROJECT_EXPORT_VERSION
            } else {
                input.format_version
            },
            source_revision: input.source_revision,
            project: input.project,
            agents,
            providers,
            sessions: input.sessions,
            messages: input.messages,
        })
    }
}
