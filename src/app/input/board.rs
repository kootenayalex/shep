//! Input handling for the session board overlay.
//!
//! Selection/geometry live in `crate::ui::board` (pure, testable). This module
//! wires keyboard and mouse input to those helpers: arrows/hjkl move selection,
//! Enter closes the board and focuses the selected pane, Esc/q close. Focus goes
//! through the runtime API path (`focus_pane_internal_via_api`) so pane focus
//! stays a shared runtime fact, not TUI-only state.
//!
//! The docket board's verbs write to the docket store directly — same
//! process, same file the server's `docket.*` handlers open — and re-sample
//! the rows straight after, so the card you just completed moves before the
//! next tick.

use crossterm::event::KeyEvent;

use crate::api::schema::{DocketKind, DocketRepeat, DocketStatus};
use crate::app::{
    state::{AppState, BoardView, Mode},
    App,
};
use crate::docket::{self, DocketError};
use crate::ui::board::{self, BoardDir};

use super::modal::{leave_modal, open_new_docket_item};

impl AppState {
    /// Open the session board, seeding the agent selection. Always opens on
    /// the configured lane board — the detail screens are somewhere you go,
    /// not somewhere you come back to. The docket selection is seeded lazily
    /// by the board itself, so opening never reads the store.
    pub(crate) fn open_board(&mut self) {
        self.board.selected = board::initial_selection(self);
        self.board.view = self.board_default_view.lanes();
        self.board.docket_notice = None;
        self.mode = Mode::Board;
    }

    /// Switch board screens.
    pub(crate) fn set_board_view(&mut self, view: BoardView) {
        self.board.view = view;
    }

    /// Move the docket selection in `dir`, from the card the board is
    /// showing as selected — which, before any key, is the first one.
    pub(crate) fn docket_move_selection(&mut self, dir: BoardDir, narrow: bool) {
        let model = board::docket_board_model(&self.docket_sample);
        let current = model.effective_selection(self.board.docket_selected);
        if let Some(next) = board::next_docket_selection(&model, current, dir, narrow) {
            self.board.docket_selected = Some(next);
        }
    }

    /// The docket item the board treats as selected.
    pub(crate) fn docket_selected_id(&self) -> Option<i64> {
        board::docket_board_model(&self.docket_sample)
            .effective_selection(self.board.docket_selected)
    }

    /// Run one store verb against the docket, then re-sample. A refusal
    /// (promoting a done item, completing an inbox one) is shown on the
    /// board, not swallowed. The selection is kept on the same lane and row
    /// when the item it pointed at has moved on, so `d d d` works down a lane.
    fn docket_apply(
        &mut self,
        op: impl FnOnce(&rusqlite::Connection) -> Result<crate::api::schema::DocketItem, DocketError>,
    ) -> Option<i64> {
        let before = board::docket_board_model(&self.docket_sample);
        let place = self.docket_selected_id().and_then(|id| before.locate(id));
        let result = docket::open_store(&self.docket_db)
            .map_err(DocketError::from)
            .and_then(|conn| op(&conn));
        let touched = match result {
            Ok(item) => Some(item.id),
            Err(err) => {
                tracing::warn!(error = %err, "docket verb refused");
                self.board.docket_notice = Some(err.to_string());
                None
            }
        };
        self.refresh_docket();
        let after = board::docket_board_model(&self.docket_sample);
        if let Some(id) = self.board.docket_selected {
            if after.locate(id).is_none() {
                self.board.docket_selected = place.and_then(|(lane, row)| {
                    let cards = after.lane(board::DocketLane::ALL[lane]);
                    cards
                        .get(row.min(cards.len().saturating_sub(1)))
                        .map(|card| card.id)
                });
            }
        }
        touched
    }

    /// `n`: a captured item in the inbox, from the title typed into the
    /// modal. The new card becomes the selection.
    pub(crate) fn docket_add_captured(&mut self, title: String) {
        let added = self.docket_apply(|conn| {
            docket::add(
                conn,
                docket::NewItem {
                    title,
                    kind: Some(DocketKind::Captured),
                    status: Some(DocketStatus::Inbox),
                    ..Default::default()
                },
            )
        });
        if let Some(id) = added {
            self.board.docket_selected = Some(id);
        }
    }

