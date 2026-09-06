//! The Codex profile uses the tools implemented by the installed codex-core.
//!
//! It deliberately cannot be converted into an API [`crate::ToolSet`]: native
//! tools include free-form patches, managed processes, and hosted tools, not
//! just JSON functions. Codex selects their schemas and executors together.

/// Revision of Ait's Codex integration instructions (not a pinned binary).
pub const CODEX_TOOL_SET_REVISION: &str = "ait-codex-native-v1";

/// Ait host instructions layered above the immutable Project instructions.
pub const CODEX_DEVELOPER_INSTRUCTIONS: &str = include_str!("../prompts/codex.md");

/// A native harness profile, only consumed by the Codex app-server adapter.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CodexToolSet;

impl CodexToolSet {
    /// Builds the app-server developer layer without replacing core's base
    /// instructions or embedding user input or copied function schemas.
    #[must_use]
    pub fn developer_instructions(self, project_instructions: Option<&str>) -> String {
        let mut instructions = CODEX_DEVELOPER_INSTRUCTIONS.trim_end().to_owned();
        if let Some(project) = project_instructions.filter(|text| !text.trim().is_empty()) {
            instructions.push_str("\n\n## Project instructions\n\n");
            instructions.push_str(project);
        }
        instructions
    }
}
