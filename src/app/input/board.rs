//! Input handling for the session board overlay (M1b).
//!
//! Selection/geometry live in `crate::ui::board` (pure, testable). This module
//! wires keyboard and mouse input to those helpers: arrows/hjkl move selection,
//! Enter closes the board and focuses the selected pane, Esc/q close. Focus goes
//! through the runtime API path (`focus_pane_internal_via_api`) so pane focus
//! stays a shared runtime fact, not TUI-only state.

use crossterm::event::KeyEvent;

use crate::app::{
    state::{AppState, Mode},
    App,
};
use crate::ui::board::{self, BoardDir};

use super::modal::leave_modal;

impl AppState {
    /// Open the session board, seeding the selection.
    pub(crate) fn open_board(&mut self) {
        self.board.selected = board::initial_selection(self);
        self.mode = Mode::Board;
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
            KeyCode::Enter => self.board_focus_selected(),
            _ => {}
        }
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
        // Start on the focused working pane.
        state.board.selected = Some(root);
        // Left jumps to the blocked column (leftmost non-empty).
        state.board_move_selection(BoardDir::Left, false);
        assert_eq!(state.board.selected, Some(second));
    }

    #[test]
    fn enter_target_matches_selection() {
        let (mut state, _root, second) = board_app();
        state.board.selected = Some(second);
        assert_eq!(state.board_enter_target(), Some((0, second)));
    }
}