    /// `p`: promote the selected inbox item as slated, undated. A date can
    /// follow from the CLI; the board's job is the decision.
    pub(crate) fn docket_promote_slated(&mut self) {
        let Some(id) = self.docket_selected_id() else {
            return;
        };
        self.docket_apply(|conn| docket::promote(conn, id, DocketKind::Slated, None, None));
    }

    /// `r`: promote the selected inbox item as recurring, weekly by default.
    pub(crate) fn docket_promote_recurring(&mut self) {
        let Some(id) = self.docket_selected_id() else {
            return;
        };
        self.docket_apply(|conn| {
            docket::promote(
                conn,
                id,
                DocketKind::Recurring,
                None,
                Some(DocketRepeat::Weekly),
            )
        });
    }

    /// `d`: complete the selected open item (a recurring one rolls forward).
    pub(crate) fn docket_complete_selected(&mut self) {
        let Some(id) = self.docket_selected_id() else {
            return;
        };
        self.docket_apply(|conn| docket::complete(conn, id));
    }

    /// `x`: discard the selected live item.
    pub(crate) fn docket_discard_selected(&mut self) {
        let Some(id) = self.docket_selected_id() else {
            return;
        };
        self.docket_apply(|conn| docket::discard(conn, id));
    }

    /// Move the board selection in `dir`. `narrow` picks the stacked traversal
    /// model (single list) over the wide grid.
    pub(crate) fn board_move_selection(&mut self, dir: BoardDir, narrow: bool) {
        if let Some(next) = board::next_selection(self, self.board.selected, dir, narrow) {
            self.board.selected = Some(next);
        }
    }

    /// The `(workspace index, pane)` the current selection resolves to.
    pub(crate) fn board_enter_target(&self) -> Option<(usize, crate::layout::PaneId)> {
        board::enter_target(self, self.board.selected)
    }
}

impl App {
    pub(crate) fn handle_board_key(&mut self, key: KeyEvent) {
        // A notice answers the last verb; the next key is a new question.
        self.state.board.docket_notice = None;
        match self.state.board.view {
            BoardView::Docket => self.handle_board_docket_key(key),
            BoardView::DocketItem => self.handle_board_docket_item_key(key),
            BoardView::Columns => self.handle_board_columns_key(key),
            BoardView::Agent => self.handle_board_agent_key(key),
        }
    }

