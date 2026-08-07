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
use super::text::{truncate_end, truncate_start};
use super::widgets::render_panel_shell;
use crate::app::state::{AppState, BoardView, TaskQueueRow};
use crate::detect::AgentState;
use crate::layout::PaneId;

/// Number of state columns (blocked, done, working, idle).
pub(crate) const BOARD_COLUMNS: usize = 4;

/// Visible rows per card: agent line, workspace/branch/age line, status line,
/// activity line, then repo path + context gauge.
const CARD_ROWS: u16 = 5;
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
    /// Where the agent is working, contracted for display (`~/vault/dev/shep`).
    pub cwd: Option<String>,
    /// The agent's own name for itself — a model, usually, when it reports one.
    pub model: Option<String>,
    /// Last line of real screen content; "what is it saying right now".
    pub activity: Option<String>,
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

/// A tiny inline gauge for the context window: `████░░ 62%`.
///
/// Rendered as a bar because the number alone doesn't read at a glance — the
/// thing worth seeing across eight cards is *which agent is nearly full*.
fn context_gauge(percent: u8) -> String {
    const WIDTH: usize = 6;
    let percent = percent.min(100);
    // Round rather than ceil, so only a genuinely full context fills the bar —
    // with six cells, ceil made 84% and 100% look identical. Any nonzero
    // reading still lights one cell so "barely used" outranks "unknown".
    let filled = ((percent as usize * WIDTH) as f64 / 100.0).round() as usize;
    let filled = if percent > 0 { filled.max(1) } else { 0 };
    format!(
        "{}{} {percent}%",
        "\u{2588}".repeat(filled),
        "\u{2591}".repeat(WIDTH.saturating_sub(filled))
    )
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
    let tab_part = match named {
        Some(name) => Some(name.to_string()),
        None if multi_tab => ws.public_tab_number(tab_idx).map(|n| format!("t{n}")),
        None => None,
    };
    let multi_pane = ws
        .tabs
        .get(tab_idx)
        .map(|tab| tab.panes.len() > 1)
        .unwrap_or(false);
    let pane_part = pane_number.filter(|_| multi_pane).map(|n| format!("p{n}"));
    match (tab_part, pane_part) {
        (Some(tab), Some(pane)) => format!("{tab}·{pane}"),
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
        let activity = terminal.and_then(|terminal| terminal.activity_line.clone());
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
            cwd,
            model: agent_model,
            activity,
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
fn render_dashboard(app: &AppState, frame: &mut Frame, area: Rect, summary: &BoardSummary) {
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    let value = Style::default().fg(p.text);
    let sep = || Span::styled("  ·  ", Style::default().fg(p.surface0));

    // Row 1 — agents and session shape.
    let mut row1 = vec![
        Span::styled(" agents ", dim),
        Span::styled(summary.agents().to_string(), value),
    ];
    if summary.attention > 0 {
        // The one number on this strip that is a call to action.
        row1.push(Span::styled(
            format!("  {} need you", summary.attention),
            Style::default().fg(p.red).add_modifier(Modifier::BOLD),
        ));
    }
    for (label, count, state, seen) in [
        ("blocked", summary.blocked, AgentState::Blocked, true),
        ("done", summary.done, AgentState::Idle, false),
        ("working", summary.working, AgentState::Working, true),
        ("idle", summary.idle, AgentState::Idle, true),
    ] {
        row1.push(sep());
        row1.push(Span::styled(
            format!("{label} "),
            Style::default().fg(super::status::state_label_color(state, seen, p)),
        ));
        row1.push(Span::styled(count.to_string(), value));
    }
    row1.push(sep());
    row1.push(Span::styled(
        format!(
            "{} ws · {} tabs · {} panes",
            summary.workspaces, summary.tabs, summary.panes
        ),
        dim,
    ));
    if summary.queued_input > 0 {
        row1.push(sep());
        row1.push(Span::styled(
            format!("\u{21e5}{} queued", summary.queued_input),
            Style::default().fg(p.teal),
        ));
    }
    if let Some(pending) = app.dashboard_sample.pending_tasks.filter(|n| *n > 0) {
        row1.push(sep());
        row1.push(Span::styled(format!("{pending} tasks"), value));
    }
    frame.render_widget(
        Paragraph::new(Line::from(row1)),
        Rect::new(area.x, area.y, area.width, 1),
    );

    if area.height < 2 {
        return;
    }

    // Row 2 — the host. Unsampled or unreadable values print as an em dash
    // rather than a confident zero.
    let vitals = app.dashboard_sample.vitals;
    let mut row2 = vec![
        Span::styled(" shep ", dim),
        Span::styled(env!("CARGO_PKG_VERSION"), value),
    ];
    row2.push(sep());
    row2.push(Span::styled("load ", dim));
    match (vitals.load_percent, vitals.cores) {
        (Some(load), Some(cores)) => {
            let color = match load {
                100..=u16::MAX => p.red,
                70..=99 => p.yellow,
                _ => p.text,
            };
            row2.push(Span::styled(format!("{load}%"), Style::default().fg(color)));
            row2.push(Span::styled(format!(" of {cores} cores"), dim));
        }
        _ => row2.push(Span::styled("\u{2014}", dim)),
    }
    row2.push(sep());
    row2.push(Span::styled("mem ", dim));
    match vitals.memory_percent {
        Some(percent) => {
            let color = match percent {
                90..=u8::MAX => p.red,
                75..=89 => p.yellow,
                _ => p.text,
            };
            row2.push(Span::styled(
                format!("{percent}%"),
                Style::default().fg(color),
            ));
            if let (Some(used), Some(total)) = (vitals.memory_used_bytes, vitals.memory_total_bytes)
            {
                row2.push(Span::styled(
                    format!(" {} of {}", human_bytes(used), human_bytes(total)),
                    dim,
                ));
            }
        }
        None => row2.push(Span::styled("\u{2014}", dim)),
    }
    frame.render_widget(
        Paragraph::new(Line::from(row2)),
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
            Span::styled("· what are my agents doing", dim),
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
            ("hjkl/↑↓←→", " move  "),
            ("t", " tasks  "),
            ("esc/q", " close"),
        ],
        BoardView::Agent => &[
            ("enter", " attach  "),
            ("t", " tasks  "),
            ("esc/q", " back to board"),
        ],
        BoardView::Tasks => &[("jk/↑↓", " move  "), ("esc/q/t", " back to board")],
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

    // Line 1: marker · dot · agent label · model … location.
    // The context meter moved to its own line as a gauge.
    let head = format!("{marker}{dot} ");
    let loc_width = card.location.chars().count();
    let model = card.model.clone().unwrap_or_default();
    let model_reserved = if model.is_empty() {
        0
    } else {
        model.chars().count() + 1
    };
    // Queued-input badge (M5 tab-to-queue): prompts waiting for idle.
    let queued = app.queued_input_count_for_pane(card.pane_id);
    let queued_label = (queued > 0).then(|| format!("\u{21e5}{queued}"));
    let queued_reserved = queued_label
        .as_ref()
        .map(|label| label.chars().count() + 1)
        .unwrap_or(0);
    let agent_budget = width
        .saturating_sub(head.chars().count())
        .saturating_sub(loc_width)
        .saturating_sub(model_reserved)
        .saturating_sub(queued_reserved)
        .saturating_sub(1);
    let mut line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(truncate_end(&card.agent_label, agent_budget), agent_style),
    ];
    if let Some(queued_label) = &queued_label {
        line1.push(Span::styled(
            format!(" {queued_label}"),
            Style::default().fg(p.teal),
        ));
    }
    if !model.is_empty() {
        line1.push(Span::styled(
            format!(" {model}"),
            Style::default().fg(p.teal),
        ));
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

    if rect.height < 4 {
        return;
    }
    // Line 4: what the agent's screen is actually saying right now. Italic and
    // dim because it is a hint, not shep's own reporting.
    if let Some(activity) = &card.activity {
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!("  {}", truncate_end(activity, width.saturating_sub(2))),
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
    // Line 5: where it is working … context gauge, right-aligned.
    let gauge = card.context_percent.map(context_gauge).unwrap_or_default();
    let gauge_reserved = if gauge.is_empty() {
        0
    } else {
        gauge.chars().count() + 1
    };
    let cwd = card.cwd.clone().unwrap_or_default();
    let cwd_budget = width.saturating_sub(2).saturating_sub(gauge_reserved);
    let mut line5 = vec![
        Span::raw("  "),
        Span::styled(truncate_start(&cwd, cwd_budget), dim),
    ];
    if !gauge.is_empty() {
        // Near-full context is the thing worth noticing, so it warms up.
        let gauge_color = match card.context_percent.unwrap_or(0) {
            85..=u8::MAX => p.red,
            60..=84 => p.yellow,
            _ => p.overlay0,
        };
        line5.push(Span::styled(
            format!(" {gauge}"),
            Style::default().fg(gauge_color),
        ));
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
    let (dot, dot_style) = state_dot(card.state, card.seen, p);
    let mut heading = vec![
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(
            card.agent_label.clone(),
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(model_name) = &card.model {
        heading.push(Span::styled(format!("  {model_name}"), dim));
    }
    row(frame, &mut y, Line::from(heading));

    let mut sub = card.workspace_label.clone();
    if let Some(branch) = &card.branch {
        sub.push_str(" · ");
        sub.push_str(branch);
    }
    if !card.location.is_empty() {
        sub.push_str(" · ");
        sub.push_str(&card.location);
    }
    if let Some(age) = card_age(app, card) {
        sub.push_str(" · last activity ");
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
        state_value.push_str(&format!(" · \u{21e5}{queued} queued"));
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
        let color = match percent {
            85..=u8::MAX => p.red,
            60..=84 => p.yellow,
            _ => p.overlay1,
        };
        kv(
            frame,
            &mut y,
            "context",
            context_gauge(percent),
            Style::default().fg(color),
        );
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
fn task_state_color(
    state: crate::tasks::TaskState,
    p: &crate::app::state::Palette,
) -> ratatui::style::Color {
    match state {
        crate::tasks::TaskState::Blocked => p.red,
        crate::tasks::TaskState::Running => p.yellow,
        crate::tasks::TaskState::Done => p.green,
        crate::tasks::TaskState::Todo => p.overlay1,
        crate::tasks::TaskState::Cancelled => p.overlay0,
    }
}

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
            Paragraph::new(Line::from(Span::styled("reading queue…", dim))),
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
        Span::styled(" in queue  ·  ", dim),
        Span::styled(running.to_string(), Style::default().fg(p.yellow)),
        Span::styled(" running  ·  ", dim),
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
    let color = task_state_color(row.state, p);
    let marker = if selected { "\u{258c}" } else { " " };
    let marker_style = if selected {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };

    // Line 1: marker · state dot · prompt … state label pinned to the right
    // edge, so the states line up into a column that can be read down.
    let label = row.state.as_str();
    let prompt_budget = width.saturating_sub(3).saturating_sub(label.len() + 2);
    let prompt = truncate_end(&row.prompt, prompt_budget);
    let used = 3 + prompt.chars().count() + label.len();
    let pad = width.saturating_sub(used).max(2);
    let line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled("\u{25cf} ", Style::default().fg(color)),
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
    let mut meta = format!("{} · {}", row.repo_label, row.runtime.as_str());
    if row.use_worktree {
        meta.push_str(" · worktree");
    }
    if row.dispatched {
        meta.push_str(" · dispatched");
    }
    meta.push_str(&format!(
        " · {}",
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
        t.set_activity_line(Some("? Do you want to make this edit to webhook.ts".into()));
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
            t.set_activity_line(Some(
                [
                    "Metamorphosing… (3s · thinking)",
                    "waiting for your approval",
                    "done — 4 files changed",
                ][i]
                    .into(),
            ));
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
        terminal.set_activity_line(Some("running the migration".into()));
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
        assert!(context_gauge(0).starts_with("\u{2591}"));
        assert_eq!(
            context_gauge(100),
            "\u{2588}\u{2588}\u{2588}\u{2588}\u{2588}\u{2588} 100%"
        );
        // Any nonzero percentage shows at least one filled cell, so a barely
        // used context is still visibly distinct from an unknown one.
        assert!(context_gauge(1).starts_with("\u{2588}"));
        // Only a full context fills the bar: a nearly-full agent must stay
        // visually distinguishable from a finished one.
        assert_ne!(context_gauge(88), context_gauge(100));
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
