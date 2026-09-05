use std::collections::HashMap;

use crate::detect::{Agent, AgentState};
use crate::layout::PaneId;
use crate::terminal::{TerminalId, TerminalState};

use super::{Tab, Workspace};

/// Detail info for a single pane, used by the agent detail panel.
pub struct PaneDetail {
    pub pane_id: PaneId,
    pub tab_idx: usize,
    pub tab_label: String,
    pub label: String,
    pub agent_label: String,
    #[allow(dead_code)]
    pub agent: Option<Agent>,
    pub state: AgentState,
    pub seen: bool,
    pub last_agent_state_change_seq: Option<u64>,
    pub custom_status: Option<String>,
    pub state_labels: HashMap<String, String>,
    pub context_percent: Option<u8>,
    pub manual_state: Option<crate::api::schema::PaneManualState>,
}

impl Tab {
    pub fn has_working_pane(&self, terminals: &HashMap<TerminalId, TerminalState>) -> bool {
        self.panes.values().any(|pane| {
            terminals
                .get(&pane.attached_terminal_id)
                .is_some_and(|terminal| terminal.state == AgentState::Working)
        })
    }

    fn pane_details(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
        tab_idx: usize,
        tab_label: &str,
    ) -> Vec<PaneDetail> {
        self.layout
            .pane_ids()
            .iter()
            .filter_map(|id| {
                let pane = self.panes.get(id)?;
                let terminal = terminals.get(&pane.attached_terminal_id)?;
                let fallback_agent_label = terminal
                    .agent_name
                    .as_deref()
                    .or_else(|| terminal.effective_agent_label())?
                    .to_string();
                let agent_label = terminal
                    .effective_display_agent()
                    .unwrap_or_else(|| fallback_agent_label.clone());
                let presentation = terminal.effective_presentation();
                Some(PaneDetail {
                    pane_id: *id,
                    tab_idx,
                    tab_label: tab_label.to_string(),
                    label: agent_label.clone(),
                    agent_label,
                    agent: terminal.effective_known_agent(),
                    state: terminal.state,
                    seen: pane.seen,
                    last_agent_state_change_seq: terminal.last_agent_state_change_seq,
                    custom_status: presentation.custom_status,
                    state_labels: presentation.state_labels,
                    context_percent: terminal.context_percent,
                    manual_state: terminal
                        .manual_state
                        .as_ref()
                        .map(crate::terminal::ManualStateOverride::as_pane_manual_state),
                })
            })
            .collect()
    }
}

/// Attention priority used for blocked-first ordering across panes and
/// workspaces. Higher sorts earlier: blocked, then done (idle+unseen), then
/// working, then idle (seen), then unknown. Pure and unit-testable.
pub(crate) fn attention_priority(state: AgentState, seen: bool) -> u8 {
    match (state, seen) {
        (AgentState::Blocked, _) => 4,
        (AgentState::Idle, false) => 3,
        (AgentState::Working, _) => 2,
        (AgentState::Idle, true) => 1,
        (AgentState::Unknown, _) => 0,
    }
}

fn pane_attention_priority(state: AgentState, seen: bool) -> u8 {
    attention_priority(state, seen)
}

impl Workspace {
    pub fn aggregate_state(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
    ) -> (AgentState, bool) {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .filter_map(|pane| {
                terminals
                    .get(&pane.attached_terminal_id)
                    .map(|terminal| (terminal.state, pane.seen))
            })
            .max_by_key(|(state, seen)| pane_attention_priority(*state, *seen))
            .unwrap_or((AgentState::Unknown, true))
    }

    pub fn has_working_pane(&self, terminals: &HashMap<TerminalId, TerminalState>) -> bool {
        self.tabs.iter().any(|tab| tab.has_working_pane(terminals))
    }

    /// Most-recent effective agent-state change across all panes in this
    /// workspace, if any. The sidebar renders `now - this` as a short age hint.
    pub fn last_agent_event_at(
        &self,
        terminals: &HashMap<TerminalId, TerminalState>,
    ) -> Option<std::time::Instant> {
        self.tabs
            .iter()
            .flat_map(|tab| tab.panes.values())
            .filter_map(|pane| terminals.get(&pane.attached_terminal_id))
            .filter_map(|terminal| terminal.last_agent_state_change_at)
            .max()
    }

