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

use super::glyphs;
use super::sidebar::{agent_panel_entries, format_event_age};
use super::status::{agent_icon_for, state_label};
use super::text::{display_width, truncate_end, truncate_start};
use super::widgets::render_panel_shell;
use crate::app::state::{AppState, BoardView, Palette, TaskQueueRow};
use crate::detect::AgentState;
use crate::layout::PaneId;

/// Number of state columns (blocked, done, working, idle).
pub(crate) const BOARD_COLUMNS: usize = 4;

/// Visible rows per card: agent line, workspace/branch/age line, status line,
/// activity line, then repo path + context gauge.
const CARD_ROWS: u16 = 5;
/// Card slot height including a one-row gap between cards.
const CARD_STRIDE: u16 = CARD_ROWS + 1;

/// The card's left gutter: selection marker, state glyph, and the space after
/// it. Every line below the first indents to here, so the glyph hangs in the
/// margin and the rest of the card is one text column. They used to indent by
/// two, which aligned them with the gap between the glyph and the name —
/// under nothing at all.
const CARD_INDENT: usize = 3;

/// Columns of air at a card's right edge.
///
/// The lanes are cut from the panel's full inner width with no gap, so the
/// rightmost card's text sat flush against the border while every other lane
/// had one — which reads as a clipped card rather than a column.
const CARD_RIGHT_MARGIN: usize = 1;

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
    /// The shortest name no other card on the board answers to.
    ///
    /// Filled by [`assign_distinct_names`] once the whole board is known, so it
    /// is `agent_label` plus only as much placement as it takes to tell this
    /// agent apart from the others. See that function for why this is not just
    /// `agent_label`.
    pub display_name: String,
    pub workspace_label: String,
    /// Tab/pane location tag, e.g. `t2·p1` (multi-tab) or `p3`.
    pub location: String,
    pub branch: Option<String>,
    pub status: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub manual_state: Option<crate::api::schema::PaneManualState>,
    pub context_percent: Option<u8>,
    /// Where the agent is working, contracted for display (`~/vault/dev/shep`).
    pub cwd: Option<String>,
    /// The agent's own name for itself — a model, usually, when it reports one.
    pub model: Option<String>,
    /// Last line of real screen content; "what is it saying right now".
    pub activity: Option<String>,
    /// The last few of them, in reading order, for a surface with the room.
    pub activity_lines: Vec<String>,
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

/// `/Users/alex/vault/dev/shep` -> `~/vault/dev/shep`. Board cards are narrow
/// and the home prefix is the same on every one of them.
fn contract_home(path: &std::path::Path) -> String {
    let display = path.display().to_string();
    let Some(home) = std::env::var_os("HOME") else {
        return display;
    };
    let home = home.to_string_lossy();
    if home.is_empty() {
        return display;
    }
    match display.strip_prefix(home.as_ref()) {
        Some("") => "~".to_string(),
        Some(rest) if rest.starts_with('/') => format!("~{rest}"),
        _ => display,
    }
}

/// A tiny inline gauge for the context window: `███▍░░ 62%`.
///
/// Rendered as a bar because the number alone doesn't read at a glance — the
/// thing worth seeing across eight cards is *which agent is nearly full*.
///
/// Drawn in eighths. Six whole cells give seven states for a hundred and one
/// percentages, so 60% and 74% were the same picture; eighths give the same
/// six columns forty-nine, which is the difference between a gauge that
/// reports and one that rounds.
///
/// The number is right-aligned in its own three columns. The gauge is pinned to
/// the card's right edge, so an unpadded `100%` would drag the bar one column
/// left of every other card's — a shifted bar in a column of bars reads as a
/// different measurement.
/// The bar is drawn cell by cell rather than as one string, and the boundary
/// cell carries the fill as foreground over the track as background. As plain
/// text the partial cell showed the panel through it and the bar read as
/// broken — a gap between the fill and the track — which every cell-exact
/// snapshot passed, because every cell was right. It took looking at pixels.
const GAUGE_WIDTH: usize = 6;

fn context_gauge_spans<'a>(percent: u8, p: &Palette) -> Vec<Span<'a>> {
    let percent = percent.min(100);
    // Near-full context is the thing worth noticing, so it warms up. Both the
    // card and the detail screen come through here: they used to each carry
    // this ladder and disagreed about the cold end.
    let color = match percent {
        85..=u8::MAX => p.red,
        60..=84 => p.yellow,
        _ => p.overlay0,
    };
    // Any nonzero reading lights something, so "barely used" still outranks
    // "unknown" — which is a different claim and draws nothing.
    let smallest = 1.0 / (GAUGE_WIDTH * 8) as f32;
    let fraction = (f32::from(percent) / 100.0).max(if percent > 0 { smallest } else { 0.0 });
    let (full, remainder, empty) = glyphs::bar_parts(fraction, GAUGE_WIDTH);
    // The whole bar sits on `surface1` — a recessed channel — and the fill is
    // drawn into it. The boundary cell's unfilled part is that same channel,
    // so the fill's edge is a hard line inside one cell rather than a hole.
    let track = Style::default().bg(p.surface1);
    let mut spans = Vec::new();
    if full > 0 {
        spans.push(Span::styled(
            glyphs::EIGHTHS[8].repeat(full),
            track.fg(color),
        ));
    }
    if remainder > 0 {
        spans.push(Span::styled(
            glyphs::EIGHTHS[remainder].to_string(),
            track.fg(color),
        ));
    }
    if empty > 0 {
        // EIGHTHS[0] is a space: nothing but the channel.
        spans.push(Span::styled(glyphs::EIGHTHS[0].repeat(empty), track));
    }
    spans.push(Span::styled(
        format!(" {percent:>3}%"),
        Style::default().fg(color),
    ));
    spans
}

