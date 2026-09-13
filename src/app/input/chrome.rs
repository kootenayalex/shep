//! Mouse on the window chrome: the titlebar and the overseer strip.
//!
//! Runs before every other mouse handler on the desktop layout and consumes
//! every event on those rows, so a click meant for the pill can never fall
//! through to the pane underneath — and a wheel over the titlebar never
//! scrolls one.

use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};

use super::App;
use crate::app::state::Mode;
use crate::ui::chrome::{titlebar_new_tab_at, titlebar_pill_at, PillHalf};

fn row_in(rect: ratatui::layout::Rect, row: u16) -> bool {
    rect.height > 0 && row >= rect.y && row < rect.y + rect.height
}

impl App {
    /// Handle a mouse event that lands on the chrome. Returns whether the
    /// event was consumed.
    pub(super) fn handle_chrome_mouse(&mut self, mouse: MouseEvent) -> bool {
        let on_titlebar = row_in(self.state.view.titlebar_rect, mouse.row);
        let on_strip = row_in(self.state.view.overseer_strip_rect, mouse.row);
        if !on_titlebar && !on_strip {
            return false;
        }
        // A gesture that began elsewhere keeps the mouse: dragging a split
        // border or a text selection up into the chrome must not cut it short.
        if self.state.drag.is_some() || self.state.selection.is_some() {
            return false;
        }
        // Only the screens the chrome fronts. A modal drawn over the top row
        // on a tiny terminal still gets its own clicks.
        if !matches!(
            self.state.mode,
            Mode::Terminal | Mode::Navigate | Mode::Prefix | Mode::Board
        ) {
            return false;
        }
        if let MouseEventKind::Down(MouseButton::Left) = mouse.kind {
            if on_strip {
                self.open_board_live();
            } else if titlebar_new_tab_at(&self.state, mouse.column, mouse.row) {
                // The tab bar's `+`, living on the breadcrumb while the bar
                // is hidden: same request the bar's button makes.
                if self.state.prompt_new_tab_name {
                    super::modal::open_new_tab_dialog(&mut self.state);
                } else {
                    self.state.request_new_tab = true;
                    self.state.mode = Mode::Terminal;
                }
            } else {
                match titlebar_pill_at(&self.state, mouse.column, mouse.row) {
                    Some(PillHalf::Board) if self.state.mode != Mode::Board => {
                        self.open_board_live();
                    }
                    Some(PillHalf::Desktop) if self.state.mode == Mode::Board => {
                        super::modal::leave_modal(&mut self.state);
                    }
                    _ => {}
                }
            }
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Rect;

    fn test_app() -> App {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        app.state = crate::ui::snapshot::fixture::session();
        app.state.mode = Mode::Terminal;
        app.state.overseer.sample = crate::app::overseer::OverseerSample::test_fixture();
        app.state.overseer_strip = true;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        app
    }

    fn click(col: u16, row: u16) -> MouseEvent {
        MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn pill(app: &App) -> crate::ui::chrome::TitlebarLayout {
        crate::ui::chrome::titlebar_layout(&app.state, None, app.state.view.titlebar_rect)
    }

    #[test]
    fn clicking_board_half_opens_board() {
        let mut app = test_app();
        let board = pill(&app).pill_board;
        assert!(board.width > 0);
        assert!(app.handle_chrome_mouse(click(board.x, board.y)));
        assert_eq!(app.state.mode, Mode::Board);
    }

    #[test]
    fn clicking_desktop_half_leaves_it() {
        let mut app = test_app();
        app.state.mode = Mode::Board;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        let desktop = pill(&app).pill_desktop;
        assert!(desktop.width > 0);
        assert!(app.handle_chrome_mouse(click(desktop.x, desktop.y)));
        assert_eq!(app.state.mode, Mode::Terminal);
        // Clicking the half that is already lit does nothing.
        let desktop = pill(&app).pill_desktop;
        assert!(app.handle_chrome_mouse(click(desktop.x, desktop.y)));
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_the_breadcrumb_plus_requests_a_new_tab() {
        let mut app = test_app();
        app.state.mouse_capture = true;
        app.state.hide_tab_bar_when_single_tab = true;
        app.state.prompt_new_tab_name = false;
        crate::ui::compute_view(&mut app.state, Rect::new(0, 0, 120, 40));
        let plus = pill(&app).new_tab;
        assert!(plus.width > 0, "the fixture's active group has one tab");
        assert!(!app.state.request_new_tab);
        assert!(app.handle_chrome_mouse(click(plus.x, plus.y)));
        assert!(app.state.request_new_tab);
        assert_eq!(app.state.mode, Mode::Terminal);
    }

    #[test]
    fn clicking_the_strip_opens_board() {
        let mut app = test_app();
        let strip = app.state.view.overseer_strip_rect;
        assert_eq!(strip.height, 1, "the fixture has a narrative to show");
        assert!(app.handle_chrome_mouse(click(5, strip.y)));
        assert_eq!(app.state.mode, Mode::Board);
    }

    #[test]
    fn titlebar_click_never_reaches_the_pane() {
        let mut app = test_app();
        let titlebar = app.state.view.titlebar_rect;
        let before = app.state.workspaces[0].focused_pane_id();
        // The active group's second pane is under the titlebar's right half;
        // a click there must not focus it.
        let pane_under = app
            .state
            .view
            .pane_infos
            .iter()
            .find(|info| info.rect.x + info.rect.width >= titlebar.width - 2)
            .map(|info| info.rect.x + 1)
            .expect("a pane spans the right edge");
        app.handle_mouse(click(pane_under, titlebar.y));
        assert_eq!(app.state.workspaces[0].focused_pane_id(), before);
        assert_eq!(app.state.mode, Mode::Terminal);

        // The wheel over the titlebar is swallowed too.
        assert!(app.handle_chrome_mouse(MouseEvent {
            kind: MouseEventKind::ScrollDown,
            column: 10,
            row: titlebar.y,
            modifiers: KeyModifiers::NONE,
        }));
        // And a click on the strip's row lands on the strip, not the pane.
        let strip = app.state.view.overseer_strip_rect;
        app.handle_mouse(click(pane_under, strip.y));
        assert_eq!(app.state.mode, Mode::Board);
    }
}
