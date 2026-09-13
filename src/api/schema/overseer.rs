//! Overseer API types: what the overseer plugin last sensed and said, the
//! chat with its headless runtime, and the tick that refreshes both, served
//! over `overseer.*`.
//!
//! These are the wire shapes of the files the plugin writes into its state
//! dir — `situation.json`'s health findings, `BOARD.md`'s narrative and its
//! source, and one `chat.jsonl` line per [`OverseerChatTurn`]. `app::overseer`
//! re-exports them under its own names so the server reads and writes exactly
//! what a client sees. Nothing here dispatches work: the overseer advises.

use serde::{Deserialize, Serialize};

/// `shep doctor`'s verdict on one check, as the plugin copied it into
/// `situation.json`. The lowercase wire spelling matches `shep doctor --json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverseerHealthLevel {
    Ok,
    Warn,
    Fail,
}

/// One health check and what it found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverseerHealthFinding {
    pub level: OverseerHealthLevel,
    pub check: String,
    #[serde(default)]
    pub detail: String,
    /// What would fix it, when the check knows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

/// Who wrote the narrative: a brain runtime, or the tick's own template.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum OverseerNarrativeSource {
    Brain,
    #[default]
    Deterministic,
}

/// Who said a chat line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum OverseerChatRole {
    You,
    Overseer,
}

/// One line of the chat with the overseer — one line of `chat.jsonl`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverseerChatTurn {
    /// Unix seconds.
    pub at: u64,
    pub role: OverseerChatRole,
    pub text: String,
}

/// The conversation the chat and the overseer's session pane share.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverseerSessionInfo {
    /// Absent until something has minted one; reading never mints.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Whether anything has begun the conversation under that id.
    pub started: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct OverseerSampleParams {
    /// How many trailing chat turns to include: 20 when omitted, `0` for
    /// none, capped at the 200 the server keeps.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub chat_turns: Option<u32>,
}

/// What the overseer knows right now, read from its state dir.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverseerSample {
    /// Whether the overseer plugin is linked to this server.
    pub plugin_linked: bool,
    /// Whether the state dir has been looked at, whatever was found.
    pub sampled: bool,
    /// `BOARD.md`, header dropped, one entry per non-empty line.
    #[serde(default)]
    pub narrative: Vec<String>,
    pub source: OverseerNarrativeSource,
    /// `hh:mm` of the last tick, when the situation says.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tick_at: Option<String>,
    /// How long since the situation was written.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub situation_age_seconds: Option<u64>,
    /// How long since a brain last answered, when one ever has.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub brain_age_seconds: Option<u64>,
    /// The headless runtime the chat asks: `[plugins.overseer] runtime`, else
    /// `SHEP_OVERSEER_RUNTIME`. Absent means the chat cannot ask anything.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    /// A `tick` the server started is still running.
    pub tick_in_flight: bool,
    #[serde(default)]
    pub health: Vec<OverseerHealthFinding>,
    /// The tail of the chat, oldest first.
    #[serde(default)]
    pub chat: Vec<OverseerChatTurn>,
    /// Every turn the server holds, however few were returned.
    pub chat_total: u64,
    /// A question is out with the runtime; its answer is an event away.
    pub chat_pending: bool,
    pub session: OverseerSessionInfo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OverseerChatParams {
    pub text: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct OverseerTickParams {
    /// Skip the tick when the situation is younger than this: 60 seconds
    /// when omitted, `0` to tick whatever its age.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_age_seconds: Option<u64>,
}
