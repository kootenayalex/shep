//! Session board overlay (M1b): a full-screen kanban-style overview answering
//! "what are all my agents doing right now". Columns are agent state, blocked
//! leftmost (Blocked | Done | Working | Idle); one card per agent pane. On
//! narrow terminals it collapses to a single stacked list grouped by state,
//! blocked group first.
//!
//! Everything here is pure TUI presentation: the model, geometry, selection
//! traversal, and enter-focus resolution are computed from `&AppState` so they
//! are unit-testable, and `render` only draws (no state mutation). Board
//! ordering reuses `agent_panel_entries` and `crate::workspace::attention_priority`
//! so it agrees with the sidebar.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::sidebar::{agent_panel_entries, format_event_age};
use super::status::{state_dot, state_label};
use super::text::truncate_end;
use super::widgets::render_panel_shell;
use crate::app::state::AppState;
use crate::detect::AgentState;
use crate::layout::PaneId;

/// Number of state columns (blocked, done, working, idle).
pub(crate) const BOARD_COLUMNS: usize = 4;

/// Visible rows per card (agent line, workspace/branch/age line, status line).
const CARD_ROWS: u16 = 3;
/// Card slot height including a one-row gap between cards.
const CARD_STRIDE: u16 = CARD_ROWS + 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BoardDir {
    Up,
    Down,
    Left,
    Right,
}

/// Column index for an agent state, blocked leftmost. `done` is idle+unseen,
/// `idle` is idle+seen. Unknown-state agents fold into the idle column (least
/// attention) so no agent silently disappears from the board.
///
/// Column order is the descending `attention_priority` order so the board and
/// the sidebar agree on what needs the user first: a finished (done) agent
/// needs eyes before a working one does.
pub(crate) fn board_column_index(state: AgentState, seen: bool) -> usize {
    match (state, seen) {
        (AgentState::Blocked, _) => 0,
        (AgentState::Idle, false) => 1,
        (AgentState::Working, _) => 2,
        (AgentState::Idle, true) => 3,
        (AgentState::Unknown, _) => 3,
    }
}

fn board_column_title(col: usize) -> &'static str {
    match col {
        0 => "blocked",
        1 => "done",
        2 => "working",
        _ => "idle",
    }
}

#[derive(Debug, Clone)]
pub(crate) struct BoardCard {
    pub ws_idx: usize,
    pub pane_id: PaneId,
    pub agent_label: String,
    pub workspace_label: String,
    /// Tab/pane location tag, e.g. `t2·p1` (multi-tab) or `p3`.
    pub location: String,
    pub branch: Option<String>,
    pub status: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub context_percent: Option<u8>,
    sort_seq: Option<u64>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BoardModel {
    pub columns: [Vec<BoardCard>; BOARD_COLUMNS],
}

impl BoardModel {
    /// Flattened card order for narrow/stacked traversal and rendering: column
    /// order (blocked, done, working, idle), each column already sorted.
    pub(crate) fn flattened(&self) -> Vec<&BoardCard> {
        self.columns.iter().flatten().collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.columns.iter().all(|cards| cards.is_empty())
    }

