//! Keys on the overseer view.
//!
//! One flat selection walks needs-you, agents, docket and proposals; enter
//! does the obvious thing for the row it is on (focus the pane, open the
//! docket item, keep the proposal); `a` and `x` keep or drop a proposal and
//! otherwise `a` goes round the views. The docket verbs go through the same
//! store path as the docket board's, by id.

use crossterm::event::{KeyCode, KeyEvent};

use crate::app::{
    state::{AppState, BoardView, Mode},
    App,
};
use crate::ui::board::BoardDir;
use crate::ui::overseer::{overseer_model, OverseerRow};

use super::modal::{leave_modal, open_keybind_help};

impl AppState {
    /// The row the overseer view treats as selected.
    pub(crate) fn overseer_selection(&self) -> Option<OverseerRow> {
        overseer_model(self).effective_selection(self.board.overseer_selected)
    }

    /// Move the overseer selection in `dir`.
    pub(crate) fn overseer_move_selection(&mut self, dir: BoardDir) {
        let model = overseer_model(self);
        let current = model.effective_selection(self.board.overseer_selected);
        if let Some(next) = model.next(current, dir) {
            self.board.overseer_selected = Some(next);
        }
    }

    /// After a proposal is kept or dropped its card is gone: land on the one
    /// that took its place, or the last proposal, or wherever the view puts
    /// a selection that no longer resolves.
    fn overseer_reseat_after_proposal(&mut self, was_at: usize) {
        let model = overseer_model(self);
        self.board.overseer_selected = model
            .proposals
            .get(was_at.min(model.proposals.len().saturating_sub(1)))
            .map(|card| OverseerRow::Proposal(card.id))
            .or_else(|| model.effective_selection(self.board.overseer_selected));
    }

    /// `a` on a proposal: promote it as slated, the docket's `p`.
    pub(crate) fn overseer_keep_proposal(&mut self, id: i64) {
        let at = self.proposal_index(id);
        self.docket_promote_slated_id(id);
        self.overseer_reseat_after_proposal(at);
    }

    /// `x` on a proposal: discard it.
    pub(crate) fn overseer_drop_proposal(&mut self, id: i64) {
        let at = self.proposal_index(id);
        self.docket_discard_id(id);
        self.overseer_reseat_after_proposal(at);
    }

    fn proposal_index(&self, id: i64) -> usize {
        overseer_model(self)
            .proposals
            .iter()
            .position(|card| card.id == id)
            .unwrap_or(0)
    }
}