    pub fn pane_details(&self, terminals: &HashMap<TerminalId, TerminalState>) -> Vec<PaneDetail> {
        self.tabs
            .iter()
            .enumerate()
            .flat_map(|(tab_idx, tab)| {
                let tab_label = self
                    .tab_display_name(tab_idx, terminals)
                    .unwrap_or_else(|| (tab_idx + 1).to_string());
                // A tab normally holds one agent, and then its label is the
                // agent's own. Only a tab that really holds several panes
                // needs the tab named in front of each of them.
                let multi_pane = tab.panes.len() > 1;
                tab.pane_details(terminals, tab_idx, &tab_label)
                    .into_iter()
                    .map(move |mut detail| {
                        if multi_pane {
                            detail.label = format!("{}·{}", detail.tab_label, detail.agent_label);
                        }
                        detail
                    })
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use super::*;
    use crate::detect::Agent;

    fn terminal_for_pane(ws: &Workspace, pane_id: PaneId) -> TerminalState {
        TerminalState::new(ws.terminal_id(pane_id).unwrap().clone(), "/tmp".into())
    }

    #[test]
    fn attention_priority_orders_blocked_first_then_done_working_idle_unknown() {
        // Strictly decreasing: blocked > done (idle+unseen) > working > idle > unknown.
        assert!(
            attention_priority(AgentState::Blocked, true)
                > attention_priority(AgentState::Idle, false)
        );
        assert!(
            attention_priority(AgentState::Idle, false)
                > attention_priority(AgentState::Working, true)
        );
        assert!(
            attention_priority(AgentState::Working, true)
                > attention_priority(AgentState::Idle, true)
        );
        assert!(
            attention_priority(AgentState::Idle, true)
                > attention_priority(AgentState::Unknown, true)
        );
        // Blocked ignores seen.
        assert_eq!(
            attention_priority(AgentState::Blocked, true),
            attention_priority(AgentState::Blocked, false)
        );
    }

    #[test]
    fn aggregate_state_all_unknown() {
        let ws = Workspace::test_new("test");
        let mut terminals = HashMap::new();
        let root = ws.tabs[0].root_pane;
        let terminal = terminal_for_pane(&ws, root);
        terminals.insert(terminal.id.clone(), terminal);
        let (state, seen) = ws.aggregate_state(&terminals);
        assert_eq!(state, AgentState::Unknown);
        assert!(seen);
    }

    #[test]
    fn aggregate_state_priority() {
        let mut ws = Workspace::test_new("test");
        let id2 = ws.test_split(Direction::Horizontal);
        let root_id = ws.tabs[0]
            .panes
            .keys()
            .find(|id| **id != id2)
            .copied()
            .unwrap();
        let mut terminals = HashMap::new();
        let mut root_terminal = terminal_for_pane(&ws, root_id);
        root_terminal.state = AgentState::Idle;
        terminals.insert(root_terminal.id.clone(), root_terminal);
        let mut second_terminal = terminal_for_pane(&ws, id2);
        second_terminal.state = AgentState::Working;
        terminals.insert(second_terminal.id.clone(), second_terminal);

        let (state, seen) = ws.aggregate_state(&terminals);

        assert_eq!(state, AgentState::Working);
        assert!(seen);
    }

    #[test]
    fn aggregate_state_done_unseen_beats_working() {
        let mut ws = Workspace::test_new("test");
        let id2 = ws.test_split(Direction::Horizontal);
        let root_id = ws.tabs[0]
            .panes
            .keys()
            .find(|id| **id != id2)
            .copied()
            .unwrap();
        let mut terminals = HashMap::new();
        let mut root_terminal = terminal_for_pane(&ws, root_id);
        root_terminal.state = AgentState::Idle;
        terminals.insert(root_terminal.id.clone(), root_terminal);
        let mut second_terminal = terminal_for_pane(&ws, id2);
        second_terminal.state = AgentState::Working;
        terminals.insert(second_terminal.id.clone(), second_terminal);
        let root = ws.tabs[0].panes.get_mut(&root_id).unwrap();
        root.seen = false;

        let (state, seen) = ws.aggregate_state(&terminals);

        assert_eq!(state, AgentState::Idle);
        assert!(!seen);
    }

    #[test]
    fn pane_details_prefers_agent_name_over_detected_agent_label() {
        let ws = Workspace::test_new("test");
        let root_pane = ws.tabs[0].root_pane;
        let mut terminals = HashMap::new();
        let mut terminal = terminal_for_pane(&ws, root_pane);
        terminal.set_detected_state(Some(Agent::Pi), AgentState::Working);
        terminal.set_agent_name("planner".into());
        terminals.insert(terminal.id.clone(), terminal);

        let labels: Vec<_> = ws
            .pane_details(&terminals)
            .into_iter()
            .map(|detail| (detail.label, detail.agent_label, detail.agent))
            .collect();

        assert_eq!(
            labels,
            vec![("planner".into(), "planner".into(), Some(Agent::Pi))]
        );
    }

    #[test]
    fn pane_details_prefixes_the_tab_only_when_it_holds_several_panes() {
        let mut ws = Workspace::test_new("test");
        ws.tabs[0].custom_name = Some("main".into());
        let root_pane = ws.tabs[0].root_pane;
        let second_tab = ws.test_add_tab(Some("review"));
        let review_pane = ws.tabs[second_tab].root_pane;
        let mut terminals = HashMap::new();
        let mut root_terminal = terminal_for_pane(&ws, root_pane);
        root_terminal.set_hook_authority(
            "test".into(),
            "pi".into(),
            AgentState::Working,
            None,
            None,
        );
        terminals.insert(root_terminal.id.clone(), root_terminal);
        let mut review_terminal = terminal_for_pane(&ws, review_pane);
        review_terminal.set_hook_authority(
            "test".into(),
            "claude".into(),
            AgentState::Idle,
            None,
            None,
        );
        terminals.insert(review_terminal.id.clone(), review_terminal);

        let labels = |ws: &Workspace, terminals: &HashMap<_, _>| -> Vec<String> {
            ws.pane_details(terminals)
                .into_iter()
                .map(|detail| detail.label)
                .collect()
        };

        // Two tabs, one agent each: the agent's own name is the whole label.
        assert_eq!(labels(&ws, &terminals), vec!["pi", "claude"]);

        // Split the review tab and the tab name comes back in front of both
        // panes, because now the tab is the thing they share.
        ws.active_tab = second_tab;
        let extra_pane = ws.test_split(ratatui::layout::Direction::Horizontal);
        let mut extra_terminal = terminal_for_pane(&ws, extra_pane);
        extra_terminal.set_hook_authority(
            "test".into(),
            "codex".into(),
            AgentState::Working,
            None,
            None,
        );
        terminals.insert(extra_terminal.id.clone(), extra_terminal);
        assert_eq!(
            labels(&ws, &terminals),
            vec!["pi", "review·claude", "review·codex"]
        );
    }

    #[test]
    fn pane_details_use_tab_vector_index_not_stable_public_tab_number() {
        let mut ws = Workspace::test_new("test");
        let removed_tab = ws.test_add_tab(Some("removed"));
        let survivor_tab = ws.test_add_tab(Some("survivor"));
        let survivor_pane = ws.tabs[survivor_tab].root_pane;
        assert!(ws.close_tab(removed_tab));

        let mut terminals = HashMap::new();
        let mut terminal = terminal_for_pane(&ws, survivor_pane);
        terminal.detected_agent = Some(Agent::Codex);
        terminals.insert(terminal.id.clone(), terminal);

        let details = ws.pane_details(&terminals);
        let survivor = details
            .iter()
            .find(|detail| detail.pane_id == survivor_pane)
            .expect("surviving tab agent should be listed");

        assert_eq!(ws.tabs[1].number, 3);
        assert_eq!(survivor.tab_idx, 1);
    }
}