    fn locate(&self, pane_id: PaneId) -> Option<(usize, usize)> {
        for (col, cards) in self.columns.iter().enumerate() {
            if let Some(row) = cards.iter().position(|card| card.pane_id == pane_id) {
                return Some((col, row));
            }
        }
        None
    }
}

/// Build the board model from the same agent panel entries the sidebar uses,
/// bucketed into state columns and sorted within each column by attention
/// priority (then most-recent state change), so ordering agrees with the
/// sidebar's priority sort.
pub(crate) fn board_model(app: &AppState) -> BoardModel {
    let mut model = BoardModel::default();
    for entry in agent_panel_entries(app) {
        let col = board_column_index(entry.state, entry.seen);
        let ws = app.workspaces.get(entry.ws_idx);
        let branch = ws.and_then(|ws| ws.branch());
        let pane_number = ws.and_then(|ws| ws.public_pane_number(entry.pane_id));
        let tab_number = ws.and_then(|ws| ws.public_tab_number(entry.tab_idx));
        let multi_tab = ws.map(|ws| ws.tabs.len() > 1).unwrap_or(false);
        let location = match (multi_tab, tab_number, pane_number) {
            (true, Some(tab), Some(pane)) => format!("t{tab}·p{pane}"),
            (_, _, Some(pane)) => format!("p{pane}"),
            _ => String::new(),
        };
        model.columns[col].push(BoardCard {
            ws_idx: entry.ws_idx,
            pane_id: entry.pane_id,
            agent_label: entry.agent_label.unwrap_or_else(|| "agent".to_string()),
            workspace_label: entry.primary_label,
            location,
            branch,
            status: entry.custom_status,
            state: entry.state,
            seen: entry.seen,
            context_percent: entry.context_percent,
            sort_seq: entry.last_agent_state_change_seq,
        });
    }
    for cards in &mut model.columns {
        cards.sort_by_key(|card| {
            (
                std::cmp::Reverse(crate::workspace::attention_priority(card.state, card.seen)),
                std::cmp::Reverse(card.sort_seq),
            )
        });
    }
    model
}

/// The initial board selection when opening: the currently focused pane if it
/// is on the board, otherwise the first card in blocked-first order.
pub(crate) fn initial_selection(app: &AppState) -> Option<PaneId> {
    let model = board_model(app);
    if model.is_empty() {
        return None;
    }
    let focused = app
        .active
        .and_then(|idx| app.workspaces.get(idx))
        .and_then(crate::workspace::Workspace::focused_pane_id);
    if let Some(pane) = focused {
        if model.locate(pane).is_some() {
            return Some(pane);
        }
    }
    model.flattened().first().map(|card| card.pane_id)
}

/// Compute the next selected pane after moving `dir` from `current`.
///
/// Wide grid: up/down move within a column with wraparound; left/right jump to
/// the nearest non-empty column in that direction (no horizontal wraparound),
/// clamping the row. Narrow stacked list: up/down traverse the flattened order
/// with wraparound; left/right are no-ops. When nothing valid is selected, the
/// first card is chosen.
pub(crate) fn next_selection(
    app: &AppState,
    current: Option<PaneId>,
    dir: BoardDir,
    narrow: bool,
) -> Option<PaneId> {
    let model = board_model(app);
    if model.is_empty() {
        return None;
    }
    let flat = model.flattened();
    let Some(current) = current.filter(|pane| model.locate(*pane).is_some()) else {
        return flat.first().map(|card| card.pane_id);
    };

    if narrow {
        let idx = flat.iter().position(|card| card.pane_id == current)?;
        let next = match dir {
            BoardDir::Up => (idx + flat.len() - 1) % flat.len(),
            BoardDir::Down => (idx + 1) % flat.len(),
            BoardDir::Left | BoardDir::Right => return Some(current),
        };
        return flat.get(next).map(|card| card.pane_id);
    }

    let (col, row) = model.locate(current)?;
    match dir {
        BoardDir::Up | BoardDir::Down => {
            let len = model.columns[col].len();
            if len == 0 {
                return Some(current);
            }
            let next_row = match dir {
                BoardDir::Up => (row + len - 1) % len,
                _ => (row + 1) % len,
            };
            model.columns[col].get(next_row).map(|card| card.pane_id)
        }
        BoardDir::Left | BoardDir::Right => {
            let Some(target_col) = nearest_nonempty_column(&model, col, dir) else {
                return Some(current);
            };
            let len = model.columns[target_col].len();
            let clamped = row.min(len.saturating_sub(1));
            model.columns[target_col]
                .get(clamped)
                .map(|card| card.pane_id)
        }
    }
}

fn nearest_nonempty_column(model: &BoardModel, col: usize, dir: BoardDir) -> Option<usize> {
    match dir {
        BoardDir::Left => (0..col).rev().find(|c| !model.columns[*c].is_empty()),
        BoardDir::Right => ((col + 1)..BOARD_COLUMNS).find(|c| !model.columns[*c].is_empty()),
        BoardDir::Up | BoardDir::Down => None,
    }
}

/// Resolve the selected card to the `(workspace index, pane)` to focus on Enter.
pub(crate) fn enter_target(app: &AppState, selected: Option<PaneId>) -> Option<(usize, PaneId)> {
    let model = board_model(app);
    let pane = selected?;
    let (col, row) = model.locate(pane)?;
    let card = model.columns[col].get(row)?;
    Some((card.ws_idx, card.pane_id))
}

// ---------------------------------------------------------------------------
// Geometry (shared by render and mouse hit-testing)
// ---------------------------------------------------------------------------

/// Full-screen area the board occupies (the whole app surface).
pub(crate) fn board_area(app: &AppState) -> Rect {
    app.view.sidebar_rect.union(app.view.terminal_area)
}

/// Whether the board should collapse to a stacked single-column layout, using
/// the same width threshold as the mobile layout switch.
pub(crate) fn is_narrow(app: &AppState) -> bool {
    super::mobile::is_mobile_width(board_area(app), app.mobile_width_threshold)
}

fn inner_area(area: Rect) -> Option<Rect> {
    if area.width < 2 || area.height < 2 {
        return None;
    }
    Some(Rect::new(
        area.x + 1,
        area.y + 1,
        area.width - 2,
        area.height - 2,
    ))
}

/// The card region inside the panel, reserving a title row and a footer row.
fn board_body(inner: Rect) -> Rect {
    if inner.height <= 2 {
        return inner;
    }
    Rect::new(
        inner.x,
        inner.y + 1,
        inner.width,
        inner.height.saturating_sub(2),
    )
}

fn column_rects(body: Rect) -> [Rect; BOARD_COLUMNS] {
    let cols = BOARD_COLUMNS as u16;
    let base = body.width / cols;
    let extra = body.width % cols;
    let mut rects = [Rect::default(); BOARD_COLUMNS];
    let mut x = body.x;
    for (i, rect) in rects.iter_mut().enumerate() {
        let w = base + if (i as u16) < extra { 1 } else { 0 };
        *rect = Rect::new(x, body.y, w, body.height);
        x = x.saturating_add(w);
    }
    rects
}

#[derive(Clone, Copy)]
struct CardSlot {
    rect: Rect,
    col: usize,
    row: usize,
}

fn wide_slots(model: &BoardModel, body: Rect) -> Vec<CardSlot> {
    let cols = column_rects(body);
    let mut slots = Vec::new();
    for (col, col_rect) in cols.iter().enumerate() {
        // One header row + one gap row before the first card.
        let body_y = col_rect.y.saturating_add(2);
        let avail = col_rect.height.saturating_sub(2);
        let max_cards = (avail / CARD_STRIDE) as usize;
        for row in 0..model.columns[col].len().min(max_cards) {
            let y = body_y + (row as u16) * CARD_STRIDE;
            slots.push(CardSlot {
                rect: Rect::new(col_rect.x, y, col_rect.width, CARD_ROWS),
                col,
                row,
            });
        }
    }
    slots
}

/// Card slots plus `(y, column)` header positions for the stacked layout.
fn narrow_slots(model: &BoardModel, body: Rect) -> (Vec<CardSlot>, Vec<(u16, usize)>) {
    let mut slots = Vec::new();
    let mut headers = Vec::new();
    let bottom = body.y + body.height;
    let mut y = body.y;
    for col in 0..BOARD_COLUMNS {
        if model.columns[col].is_empty() || y >= bottom {
            continue;
        }
        headers.push((y, col));
        y = y.saturating_add(1);
        for row in 0..model.columns[col].len() {
            if y.saturating_add(CARD_ROWS) > bottom {
                break;
            }
            slots.push(CardSlot {
                rect: Rect::new(body.x, y, body.width, CARD_ROWS),
                col,
                row,
            });
            y = y.saturating_add(CARD_STRIDE);
        }
        y = y.saturating_add(1);
    }
    (slots, headers)
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn slots_for(app: &AppState, model: &BoardModel) -> Option<(Rect, Vec<CardSlot>)> {
    let inner = inner_area(board_area(app))?;
    let body = board_body(inner);
    let slots = if is_narrow(app) {
        narrow_slots(model, body).0
    } else {
        wide_slots(model, body)
    };
    Some((body, slots))
}

/// The `(workspace index, pane)` of the card under a click, if any.
pub(crate) fn card_at(app: &AppState, col: u16, row: u16) -> Option<(usize, PaneId)> {
    let model = board_model(app);
    let (_, slots) = slots_for(app, &model)?;
    let slot = slots
        .into_iter()
        .find(|slot| rect_contains(slot.rect, col, row))?;
    let card = model.columns[slot.col].get(slot.row)?;
    Some((card.ws_idx, card.pane_id))
}

/// The pane under a pointer position, if any (used for hover selection).
pub(crate) fn pane_at(app: &AppState, col: u16, row: u16) -> Option<PaneId> {
    card_at(app, col, row).map(|(_, pane)| pane)
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn card_age(app: &AppState, card: &BoardCard) -> Option<String> {
    let terminal_id = app.workspaces.get(card.ws_idx)?.terminal_id(card.pane_id)?;
    let at = app.terminals.get(terminal_id)?.last_agent_state_change_at?;
    Some(format_event_age(
        std::time::Instant::now().saturating_duration_since(at),
    ))
}

pub(super) fn render_board_overlay(app: &AppState, frame: &mut Frame) {
    let area = board_area(app);
    let Some(inner) = render_panel_shell(frame, area, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };

    render_title(app, frame, Rect::new(inner.x, inner.y, inner.width, 1));

    let model = board_model(app);
    let body = board_body(inner);
    let footer_y = inner.y + inner.height.saturating_sub(1);
    render_footer(app, frame, Rect::new(inner.x, footer_y, inner.width, 1));

    if body.height == 0 || body.width == 0 {
        return;
    }
    if model.is_empty() {
        frame.render_widget(
            Paragraph::new(" no agents running").style(Style::default().fg(app.palette.overlay0)),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }

    if is_narrow(app) {
        render_narrow(app, frame, &model, body);
    } else {
        render_wide(app, frame, &model, body);
    }
}

fn render_title(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let title = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);
    let line = Line::from(vec![
        Span::styled(" session board ", title),
        Span::styled("· what are my agents doing", dim),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_footer(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);
    let line = Line::from(vec![
        Span::styled(" enter", key),
        Span::styled(" focus  ", dim),
        Span::styled("hjkl/↑↓←→", key),
        Span::styled(" move  ", dim),
        Span::styled("esc/q", key),
        Span::styled(" close", dim),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_column_header(app: &AppState, frame: &mut Frame, area: Rect, col: usize, count: usize) {
    let p = &app.palette;
    let color = super::status::state_label_color(header_state(col), header_seen(col), p);
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", board_column_title(col)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {count}"), Style::default().fg(p.overlay0)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn header_state(col: usize) -> AgentState {
    match col {
        0 => AgentState::Blocked,
        1 => AgentState::Working,
        _ => AgentState::Idle,
    }
}

fn header_seen(col: usize) -> bool {
    // Idle column is seen; done column (2) is unseen.
    col != 2
}

fn render_wide(app: &AppState, frame: &mut Frame, model: &BoardModel, body: Rect) {
    let cols = column_rects(body);
    let slots = wide_slots(model, body);
    for (col, col_rect) in cols.iter().enumerate() {
        render_column_header(
            app,
            frame,
            Rect::new(col_rect.x, col_rect.y, col_rect.width, 1),
            col,
            model.columns[col].len(),
        );
    }
    for slot in slots {
        if let Some(card) = model.columns[slot.col].get(slot.row) {
            let selected = app.board.selected == Some(card.pane_id);
            render_card(app, frame, slot.rect, card, selected);
        }
    }
}

fn render_narrow(app: &AppState, frame: &mut Frame, model: &BoardModel, body: Rect) {
    let (slots, headers) = narrow_slots(model, body);
    for (y, col) in headers {
        render_column_header(
            app,
            frame,
            Rect::new(body.x, y, body.width, 1),
            col,
            model.columns[col].len(),
        );
    }
    for slot in slots {
        if let Some(card) = model.columns[slot.col].get(slot.row) {
            let selected = app.board.selected == Some(card.pane_id);
            render_card(app, frame, slot.rect, card, selected);
        }
    }
}

fn render_card(app: &AppState, frame: &mut Frame, rect: Rect, card: &BoardCard, selected: bool) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let p = &app.palette;
    let width = rect.width as usize;
    if selected {
        let buf = frame.buffer_mut();
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                buf[(x, y)].set_style(Style::default().bg(p.surface0));
            }
        }
    }
    let (dot, dot_style) = state_dot(card.state, card.seen, p);
    let marker = if selected { "▌" } else { " " };
    let marker_style = if selected {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    let agent_style = if selected {
        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.text)
    };
    let dim = Style::default().fg(p.overlay0);

    // Line 1: marker · dot · agent label · context% … location.
    let head = format!("{marker}{dot} ");
    let loc_width = card.location.chars().count();
    let percent = card
        .context_percent
        .map(|percent| format!("{percent}%"))
        .unwrap_or_default();
    let percent_reserved = if percent.is_empty() {
        0
    } else {
        percent.chars().count() + 1
    };
    let agent_budget = width
        .saturating_sub(head.chars().count())
        .saturating_sub(loc_width)
        .saturating_sub(percent_reserved)
        .saturating_sub(1);
    let mut line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(truncate_end(&card.agent_label, agent_budget), agent_style),
    ];
    if !percent.is_empty() {
        line1.push(Span::styled(format!(" {percent}"), dim));
    }
    if !card.location.is_empty() {
        line1.push(Span::styled(format!(" {}", card.location), dim));
    }
    frame.render_widget(
        Paragraph::new(Line::from(line1)),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );

    if rect.height < 2 {
        return;
    }
    // Line 2: workspace · branch … age.
    let age = card_age(app, card).unwrap_or_default();
    let mut meta = card.workspace_label.clone();
    if let Some(branch) = &card.branch {
        meta.push_str(" · ");
        meta.push_str(branch);
    }
    let meta_budget = width
        .saturating_sub(2)
        .saturating_sub(age.chars().count() + 1);
    let mut line2 = vec![
        Span::raw("  "),
        Span::styled(truncate_end(&meta, meta_budget), dim),
    ];
    if !age.is_empty() {
        line2.push(Span::styled(format!(" {age}"), dim));
    }
    frame.render_widget(
        Paragraph::new(Line::from(line2)),
        Rect::new(rect.x, rect.y + 1, rect.width, 1),
    );

    if rect.height < 3 {
        return;
    }
    // Line 3: one-line status message (or the state label as a fallback).
    let status = card
        .status
        .clone()
        .unwrap_or_else(|| state_label(card.state, card.seen).to_string());
    let status_style =
        Style::default().fg(super::status::state_label_color(card.state, card.seen, p));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("  {}", truncate_end(&status, width.saturating_sub(2))),
            status_style,
        ))),
        Rect::new(rect.x, rect.y + 2, rect.width, 1),
    );
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use super::*;
    use crate::detect::{Agent, AgentState};
    use crate::workspace::Workspace;