impl App {
    pub(crate) fn handle_board_overseer_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => leave_modal(&mut self.state),
            KeyCode::Up | KeyCode::Char('k') => self.state.overseer_move_selection(BoardDir::Up),
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.overseer_move_selection(BoardDir::Down)
            }
            KeyCode::Enter => self.overseer_enter(),
            KeyCode::Char('a') => match self.state.overseer_selection() {
                Some(OverseerRow::Proposal(id)) => self.state.overseer_keep_proposal(id),
                _ => self.state.cycle_board_view(),
            },
            KeyCode::Char('x') => {
                if let Some(OverseerRow::Proposal(id)) = self.state.overseer_selection() {
                    self.state.overseer_drop_proposal(id);
                }
            }
            KeyCode::Char('?') => {
                // Help over the board, and back to the board when it closes.
                self.state.board.suspended = true;
                open_keybind_help(&mut self.state);
            }
            // Reserved for the chat, which lands in the next phase.
            KeyCode::Tab => {}
            _ => {}
        }
    }

    /// Enter on the selected row: a pane row focuses its pane and leaves the
    /// board; a docket row opens the item; a proposal is kept.
    pub(crate) fn overseer_enter(&mut self) {
        let model = overseer_model(&self.state);
        let Some(row) = model.effective_selection(self.state.board.overseer_selected) else {
            return;
        };
        match row {
            OverseerRow::NeedsYou(_) | OverseerRow::Agent(_) => {
                if let Some((ws_idx, pane_id)) = model.pane_target(row) {
                    self.board_focus_pane(ws_idx, pane_id);
                }
            }
            OverseerRow::Docket(id) => {
                self.state.board.docket_selected = Some(id);
                self.state.set_board_view(BoardView::DocketItem);
            }
            OverseerRow::Proposal(id) => self.state.overseer_keep_proposal(id),
        }
    }

    /// The header's one button. The session pane itself lands in a later
    /// phase; until then the button says so where the docket notice goes.
    pub(crate) fn open_overseer_session(&mut self) {
        if self.state.mode != Mode::Board {
            return;
        }
        self.state.board.docket_notice =
            Some("the overseer's session lands in the next phase".to_string());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{AppState, Mode};
    use crate::detect::{Agent, AgentState};
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Direction;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn set_state(state: &mut AppState, ws_idx: usize, pane: crate::layout::PaneId, s: AgentState) {
        let terminal_id = state.workspaces[ws_idx].tabs[0]
            .panes
            .get(&pane)
            .expect("pane")
            .attached_terminal_id
            .clone();
        let terminal = state.terminals.get_mut(&terminal_id).expect("terminal");
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = s;
    }

    /// An app on the overseer view: two agents in one group (one blocked),
    /// and a scratch docket with an inbox item, a due item and a proposal.
    fn overseer_app() -> (App, std::path::PathBuf, crate::layout::PaneId) {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        let mut ws = Workspace::test_new("one");
        let root = ws.tabs[0].root_pane;
        let second = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(root);
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        set_state(&mut state, 0, root, AgentState::Working);
        set_state(&mut state, 0, second, AgentState::Blocked);
        let path = crate::app::state::test_docket_db_path();
        state.docket_db = path.clone();
        {
            let conn = crate::docket::open_store(&path).expect("scratch docket");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "captured thing".into(),
                    ..Default::default()
                },
            )
            .expect("inbox item");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "due thing".into(),
                    kind: Some(crate::api::schema::DocketKind::Slated),
                    due: Some("2020-01-01".into()),
                    ..Default::default()
                },
            )
            .expect("due item");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "proposed thing".into(),
                    source: Some(serde_json::json!({"kind": "situation", "ref": "pane p2"})),
                    ..Default::default()
                },
            )
            .expect("proposal");
        }
        state.refresh_docket();
        app.state = state;
        app.state.open_board();
        (app, path, second)
    }

    fn cleanup(path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn status_of(app: &App, id: i64) -> Option<crate::api::schema::DocketStatus> {
        app.state
            .docket_sample
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.status)
    }

    #[tokio::test]
    async fn enter_on_agent_focuses_and_leaves() {
        let (mut app, path, blocked) = overseer_app();
        assert_eq!(app.state.board.view, BoardView::Overseer);
        // The blocked agent leads the needs-you region and is selected.
        assert_eq!(
            app.state.overseer_selection(),
            Some(OverseerRow::NeedsYou(blocked))
        );
        // Walk down to the agents table and pick the blocked one there too.
        app.handle_board_overseer_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.state.overseer_selection(),
            Some(OverseerRow::Agent(blocked))
        );
        app.handle_board_overseer_key(key(KeyCode::Enter));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(
            app.state.workspaces[0].focused_pane_id(),
            Some(blocked),
            "enter focused the selected pane"
        );
        cleanup(&path);
    }

    #[tokio::test]
    async fn enter_on_docket_row_opens_the_item() {
        let (mut app, path, _) = overseer_app();
        let model = overseer_model(&app.state);
        let due_id = model.docket.first().expect("due row").id;
        app.state.board.overseer_selected = Some(OverseerRow::Docket(due_id));
        app.handle_board_overseer_key(key(KeyCode::Enter));
        assert_eq!(app.state.board.view, BoardView::DocketItem);
        assert_eq!(app.state.board.docket_selected, Some(due_id));
        assert_eq!(app.state.mode, Mode::Board);
        // Esc from the item steps back to the docket lanes, as it always did.
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.board.view, BoardView::Docket);
        cleanup(&path);
    }

    #[tokio::test]
    async fn keep_slates_and_drop_discards_a_proposal() {
        let (mut app, path, _) = overseer_app();
        let proposals = overseer_model(&app.state).proposals;
        assert_eq!(proposals.len(), 1);
        let id = proposals[0].id;
        app.state.board.overseer_selected = Some(OverseerRow::Proposal(id));
        app.handle_board_overseer_key(key(KeyCode::Char('a')));
        assert_eq!(
            status_of(&app, id),
            Some(crate::api::schema::DocketStatus::Open)
        );
        assert!(app.state.board.docket_notice.is_none());
        // No proposals left: the selection lands somewhere real.
        let sel = app.state.overseer_selection().expect("a selection");
        assert!(!matches!(sel, OverseerRow::Proposal(_)));

        // A second proposal, dropped this time.
        {
            let conn = crate::docket::open_store(&path).expect("scratch docket");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "another".into(),
                    source: Some(serde_json::json!({"kind": "situation", "ref": "disk"})),
                    ..Default::default()
                },
            )
            .expect("proposal");
        }
        app.state.refresh_docket();
        let id = overseer_model(&app.state).proposals[0].id;
        app.state.board.overseer_selected = Some(OverseerRow::Proposal(id));
        app.handle_board_overseer_key(key(KeyCode::Char('x')));
        assert_eq!(
            status_of(&app, id),
            Some(crate::api::schema::DocketStatus::Discarded)
        );
        // `a` with nothing proposed selected goes round the views instead.
        app.state.board.overseer_selected = None;
        app.handle_board_overseer_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Columns);
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_cycles_overseer_columns_docket() {
        let (mut app, path, _) = overseer_app();
        app.state.board.overseer_selected = None;
        // From the first agent row, not a proposal, `a` is the view cycle.
        let first_agent = overseer_model(&app.state)
            .rows()
            .into_iter()
            .find(|row| matches!(row, OverseerRow::Agent(_)))
            .expect("an agent row");
        app.state.board.overseer_selected = Some(first_agent);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Columns);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Docket);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Overseer);
        cleanup(&path);
    }

    #[tokio::test]
    async fn help_from_the_board_returns_to_the_board() {
        let (mut app, path, _) = overseer_app();
        app.handle_board_key(key(KeyCode::Char('?')));
        assert_eq!(app.state.mode, Mode::KeybindHelp);
        assert!(app.state.board.suspended);
        assert!(app.state.board_underlay());
        super::super::modal::handle_keybind_help_key(&mut app.state, key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Board);
        assert!(!app.state.board.suspended);
        assert_eq!(app.state.board.view, BoardView::Overseer);
        // And esc from the board itself leaves it.
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Terminal);
        cleanup(&path);
    }

    #[tokio::test]
    async fn opening_a_stale_board_invokes_tick_once() {
        let (mut app, path, _) = overseer_app();
        app.state.mode = Mode::Terminal;
        // A fake overseer plugin whose tick is `true`.
        let plugin_root = crate::app::overseer::test_state_dir();
        std::fs::create_dir_all(&plugin_root).expect("plugin root");
        let manifest_path = plugin_root.join("shep-plugin.toml");
        std::fs::write(&manifest_path, "id = 'overseer'\n").expect("manifest");
        app.state.installed_plugins.insert(
            "overseer".into(),
            crate::api::schema::InstalledPluginInfo {
                plugin_id: "overseer".into(),
                name: "Overseer".into(),
                version: "0.1.0".into(),
                min_shep_version: "0.7.3".into(),
                description: None,
                manifest_path: manifest_path.display().to_string(),
                plugin_root: plugin_root.display().to_string(),
                enabled: true,
                platforms: None,
                build: Vec::new(),
                actions: vec![crate::api::schema::PluginManifestAction {
                    id: "tick".into(),
                    title: "tick".into(),
                    description: None,
                    contexts: Vec::new(),
                    platforms: None,
                    command: vec!["true".into()],
                }],
                events: Vec::new(),
                panes: Vec::new(),
                link_handlers: Vec::new(),
                source: crate::api::schema::PluginSourceInfo::default(),
                warnings: Vec::new(),
            },
        );
        // The situation was never written: stale by definition.
        app.open_board_live();
        assert_eq!(app.state.mode, Mode::Board);
        let ticks = |app: &App| {
            app.state
                .plugin_command_logs
                .iter()
                .filter(|log| log.action_id.as_deref() == Some("tick"))
                .count()
        };
        assert_eq!(ticks(&app), 1);
        let log_id = app.state.board.tick_in_flight.clone().expect("in flight");

        // Reopening while the tick runs does not stack another.
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(ticks(&app), 1);

        // The tick finishing clears the guard and re-reads the dir.
        app.handle_internal_event(crate::events::AppEvent::PluginCommandFinished {
            log_id,
            finished_unix_ms: 0,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            error: None,
        });
        assert!(app.state.board.tick_in_flight.is_none());

        // Still stale (nothing wrote a situation): a third open asks again.
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(ticks(&app), 2);
        let _ = std::fs::remove_dir_all(&plugin_root);
        cleanup(&path);
    }

    #[tokio::test]
    async fn opening_without_the_plugin_is_quiet() {
        let (mut app, path, _) = overseer_app();
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(app.state.mode, Mode::Board);
        assert!(app.state.board.tick_in_flight.is_none());
        assert!(app.state.plugin_command_logs.is_empty());
        assert!(app.state.board.docket_notice.is_none());
        cleanup(&path);
    }
}
