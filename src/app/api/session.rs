use crate::api::schema::{ResponseResult, SessionSnapshot};
use crate::app::App;

use super::responses::encode_success;

impl App {
    pub(super) fn handle_session_snapshot(&mut self, id: String) -> String {
        encode_success(
            id,
            ResponseResult::SessionSnapshot {
                snapshot: Box::new(self.session_snapshot()),
            },
        )
    }

    fn session_snapshot(&self) -> SessionSnapshot {
        let focused_workspace_id = self
            .state
            .active
            .map(|ws_idx| self.public_workspace_id(ws_idx));
        let focused_tab_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_tab_id(ws_idx, ws.active_tab)
        });
        let focused_pane_id = self.state.active.and_then(|ws_idx| {
            let ws = self.state.workspaces.get(ws_idx)?;
            self.public_pane_id(ws_idx, ws.focused_pane_id()?)
        });

        let mut workspaces = Vec::new();
        let mut tabs = Vec::new();
        let mut layouts = Vec::new();
        for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
            workspaces.push(self.workspace_info(ws_idx));
            for tab_idx in 0..ws.tabs.len() {
                if let Some(tab) = self.tab_info(ws_idx, tab_idx) {
                    tabs.push(tab);
                }
                if let Some(layout) = self.pane_layout_snapshot(ws_idx, tab_idx) {
                    layouts.push(layout);
                }
            }
        }

        SessionSnapshot {
            version: crate::build_info::version(),
            protocol: crate::protocol::PROTOCOL_VERSION,
            focused_workspace_id,
            focused_tab_id,
            focused_pane_id,
            workspaces,
            tabs,
            panes: self.collect_panes_for_workspace(None).unwrap_or_default(),
            layouts,
            agents: self.collect_agent_infos(),
        }
    }
}

impl App {
    pub(super) fn handle_session_overview(&mut self, id: String) -> String {
        // An API client has no TUI tick behind it, so sample here. The
        // interval guard makes a polling client cost the same as the board.
        self.state
            .dashboard_sample
            .refresh_if_stale(std::time::Instant::now());
        encode_success(
            id,
            ResponseResult::SessionOverview {
                overview: Box::new(self.session_overview()),
            },
        )
    }