/// The gauge's text, for measuring it and for tests.
fn spans_text(spans: &[Span<'_>]) -> String {
    spans.iter().map(|s| s.content.as_ref()).collect()
}

#[cfg(test)]
fn context_gauge(percent: u8) -> String {
    spans_text(&context_gauge_spans(percent, &Palette::shep()))
}

/// Human-readable "where is this agent" tag.
///
/// A named tab is the whole point of naming it, so the name wins over the
/// number: `docs·p2` rather than `t3·p2`. The pane number is only worth the
/// width when the tab actually holds more than one pane, and the tab part is
/// only worth it when the workspace has more than one tab or the tab was
/// deliberately named.
fn location_label(
    app: &AppState,
    ws_idx: usize,
    tab_idx: usize,
    pane_number: Option<usize>,
    multi_tab: bool,
) -> String {
    let Some(ws) = app.workspaces.get(ws_idx) else {
        return String::new();
    };
    let named = ws
        .tabs
        .get(tab_idx)
        .and_then(|tab| tab.custom_name.as_deref());
    // A tab is one agent, so its number says nothing the agent name does
    // not; only a deliberately named tab earns the width.
    let _ = multi_tab;
    let tab_part = named.map(str::to_string);
    let multi_pane = ws
        .tabs
        .get(tab_idx)
        .map(|tab| tab.panes.len() > 1)
        .unwrap_or(false);
    let pane_part = pane_number.filter(|_| multi_pane).map(|n| format!("p{n}"));
    match (tab_part, pane_part) {
        (Some(tab), Some(pane)) => format!("{tab}{}{pane}", glyphs::SEP),
        (Some(tab), None) => tab,
        (None, Some(pane)) => pane,
        (None, None) => String::new(),
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
        let multi_tab = ws.map(|ws| ws.tabs.len() > 1).unwrap_or(false);
        let location = location_label(app, entry.ws_idx, entry.tab_idx, pane_number, multi_tab);
        let terminal = ws
            .and_then(|ws| ws.terminal_id(entry.pane_id))
            .and_then(|id| app.terminals.get(id));
        // `display_agent` is what the agent calls itself when it reports one
        // (claude reports its model here); `agent_label` is shep's own name for
        // it, already on line 1, so don't repeat it.
        let agent_model = terminal
            .and_then(|terminal| terminal.effective_display_agent())
            .filter(|model| Some(model.as_str()) != entry.agent_label.as_deref());
        let cwd = terminal.map(|terminal| contract_home(&terminal.cwd));
        let activity_lines = terminal
            .map(|terminal| terminal.activity_lines.clone())
            .unwrap_or_default();
        let activity = activity_lines.last().cloned();
        let agent_label = entry.agent_label.unwrap_or_else(|| "agent".to_string());
        model.columns[col].push(BoardCard {
            ws_idx: entry.ws_idx,
            pane_id: entry.pane_id,
            display_name: agent_label.clone(),
            agent_label,
            workspace_label: entry.primary_label,
            location,
            branch,
            status: entry.custom_status,
            state: entry.state,
            seen: entry.seen,
            manual_state: entry.manual_state,
            context_percent: entry.context_percent,
            cwd,
            model: agent_model,
            activity,
            activity_lines,
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
    assign_distinct_names(&mut model);
    model
}

/// Give every card the shortest name no other card answers to.
///
/// Five claude sessions in the same repo are all "claude" — true, and useless
/// for telling them apart. Spending detail everywhere is no better: a board
/// where every card reads `claude · shep · master · docs` has the same problem
/// in a longer form. So detail is spent only where it buys a distinction. An
/// agent that is already the only "claude" stays "claude", and only the ones
/// that collide grow a workspace, then a branch, then a location.
///
/// This lives here, on the shared board model, rather than in any one client:
/// the desktop board and the companion both render whatever this produces, so
/// the two cannot drift into calling the same agent different things.
fn assign_distinct_names(model: &mut BoardModel) {
    let candidates: Vec<(PaneId, Vec<String>)> = model
        .flattened()
        .iter()
        .map(|card| (card.pane_id, name_candidates(card)))
        .collect();
    let depth = candidates
        .iter()
        .map(|(_, names)| names.len())
        .max()
        .unwrap_or(0);

    let mut resolved: std::collections::HashMap<PaneId, String> = std::collections::HashMap::new();
    for level in 0..depth {
        // At this level, every still-ambiguous card proposes its name; the ones
        // whose proposal is unique keep it and stop growing.
        let mut proposals: Vec<(PaneId, String)> = Vec::new();
        for (pane_id, names) in &candidates {
            if resolved.contains_key(pane_id) {
                continue;
            }
            let name = names.get(level).or_else(|| names.last());
            if let Some(name) = name {
                proposals.push((*pane_id, name.clone()));
            }
        }
        for (pane_id, name) in &proposals {
            let unique = proposals.iter().filter(|(_, other)| other == name).count() == 1;
            if unique {
                resolved.insert(*pane_id, name.clone());
            }
        }
    }

    for (pane_id, names) in &candidates {
        if !resolved.contains_key(pane_id) {
            // Nothing about placement separated these two. The pane id always
            // does, and is the last resort precisely because `w2:p1` is the
            // unreadable thing this exists to avoid.
            let fallback = names.last().cloned().unwrap_or_default();
            resolved.insert(
                *pane_id,
                format!("{fallback} {} {}", glyphs::SEP, pane_id.raw()),
            );
        }
    }

    for cards in &mut model.columns {
        for card in cards.iter_mut() {
            if let Some(name) = resolved.get(&card.pane_id) {
                card.display_name = name.clone();
            }
        }
    }
}

/// Increasingly specific names for one card, shortest first.
fn name_candidates(card: &BoardCard) -> Vec<String> {
    let mut names = vec![card.agent_label.clone()];
    let mut accumulated = card.agent_label.clone();
    let extras = [
        Some(card.workspace_label.clone()).filter(|l| !l.is_empty()),
        card.branch.clone(),
        Some(card.location.clone()).filter(|l| !l.is_empty()),
    ];
    for extra in extras.into_iter().flatten() {
        accumulated = format!("{accumulated} {} {extra}", glyphs::SEP);
        names.push(accumulated.clone());
    }
    names
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

/// Narrowest lane that can still hold a card.
///
/// Below this the gutter, the agent name and the location cannot coexist and
/// the name — the one thing a card exists to tell you — is the part that gives
/// way: on a standard 80-column terminal the four lanes were 20 columns each
/// and every card read `◉ claude · … ⇥2 p1`.
const MIN_CARD_WIDTH: u16 = 24;

/// Whether the board should collapse to a stacked single-column layout.
///
/// Two conditions, because the board is four columns wide where the rest of the
/// app is one: it stacks when the terminal is phone-narrow *or* when splitting
/// it four ways would leave lanes too thin to read. Using only the mobile
/// threshold meant a four-column board on an 80-column terminal, which is
/// exactly where stacking helps most — the same cards, at full width, one at a
/// time.
pub(crate) fn is_narrow(app: &AppState) -> bool {
    let area = board_area(app);
    if let Some(inner) = inner_area(area) {
        if inner.width / (BOARD_COLUMNS as u16) < MIN_CARD_WIDTH {
            return true;
        }
    }
    super::mobile::is_mobile_width(area, app.mobile_width_threshold)
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

/// Rows the dashboard strip occupies below the title: session pulse, then
/// host/system vitals.
const DASHBOARD_ROWS: u16 = 2;

/// The card region inside the panel, reserving the title row, the dashboard
/// strip, and the footer row. On a short terminal the dashboard yields first —
/// the cards are the point of the screen.
fn board_body(inner: Rect) -> Rect {
    if inner.height <= 2 {
        return inner;
    }
    let reserved = 2 + dashboard_rows(inner);
    if inner.height <= reserved {
        return Rect::new(
            inner.x,
            inner.y + 1,
            inner.width,
            inner.height.saturating_sub(2),
        );
    }
    Rect::new(
        inner.x,
        inner.y + 1 + dashboard_rows(inner),
        inner.width,
        inner.height.saturating_sub(reserved),
    )
}

/// How many dashboard rows fit. Below this the strip is dropped entirely
/// rather than half-rendered.
/// Body rect for a detail screen: everything between the title row and the
/// footer row, indented one column so text does not sit on the panel border.
fn detail_body(inner: Rect) -> Rect {
    if inner.height <= 2 || inner.width <= 2 {
        return Rect::new(inner.x, inner.y, 0, 0);
    }
    Rect::new(
        inner.x + 1,
        inner.y + 2,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(3),
    )
}

fn dashboard_rows(inner: Rect) -> u16 {
    if inner.height >= 12 {
        DASHBOARD_ROWS
    } else {
        0
    }
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

/// The session-wide numbers behind the dashboard strip. Pure arithmetic over
/// `AppState` plus the sampled host facts, so it is unit-testable.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct BoardSummary {
    pub blocked: usize,
    pub done: usize,
    pub working: usize,
    pub idle: usize,
    /// Agents that want the user: blocked or finished-and-unseen.
    pub attention: usize,
    pub workspaces: usize,
    pub tabs: usize,
    pub panes: usize,
    /// Prompts queued across all panes, waiting for their agent to go idle.
    pub queued_input: usize,
}

impl BoardSummary {
    pub(crate) fn agents(&self) -> usize {
        self.blocked + self.done + self.working + self.idle
    }
}

pub(crate) fn board_summary(app: &AppState, model: &BoardModel) -> BoardSummary {
    let counts = [
        model.columns[0].len(),
        model.columns[1].len(),
        model.columns[2].len(),
        model.columns[3].len(),
    ];
    BoardSummary {
        blocked: counts[0],
        done: counts[1],
        working: counts[2],
        idle: counts[3],
        // Blocked agents are stuck and done-but-unseen agents are finished
        // without anyone looking: both are waiting on the user, and together
        // they are the only number on this strip worth reacting to.
        attention: counts[0] + counts[1],
        workspaces: app.workspaces.len(),
        tabs: app.workspaces.iter().map(|ws| ws.tabs.len()).sum(),
        panes: app
            .workspaces
            .iter()
            .flat_map(|ws| ws.tabs.iter())
            .map(|tab| tab.panes.len())
            .sum(),
        queued_input: app.queued_pane_input.values().map(Vec::len).sum(),
    }
}

/// `1234567890` -> `1.1G`. Bytes on a status strip are only ever read as a
/// magnitude.
fn human_bytes(bytes: u64) -> String {
    const UNITS: [(u64, &str); 3] = [(1024 * 1024 * 1024, "G"), (1024 * 1024, "M"), (1024, "K")];
    for (scale, suffix) in UNITS {
        if bytes >= scale {
            return format!("{:.1}{suffix}", bytes as f64 / scale as f64);
        }
    }
    format!("{bytes}B")
}

/// Two-row dashboard: the session pulse, then the host it runs on.
/// Assemble a strip of facts into one line, dropping whole facts that will not
/// fit.
///
/// A `Paragraph` clipped at the terminal's edge leaves debris. On an 80-column
/// board this strip ended `·  3` — the head of "3 ws · 3 tabs · 5 panes",
/// reading as a count of something unnamed — and at 120 it ended on a dangling
/// separator promising a fact that was not there.
///
/// Facts are given in the order a glance wants them, and this stops at the
/// first one that does not fit rather than skipping ahead to a shorter one:
/// a strip that is a prefix of a known order can be read, and a gap-toothed
/// subset of it cannot.
fn fit_strip<'a>(facts: Vec<Vec<Span<'a>>>, sep: &Span<'a>, width: usize) -> Vec<Span<'a>> {
    let span_width =
        |spans: &[Span<'a>]| -> usize { spans.iter().map(|s| display_width(&s.content)).sum() };
    let sep_width = display_width(&sep.content);
    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for fact in facts {
        let lead = if out.is_empty() { 0 } else { sep_width };
        if used + lead + span_width(&fact) > width {
            break;
        }
        used += lead + span_width(&fact);
        if lead > 0 {
            out.push(sep.clone());
        }
        out.extend(fact);
    }
    out
}

fn render_dashboard(app: &AppState, frame: &mut Frame, area: Rect, summary: &BoardSummary) {
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    let value = Style::default().fg(p.text);
    let sep = Span::styled(glyphs::SEP_WIDE, Style::default().fg(p.surface0));
    let width = area.width as usize;

    // Row 1 — agents and session shape, most worth knowing first.
    let mut lead = vec![
        Span::styled(" agents ", dim),
        Span::styled(summary.agents().to_string(), value),
    ];
    if summary.attention > 0 {
        // The one number on this strip that is a call to action, so it rides
        // with the head count instead of being a fact that can fall off.
        lead.push(Span::styled(
            format!("  {} need you", summary.attention),
            Style::default().fg(p.red).add_modifier(Modifier::BOLD),
        ));
    }
    let mut facts = vec![lead];
    for (label, count, state, seen) in [
        ("blocked", summary.blocked, AgentState::Blocked, true),
        ("done", summary.done, AgentState::Idle, false),
        ("working", summary.working, AgentState::Working, true),
        ("idle", summary.idle, AgentState::Idle, true),
    ] {
        facts.push(vec![
            Span::styled(
                format!("{label} "),
                Style::default().fg(super::status::state_label_color(state, seen, p)),
            ),
            Span::styled(count.to_string(), value),
        ]);
    }
    if summary.queued_input > 0 {
        facts.push(vec![Span::styled(
            format!("{}{} queued", glyphs::QUEUED, summary.queued_input),
            Style::default().fg(p.teal),
        )]);
    }
    if let Some(pending) = app.dashboard_sample.pending_tasks.filter(|n| *n > 0) {
        facts.push(vec![Span::styled(format!("{pending} tasks"), value)]);
    }
    // Session shape last: it describes the furniture, not the work.
    facts.push(vec![Span::styled(
        format!(
            "{} ws {s} {} tabs {s} {} panes",
            summary.workspaces,
            summary.tabs,
            summary.panes,
            s = glyphs::SEP
        ),
        dim,
    )]);
    frame.render_widget(
        Paragraph::new(Line::from(fit_strip(facts, &sep, width))),
        Rect::new(area.x, area.y, area.width, 1),
    );

    if area.height < 2 {
        return;
    }

    // Row 2 — the host. Unsampled or unreadable values print as an em dash
    // rather than a confident zero.
    let vitals = app.dashboard_sample.vitals;
    let mut rows = vec![vec![
        Span::styled(" shep ", dim),
        Span::styled(env!("CARGO_PKG_VERSION"), value),
    ]];
    let mut load = vec![Span::styled("load ", dim)];
    match (vitals.load_percent, vitals.cores) {
        (Some(percent), Some(cores)) => {
            let color = match percent {
                100..=u16::MAX => p.red,
                70..=99 => p.yellow,
                _ => p.text,
            };
            load.push(Span::styled(
                format!("{percent}%"),
                Style::default().fg(color),
            ));
            load.push(Span::styled(format!(" of {cores} cores"), dim));
        }
        _ => load.push(Span::styled(glyphs::DASH, dim)),
    }
    rows.push(load);
    let mut mem = vec![Span::styled("mem ", dim)];
    match vitals.memory_percent {
        Some(percent) => {
            let color = match percent {
                90..=u8::MAX => p.red,
                75..=89 => p.yellow,
                _ => p.text,
            };
            mem.push(Span::styled(
                format!("{percent}%"),
                Style::default().fg(color),
            ));
            if let (Some(used), Some(total)) = (vitals.memory_used_bytes, vitals.memory_total_bytes)
            {
                mem.push(Span::styled(
                    format!(" {} of {}", human_bytes(used), human_bytes(total)),
                    dim,
                ));
            }
        }
        None => mem.push(Span::styled(glyphs::DASH, dim)),
    }
    rows.push(mem);
    frame.render_widget(
        Paragraph::new(Line::from(fit_strip(rows, &sep, width))),
        Rect::new(area.x, area.y + 1, area.width, 1),
    );
}

pub(super) fn render_board_overlay(
    app: &AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    frame: &mut Frame,
) {
    let area = board_area(app);
    let Some(inner) = render_panel_shell(frame, area, app.palette.accent, app.palette.panel_bg)
    else {
        return;
    };

    render_title(app, frame, Rect::new(inner.x, inner.y, inner.width, 1));

    let model = board_model(app);
    let footer_y = inner.y + inner.height.saturating_sub(1);
    render_footer(app, frame, Rect::new(inner.x, footer_y, inner.width, 1));

    // The detail screens replace the dashboard and columns entirely; they keep
    // only the panel shell, title, and footer so the board stays recognisable.
    match app.board.view {
        BoardView::Agent => {
            let body = detail_body(inner);
            render_agent_detail(app, terminal_runtimes, frame, &model, body);
            return;
        }
        BoardView::Tasks => {
            let body = detail_body(inner);
            render_task_queue(app, frame, body);
            return;
        }
        BoardView::Columns => {}
    }

    let body = board_body(inner);

    let rows = dashboard_rows(inner);
    if rows > 0 {
        let summary = board_summary(app, &model);
        render_dashboard(
            app,
            frame,
            Rect::new(inner.x, inner.y + 1, inner.width, rows),
            &summary,
        );
    }

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
    // On a detail screen the title doubles as the breadcrumb back to the board.
    let line = match app.board.view {
        BoardView::Columns => Line::from(vec![
            Span::styled(" session board ", title),
            Span::styled(format!("{} what are my agents doing", glyphs::SEP), dim),
        ]),
        BoardView::Agent => Line::from(vec![
            Span::styled(" session board ", dim),
            Span::styled("/ ", dim),
            Span::styled("agent", title),
        ]),
        BoardView::Tasks => Line::from(vec![
            Span::styled(" session board ", dim),
            Span::styled("/ ", dim),
            Span::styled("task queue", title),
        ]),
    };
    frame.render_widget(Paragraph::new(line), area);
}

fn render_footer(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);
    // Each screen advertises only the keys that do something on it, and every
    // screen says what esc does — from a detail screen that is "back", not
    // "close", so the board is always one step away.
    let hints: &[(&str, &str)] = match app.board.view {
        BoardView::Columns => &[
            ("enter", " focus  "),
            ("i", " inspect  "),
            (glyphs::KEYS_ARROWS, " move  "),
            ("t", " tasks  "),
            ("esc/q", " close"),
        ],
        BoardView::Agent => &[
            ("enter", " attach  "),
            ("t", " tasks  "),
            ("esc/q", " back to board"),
        ],
        BoardView::Tasks => &[
            (glyphs::KEYS_VERTICAL, " move  "),
            ("esc/q/t", " back to board"),
        ],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in hints {
        spans.push(Span::styled(*k, key));
        spans.push(Span::styled(*label, dim));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
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
    let (dot, dot_style) = agent_icon_for(
        card.state,
        card.seen,
        card.manual_state.as_ref(),
        app.spinner_tick,
        p,
    );
    let marker = if selected { glyphs::MARKER } else { " " };
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

    // Every line draws inside this, leaving CARD_RIGHT_MARGIN of air.
    let content = width.saturating_sub(CARD_RIGHT_MARGIN);
    let indent = " ".repeat(CARD_INDENT);
    // Spaces that push a trailing fact out to the content edge. At least one,
    // so a right-aligned fact never touches the text it follows.
    let pin = |used: usize, trailing: usize| {
        " ".repeat(content.saturating_sub(used).saturating_sub(trailing).max(1))
    };

    // Line 1: marker · glyph · agent · model · queued … location.
    //
    // The location is pinned right rather than trailing the name, so the lane
    // reads as a column of places instead of a ragged edge — and so this line
    // is the same line the phone's card draws.
    let model = card.model.clone().unwrap_or_default();
    let model_reserved = if model.is_empty() {
        0
    } else {
        display_width(&model) + 1
    };
    // Queued-input badge (M5 tab-to-queue): prompts waiting for idle.
    let queued = app.queued_input_count_for_pane(card.pane_id);
    let queued_label = (queued > 0).then(|| format!("{}{queued}", glyphs::QUEUED));
    let queued_reserved = queued_label
        .as_ref()
        .map(|label| display_width(label) + 1)
        .unwrap_or(0);
    let loc_width = display_width(&card.location);
    let loc_reserved = if loc_width == 0 { 0 } else { loc_width + 1 };
    let agent_budget = content
        .saturating_sub(CARD_INDENT)
        .saturating_sub(loc_reserved)
        .saturating_sub(model_reserved)
        .saturating_sub(queued_reserved);
    let name = truncate_end(&card.display_name, agent_budget);
    let mut used = CARD_INDENT + display_width(&name) + model_reserved + queued_reserved;
    let mut line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(name, agent_style),
    ];
    if !model.is_empty() {
        line1.push(Span::styled(
            format!(" {model}"),
            Style::default().fg(p.teal),
        ));
    }
    if let Some(queued_label) = &queued_label {
        line1.push(Span::styled(
            format!(" {queued_label}"),
            Style::default().fg(p.teal),
        ));
    }
    if loc_width > 0 {
        line1.push(Span::raw(pin(used, loc_width)));
        line1.push(Span::styled(card.location.clone(), dim));
    }
    frame.render_widget(
        Paragraph::new(Line::from(line1)),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );

    if rect.height < 2 {
        return;
    }
    // Line 2: workspace · branch … age, pinned right.
    let age = card_age(app, card).unwrap_or_default();
    let age_width = display_width(&age);
    let mut meta = card.workspace_label.clone();
    if let Some(branch) = &card.branch {
        meta.push_str(glyphs::SEP_SPACED);
        meta.push_str(branch);
    }
    let meta_budget = content
        .saturating_sub(CARD_INDENT)
        .saturating_sub(if age_width == 0 { 0 } else { age_width + 1 });
    let meta = truncate_end(&meta, meta_budget);
    used = CARD_INDENT + display_width(&meta);
    let mut line2 = vec![Span::raw(indent.clone()), Span::styled(meta, dim)];
    if age_width > 0 {
        line2.push(Span::raw(pin(used, age_width)));
        line2.push(Span::styled(age, dim));
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
            format!(
                "{indent}{}",
                truncate_end(&status, content.saturating_sub(CARD_INDENT))
            ),
            status_style,
        ))),
        Rect::new(rect.x, rect.y + 2, rect.width, 1),
    );

    if rect.height < 4 {
        return;
    }
    // Line 4: what the agent's screen is actually saying right now. Italic and
    // dim because it is a hint, not shep's own reporting.
    if let Some(activity) = &card.activity {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "{indent}{}",
                    truncate_end(activity, content.saturating_sub(CARD_INDENT))
                ),
                Style::default()
                    .fg(p.overlay0)
                    .add_modifier(Modifier::ITALIC),
            ))),
            Rect::new(rect.x, rect.y + 3, rect.width, 1),
        );
    }

    if rect.height < 5 {
        return;
    }
    // Line 5: where it is working … context gauge, pinned right so the gauges
    // stack into a column that can be read down for the one about to fill.
    let gauge = card
        .context_percent
        .map(|percent| context_gauge_spans(percent, p))
        .unwrap_or_default();
    let gauge_width = display_width(&spans_text(&gauge));
    let cwd = card.cwd.clone().unwrap_or_default();
    let cwd_budget = content
        .saturating_sub(CARD_INDENT)
        .saturating_sub(if gauge_width == 0 { 0 } else { gauge_width + 1 });
    let cwd = truncate_start(&cwd, cwd_budget);
    used = CARD_INDENT + display_width(&cwd);
    let mut line5 = vec![Span::raw(indent), Span::styled(cwd, dim)];
    if gauge_width > 0 {
        line5.push(Span::raw(pin(used, gauge_width)));
        line5.extend(gauge);
    }
    frame.render_widget(
        Paragraph::new(Line::from(line5)),
        Rect::new(rect.x, rect.y + 4, rect.width, 1),
    );
}