    fn set_state(
        state: &mut AppState,
        ws_idx: usize,
        tab_idx: usize,
        pane_id: PaneId,
        agent_state: AgentState,
        seen: bool,
    ) {
        let terminal_id = state.workspaces[ws_idx].tabs[tab_idx]
            .panes
            .get(&pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        state.workspaces[ws_idx].tabs[tab_idx]
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .seen = seen;
        let terminal = state.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = agent_state;
    }

    /// Priority-sequence a pane so within-column ordering is deterministic.
    fn set_seq(state: &mut AppState, ws_idx: usize, tab_idx: usize, pane_id: PaneId, seq: u64) {
        let terminal_id = state.workspaces[ws_idx].tabs[tab_idx]
            .panes
            .get(&pane_id)
            .unwrap()
            .attached_terminal_id
            .clone();
        state
            .terminals
            .get_mut(&terminal_id)
            .unwrap()
            .last_agent_state_change_seq = Some(seq);
    }

    fn board_state() -> (AppState, Vec<PaneId>) {
        // Workspace 0: two panes (blocked, working). Workspace 1: one pane (done).
        let mut first = Workspace::test_new("one");
        let first_root = first.tabs[0].root_pane;
        let first_second = first.test_split(Direction::Horizontal);
        first.tabs[0].layout.focus_pane(first_root);
        let second = Workspace::test_new("two");
        let second_root = second.tabs[0].root_pane;

        let mut state = AppState::test_new();
        state.workspaces = vec![first, second];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        set_state(&mut state, 0, 0, first_root, AgentState::Blocked, true);
        set_state(&mut state, 0, 0, first_second, AgentState::Working, true);
        set_state(&mut state, 1, 0, second_root, AgentState::Idle, false);
        (state, vec![first_root, first_second, second_root])
    }

    #[test]
    fn board_groups_panes_into_state_columns_blocked_first() {
        let (state, panes) = board_state();
        let model = board_model(&state);
        assert_eq!(model.columns[0].len(), 1, "blocked column");
        assert_eq!(model.columns[0][0].pane_id, panes[0]);
        assert_eq!(model.columns[1][0].pane_id, panes[2], "done column");
        assert_eq!(model.columns[2][0].pane_id, panes[1], "working column");
        assert!(model.columns[3].is_empty(), "idle column empty");
    }

    #[test]
    fn column_index_agrees_with_attention_priority() {
        // Blocked column (0) must be strictly more urgent than done (1),
        // done than working (2), working than idle (3): board column order is
        // the descending attention_priority order.
        let buckets = [
            (AgentState::Blocked, true),
            (AgentState::Idle, false),
            (AgentState::Working, true),
            (AgentState::Idle, true),
        ];
        for pair in buckets.windows(2) {
            let (sa, la) = pair[0];
            let (sb, lb) = pair[1];
            assert_eq!(board_column_index(sa, la) + 1, board_column_index(sb, lb));
            assert!(
                crate::workspace::attention_priority(sa, la)
                    > crate::workspace::attention_priority(sb, lb)
            );
        }
    }

    #[test]
    fn within_column_orders_by_attention_then_recency() {
        // Two done panes (idle+unseen) in the same column, ordered by seq desc.
        let mut ws = Workspace::test_new("one");
        let root = ws.tabs[0].root_pane;
        let second = ws.test_split(Direction::Horizontal);
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        set_state(&mut state, 0, 0, root, AgentState::Idle, false);
        set_state(&mut state, 0, 0, second, AgentState::Idle, false);
        set_seq(&mut state, 0, 0, root, 10);
        set_seq(&mut state, 0, 0, second, 20);

        let model = board_model(&state);
        assert_eq!(model.columns[1][0].pane_id, second, "higher seq first");
        assert_eq!(model.columns[1][1].pane_id, root);
    }

    #[test]
    fn unknown_state_agents_fold_into_idle_column() {
        let ws = Workspace::test_new("one");
        let root = ws.tabs[0].root_pane;
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        set_state(&mut state, 0, 0, root, AgentState::Unknown, true);
        let model = board_model(&state);
        assert_eq!(model.columns[3].len(), 1);
        assert_eq!(model.columns[3][0].pane_id, root);
    }

    #[test]
    fn wide_left_right_moves_across_columns_and_stops_at_edges() {
        let (state, panes) = board_state();
        // From blocked (col 0), right -> done (col 1) -> working (col 2).
        let sel = next_selection(&state, Some(panes[0]), BoardDir::Right, false);
        assert_eq!(sel, Some(panes[2]));
        let sel = next_selection(&state, sel, BoardDir::Right, false);
        assert_eq!(sel, Some(panes[1]));
        // No non-empty column to the right of working here: stays put (idle empty).
        let sel = next_selection(&state, sel, BoardDir::Right, false);
        assert_eq!(sel, Some(panes[1]));
        // Back left across the empty gap: working -> done -> blocked.
        let sel = next_selection(&state, Some(panes[1]), BoardDir::Left, false);
        assert_eq!(sel, Some(panes[2]));
        let sel = next_selection(&state, sel, BoardDir::Left, false);
        assert_eq!(sel, Some(panes[0]));
        // Leftmost column: nothing further left, stays.
        let sel = next_selection(&state, sel, BoardDir::Left, false);
        assert_eq!(sel, Some(panes[0]));
    }

    #[test]
    fn wide_up_down_wraps_within_column() {
        // One column with three cards; up/down wrap around.
        let mut ws = Workspace::test_new("one");
        let a = ws.tabs[0].root_pane;
        let b = ws.test_split(Direction::Horizontal);
        let c = ws.test_split(Direction::Vertical);
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        for (pane, seq) in [(a, 30u64), (b, 20), (c, 10)] {
            set_state(&mut state, 0, 0, pane, AgentState::Blocked, true);
            set_seq(&mut state, 0, 0, pane, seq);
        }
        // Column order by seq desc: a, b, c.
        let model = board_model(&state);
        let order: Vec<PaneId> = model.columns[0].iter().map(|c| c.pane_id).collect();
        assert_eq!(order, vec![a, b, c]);

        let down = next_selection(&state, Some(a), BoardDir::Down, false);
        assert_eq!(down, Some(b));
        // Wrap from bottom back to top.
        let wrap = next_selection(&state, Some(c), BoardDir::Down, false);
        assert_eq!(wrap, Some(a));
        // Wrap from top up to bottom.
        let up = next_selection(&state, Some(a), BoardDir::Up, false);
        assert_eq!(up, Some(c));
    }

    #[test]
    fn empty_or_invalid_selection_lands_on_first_card() {
        let (state, panes) = board_state();
        assert_eq!(
            next_selection(&state, None, BoardDir::Down, false),
            Some(panes[0])
        );
        // A pane not on the board is treated as no selection.
        assert_eq!(
            next_selection(&state, Some(PaneId::from_raw(9999)), BoardDir::Up, false),
            Some(panes[0])
        );
    }

    #[test]
    fn narrow_up_down_traverses_flattened_groups_with_wraparound() {
        let (state, panes) = board_state();
        // Flattened order: blocked, done, working.
        let sel = next_selection(&state, Some(panes[0]), BoardDir::Down, true);
        assert_eq!(sel, Some(panes[2]));
        let sel = next_selection(&state, sel, BoardDir::Down, true);
        assert_eq!(sel, Some(panes[1]));
        // Wrap back to the first (blocked) card.
        let sel = next_selection(&state, sel, BoardDir::Down, true);
        assert_eq!(sel, Some(panes[0]));
        // Left/right are no-ops in narrow mode.
        assert_eq!(
            next_selection(&state, Some(panes[1]), BoardDir::Left, true),
            Some(panes[1])
        );
    }

    #[test]
    fn enter_target_resolves_selected_pane_to_its_workspace() {
        let (state, panes) = board_state();
        assert_eq!(enter_target(&state, Some(panes[0])), Some((0, panes[0])));
        assert_eq!(enter_target(&state, Some(panes[2])), Some((1, panes[2])));
        assert_eq!(enter_target(&state, None), None);
        assert_eq!(enter_target(&state, Some(PaneId::from_raw(9999))), None);
    }

    #[test]
    fn initial_selection_prefers_focused_pane_then_first_blocked() {
        let (mut state, panes) = board_state();
        // Focused pane is first_root (blocked) -> selected.
        assert_eq!(initial_selection(&state), Some(panes[0]));
        // Focus the working pane; it's on the board, so it is preferred.
        state.workspaces[0].tabs[0].layout.focus_pane(panes[1]);
        assert_eq!(initial_selection(&state), Some(panes[1]));
    }
}