    /// Build the overview from the same model the session board renders.
    ///
    /// Reusing `board_model` is the point: bucketing and attention ordering
    /// live in exactly one place, so a client rendering this payload and the
    /// desktop board cannot drift apart as the ordering rules evolve.
    fn session_overview(&self) -> crate::api::schema::SessionOverview {
        use crate::api::schema::{
            SessionOverview, SessionOverviewAgent, SessionOverviewHost, SessionOverviewTotals,
        };

        let model = crate::ui::board::board_model(&self.state);
        let summary = crate::ui::board::board_summary(&self.state, &model);
        let now = std::time::Instant::now();

        let mut agents = Vec::new();
        for card in model.flattened() {
            let Some(ws) = self.state.workspaces.get(card.ws_idx) else {
                continue;
            };
            let tab_idx = match ws.find_tab_index_for_pane(card.pane_id) {
                Some(tab_idx) => tab_idx,
                None => continue,
            };
            let terminal = ws
                .terminal_id(card.pane_id)
                .and_then(|id| self.state.terminals.get(id));
            let state_age_seconds = terminal
                .and_then(|terminal| terminal.last_agent_state_change_at)
                .map(|at| now.saturating_duration_since(at).as_secs());
            agents.push(SessionOverviewAgent {
                pane_id: self
                    .public_pane_id(card.ws_idx, card.pane_id)
                    .unwrap_or_default(),
                workspace_id: self.public_workspace_id(card.ws_idx),
                tab_id: self.public_tab_id(card.ws_idx, tab_idx).unwrap_or_default(),
                tab_name: ws
                    .tab_display_name(tab_idx, &self.state.terminals)
                    .unwrap_or_default(),
                pane_number: ws.public_pane_number(card.pane_id).map(|n| n as u64),
                workspace_label: card.workspace_label.clone(),
                branch: card.branch.clone(),
                name: Some(card.agent_label.clone()),
                display_name: card.display_name.clone(),
                display_agent: card.model.clone(),
                agent_status: crate::app::api_helpers::pane_agent_status(card.state, card.seen),
                unseen: !card.seen,
                custom_status: card.status.clone(),
                manual_state: card.manual_state.clone(),
                activity_line: card.activity.clone(),
                activity_lines: card.activity_lines.clone(),
                context_percent: card.context_percent,
                cwd: card.cwd.clone(),
                state_age_seconds,
                queued_input: self.state.queued_input_count_for_pane(card.pane_id) as u64,
                focused: ws.focused_pane_id() == Some(card.pane_id)
                    && self.state.active == Some(card.ws_idx),
            });
        }

        let vitals = self.state.dashboard_sample.vitals;
        let custom_states = self
            .state
            .states_config
            .custom
            .iter()
            .map(|custom| crate::api::schema::PaneManualState {
                name: custom.name.clone(),
                label: custom.label().to_string(),
                tier: custom.tier,
            })
            .collect();
        SessionOverview {
            totals: SessionOverviewTotals {
                agents: summary.agents() as u64,
                blocked: summary.blocked as u64,
                done: summary.done as u64,
                working: summary.working as u64,
                idle: summary.idle as u64,
                attention: summary.attention as u64,
                workspaces: summary.workspaces as u64,
                tabs: summary.tabs as u64,
                panes: summary.panes as u64,
                queued_input: summary.queued_input as u64,
                pending_tasks: self.state.dashboard_sample.pending_tasks.map(|n| n as u64),
            },
            host: SessionOverviewHost {
                version: crate::build_info::version(),
                load_percent: vitals.load_percent.map(u64::from),
                cores: vitals.cores.map(|n| n as u64),
                memory_percent: vitals.memory_percent,
                memory_total_bytes: vitals.memory_total_bytes,
                memory_used_bytes: vitals.memory_used_bytes,
            },
            agents,
            custom_states,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{EmptyParams, Method, ResponseResult, SuccessResponse};
    use crate::{config::Config, workspace::Workspace};

    fn app_with_two_tabs() -> crate::app::App {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = crate::app::App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut workspace = Workspace::test_new("snapshot");
        workspace.test_add_tab(None);
        app.state.workspaces = vec![workspace];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app
    }

    #[test]
    fn session_overview_carries_placement_totals_and_host_facts() {
        use crate::detect::{Agent, AgentState};

        let mut app = app_with_two_tabs();
        app.state.workspaces[0].tabs[0].set_custom_name("review".into());
        let pane = app.state.workspaces[0].tabs[0].root_pane;
        let terminal_id = app.state.workspaces[0]
            .terminal_id(pane)
            .expect("terminal")
            .clone();
        {
            let terminal = app.state.terminals.get_mut(&terminal_id).expect("terminal");
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = AgentState::Blocked;
            terminal.set_activity_lines(vec![
                "reading src/webhook.ts".into(),
                "? Do you want to make this edit".into(),
                "waiting for approval".into(),
            ]);
            terminal.set_context_percent(Some(62));
        }

        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_overview".into(),
            method: Method::SessionOverview(EmptyParams::default()),
        });
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::SessionOverview { overview } = success.result else {
            panic!("expected session overview response");
        };

        assert_eq!(overview.totals.workspaces, 1);
        assert_eq!(overview.totals.tabs, 2);
        assert_eq!(overview.totals.blocked, 1);
        // A blocked agent is by definition waiting on the user.
        assert_eq!(overview.totals.attention, 1);

        let blocked = overview
            .agents
            .first()
            .expect("blocked agent sorts to the front");
        assert_eq!(blocked.tab_name, "review", "tab name, not its number");
        // A client with one line of room reads the last one; a client with
        // more reads them in the order the agent printed them.
        assert_eq!(
            blocked.activity_line.as_deref(),
            Some("waiting for approval")
        );
        assert_eq!(
            blocked.activity_lines,
            vec![
                "reading src/webhook.ts".to_string(),
                "? Do you want to make this edit".to_string(),
                "waiting for approval".to_string(),
            ]
        );
        assert_eq!(blocked.context_percent, Some(62));
        assert!(!blocked.pane_id.is_empty());
        assert!(!blocked.workspace_id.is_empty());

        // Host facts are sampled by the handler itself, with no TUI tick.
        assert_eq!(overview.host.version, crate::build_info::version());
        assert!(
            overview.host.cores.is_some_and(|cores| cores > 0),
            "vitals should be sampled on demand: {:?}",
            overview.host
        );
    }

    #[test]
    fn session_snapshot_bootstraps_runtime_resources() {
        let mut app = app_with_two_tabs();
        let response = app.handle_api_request(crate::api::schema::Request {
            id: "req_snapshot".into(),
            method: Method::SessionSnapshot(EmptyParams::default()),
        });

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::SessionSnapshot { snapshot } = success.result else {
            panic!("expected session snapshot response");
        };
        assert_eq!(success.id, "req_snapshot");
        assert_eq!(snapshot.workspaces.len(), 1);
        assert_eq!(snapshot.tabs.len(), 2);
        assert_eq!(snapshot.panes.len(), 2);
        assert_eq!(snapshot.layouts.len(), 2);
        assert_eq!(
            snapshot.focused_workspace_id.as_deref(),
            Some(snapshot.workspaces[0].workspace_id.as_str())
        );
        assert_eq!(
            snapshot.focused_tab_id.as_deref(),
            Some(snapshot.tabs[0].tab_id.as_str())
        );
        assert_eq!(
            snapshot.focused_pane_id.as_deref(),
            Some(snapshot.panes[0].pane_id.as_str())
        );
    }
}