// ---------------------------------------------------------------------------
// Detail screens
// ---------------------------------------------------------------------------

/// Width of the label column in the detail key/value block.
const DETAIL_KEY_WIDTH: usize = 13;

/// The selected card, or the first one on the board when the selection no
/// longer resolves (the agent it pointed at can exit while the board is open).
pub(crate) fn detail_card<'a>(app: &AppState, model: &'a BoardModel) -> Option<&'a BoardCard> {
    let selected = app.board.selected;
    let cards = model.flattened();
    cards
        .iter()
        .find(|card| Some(card.pane_id) == selected)
        .or_else(|| cards.first())
        .copied()
}

/// Everything shep knows about one agent, plus a window onto what its screen
/// is actually showing — the board's "tell me more" without attaching.
fn render_agent_detail(
    app: &AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    frame: &mut Frame,
    model: &BoardModel,
    body: Rect,
) {
    if body.width == 0 || body.height == 0 {
        return;
    }
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    let Some(card) = detail_card(app, model) else {
        frame.render_widget(
            Paragraph::new("no agent selected").style(dim),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    };
    let width = body.width as usize;
    let mut y = body.y;
    let bottom = body.y + body.height;
    let row = |frame: &mut Frame, y: &mut u16, line: Line<'static>| {
        if *y < bottom {
            frame.render_widget(Paragraph::new(line), Rect::new(body.x, *y, body.width, 1));
            *y += 1;
        }
    };

    // Heading: the same dot/name/model identity the card leads with, at rest.
    let (dot, dot_style) = agent_icon_for(
        card.state,
        card.seen,
        card.manual_state.as_ref(),
        app.spinner_tick,
        p,
    );
    let mut heading = vec![
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(
            card.display_name.clone(),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(model_name) = &card.model {
        heading.push(Span::styled(format!("  {model_name}"), dim));
    }
    row(frame, &mut y, Line::from(heading));

    let mut sub = card.workspace_label.clone();
    if let Some(branch) = &card.branch {
        sub.push_str(glyphs::SEP_SPACED);
        sub.push_str(branch);
    }
    if !card.location.is_empty() {
        sub.push_str(glyphs::SEP_SPACED);
        sub.push_str(&card.location);
    }
    if let Some(age) = card_age(app, card) {
        sub.push_str(&format!(" {} last activity ", glyphs::SEP));
        sub.push_str(&age);
    }
    row(
        frame,
        &mut y,
        Line::from(Span::styled(truncate_end(&sub, width), dim)),
    );
    row(frame, &mut y, Line::from(""));

    // Key/value block. Each value keeps the colour it has on the card so the
    // two screens read as the same information, not two reports of it.
    let queued = app.queued_input_count_for_pane(card.pane_id);
    let mut state_value = state_label(card.state, card.seen).to_string();
    if queued > 0 {
        state_value.push_str(&format!(
            " {} {}{queued} queued",
            glyphs::SEP,
            glyphs::QUEUED
        ));
    }
    let value_width = width.saturating_sub(DETAIL_KEY_WIDTH);
    let kv = |frame: &mut Frame, y: &mut u16, key: &str, value: String, style: Style| {
        row(
            frame,
            y,
            Line::from(vec![
                Span::styled(format!("{key:<DETAIL_KEY_WIDTH$}"), dim),
                Span::styled(truncate_end(&value, value_width), style),
            ]),
        );
    };
    kv(
        frame,
        &mut y,
        "state",
        state_value,
        Style::default().fg(super::status::state_label_color(card.state, card.seen, p)),
    );
    if let Some(status) = &card.status {
        kv(
            frame,
            &mut y,
            "status",
            status.clone(),
            Style::default().fg(p.text),
        );
    }
    if let Some(activity) = &card.activity {
        kv(
            frame,
            &mut y,
            "activity",
            activity.clone(),
            Style::default()
                .fg(p.overlay1)
                .add_modifier(Modifier::ITALIC),
        );
    }
    if let Some(cwd) = &card.cwd {
        kv(
            frame,
            &mut y,
            "working dir",
            cwd.clone(),
            Style::default().fg(p.text),
        );
    }
    if let Some(percent) = card.context_percent {
        // Straight through `context_gauge_spans` so the boundary cell keeps its
        // track background here too, rather than being flattened into one
        // colour by `kv`.
        let mut line = vec![Span::styled(
            format!("{:<DETAIL_KEY_WIDTH$}", "context"),
            dim,
        )];
        line.extend(context_gauge_spans(percent, p));
        row(frame, &mut y, Line::from(line));
    }
    row(frame, &mut y, Line::from(""));

    // The live screen. Read from the pane runtime the same way the navigator
    // reads runtime facts; absent (headless, or a pane with no runtime) it
    // simply says so rather than drawing an empty frame.
    if y >= bottom {
        return;
    }
    let screen_rect = Rect::new(body.x, y, body.width, bottom - y);
    render_agent_screen(app, terminal_runtimes, frame, card, screen_rect);
}

/// The bordered "live screen" excerpt at the foot of the agent detail.
fn render_agent_screen(
    app: &AppState,
    terminal_runtimes: &crate::terminal::TerminalRuntimeRegistry,
    frame: &mut Frame,
    card: &BoardCard,
    rect: Rect,
) {
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled("live screen", dim))),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );
    if rect.height < 3 {
        return;
    }
    let inner = Rect::new(
        rect.x,
        rect.y + 1,
        rect.width,
        rect.height.saturating_sub(1),
    );
    let visible = inner.height as usize;
    let text = app
        .runtime_for_pane_in_workspace(terminal_runtimes, card.ws_idx, card.pane_id)
        .map(|runtime| runtime.recent_text(visible));
    let Some(text) = text else {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                "  (no live screen for this pane)",
                dim,
            ))),
            Rect::new(inner.x, inner.y, inner.width, 1),
        );
        return;
    };
    // Keep the last `visible` non-empty-tail lines: trailing blank rows are
    // the agent's input-box padding and would push real output off the top.
    let mut lines: Vec<&str> = text.lines().collect();
    while lines.last().is_some_and(|line| line.trim().is_empty()) {
        lines.pop();
    }
    let start = lines.len().saturating_sub(visible);
    let width = inner.width as usize;
    for (offset, line) in lines[start..].iter().enumerate() {
        let y = inner.y + offset as u16;
        if y >= inner.y + inner.height {
            break;
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                truncate_end(line, width),
                Style::default().fg(p.subtext0),
            ))),
            Rect::new(inner.x, y, inner.width, 1),
        );
    }
}