    /// The docket lanes. Movement is the agent board's; the verbs are the
    /// store's, one key each, and `a` flips to the agent lanes.
    fn handle_board_docket_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        let narrow = board::is_docket_narrow(&self.state);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => leave_modal(&mut self.state),
            KeyCode::Left | KeyCode::Char('h') => {
                self.state.docket_move_selection(BoardDir::Left, narrow)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.state.docket_move_selection(BoardDir::Right, narrow)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.docket_move_selection(BoardDir::Up, narrow)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.docket_move_selection(BoardDir::Down, narrow)
            }
            KeyCode::Enter | KeyCode::Char('i') if self.state.docket_selected_id().is_some() => {
                self.state.set_board_view(BoardView::DocketItem)
            }
            KeyCode::Char('a') => self.state.set_board_view(BoardView::Columns),
            KeyCode::Char('n') => open_new_docket_item(&mut self.state),
            _ => self.handle_docket_verb(key),
        }
    }

    /// Docket item detail. Esc steps back to the lanes rather than closing
    /// the board; the verbs still apply to the item on screen.
    fn handle_board_docket_item_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') | KeyCode::Enter => {
                self.state.set_board_view(BoardView::Docket)
            }
            _ => self.handle_docket_verb(key),
        }
    }

    /// The store verbs both docket screens share.
    fn handle_docket_verb(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Char('p') => self.state.docket_promote_slated(),
            KeyCode::Char('r') => self.state.docket_promote_recurring(),
            KeyCode::Char('d') => self.state.docket_complete_selected(),
            KeyCode::Char('x') => self.state.docket_discard_selected(),
            _ => {}
        }
    }

    fn handle_board_columns_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        let narrow = board::is_narrow(&self.state);
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => leave_modal(&mut self.state),
            KeyCode::Left | KeyCode::Char('h') => {
                self.state.board_move_selection(BoardDir::Left, narrow)
            }
            KeyCode::Right | KeyCode::Char('l') => {
                self.state.board_move_selection(BoardDir::Right, narrow)
            }
            KeyCode::Up | KeyCode::Char('k') => {
                self.state.board_move_selection(BoardDir::Up, narrow)
            }
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.board_move_selection(BoardDir::Down, narrow)
            }
            // Move the selected agent's whole lane, which is the group's own
            // order — a shared session fact, so it goes through the API and
            // survives a handoff rather than living in this client.
            KeyCode::Char('<') | KeyCode::Char('H') => self.board_move_lane(-1),
            KeyCode::Char('>') | KeyCode::Char('L') => self.board_move_lane(1),
            KeyCode::Enter => self.board_focus_selected(),
            // Inspect without attaching. Enter stays the fast path straight
            // into the pane; this is the "tell me more first" path.
            KeyCode::Char('i') if self.state.board.selected.is_some() => {
                self.state.set_board_view(BoardView::Agent)
            }
            KeyCode::Char('a') => self.state.set_board_view(BoardView::Docket),
            _ => {}
        }
    }

    /// Agent detail. Esc steps back to the columns rather than closing the
    /// board, so the board is never more than one Esc away from itself.
    fn handle_board_agent_key(&mut self, key: KeyEvent) {
        use crossterm::event::KeyCode;
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('i') => {
                self.state.set_board_view(BoardView::Columns)
            }
            KeyCode::Enter => self.board_focus_selected(),
            _ => {}
        }
    }

    /// Move the lane holding the selection one place left or right.
    ///
    /// `workspace.move` reorders groups for the whole session, so the board,
    /// the sidebar and the phone all see the same order afterwards.
    fn board_move_lane(&mut self, delta: isize) {
        let Some((ws_idx, _)) = self.state.board_enter_target() else {
            return;
        };
        let target = ws_idx as isize + delta;
        if target < 0 || target >= self.state.workspaces.len() as isize {
            return;
        }
        self.move_workspace_via_api(ws_idx, target as usize);
    }

    fn board_focus_selected(&mut self) {
        if let Some((ws_idx, pane_id)) = self.state.board_enter_target() {
            self.focus_pane_internal_via_api(ws_idx, pane_id);
        }
        leave_modal(&mut self.state);
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use crate::app::state::{AppState, Mode};
    use crate::detect::{Agent, AgentState};
    use crate::ui::board::BoardDir;
    use crate::workspace::Workspace;

    fn set_state(
        state: &mut AppState,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        s: AgentState,
    ) {
        let terminal_id = state.workspaces[ws_idx].tabs[0]
            .panes
            .get(&pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        let terminal = state.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = s;
    }

    fn board_app() -> (AppState, crate::layout::PaneId, crate::layout::PaneId) {
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
        (state, root, second)
    }

    #[test]
    fn open_board_seeds_selection_and_mode() {
        let (mut state, _root, _second) = board_app();
        state.open_board();
        assert_eq!(state.mode, Mode::Board);
        // Focused pane (root) is working, not blocked, but it is on the board so
        // it is preferred as the initial selection.
        assert!(state.board.selected.is_some());
    }

    #[test]
    fn move_selection_updates_board_state() {
        let (mut state, root, second) = board_app();
        state.open_board();
        // Both agents share one group, so they share a lane: up and down move
        // between them, and the blocked one sorts first.
        state.board.selected = Some(root);
        state.board_move_selection(BoardDir::Up, false);
        assert_eq!(state.board.selected, Some(second));
        state.board_move_selection(BoardDir::Down, false);
        assert_eq!(state.board.selected, Some(root));
    }

    #[test]
    fn open_board_always_lands_on_the_configured_lanes() {
        use crate::app::state::BoardView;
        let (mut state, _root, _second) = board_app();
        // The docket by default, and never a detail screen.
        state.board.view = BoardView::Agent;
        state.open_board();
        assert_eq!(state.board.view, BoardView::Docket);
        state.board_default_view = BoardView::Columns;
        state.board.view = BoardView::DocketItem;
        state.open_board();
        assert_eq!(state.board.view, BoardView::Columns);
    }

    #[test]
    fn enter_target_matches_selection() {
        let (mut state, _root, second) = board_app();
        state.board.selected = Some(second);
        assert_eq!(state.board_enter_target(), Some((0, second)));
    }

    // -----------------------------------------------------------------------
    // The docket board
    // -----------------------------------------------------------------------

    use crate::api::schema::{DocketKind, DocketRepeat, DocketStatus};
    use crate::app::state::BoardView;

    /// A state whose docket is a scratch file this test alone owns, seeded
    /// with one inbox item, one open item and one weekly item, all read into
    /// the sample.
    fn docket_app() -> (AppState, std::path::PathBuf) {
        let mut state = AppState::test_new();
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
                    title: "slated thing".into(),
                    kind: Some(DocketKind::Slated),
                    ..Default::default()
                },
            )
            .expect("slated item");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "weekly thing".into(),
                    kind: Some(DocketKind::Recurring),
                    repeat: Some(DocketRepeat::Weekly),
                    ..Default::default()
                },
            )
            .expect("recurring item");
        }
        state.refresh_docket();
        state.open_board();
        (state, path)
    }

    fn cleanup(path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn status_of(state: &AppState, id: i64) -> Option<DocketStatus> {
        state
            .docket_sample
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.status)
    }

    #[test]
    fn open_board_seeds_no_docket_selection_but_the_board_treats_the_first_as_selected() {
        let (state, path) = docket_app();
        assert_eq!(state.board.view, BoardView::Docket);
        assert_eq!(state.board.docket_selected, None);
        assert_eq!(state.docket_selected_id(), Some(1), "the inbox item leads");
        cleanup(&path);
    }

    #[test]
    fn docket_verbs_go_through_the_store_and_resample() {
        let (mut state, path) = docket_app();
        // `p` on the inbox item: slated, open, and it leaves the inbox lane.
        state.board.docket_selected = Some(1);
        state.docket_promote_slated();
        assert_eq!(status_of(&state, 1), Some(DocketStatus::Open));
        assert!(state.board.docket_notice.is_none());
        // `d` on the weekly item: still open, due rolled a week past today.
        state.board.docket_selected = Some(3);
        state.docket_complete_selected();
        assert_eq!(status_of(&state, 3), Some(DocketStatus::Open));
        let due = state
            .docket_sample
            .rows
            .iter()
            .find(|r| r.id == 3)
            .unwrap()
            .due
            .clone();
        let today = state.docket_sample.today.expect("sampled today");
        assert_eq!(
            due.as_deref().and_then(crate::docket::dates::Date::parse),
            Some(today.add_days(7))
        );
        // `d` on the slated item: done.
        state.board.docket_selected = Some(2);
        state.docket_complete_selected();
        assert_eq!(status_of(&state, 2), Some(DocketStatus::Done));
        // `x` on the promoted item: discarded and off the board.
        state.board.docket_selected = Some(1);
        state.docket_discard_selected();
        assert_eq!(status_of(&state, 1), Some(DocketStatus::Discarded));
        assert_ne!(state.board.docket_selected, Some(1));
        cleanup(&path);
    }

    #[test]
    fn a_refused_verb_leaves_a_notice_and_the_rows_alone() {
        let (mut state, path) = docket_app();
        // Completing an inbox item is not a thing.
        state.board.docket_selected = Some(1);
        state.docket_complete_selected();
        assert_eq!(status_of(&state, 1), Some(DocketStatus::Inbox));
        let notice = state.board.docket_notice.clone().expect("notice");
        assert!(notice.contains("only open items"), "{notice}");
        // Promoting an open item is not a thing either.
        state.board.docket_selected = Some(2);
        state.docket_promote_recurring();
        assert_eq!(status_of(&state, 2), Some(DocketStatus::Open));
        assert!(state.board.docket_notice.is_some());
        cleanup(&path);
    }

    #[test]
    fn promoting_as_recurring_defaults_to_weekly() {
        let (mut state, path) = docket_app();
        state.board.docket_selected = Some(1);
        state.docket_promote_recurring();
        let row = state.docket_sample.rows.iter().find(|r| r.id == 1).unwrap();
        assert_eq!(row.kind, DocketKind::Recurring);
        assert_eq!(row.repeat, Some(DocketRepeat::Weekly));
        assert_eq!(row.status, DocketStatus::Open);
        cleanup(&path);
    }

    #[test]
    fn the_selection_stays_on_the_lane_and_row_after_its_card_moves_on() {
        let (mut state, path) = docket_app();
        // Two inbox items; discard the first and the selection lands on the
        // one that took its row.
        {
            let conn = crate::docket::open_store(&path).expect("scratch docket");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "second captured".into(),
                    ..Default::default()
                },
            )
            .expect("inbox item");
        }
        state.refresh_docket();
        let inbox: Vec<i64> = crate::ui::board::docket_board_model(&state.docket_sample)
            .lane(crate::ui::board::DocketLane::Inbox)
            .iter()
            .map(|card| card.id)
            .collect();
        assert_eq!(inbox.len(), 2);
        state.board.docket_selected = Some(inbox[0]);
        state.docket_discard_selected();
        assert_eq!(state.board.docket_selected, Some(inbox[1]));
        cleanup(&path);
    }

    #[test]
    fn a_new_item_lands_in_the_inbox_and_is_selected() {
        let (mut state, path) = docket_app();
        state.docket_add_captured("from the board".into());
        let row = state
            .docket_sample
            .rows
            .iter()
            .find(|r| r.title == "from the board")
            .expect("added row");
        assert_eq!(row.status, DocketStatus::Inbox);
        assert_eq!(row.kind, DocketKind::Captured);
        assert_eq!(state.board.docket_selected, Some(row.id));
        cleanup(&path);
    }

    #[test]
    fn docket_selection_moves_between_lanes() {
        let (mut state, path) = docket_app();
        state.view.terminal_area = ratatui::layout::Rect::new(0, 0, 200, 50);
        state.view.sidebar_rect = ratatui::layout::Rect::new(0, 0, 200, 50);
        state.docket_move_selection(BoardDir::Right, false);
        // Inbox -> slated (the due lane is empty and skipped).
        assert_eq!(state.board.docket_selected, Some(2));
        state.docket_move_selection(BoardDir::Right, false);
        assert_eq!(state.board.docket_selected, Some(3));
        state.docket_move_selection(BoardDir::Left, false);
        state.docket_move_selection(BoardDir::Left, false);
        assert_eq!(state.board.docket_selected, Some(1));
        cleanup(&path);
    }

    fn key(code: crossterm::event::KeyCode) -> crossterm::event::KeyEvent {
        crossterm::event::KeyEvent::new(code, crossterm::event::KeyModifiers::NONE)
    }

    #[tokio::test]
    async fn docket_keys_toggle_boards_open_the_detail_and_capture_a_title() {
        use crossterm::event::KeyCode;
        let mut app = crate::app::App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        let (state, path) = docket_app();
        app.state = state;
        assert_eq!(app.state.mode, Mode::Board);
        assert_eq!(app.state.board.view, BoardView::Docket);

        // `a` flips to the agent lanes and back.
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Columns);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Docket);

        // `i` opens the item, esc steps back to the lanes, not out.
        app.handle_board_key(key(KeyCode::Char('i')));
        assert_eq!(app.state.board.view, BoardView::DocketItem);
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.board.view, BoardView::Docket);
        assert_eq!(app.state.mode, Mode::Board);

        // A verb from the detail screen works on the item on screen.
        app.state.board.docket_selected = Some(2);
        app.handle_board_key(key(KeyCode::Char('i')));
        app.handle_board_key(key(KeyCode::Char('d')));
        assert_eq!(status_of(&app.state, 2), Some(DocketStatus::Done));
        app.handle_board_key(key(KeyCode::Esc));

        // `n` opens the text prompt; enter files the title and returns to
        // the board rather than to a pane.
        app.handle_board_key(key(KeyCode::Char('n')));
        assert_eq!(app.state.mode, Mode::NewDocketItem);
        for c in "typed on the board".chars() {
            app.handle_rename_key_via_api(key(KeyCode::Char(c)));
        }
        app.handle_rename_key_via_api(key(KeyCode::Enter));
        assert_eq!(app.state.mode, Mode::Board);
        assert!(app
            .state
            .docket_sample
            .rows
            .iter()
            .any(|r| r.title == "typed on the board" && r.status == DocketStatus::Inbox));

        // Esc from the prompt also lands on the board.
        app.handle_board_key(key(KeyCode::Char('n')));
        app.handle_rename_key_via_api(key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Board);

        // Esc from the lanes closes the board.
        app.handle_board_key(key(KeyCode::Esc));
        assert_ne!(app.state.mode, Mode::Board);
        cleanup(&path);
    }
}
