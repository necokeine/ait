//! Paseo terminal payloads and binary opcodes, pinned by ADR-033.
//!
//! Rust translation and modifications: see third-party/paseo/NOTICE and LICENSE.

use serde::{Deserialize, Serialize};

/// All implemented terminal methods, including the uncorrelated input event.
pub const CAPABILITIES: &[&str] = &[
    "terminal.list.request",
    "terminal.list.subscribe.request",
    "terminal.list.unsubscribe.request",
    "terminal.create.request",
    "terminal.rename.request",
    "terminal.subscribe.request",
    "terminal.unsubscribe.request",
    "terminal.input",
    "terminal.kill.request",
    "terminal.capture.request",
];

/// PTY dimensions in character cells.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub struct Size {
    /// Visible rows.
    pub rows: u16,
    /// Visible columns.
    pub cols: u16,
}

impl Default for Size {
    fn default() -> Self {
        Self { rows: 24, cols: 80 }
    }
}

impl Size {
    /// Check nonzero dimensions and the bounded screen allocation.
    ///
    /// # Errors
    /// Returns `Error::Invalid` above 200 columns, 100 rows, or 10,000 visible cells.
    pub fn validate(self) -> Result<Self, crate::Error> {
        if self.rows == 0
            || self.cols == 0
            || self.rows > 100
            || self.cols > 200
            || usize::from(self.rows) * usize::from(self.cols) > 10_000
        {
            return Err(crate::Error::Invalid);
        }
        Ok(self)
    }
}

/// Optional workspace or directory filter; workspace identity takes precedence.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ListRequest {
    /// Absolute directory, including owned descendants.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Stable workspace identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub workspace_id: Option<String>,
}

/// Create a local PTY process in an active workspace.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CreateRequest {
    /// Absolute process working directory.
    pub cwd: String,
    /// Active workspace; omitted values resolve the deepest matching workspace root.
    pub workspace_id: Option<String>,
    /// Display name; defaults to the next directory-local terminal number.
    pub name: Option<String>,
    /// Retired Paseo option; nonempty values are rejected.
    pub agent_id: Option<String>,
    /// Executable, or the host's default shell.
    pub command: Option<String>,
    /// Executable arguments, without shell interpolation.
    #[serde(default)]
    pub args: Vec<String>,
    /// Initial character dimensions.
    #[serde(default)]
    pub size: Size,
}

/// Address one terminal.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalRequest {
    /// Terminal identity.
    pub terminal_id: String,
}

/// Rename the terminal title without changing the process name.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RenameRequest {
    /// Terminal identity.
    pub terminal_id: String,
    /// Trimmed title, from one to 200 UTF-16 code units.
    pub title: String,
}

/// Capture rendered rows, with inclusive indices and negative indexing from the end.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CaptureRequest {
    /// Terminal identity.
    pub terminal_id: String,
    /// Inclusive first row; defaults to zero.
    pub start: Option<i64>,
    /// Inclusive last row; defaults to the final visible row.
    pub end: Option<i64>,
    /// Compatibility flag; rendered cells contain no ANSI control sequences.
    #[serde(default = "default_true")]
    pub strip_ansi: bool,
}

const fn default_true() -> bool {
    true
}

/// Terminal metadata. Activity remains null until shell/agent hooks are installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TerminalInfo {
    /// Process identity.
    pub id: String,
    /// Creation name.
    pub name: String,
    /// Canonical process directory.
    pub cwd: String,
    /// Owning active workspace.
    pub workspace_id: String,
    /// Explicit or OSC title.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Hook activity, currently null.
    pub activity: Option<serde_json::Value>,
}

/// Subscribe to a terminal output stream.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SubscribeRequest {
    /// Terminal identity.
    pub terminal_id: String,
    /// Restore policy; absent means a legacy JSON state snapshot.
    pub restore: Option<Restore>,
}

/// Restore policy and optional initial resize claim.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Restore {
    /// Live-only, bounded visible restore, or the full retained screen.
    pub mode: RestoreMode,
    /// Visible restore history; defaults to 200, clamped to 500.
    pub scrollback_lines: Option<usize>,
    /// Initial resize claim.
    pub size: Option<Size>,
}

/// Initial stream representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum RestoreMode {
    /// Start at the current output cursor.
    Live,
    /// Restore the screen and at most 500 history rows.
    VisibleSnapshot,
    /// Restore all retained history and the screen.
    FullSnapshot,
}

/// A resize can claim control or update only the current owner's dimensions.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResizeIntent {
    /// Take size ownership; also the legacy default.
    #[default]
    Claim,
    /// Apply only for the owning physical connection.
    Update,
}

/// Binary resize payload.
#[derive(Debug, Clone, Deserialize)]
pub struct Resize {
    /// New dimensions.
    #[serde(flatten)]
    pub size: Size,
    /// Ownership intent.
    #[serde(default)]
    pub intent: ResizeIntent,
}

/// Pointer transition requested by a terminal client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MouseAction {
    /// Button pressed.
    Down,
    /// Button released.
    Up,
    /// Pointer moved.
    Move,
}

/// Text-channel input message.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Input {
    /// UTF-8 bytes sent to the process.
    Input {
        /// Text, including control characters and paste delimiters.
        data: String,
    },
    /// PTY resize with physical-connection ownership.
    Resize(Resize),
    /// Pointer event encoded according to the application's enabled mouse mode.
    Mouse {
        /// Zero-based row.
        row: u16,
        /// Zero-based column.
        col: u16,
        /// Xterm button number.
        button: u8,
        /// Pointer transition.
        action: MouseAction,
    },
}

/// Uncorrelated terminal input event.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InputRequest {
    /// Terminal identity.
    pub terminal_id: String,
    /// Input, size, or pointer transition.
    pub message: Input,
}

/// Binary terminal stream opcode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Opcode {
    /// Server output bytes.
    Output = 1,
    /// Client input bytes.
    Input = 2,
    /// JSON resize payload in either direction.
    Resize = 3,
    /// Legacy JSON terminal state.
    Snapshot = 4,
    /// ANSI restore, including the live input-mode preamble.
    Restore = 5,
}

/// Encode one frame with the connection-local slot and payload.
#[must_use]
pub fn frame(opcode: Opcode, slot: u8, payload: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(payload.len() + 2);
    bytes.extend_from_slice(&[opcode as u8, slot]);
    bytes.extend_from_slice(payload);
    bytes
}

/// Decode a client frame, rejecting server-only opcodes and malformed headers.
///
/// # Errors
/// Returns `Error::Invalid` for a short frame or a server-only/unknown opcode.
pub fn client_frame(bytes: &[u8]) -> Result<(u8, Input), crate::Error> {
    let [opcode, slot, payload @ ..] = bytes else {
        return Err(crate::Error::Invalid);
    };
    let input = match opcode {
        2 => Input::Input {
            data: String::from_utf8(payload.to_vec()).map_err(|_| crate::Error::Invalid)?,
        },
        3 => Input::Resize(serde_json::from_slice(payload).map_err(|_| crate::Error::Invalid)?),
        _ => return Err(crate::Error::Invalid),
    };
    Ok((*slot, input))
}

#[cfg(test)]
mod tests;