/// Colour for a task-queue state, matching the agent-state language: red is
/// blocked, yellow is running, green is done, dim is waiting.
/// The dispatch queue behind the dashboard's `tasks` count: what is waiting,
/// what is running, and which repo each one belongs to.
fn render_task_queue(app: &AppState, frame: &mut Frame, body: Rect) {
    if body.width == 0 || body.height == 0 {
        return;
    }
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    let rows = &app.task_queue.rows;

    if !app.task_queue.sampled {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("reading queue{}", glyphs::ELLIPSIS),
                dim,
            ))),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }
    if rows.is_empty() {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled("queue is empty", dim))),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }

    // Header: the same counts the dashboard shows, broken out by state.
    let running = rows
        .iter()
        .filter(|row| row.state == crate::tasks::TaskState::Running)
        .count();
    let waiting = rows
        .iter()
        .filter(|row| row.state == crate::tasks::TaskState::Todo)
        .count();
    let header = Line::from(vec![
        Span::styled(rows.len().to_string(), Style::default().fg(p.text)),
        Span::styled(format!(" in queue{}", glyphs::SEP_WIDE), dim),
        Span::styled(running.to_string(), Style::default().fg(p.yellow)),
        Span::styled(format!(" running{}", glyphs::SEP_WIDE), dim),
        Span::styled(waiting.to_string(), Style::default().fg(p.overlay1)),
        Span::styled(" waiting", dim),
    ]);
    frame.render_widget(
        Paragraph::new(header),
        Rect::new(body.x, body.y, body.width, 1),
    );

    // Two rows per task plus a blank separator, same rhythm as a board card.
    const TASK_STRIDE: u16 = 3;
    let list_top = body.y + 2;
    if list_top >= body.y + body.height {
        return;
    }
    let list_height = body.y + body.height - list_top;
    let capacity = (list_height / TASK_STRIDE).max(1) as usize;
    // Scroll the window so the selection stays visible.
    let selected = app.board.task_selected.min(rows.len().saturating_sub(1));
    let start = selected.saturating_sub(capacity.saturating_sub(1));
    for (offset, row) in rows[start..].iter().take(capacity).enumerate() {
        let y = list_top + offset as u16 * TASK_STRIDE;
        render_task_row(app, frame, row, start + offset == selected, {
            Rect::new(body.x, y, body.width, 2)
        });
    }
}

