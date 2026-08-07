use serde::{Deserialize, Serialize};

use super::agents::AgentInfo;
use super::panes::{PaneInfo, PaneLayoutSnapshot};
use super::tabs::TabInfo;
use super::workspaces::WorkspaceInfo;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionSnapshot {
    pub version: String,
    pub protocol: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_workspace_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_tab_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub focused_pane_id: Option<String>,
    pub workspaces: Vec<WorkspaceInfo>,
    pub tabs: Vec<TabInfo>,
    pub panes: Vec<PaneInfo>,
    pub layouts: Vec<PaneLayoutSnapshot>,
    pub agents: Vec<AgentInfo>,
}

/// One agent as the session overview presents it: the runtime facts a client
/// needs to render an at-a-glance board without a second round-trip per agent.
///
/// Deliberately additive to `AgentInfo` rather than a replacement — this
/// carries the *placement* and *display* facts (which tab, how stale, what the
/// screen last said) that a list view needs and a single-agent lookup does not.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionOverviewAgent {
    pub pane_id: String,
    pub workspace_id: String,
    pub tab_id: String,
    /// Human name for the tab: its custom name when set, otherwise its number.
    pub tab_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pane_number: Option<u64>,
    pub workspace_label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The name to show for this agent: shep's own label plus only as much
    /// placement as it takes to tell it apart from the other agents in the
    /// session. Clients should render this rather than deriving their own, so
    /// every surface calls the same agent the same thing.
    pub display_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_agent: Option<String>,
    pub agent_status: super::common::AgentStatus,
    /// True when the agent finished without anyone having looked at the pane.
    #[serde(default, skip_serializing_if = "super::is_false")]
    pub unseen: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_status: Option<String>,
    /// Last line of real content on the pane's screen; a display hint only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity_line: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// Seconds since this agent's state last changed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state_age_seconds: Option<u64>,
    /// Prompts queued for this pane, waiting for it to go idle.
    #[serde(default, skip_serializing_if = "is_zero_u64")]
    pub queued_input: u64,
    pub focused: bool,
}

pub(crate) fn is_zero_u64(value: &u64) -> bool {
    *value == 0
}

/// Session-wide counts for an overview header.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct SessionOverviewTotals {
    pub agents: u64,
    pub blocked: u64,
    pub done: u64,
    pub working: u64,
    pub idle: u64,
    /// Agents waiting on the user: blocked, plus finished-and-unseen.
    pub attention: u64,
    pub workspaces: u64,
    pub tabs: u64,
    pub panes: u64,
    pub queued_input: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_tasks: Option<u64>,
}

/// Coarse facts about the machine the session runs on.
///
/// Every field is optional: a host that cannot answer honestly reports nothing
/// rather than a plausible zero. There is no separate GPU memory figure —
/// on unified-memory hardware there is nothing separate to report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema, Default)]
pub struct SessionOverviewHost {
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub load_percent: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cores: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_total_bytes: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub memory_used_bytes: Option<u64>,
}

/// Everything a client needs to render a session overview in one request.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionOverview {
    pub totals: SessionOverviewTotals,
    pub host: SessionOverviewHost,
    /// Agents in attention order: blocked first, then finished-unseen, then
    /// working, then idle — the same order the session board uses.
    pub agents: Vec<SessionOverviewAgent>,
}