fn render_task_row(
    app: &AppState,
    frame: &mut Frame,
    row: &TaskQueueRow,
    selected: bool,
    rect: Rect,
) {
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let p = &app.palette;
    if selected {
        let buf = frame.buffer_mut();
        for y in rect.top()..rect.bottom() {
            for x in rect.left()..rect.right() {
                buf[(x, y)].set_style(Style::default().bg(p.surface0));
            }
        }
    }
    let dim = Style::default().fg(p.overlay0);
    let width = rect.width as usize;
    let task = super::status::task_appearance(row.state, app.spinner_tick);
    let color = task.color(p);
    let marker = if selected { glyphs::MARKER } else { " " };
    let marker_style = if selected {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };

    // Line 1: marker · state dot · prompt … state label pinned to the right
    // edge, so the states line up into a column that can be read down.
    let label = task.label;
    let prompt_budget = width.saturating_sub(3).saturating_sub(label.len() + 2);
    let prompt = truncate_end(&row.prompt, prompt_budget);
    let used = 3 + prompt.chars().count() + label.len();
    let pad = width.saturating_sub(used).max(2);
    let line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        // The state's own glyph, not a filled dot in five colours: three of
        // these rows used to be distinguishable by hue alone.
        Span::styled(format!("{} ", task.glyph), Style::default().fg(color)),
        Span::styled(prompt, Style::default().fg(p.text)),
        Span::styled(
            format!("{}{label}", " ".repeat(pad)),
            Style::default().fg(color),
        ),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(line1)),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );

    if rect.height < 2 {
        return;
    }
    // Line 2: where it will run.
    let mut meta = format!(
        "{} {} {}",
        row.repo_label,
        glyphs::SEP,
        row.runtime.as_str()
    );
    if row.use_worktree {
        meta.push_str(&format!(" {} worktree", glyphs::SEP));
    }
    if row.dispatched {
        meta.push_str(&format!(" {} dispatched", glyphs::SEP));
    }
    meta.push_str(&format!(
        " {} {}",
        glyphs::SEP,
        format_event_age(std::time::Duration::from_secs(row.age_secs.max(0) as u64))
    ));
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            format!("   {}", truncate_end(&meta, width.saturating_sub(3))),
            dim,
        ))),
        Rect::new(rect.x, rect.y + 1, rect.width, 1),
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
    fn card_shows_queued_input_badge() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, panes) = board_state();
        state
            .queued_pane_input
            .insert(panes[0], vec!["one".into(), "two".into()]);
        let model = board_model(&state);
        let card = &model.columns[0][0];
        assert_eq!(card.pane_id, panes[0]);

        let mut terminal = Terminal::new(TestBackend::new(40, 4)).expect("test terminal");
        terminal
            .draw(|frame| render_card(&state, frame, Rect::new(0, 0, 40, 3), card, false))
            .expect("card should render");

        let buffer = terminal.backend().buffer();
        let row: String = (0..40).map(|x| buffer[(x, 0)].symbol()).collect();
        assert!(
            row.contains("\u{21e5}2"),
            "queued badge should render: {row:?}"
        );
    }

    /// Seed a queue sample so the task screen has something to draw without
    /// touching the real task database.
    fn seed_task_queue(state: &mut AppState) {
        use crate::app::state::TaskQueueRow;
        use crate::tasks::{TaskRuntime, TaskState};
        state.task_queue.sampled = true;
        state.task_queue.sampled_at = Some(std::time::Instant::now());
        state.task_queue.rows = vec![
            TaskQueueRow {
                id: 1,
                prompt: "harden the stripe webhook signature check".into(),
                state: TaskState::Running,
                repo_label: "workmayt".into(),
                runtime: TaskRuntime::Claude,
                use_worktree: true,
                dispatched: true,
                age_secs: 240,
            },
            TaskQueueRow {
                id: 2,
                prompt: "draft the 0.9.0 release notes".into(),
                state: TaskState::Todo,
                repo_label: "shep".into(),
                runtime: TaskRuntime::Opencode,
                use_worktree: false,
                dispatched: false,
                age_secs: 1140,
            },
            TaskQueueRow {
                id: 3,
                prompt: "unblock the gitea 403 on emberline".into(),
                state: TaskState::Blocked,
                repo_label: "emberline".into(),
                runtime: TaskRuntime::Claude,
                use_worktree: false,
                dispatched: true,
                age_secs: 660,
            },
        ];
    }

    #[test]
    fn task_queue_screen_lists_rows_with_state_and_repo() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, _panes) = board_state();
        seed_task_queue(&mut state);
        state.board.task_selected = 1;

        let mut terminal = Terminal::new(TestBackend::new(80, 14)).expect("test terminal");
        terminal
            .draw(|frame| render_task_queue(&state, frame, Rect::new(0, 0, 80, 14)))
            .expect("task queue should render");

        let buffer = terminal.backend().buffer();
        let rows: Vec<String> = (0..14)
            .map(|y| (0..80).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let screen = rows.join("\n");
        assert!(screen.contains("3 in queue"), "header counts: {screen}");
        assert!(screen.contains("1 running"), "header counts: {screen}");
        assert!(
            screen.contains("harden the stripe webhook"),
            "prompt: {screen}"
        );
        assert!(
            screen.contains("workmayt · claude · worktree"),
            "meta line: {screen}"
        );
        assert!(screen.contains("blocked"), "state label: {screen}");
        // The selected row (index 1) carries the marker, no other row does.
        let marked: Vec<&String> = rows.iter().filter(|row| row.contains('▌')).collect();
        assert_eq!(marked.len(), 1, "exactly one selection marker: {screen}");
        assert!(
            marked[0].contains("draft the 0.9.0 release notes"),
            "marker on the selected row: {:?}",
            marked[0]
        );
    }

    #[test]
    fn task_queue_screen_distinguishes_unsampled_from_empty() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, _panes) = board_state();
        let render = |state: &AppState| {
            let mut terminal = Terminal::new(TestBackend::new(40, 3)).expect("test terminal");
            terminal
                .draw(|frame| render_task_queue(state, frame, Rect::new(0, 0, 40, 3)))
                .expect("render");
            let buffer = terminal.backend().buffer();
            (0..40).map(|x| buffer[(x, 0)].symbol()).collect::<String>()
        };
        // Never sampled is not the same claim as "you have no tasks".
        assert!(render(&state).contains("reading queue"));
        state.task_queue.sampled = true;
        assert!(render(&state).contains("queue is empty"));
    }

    // Constructing a pane runtime needs a reactor, like the pane render tests.
    #[tokio::test]
    async fn agent_detail_shows_the_panes_live_screen() {
        use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, panes) = board_state();
        state.board.view = BoardView::Agent;
        state.board.selected = Some(panes[0]);
        state.workspaces[0].tabs[0].runtimes.insert(
            panes[0],
            TerminalRuntime::test_with_scrollback_bytes(
                60,
                6,
                4096,
                b"cargo clippy --all-targets\r\nwarning: unused import\r\n? apply the fix\r\n",
            ),
        );

        let mut terminal = Terminal::new(TestBackend::new(70, 20)).expect("test terminal");
        let runtimes = TerminalRuntimeRegistry::new();
        let model = board_model(&state);
        terminal
            .draw(|frame| {
                render_agent_detail(&state, &runtimes, frame, &model, Rect::new(0, 0, 70, 20))
            })
            .expect("agent detail should render");

        let buffer = terminal.backend().buffer();
        let screen: String = (0..20)
            .map(|y| (0..70).map(|x| buffer[(x, y)].symbol()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(screen.contains("live screen"), "screen heading: {screen}");
        assert!(
            screen.contains("? apply the fix"),
            "the pane's own output should be on screen: {screen}"
        );
        assert!(
            !screen.contains("no live screen"),
            "a pane with a runtime is not a pane without one: {screen}"
        );
    }

    #[test]
    fn agent_detail_falls_back_when_the_selection_no_longer_resolves() {
        // An agent can exit while the board is open; the detail screen should
        // land on a real card rather than render nothing.
        let (mut state, panes) = board_state();
        let model = board_model(&state);
        state.board.selected = Some(panes[2]);
        assert_eq!(
            detail_card(&state, &model).map(|c| c.pane_id),
            Some(panes[2])
        );
        state.board.selected = Some(PaneId::from_raw(9999));
        assert!(
            detail_card(&state, &model).is_some(),
            "stale selection should fall back to the first card"
        );
        state.board.selected = None;
        assert!(detail_card(&state, &model).is_some());
    }

    #[tokio::test]
    #[ignore = "visual preview, run with --nocapture"]
    async fn preview_agent_detail() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, panes) = board_state();
        state.workspaces[0].tabs[0].runtimes.insert(
            panes[0],
            crate::terminal::TerminalRuntime::test_with_scrollback_bytes(
                100,
                8,
                8192,
                b"\xe2\x97\x8f Update(apps/api/src/stripe/webhook.ts)\r\n  41 additions, 8 removals\r\n\r\n? Do you want to make this edit to webhook.ts\r\n  1. Yes  2. Yes, allow all edits  3. No\r\n",
            ),
        );
        state.view.terminal_area = Rect::new(0, 0, 110, 26);
        state.view.sidebar_rect = Rect::new(0, 0, 110, 26);
        state.board.view = crate::app::state::BoardView::Agent;
        state.board.selected = Some(panes[0]);
        state
            .queued_pane_input
            .insert(panes[0], vec!["next".into()]);
        let tid = state.workspaces[0]
            .terminal_id(panes[0])
            .expect("terminal")
            .clone();
        let t = state.terminals.get_mut(&tid).expect("terminal");
        t.set_activity_lines(vec!["? Do you want to make this edit to webhook.ts".into()]);
        t.set_context_percent(Some(74));
        t.cwd = std::path::PathBuf::from("/Users/alex/vault/dev/workmayt");

        let mut term = Terminal::new(TestBackend::new(110, 26)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        for y in 0..26 {
            let row: String = (0..110).map(|x| buffer[(x, y)].symbol()).collect();
            println!("{}", row.trim_end());
        }
    }

    #[test]
    #[ignore = "visual preview, run with --nocapture"]
    fn preview_task_queue() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, _panes) = board_state();
        state.view.terminal_area = Rect::new(0, 0, 110, 20);
        state.view.sidebar_rect = Rect::new(0, 0, 110, 20);
        state.board.view = crate::app::state::BoardView::Tasks;
        state.board.task_selected = 1;
        seed_task_queue(&mut state);

        let mut term = Terminal::new(TestBackend::new(110, 20)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        for y in 0..20 {
            let row: String = (0..110).map(|x| buffer[(x, y)].symbol()).collect();
            println!("{}", row.trim_end());
        }
    }

    #[test]
    #[ignore = "visual preview, run with --nocapture"]
    fn preview_full_board() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, panes) = board_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 34);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 34);
        state
            .dashboard_sample
            .refresh_if_stale(std::time::Instant::now());
        state
            .queued_pane_input
            .insert(panes[1], vec!["do the thing".into()]);
        state.workspaces[0].tabs[0].set_custom_name("review".into());
        for (i, pane) in panes.iter().enumerate() {
            let tid = state.workspaces[if i == 2 { 1 } else { 0 }]
                .terminal_id(*pane)
                .expect("terminal")
                .clone();
            let t = state.terminals.get_mut(&tid).expect("terminal");
            t.set_activity_lines(vec![[
                "Metamorphosing… (3s · thinking)",
                "waiting for your approval",
                "done — 4 files changed",
            ][i]
                .into()]);
            t.set_context_percent(Some([88, 41, 12][i]));
            t.cwd = std::path::PathBuf::from(
                [
                    "/Users/alex/vault/dev/shep",
                    "/Users/alex/vault/dev/shep-android",
                    "/Users/alex/vault/dev/atlas",
                ][i],
            );
        }
        let mut term = Terminal::new(TestBackend::new(150, 34)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        for y in 0..34 {
            let row: String = (0..150).map(|x| buffer[(x, y)].symbol()).collect();
            println!("{}", row.trim_end());
        }
    }

    #[test]
    fn dashboard_counts_agents_session_shape_and_queued_input() {
        let (mut state, panes) = board_state();
        state
            .queued_pane_input
            .insert(panes[0], vec!["one".into(), "two".into()]);
        let summary = board_summary(&state, &board_model(&state));

        assert_eq!(summary.agents(), 3);
        assert_eq!((summary.blocked, summary.done, summary.working), (1, 1, 1));
        // Blocked (1) plus done-and-unseen (1) are the agents waiting on Alex.
        assert_eq!(summary.attention, 2);
        assert_eq!(summary.workspaces, 2);
        assert_eq!(summary.panes, 3, "two panes in ws0, one in ws1");
        assert_eq!(summary.queued_input, 2);
    }

    #[test]
    fn dashboard_strip_renders_the_pulse_and_the_host() {
        use ratatui::{backend::TestBackend, Terminal};
        let (state, _) = board_state();
        let summary = board_summary(&state, &board_model(&state));
        let mut term = Terminal::new(TestBackend::new(120, 2)).expect("test terminal");
        term.draw(|frame| render_dashboard(&state, frame, Rect::new(0, 0, 120, 2), &summary))
            .expect("dashboard should render");
        let buffer = term.backend().buffer();
        let row = |y: u16| -> String { (0..120).map(|x| buffer[(x, y)].symbol()).collect() };

        let pulse = row(0);
        assert!(pulse.contains("agents 3"), "{pulse:?}");
        assert!(pulse.contains("2 need you"), "{pulse:?}");
        assert!(pulse.contains("2 ws"), "{pulse:?}");
        // Unsampled host facts must read as em dashes, never as 0%.
        let host = row(1);
        assert!(host.contains(env!("CARGO_PKG_VERSION")), "{host:?}");
        assert!(host.contains('\u{2014}'), "unsampled vitals: {host:?}");
        assert!(!host.contains("0%"), "must not invent a reading: {host:?}");
    }

    #[test]
    fn dashboard_yields_its_rows_before_the_cards_do() {
        // A short terminal drops the strip entirely rather than squeezing the
        // cards out of existence.
        assert_eq!(dashboard_rows(Rect::new(0, 0, 80, 10)), 0);
        assert!(dashboard_rows(Rect::new(0, 0, 80, 30)) > 0);
        let short = board_body(Rect::new(0, 0, 80, 10));
        assert_eq!(short.height, 8, "title + footer only");
    }

    #[test]
    fn card_renders_activity_line_repo_path_and_context_gauge() {
        use ratatui::{backend::TestBackend, Terminal};
        let (mut state, panes) = board_state();
        let terminal_id = state.workspaces[0]
            .terminal_id(panes[0])
            .expect("terminal")
            .clone();
        let terminal = state.terminals.get_mut(&terminal_id).expect("terminal");
        terminal.set_activity_lines(vec!["running the migration".into()]);
        terminal.set_context_percent(Some(62));
        terminal.cwd = std::path::PathBuf::from("/tmp/deep/nested/repo");

        let model = board_model(&state);
        let card = &model.columns[0][0];
        let mut term = Terminal::new(TestBackend::new(48, 6)).expect("test terminal");
        term.draw(|frame| render_card(&state, frame, Rect::new(0, 0, 48, 5), card, false))
            .expect("card should render");
        let buffer = term.backend().buffer();
        let row = |y: u16| -> String { (0..48).map(|x| buffer[(x, y)].symbol()).collect() };

        assert!(
            row(3).contains("running the migration"),
            "activity line: {:?}",
            row(3)
        );
        let last = row(4);
        assert!(last.contains("nested/repo"), "repo path: {last:?}");
        assert!(last.contains("62%"), "context gauge: {last:?}");
        assert!(
            last.contains('\u{2588}'),
            "gauge should draw a bar: {last:?}"
        );
    }

    #[test]
    fn context_gauge_fills_proportionally_and_clamps() {
        // Empty draws only the channel, which is bare background.
        assert!(context_gauge(0).starts_with("      "));
        assert_eq!(context_gauge(100), "██████ 100%");
        // Any nonzero percentage lights something, so a barely used context is
        // still visibly distinct from an unknown one — an eighth now, rather
        // than a whole cell, because a whole cell overstated 1% by sixteen.
        assert!(!context_gauge(1).starts_with("  "));
        // Only a full context fills the bar: a nearly-full agent must stay
        // visually distinguishable from a finished one.
        assert_ne!(context_gauge(88), context_gauge(100));
    }

    /// What eighths buy: six whole cells give seven states across a hundred
    /// and one percentages, so 60% and 74% used to be the same picture.
    #[test]
    fn the_context_gauge_resolves_within_a_cell() {
        assert_ne!(context_gauge(60), context_gauge(74));
        let distinct: std::collections::HashSet<String> = (0..=100)
            .map(|percent| {
                let g = context_gauge(percent);
                g.split(' ').next().unwrap_or_default().to_string()
            })
            .collect();
        // Forty-eight eighths plus empty.
        assert_eq!(distinct.len(), 49);
    }

    /// Every gauge is the same width, so a lane of them is a column of bars
    /// with a column of numbers beside it rather than a staircase.
    #[test]
    fn every_gauge_measures_the_same() {
        let widths: std::collections::HashSet<usize> = (0..=100)
            .map(|p| display_width(&context_gauge(p)))
            .collect();
        assert_eq!(widths.len(), 1, "gauge widths: {widths:?}");
    }

    /// The facts that do not fit come off whole.
    ///
    /// Clipping the `Paragraph` instead left `·  3` on an 80-column board — the
    /// head of "3 ws · 3 tabs · 5 panes", which reads as a count of something
    /// that is never named.
    #[test]
    fn the_dashboard_drops_whole_facts_rather_than_clipping_one() {
        use ratatui::{backend::TestBackend, Terminal};
        let (state, _) = board_state();
        let model = board_model(&state);
        let summary = board_summary(&state, &model);
        for width in 20u16..=140 {
            let mut terminal = Terminal::new(TestBackend::new(width, 2)).expect("test terminal");
            terminal
                .draw(|frame| render_dashboard(&state, frame, Rect::new(0, 0, width, 2), &summary))
                .expect("dashboard should render");
            let buffer = terminal.backend().buffer();
            for y in 0..2 {
                let row: String = (0..width).map(|x| buffer[(x, y)].symbol()).collect();
                let row = row.trim_end();
                assert!(
                    !row.ends_with(glyphs::SEP),
                    "width {width} row {y} ends on a separator: {row:?}"
                );
                // Every fact ends in a word or a digit — never a bare fragment
                // of a longer phrase.
                if let Some(last) = row.split_whitespace().next_back() {
                    assert!(
                        last.chars().next_back().is_some_and(|c| c.is_alphanumeric()
                            || c == '%'
                            || c == glyphs::DASH.chars().next().unwrap_or('-')),
                        "width {width} row {y} ends mid-fact: {row:?}"
                    );
                }
            }
        }
    }

    /// Four lanes need four times the room, so the board stacks well before
    /// the rest of the app does. At 80 columns the lanes were 20 wide and
    /// every card elided its agent's name away to nothing.
    #[test]
    fn the_board_stacks_before_its_lanes_get_too_thin_to_read() {
        let (mut state, _) = board_state();
        let wide = Rect::new(0, 0, 120, 40);
        let standard = Rect::new(0, 0, 80, 24);
        state.view.sidebar_rect = standard;
        state.view.terminal_area = standard;
        assert!(is_narrow(&state), "80 columns is four 20-column lanes");
        state.view.sidebar_rect = wide;
        state.view.terminal_area = wide;
        assert!(!is_narrow(&state), "120 columns has room for four lanes");
    }

    /// Location, age and gauge sit on the card's right edge, one column in.
    ///
    /// They used to trail whatever text came before them, so they only looked
    /// aligned when that text happened to be long enough to truncate — which
    /// is to say a lane of short names produced a ragged right edge.
    #[test]
    fn a_cards_trailing_facts_pin_to_the_right_edge() {
        use ratatui::{backend::TestBackend, Terminal};
        let (state, panes) = board_state();
        let model = board_model(&state);
        let card = &model.columns[0][0];
        assert_eq!(card.pane_id, panes[0]);

        const WIDTH: u16 = 60;
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, 5)).expect("test terminal");
        terminal
            .draw(|frame| render_card(&state, frame, Rect::new(0, 0, WIDTH, 5), card, false))
            .expect("card should render");
        let buffer = terminal.backend().buffer();
        let row_at = |y: u16| -> String { (0..WIDTH).map(|x| buffer[(x, y)].symbol()).collect() };
        for y in 0..5u16 {
            assert!(
                row_at(y).ends_with(' '),
                "row {y} has no right margin: {:?}",
                row_at(y)
            );
        }
        // Only the rows that actually carry a trailing fact — a card whose
        // agent has no recorded age has nothing to pin on its second line.
        let trailing = [
            (0u16, card.location.clone()),
            (1, card_age(&state, card).unwrap_or_default()),
            (
                4,
                card.context_percent.map(context_gauge).unwrap_or_default(),
            ),
        ];
        for (y, fact) in trailing {
            if fact.is_empty() {
                continue;
            }
            let row = row_at(y);
            let drawn = row.trim_end();
            // The margin is exactly one column: anything wider means the fact
            // stopped short of the edge instead of pinning to it.
            assert_eq!(
                drawn.chars().count() + CARD_RIGHT_MARGIN,
                WIDTH as usize,
                "row {y} is not pinned right: {row:?}"
            );
            assert!(
                drawn.ends_with(fact.trim_end()),
                "row {y} should end with {fact:?}: {row:?}"
            );
        }
    }

    #[test]
    fn location_prefers_the_tab_name_over_its_number() {
        let (mut state, panes) = board_state();
        // Workspace 0 has one tab holding two panes, so the pane number earns
        // its width but the tab part does not — until the tab is named.
        let model = board_model(&state);
        assert_eq!(model.columns[0][0].location, "p1");

        state.workspaces[0].tabs[0].set_custom_name("review".into());
        let model = board_model(&state);
        let card = model
            .flattened()
            .into_iter()
            .find(|card| card.pane_id == panes[0])
            .expect("blocked card");
        assert_eq!(card.location, "review·p1");
    }

    #[test]
    fn location_drops_the_pane_number_for_a_single_pane_tab() {
        // Workspace 1 is a lone unnamed pane in a lone unnamed tab: "p1" and
        // "t1" are both noise, so the tag is empty rather than decorative.
        let (state, _) = board_state();
        assert_eq!(model_card(&state, 1).location, "");
    }

    /// The single card in the given workspace.
    fn model_card(state: &AppState, ws_idx: usize) -> BoardCard {
        board_model(state)
            .flattened()
            .into_iter()
            .find(|card| card.ws_idx == ws_idx)
            .cloned()
            .expect("card for workspace")
    }

    /// A card builder for the naming rules alone — they only read the four
    /// name-bearing fields, so the rest stays at its cheapest.
    fn named_card(
        pane: u32,
        agent: &str,
        workspace: &str,
        branch: Option<&str>,
        location: &str,
    ) -> BoardCard {
        BoardCard {
            ws_idx: 0,
            pane_id: PaneId::from_raw(pane),
            agent_label: agent.to_string(),
            display_name: agent.to_string(),
            workspace_label: workspace.to_string(),
            location: location.to_string(),
            branch: branch.map(str::to_string),
            status: None,
            state: crate::detect::AgentState::Idle,
            seen: true,
            manual_state: None,
            context_percent: None,
            cwd: None,
            model: None,
            activity: None,
            activity_lines: Vec::new(),
            sort_seq: None,
        }
    }

    fn names_for(cards: Vec<BoardCard>) -> Vec<String> {
        let mut model = BoardModel::default();
        model.columns[3] = cards;
        assign_distinct_names(&mut model);
        model.columns[3]
            .iter()
            .map(|card| card.display_name.clone())
            .collect()
    }

    /// An agent that is already the only one of its name pays nothing for the
    /// others' ambiguity.
    #[test]
    fn distinct_names_spend_detail_only_where_it_buys_a_distinction() {
        let names = names_for(vec![
            named_card(1, "claude", "shep", Some("master"), "p1"),
            named_card(2, "claude", "workmayt", Some("master"), "p1"),
            named_card(3, "opencode", "shep", Some("master"), "p1"),
        ]);
        // opencode is unique at the shortest level and stays short.
        assert_eq!(names[2], "opencode");
        // The two claudes are separated by workspace, and stop there.
        assert_eq!(names[0], "claude · shep");
        assert_eq!(names[1], "claude · workmayt");
    }

    /// Detail keeps growing only for the cards that are still colliding.
    #[test]
    fn distinct_names_grow_through_branch_then_location() {
        let names = names_for(vec![
            named_card(1, "claude", "shep", Some("master"), "docs"),
            named_card(2, "claude", "shep", Some("fix/push"), "p1"),
            named_card(3, "claude", "shep", Some("master"), "board"),
        ]);
        // Unique once the branch is added.
        assert_eq!(names[1], "claude · shep · fix/push");
        // Same branch: these two need the location too.
        assert_eq!(names[0], "claude · shep · master · docs");
        assert_eq!(names[2], "claude · shep · master · board");
    }

    /// The pane id is the last resort, not the first: it appears only when
    /// nothing readable separates two agents.
    #[test]
    fn distinct_names_fall_back_to_the_pane_id_only_when_nothing_else_differs() {
        let names = names_for(vec![
            named_card(7, "claude", "shep", Some("master"), "p1"),
            named_card(9, "claude", "shep", Some("master"), "p1"),
        ]);
        assert_eq!(names[0], "claude · shep · master · p1 · 7");
        assert_eq!(names[1], "claude · shep · master · p1 · 9");
    }

    /// One agent on the board never grows a suffix.
    #[test]
    fn a_lone_agent_keeps_its_plain_name() {
        let names = names_for(vec![named_card(1, "claude", "shep", Some("master"), "p1")]);
        assert_eq!(names, vec!["claude".to_string()]);
    }

    /// Empty placement fields must not produce dangling separators.
    #[test]
    fn distinct_names_skip_placement_the_session_does_not_have() {
        let names = names_for(vec![
            named_card(1, "claude", "", None, ""),
            named_card(2, "claude", "", None, ""),
        ]);
        assert_eq!(names[0], "claude · 1");
        assert_eq!(names[1], "claude · 2");
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
