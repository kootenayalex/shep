//! Session board overlay: two full-screen lane boards one key apart.
//!
//! The docket board answers "what is owed" — inbox, due, slated, recurring,
//! done — one card per docket item, drawn from the rows sampled into
//! `AppState::docket_sample`. The agent board answers "what are all my agents
//! doing right now": one lane per group, one card per agent pane. On narrow
//! terminals either collapses to a single stacked list, lane by lane.
//!
//! Everything here is pure TUI presentation: the model, geometry, selection
//! traversal, and enter-focus resolution are computed from `&AppState` so they
//! are unit-testable, and `render` only draws (no state mutation). Board
//! ordering reuses `agent_panel_entries` and `crate::workspace::attention_priority`
//! so it agrees with the sidebar.

use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::glyphs;
use super::sidebar::{agent_panel_entries, format_event_age};
use super::status::{agent_icon_for, docket_appearance, state_label, DocketUrgency};
use super::text::{display_width, truncate_end, truncate_start};
use super::widgets::render_panel_shell;
use crate::api::schema::{DocketKind, DocketRepeat, DocketStatus};
use crate::app::state::{AppState, BoardView, DocketSample, Palette};
use crate::detect::AgentState;
use crate::docket::dates::Date;
use crate::layout::PaneId;

/// Visible rows per agent card: agent line, branch/age line, status line, the
/// session's own summary, then where it is working plus the context gauge.
/// Cards are laid out with a one-row gap between them.
const CARD_ROWS: u16 = 5;

/// The card's left gutter: selection marker, state glyph, and the space after
/// it. Every line below the first indents to here, so the glyph hangs in the
/// margin and the rest of the card is one text column. They used to indent by
/// two, which aligned them with the gap between the glyph and the name —
/// under nothing at all.
const CARD_INDENT: usize = 3;

/// The least room the working directory gets on the card's fact line before it
/// yields to the facts after it. `~/…/shep` still names the repo.
const MIN_CWD_WIDTH: usize = 12;

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

/// Which tally an agent belongs to, for [`BoardSummary`]. `done` is
/// idle-and-unseen, `idle` is idle-and-seen, and an unknown state counts as
/// idle so no agent goes uncounted.
///
/// This used to be the board's *axis*. It is now only arithmetic: the lanes are
/// the user's own groups, and state is carried by the card's glyph and colour.
fn summary_bucket(state: AgentState, seen: bool) -> usize {
    match (state, seen) {
        (AgentState::Blocked, _) => 0,
        (AgentState::Idle, false) => 1,
        (AgentState::Working, _) => 2,
        (AgentState::Idle, true) | (AgentState::Unknown, _) => 3,
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
    /// Whether this agent answers to a name of its own rather than to its
    /// tool's — an explicit shep rename, or the name the agent published for
    /// itself. A named agent is never decorated with placement it did not ask
    /// for. A pane whose `agent_name` is just the tool's own label ("claude")
    /// is not named: that is the collision the decoration exists to resolve.
    pub named: bool,
    /// Last line of real screen content; "what is it saying right now".
    pub activity: Option<String>,
    /// What the agent says about its own session — a brief title, and the
    /// permission mode and churn it is running under. Sourced from the agent's
    /// own session file rather than scraped off its screen; `None` for an agent
    /// with no session-facts manifest.
    pub summary: Option<String>,
    pub permission_mode: Option<String>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
    /// The last few of them, in reading order, for a surface with the room.
    pub activity_lines: Vec<String>,
    sort_seq: Option<u64>,
}

/// One lane of the board: a group, and the agents in it.
///
/// An empty group keeps its lane. A lane you are about to fill should not
/// vanish, and a group disappearing when its last agent exits reads as the
/// group having been closed.
#[derive(Debug, Clone)]
pub(crate) struct BoardLane {
    pub ws_idx: usize,
    pub title: String,
    pub cards: Vec<BoardCard>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct BoardModel {
    /// One lane per group, in the session's own group order.
    pub lanes: Vec<BoardLane>,
}

impl BoardModel {
    /// Flattened card order for narrow/stacked traversal and rendering: lane
    /// order, each lane already sorted.
    pub(crate) fn flattened(&self) -> Vec<&BoardCard> {
        self.lanes
            .iter()
            .flat_map(|lane| lane.cards.iter())
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lanes.iter().all(|lane| lane.cards.is_empty())
    }

    fn cards(&self, lane: usize) -> &[BoardCard] {
        self.lanes
            .get(lane)
            .map(|lane| lane.cards.as_slice())
            .unwrap_or(&[])
    }

    /// `(lane, row)` of a pane's card.
    fn locate(&self, pane_id: PaneId) -> Option<(usize, usize)> {
        for (idx, lane) in self.lanes.iter().enumerate() {
            if let Some(row) = lane.cards.iter().position(|card| card.pane_id == pane_id) {
                return Some((idx, row));
            }
        }
        None
    }

    /// The lane holding `pane_id`, for painting a lane header as focused.
    fn lane_of(&self, pane_id: Option<PaneId>) -> Option<usize> {
        self.locate(pane_id?).map(|(lane, _)| lane)
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

/// Build the board model from the same agent entries the sidebar tree uses:
/// one lane per group, in session order, each lane sorted by attention priority
/// (then most-recent state change), so ordering agrees with the sidebar's
/// priority sort.
pub(crate) fn board_model(app: &AppState) -> BoardModel {
    let mut model = BoardModel {
        lanes: app
            .workspaces
            .iter()
            .enumerate()
            .map(|(ws_idx, ws)| BoardLane {
                ws_idx,
                title: ws.display_name(),
                cards: Vec::new(),
            })
            .collect(),
    };
    for entry in agent_panel_entries(app) {
        let Some(lane) = model
            .lanes
            .iter()
            .position(|lane| lane.ws_idx == entry.ws_idx)
        else {
            continue;
        };
        let ws = app.workspaces.get(entry.ws_idx);
        let branch = ws.and_then(|ws| ws.branch());
        let pane_number = ws.and_then(|ws| ws.public_pane_number(entry.pane_id));
        let location = location_label(app, entry.ws_idx, entry.tab_idx, pane_number);
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
        let facts = terminal
            .map(|terminal| terminal.session_facts.clone())
            .unwrap_or_default();
        let agent_label = entry.agent_label.unwrap_or_else(|| "agent".to_string());
        model.lanes[lane].cards.push(BoardCard {
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
            named: terminal.is_some_and(|terminal| {
                terminal
                    .agent_name
                    .as_deref()
                    .is_some_and(|name| Some(name) != terminal.effective_agent_label())
            }),
            activity,
            activity_lines,
            // The agent's own line about what it is doing beats the session
            // title, which names the hour rather than the minute.
            summary: facts.summary.or(facts.title),
            permission_mode: facts.permission_mode,
            cost_usd: facts.cost_usd,
            lines_added: facts.lines_added,
            lines_removed: facts.lines_removed,
            sort_seq: entry.last_agent_state_change_seq,
        });
    }
    for lane in &mut model.lanes {
        lane.cards.sort_by_key(|card| {
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
    // A named agent is already the answer to "which one is this", so it keeps
    // its name whole. Only the ones still called after their tool need placement
    // spent on them.
    let candidates: Vec<(PaneId, Vec<String>)> = model
        .flattened()
        .iter()
        .filter(|card| !card.named)
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

    for lane in &mut model.lanes {
        for card in lane.cards.iter_mut() {
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
    let lanes: Vec<Vec<PaneId>> = model
        .lanes
        .iter()
        .map(|lane| lane.cards.iter().map(|card| card.pane_id).collect())
        .collect();
    step_selection(&lanes, current, dir, narrow)
}

/// The traversal both boards share, over lanes of card keys.
///
/// Wide grid: up/down move within a lane with wraparound; left/right jump to
/// the nearest non-empty lane in that direction (no horizontal wraparound),
/// clamping the row. Narrow stacked list: up/down traverse the flattened
/// order with wraparound; left/right are no-ops. When nothing valid is
/// selected, the first card is chosen; when there are no cards, `None`.
fn step_selection<K: Copy + PartialEq>(
    lanes: &[Vec<K>],
    current: Option<K>,
    dir: BoardDir,
    narrow: bool,
) -> Option<K> {
    let flat: Vec<K> = lanes.iter().flatten().copied().collect();
    if flat.is_empty() {
        return None;
    }
    let locate = |key: K| -> Option<(usize, usize)> {
        lanes.iter().enumerate().find_map(|(lane, keys)| {
            keys.iter()
                .position(|candidate| *candidate == key)
                .map(|row| (lane, row))
        })
    };
    let Some(current) = current.filter(|key| locate(*key).is_some()) else {
        return flat.first().copied();
    };

    if narrow {
        let idx = flat.iter().position(|key| *key == current)?;
        let next = match dir {
            BoardDir::Up => (idx + flat.len() - 1) % flat.len(),
            BoardDir::Down => (idx + 1) % flat.len(),
            BoardDir::Left | BoardDir::Right => return Some(current),
        };
        return flat.get(next).copied();
    }

    let (lane, row) = locate(current)?;
    match dir {
        BoardDir::Up | BoardDir::Down => {
            let len = lanes[lane].len();
            if len == 0 {
                return Some(current);
            }
            let next_row = match dir {
                BoardDir::Up => (row + len - 1) % len,
                _ => (row + 1) % len,
            };
            lanes[lane].get(next_row).copied()
        }
        BoardDir::Left | BoardDir::Right => {
            // Empty lanes are drawn but not stopped at: there is nothing
            // there to select.
            let target = match dir {
                BoardDir::Left => (0..lane).rev().find(|idx| !lanes[*idx].is_empty()),
                _ => ((lane + 1)..lanes.len()).find(|idx| !lanes[*idx].is_empty()),
            };
            let Some(target) = target else {
                return Some(current);
            };
            let clamped = row.min(lanes[target].len().saturating_sub(1));
            lanes[target].get(clamped).copied()
        }
    }
}

/// Resolve the selected card to the `(workspace index, pane)` to focus on Enter.
pub(crate) fn enter_target(app: &AppState, selected: Option<PaneId>) -> Option<(usize, PaneId)> {
    let model = board_model(app);
    let pane = selected?;
    let (lane, row) = model.locate(pane)?;
    let card = model.cards(lane).get(row)?;
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
/// Two conditions, because the board is as many lanes wide as there are groups
/// where the rest of the app is one: it stacks when the terminal is
/// phone-narrow *or* when dividing it by the live lane count would leave lanes
/// too thin to read. The threshold moves with the number of groups — six groups
/// stack on a terminal where two would not.
pub(crate) fn is_narrow_with_lanes(app: &AppState, lanes: usize) -> bool {
    let area = board_area(app);
    if let Some(inner) = inner_area(area) {
        if inner.width / (lanes.max(1) as u16) < MIN_CARD_WIDTH {
            return true;
        }
    }
    super::mobile::is_mobile_width(area, app.mobile_width_threshold)
}

pub(crate) fn is_narrow(app: &AppState) -> bool {
    is_narrow_with_lanes(app, board_model(app).lanes.len())
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

/// Divide the body evenly between the lanes, giving the leftmost lanes the
/// remainder so the board fills its width exactly.
fn lane_rects(body: Rect, lanes: usize) -> Vec<Rect> {
    if lanes == 0 {
        return Vec::new();
    }
    let count = lanes as u16;
    let base = body.width / count;
    let extra = body.width % count;
    let mut rects = Vec::with_capacity(lanes);
    let mut x = body.x;
    for i in 0..count {
        let w = base + if i < extra { 1 } else { 0 };
        rects.push(Rect::new(x, body.y, w, body.height));
        x = x.saturating_add(w);
    }
    rects
}

#[derive(Clone, Copy)]
struct CardSlot {
    rect: Rect,
    lane: usize,
    row: usize,
}

/// Every agent card is [`CARD_ROWS`] tall.
fn agent_card_heights(model: &BoardModel) -> Vec<Vec<u16>> {
    model
        .lanes
        .iter()
        .map(|lane| vec![CARD_ROWS; lane.cards.len()])
        .collect()
}

fn wide_slots(model: &BoardModel, body: Rect) -> Vec<CardSlot> {
    wide_slots_for(&agent_card_heights(model), body)
}

fn narrow_slots(model: &BoardModel, body: Rect) -> (Vec<CardSlot>, Vec<(u16, usize)>) {
    narrow_slots_for(&agent_card_heights(model), body)
}

/// Card slots for the wide layout, from each lane's card heights. A card that
/// does not fit whole below the last one is not drawn at all: half a card
/// reads as a clipped card, not as "more below".
fn wide_slots_for(heights: &[Vec<u16>], body: Rect) -> Vec<CardSlot> {
    let rects = lane_rects(body, heights.len());
    let mut slots = Vec::new();
    for (lane, lane_rect) in rects.iter().enumerate() {
        // One header row + one gap row before the first card.
        let mut y = lane_rect.y.saturating_add(2);
        let bottom = lane_rect.y.saturating_add(lane_rect.height);
        for (row, height) in heights[lane].iter().copied().enumerate() {
            if y.saturating_add(height) > bottom {
                break;
            }
            slots.push(CardSlot {
                rect: Rect::new(lane_rect.x, y, lane_rect.width, height),
                lane,
                row,
            });
            // One gap row between cards.
            y = y.saturating_add(height).saturating_add(1);
        }
    }
    slots
}

/// Card slots plus `(y, lane)` header positions for the stacked layout. An
/// empty lane draws no section header here: stacked, a heading with nothing
/// under it is just a lost row.
fn narrow_slots_for(heights: &[Vec<u16>], body: Rect) -> (Vec<CardSlot>, Vec<(u16, usize)>) {
    let mut slots = Vec::new();
    let mut headers = Vec::new();
    let bottom = body.y + body.height;
    let mut y = body.y;
    for (lane, lane_heights) in heights.iter().enumerate() {
        // A heading whose first card would not fit under it is a heading
        // over nothing, and `due 2 !1` over an empty row reads as a bug.
        let first = lane_heights.first().copied().unwrap_or(0);
        if lane_heights.is_empty() || y.saturating_add(1).saturating_add(first) > bottom {
            continue;
        }
        headers.push((y, lane));
        y = y.saturating_add(1);
        for (row, height) in lane_heights.iter().copied().enumerate() {
            if y.saturating_add(height) > bottom {
                break;
            }
            slots.push(CardSlot {
                rect: Rect::new(body.x, y, body.width, height),
                lane,
                row,
            });
            y = y.saturating_add(height).saturating_add(1);
        }
        // The gap after a lane's last card is the gap before the next
        // heading. A second blank row spent here was the row that kept the
        // due lane's second card off an 80×24 screen.
    }
    (slots, headers)
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.x + rect.width && row >= rect.y && row < rect.y + rect.height
}

fn slots_for(app: &AppState, model: &BoardModel) -> Option<(Rect, Vec<CardSlot>)> {
    let inner = inner_area(board_area(app))?;
    let body = board_body(inner);
    let slots = if is_narrow_with_lanes(app, model.lanes.len()) {
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
    let card = model.cards(slot.lane).get(slot.row)?;
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
    let mut counts = [0usize; 4];
    for card in model.flattened() {
        counts[summary_bucket(card.state, card.seen)] += 1;
    }
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

fn render_dashboard(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    summary: &BoardSummary,
    docket: &DocketBoardModel,
) {
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
    // The docket, once it has been read: what is due is a call to action of
    // its own, so it warms up when the number is not zero. On the docket
    // board it is the fact worth keeping when the strip runs out of room, so
    // it rides right behind the head count there; on the agent lanes it
    // follows the agent states.
    if app.docket_sample.sampled {
        let due = docket.due_count();
        let due_style = if due > 0 {
            Style::default().fg(p.peach)
        } else {
            value
        };
        let fact = vec![
            Span::styled("docket ", dim),
            Span::styled(due.to_string(), due_style),
            Span::styled(" due ", dim),
            Span::styled(glyphs::SEP, Style::default().fg(p.surface1)),
            Span::styled(format!(" {}", docket.inbox_count()), value),
            Span::styled(" inbox", dim),
        ];
        if app.board.view == BoardView::Docket {
            facts.insert(1, fact);
        } else {
            facts.push(fact);
        }
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

    // A detail screen replaces the dashboard and lanes entirely; it keeps
    // only the panel shell, title, and footer so the board stays recognisable.
    let docket = docket_board_model(&app.docket_sample);
    match app.board.view {
        BoardView::Agent => {
            let body = detail_body(inner);
            render_agent_detail(app, terminal_runtimes, frame, &model, body);
            return;
        }
        BoardView::DocketItem => {
            let body = detail_body(inner);
            render_docket_detail(app, frame, &docket, body);
            return;
        }
        BoardView::Columns | BoardView::Docket => {}
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
            &docket,
        );
    }

    if body.height == 0 || body.width == 0 {
        return;
    }
    if app.board.view == BoardView::Docket {
        render_docket_lanes(app, frame, &docket, body);
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
        BoardView::Docket => Line::from(vec![
            Span::styled(" docket ", title),
            Span::styled(format!("{} what needs doing", glyphs::SEP), dim),
        ]),
        BoardView::DocketItem => Line::from(vec![
            Span::styled(" docket ", dim),
            Span::styled("/ ", dim),
            Span::styled("item", title),
        ]),
        BoardView::Columns => Line::from(vec![
            Span::styled(" session board ", title),
            Span::styled(format!("{} what are my agents doing", glyphs::SEP), dim),
        ]),
        BoardView::Agent => Line::from(vec![
            Span::styled(" session board ", dim),
            Span::styled("/ ", dim),
            Span::styled("agent", title),
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
        // The docket's verbs are its keys; the arrows are left unsaid here
        // because at 80 columns the row has room for the verbs or the
        // arrows, and the verbs are the ones a person cannot guess.
        BoardView::Docket => &[
            ("i", " inspect  "),
            ("n", " new  "),
            ("p", " slate  "),
            ("r", " recur  "),
            ("d", " done  "),
            ("x", " discard  "),
            ("a", " agents  "),
            ("esc", " close"),
        ],
        BoardView::DocketItem => &[
            ("p", " slate  "),
            ("r", " recur  "),
            ("d", " done  "),
            ("x", " discard  "),
            ("esc", " back to docket"),
        ],
        BoardView::Columns => &[
            ("enter", " focus  "),
            ("i", " inspect  "),
            (glyphs::KEYS_ARROWS, " move  "),
            ("<>", " move group  "),
            ("a", " docket  "),
            ("esc/q", " close"),
        ],
        BoardView::Agent => &[("enter", " attach  "), ("esc/q", " back to board")],
    };
    let mut spans = vec![Span::raw(" ")];
    for (k, label) in hints {
        spans.push(Span::styled(*k, key));
        spans.push(Span::styled(*label, dim));
    }
    // A refused docket verb says why, in the store's words, where the eye
    // already is. It rides the footer's right edge and is gone on the next key.
    if let Some(notice) = app.board.docket_notice.as_deref() {
        let used: usize = spans.iter().map(|s| display_width(&s.content)).sum();
        let room = (area.width as usize).saturating_sub(used + 2);
        let text = truncate_end(&format!("! {notice}"), room);
        let pad = (area.width as usize)
            .saturating_sub(used)
            .saturating_sub(display_width(&text) + 1);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(text, Style::default().fg(p.peach)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

/// A lane heading: the group's name and how many agents are in it.
///
/// `accent` marks the lane holding the selection — focus, never a state. A lane
/// is a group now, so painting it a state colour would be a claim about the
/// group that is not true of the cards inside it.
fn render_lane_header(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    title: &str,
    count: usize,
    focused: bool,
) {
    let p = &app.palette;
    let color = if focused { p.accent } else { p.overlay0 };
    let title_width =
        area.width
            .saturating_sub(display_width(&format!(" {count}")) as u16 + 1) as usize;
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", truncate_end(title, title_width)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(format!(" {count}"), Style::default().fg(p.overlay0)),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

fn render_wide(app: &AppState, frame: &mut Frame, model: &BoardModel, body: Rect) {
    let rects = lane_rects(body, model.lanes.len());
    let slots = wide_slots(model, body);
    let focused = model.lane_of(app.board.selected);
    for (idx, lane_rect) in rects.iter().enumerate() {
        let Some(lane) = model.lanes.get(idx) else {
            continue;
        };
        render_lane_header(
            app,
            frame,
            Rect::new(lane_rect.x, lane_rect.y, lane_rect.width, 1),
            &lane.title,
            lane.cards.len(),
            focused == Some(idx),
        );
    }
    for slot in slots {
        if let Some(card) = model.cards(slot.lane).get(slot.row) {
            let selected = app.board.selected == Some(card.pane_id);
            render_card(app, frame, slot.rect, card, selected);
        }
    }
}

fn render_narrow(app: &AppState, frame: &mut Frame, model: &BoardModel, body: Rect) {
    let (slots, headers) = narrow_slots(model, body);
    let focused = model.lane_of(app.board.selected);
    for (y, idx) in headers {
        let Some(lane) = model.lanes.get(idx) else {
            continue;
        };
        render_lane_header(
            app,
            frame,
            Rect::new(body.x, y, body.width, 1),
            &lane.title,
            lane.cards.len(),
            focused == Some(idx),
        );
    }
    for slot in slots {
        if let Some(card) = model.cards(slot.lane).get(slot.row) {
            let selected = app.board.selected == Some(card.pane_id);
            render_card(app, frame, slot.rect, card, selected);
        }
    }
}

/// How an agent's permission mode reads on a card, when it is worth a glance.
///
/// `plan` and `bypassPermissions` are the two that change what the agent may do
/// to a repo without asking; the ordinary modes say nothing and draw nothing.
/// The ink comes from the existing tiers: mauve for plan, peach for bypass — a
/// warning, not a stop, so red stays for blocked.
fn permission_mode_badge(mode: Option<&str>, p: &Palette) -> Option<(&'static str, Color)> {
    match mode? {
        "plan" => Some(("plan", p.mauve)),
        "bypassPermissions" => Some(("bypass", p.peach)),
        _ => None,
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
    // Line 2: branch … age, pinned right.
    //
    // The group is the lane's own heading now, so repeating it on every card in
    // that lane spends width saying what the column already says.
    let age = card_age(app, card).unwrap_or_default();
    let age_width = display_width(&age);
    let meta = card.branch.clone().unwrap_or_default();
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
    // Line 4: what the agent says this session is about, falling back to what
    // its screen is saying when it publishes no title. Italic and dim either
    // way: it is a hint, now a sourced one.
    if let Some(activity) = card.summary.as_ref().or(card.activity.as_ref()) {
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
    // Line 5: where it is working, how it is running, what it has changed …
    // context gauge, pinned right so the gauges stack into a column that can be
    // read down for the one about to fill.
    //
    // The facts go through `fit_strip`, so a narrow card drops whole facts from
    // the right rather than clipping one mid-word. Cost is last because it is
    // the one worth least at a glance.
    let gauge = card
        .context_percent
        .map(|percent| context_gauge_spans(percent, p))
        .unwrap_or_default();
    let gauge_width = display_width(&spans_text(&gauge));
    let strip_budget = content
        .saturating_sub(CARD_INDENT)
        .saturating_sub(if gauge_width == 0 { 0 } else { gauge_width + 1 });
    let mut facts: Vec<Vec<Span>> = Vec::new();
    if let Some((label, color)) = permission_mode_badge(card.permission_mode.as_deref(), p) {
        facts.push(vec![Span::styled(label, Style::default().fg(color))]);
    }
    if let (Some(added), Some(removed)) = (card.lines_added, card.lines_removed) {
        if added > 0 || removed > 0 {
            facts.push(vec![
                Span::styled(format!("+{added}"), Style::default().fg(p.green)),
                Span::styled(format!("/-{removed}"), Style::default().fg(p.red)),
            ]);
        }
    }
    if let Some(cost) = card.cost_usd.filter(|cost| *cost > 0.0) {
        facts.push(vec![Span::styled(format!("${cost:.2}"), dim)]);
    }
    // The path leads, but it does not get to eat the line: it is truncated to
    // whatever the other facts leave, down to a floor that still shows the last
    // directory. Otherwise a deep path silently pushed every sourced fact off
    // the card.
    let trailing: usize = facts
        .iter()
        .map(|fact| {
            fact.iter()
                .map(|span| display_width(&span.content))
                .sum::<usize>()
                + display_width(glyphs::SEP_SPACED)
        })
        .sum();
    if let Some(cwd) = card.cwd.as_ref().filter(|cwd| !cwd.is_empty()) {
        let cwd_budget = strip_budget
            .saturating_sub(trailing)
            .max(MIN_CWD_WIDTH.min(strip_budget));
        facts.insert(0, vec![Span::styled(truncate_start(cwd, cwd_budget), dim)]);
    }
    let strip = fit_strip(facts, &Span::styled(glyphs::SEP_SPACED, dim), strip_budget);
    used = CARD_INDENT + display_width(&spans_text(&strip));
    let mut line5 = vec![Span::raw(indent)];
    line5.extend(strip);
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
    // The heading is already the agent's name; a location that only repeats it
    // (a tab named after its one agent) says nothing twice.
    if !card.location.is_empty() && !card.location.eq_ignore_ascii_case(&card.display_name) {
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

// ---------------------------------------------------------------------------
// The docket board
// ---------------------------------------------------------------------------

/// The docket's lanes, in reading order: what needs deciding, what is due,
/// then the two shapes of "later", then what is finished.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocketLane {
    /// Captured items waiting to be promoted or discarded.
    Inbox,
    /// Open items due today or earlier — overdue first, by construction of
    /// the store's order.
    Due,
    /// Open one-off items dated later, or undated.
    Slated,
    /// Open recurring items not due yet.
    Recurring,
    /// The last few completed items, newest first.
    Done,
}

impl DocketLane {
    pub(crate) const ALL: [DocketLane; 5] = [
        DocketLane::Inbox,
        DocketLane::Due,
        DocketLane::Slated,
        DocketLane::Recurring,
        DocketLane::Done,
    ];

    pub(crate) fn title(self) -> &'static str {
        match self {
            DocketLane::Inbox => "inbox",
            DocketLane::Due => "due",
            DocketLane::Slated => "slated",
            DocketLane::Recurring => "recurring",
            DocketLane::Done => "done",
        }
    }
}

/// A stacked docket card: id and title, kind and date.
const COMPACT_CARD_ROWS: u16 = 2;

/// How many finished items the done lane keeps. The lane is a receipt, not an
/// archive: `shep docket list` has the rest.
const DONE_LANE_LIMIT: usize = 10;

/// A docket item's date, relative to today, as the card says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DueLabel {
    /// `due < today`, by this many days.
    Overdue(i64),
    Today,
    /// `due > today`, by this many days.
    In(i64),
    /// No date.
    Undated,
}

impl DueLabel {
    /// `overdue 3d` / `due today` / `in 5d` / `—`. Overdue reads as a count of
    /// days late rather than a date: "2026-09-08" is a fact, "overdue 3d" is
    /// the complaint.
    pub(crate) fn text(self) -> String {
        match self {
            DueLabel::Overdue(days) => format!("overdue {days}d"),
            DueLabel::Today => "due today".to_string(),
            DueLabel::In(days) => format!("in {days}d"),
            DueLabel::Undated => glyphs::DASH.to_string(),
        }
    }

    fn urgency(self) -> DocketUrgency {
        match self {
            DueLabel::Overdue(_) => DocketUrgency::Overdue,
            DueLabel::Today => DocketUrgency::Today,
            DueLabel::In(_) | DueLabel::Undated => DocketUrgency::Later,
        }
    }
}

/// Where a date stands against today. An unparseable date is undated rather
/// than a guess; `today` unknown (never sampled) means nothing is ever late.
pub(crate) fn due_label(due: Option<&str>, today: Option<Date>) -> DueLabel {
    let (Some(due), Some(today)) = (due.and_then(Date::parse), today) else {
        return DueLabel::Undated;
    };
    let delta = due.to_days() - today.to_days();
    if delta < 0 {
        DueLabel::Overdue(-delta)
    } else if delta == 0 {
        DueLabel::Today
    } else {
        DueLabel::In(delta)
    }
}

/// One docket item as a card draws it: every field already reduced to the
/// string the card shows, so render does no parsing.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct DocketCard {
    pub id: i64,
    pub title: String,
    pub kind: DocketKind,
    pub status: DocketStatus,
    pub due: DueLabel,
    /// The date itself, for the detail screen.
    pub due_date: Option<String>,
    pub repeat: Option<DocketRepeat>,
    /// Provenance reduced to one short tag: `board.rs:42` for a file, `pane
    /// p3` for a session. `None` when the source says nothing a card can use.
    pub source: Option<String>,
    /// The same provenance unabridged, for the detail screen.
    pub source_full: Option<String>,
    pub notes: Option<String>,
}

impl DocketCard {
    /// Only an open item is overdue: the inbox has no date yet and done has
    /// no date any more.
    pub(crate) fn overdue(&self) -> bool {
        self.status == DocketStatus::Open && matches!(self.due, DueLabel::Overdue(_))
    }

    fn urgency(&self) -> DocketUrgency {
        if self.status == DocketStatus::Open {
            self.due.urgency()
        } else {
            DocketUrgency::Later
        }
    }

    /// The first line of the notes, for the card.
    fn notes_line(&self) -> Option<&str> {
        self.notes
            .as_deref()?
            .lines()
            .map(str::trim)
            .find(|line| !line.is_empty())
    }

    /// Rows the card draws: id and title, kind and date, then a source row and
    /// a notes row only when there is one. A card says what it has to say and
    /// no more, so a bare inbox item is two rows and a sourced, annotated one
    /// is four.
    pub(crate) fn rows(&self) -> u16 {
        2 + u16::from(self.source.is_some()) + u16::from(self.notes_line().is_some())
    }
}

/// `{"file": "/a/b/memory.md", "line": 12}` -> `(memory.md:12, ~/a/b/memory.md:12)`;
/// `{"pane": "p3", "session": "s"}` -> `(pane p3, pane p3 · session s)`.
/// Anything else is not worth a row.
fn source_tags(source: Option<&serde_json::Value>) -> Option<(String, String)> {
    let source = source?;
    let json_text = |value: &serde_json::Value| match value {
        serde_json::Value::String(text) => text.clone(),
        other => other.to_string(),
    };
    if let Some(file) = source.get("file").and_then(serde_json::Value::as_str) {
        let base = std::path::Path::new(file)
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(file);
        let full = contract_home(std::path::Path::new(file));
        return Some(
            match source.get("line").and_then(serde_json::Value::as_i64) {
                Some(line) => (format!("{base}:{line}"), format!("{full}:{line}")),
                None => (base.to_string(), full),
            },
        );
    }
    if let Some(pane) = source.get("pane") {
        let short = format!("pane {}", json_text(pane));
        let full = match source.get("session") {
            Some(session) => format!(
                "{short}{}session {}",
                glyphs::SEP_SPACED,
                json_text(session)
            ),
            None => short.clone(),
        };
        return Some((short, full));
    }
    None
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct DocketBoardModel {
    /// One lane per [`DocketLane`], in that order, empty lanes kept.
    pub lanes: Vec<(DocketLane, Vec<DocketCard>)>,
}

impl DocketBoardModel {
    pub(crate) fn flattened(&self) -> Vec<&DocketCard> {
        self.lanes
            .iter()
            .flat_map(|(_, cards)| cards.iter())
            .collect()
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.lanes.iter().all(|(_, cards)| cards.is_empty())
    }

    fn cards(&self, lane: usize) -> &[DocketCard] {
        self.lanes
            .get(lane)
            .map(|(_, cards)| cards.as_slice())
            .unwrap_or(&[])
    }

    /// The cards of one lane, by name.
    pub(crate) fn lane(&self, which: DocketLane) -> &[DocketCard] {
        self.lanes
            .iter()
            .find(|(lane, _)| *lane == which)
            .map(|(_, cards)| cards.as_slice())
            .unwrap_or(&[])
    }

    /// `(lane, row)` of an item's card.
    pub(crate) fn locate(&self, id: i64) -> Option<(usize, usize)> {
        for (idx, (_, cards)) in self.lanes.iter().enumerate() {
            if let Some(row) = cards.iter().position(|card| card.id == id) {
                return Some((idx, row));
            }
        }
        None
    }

    fn lane_of(&self, id: Option<i64>) -> Option<usize> {
        self.locate(id?).map(|(lane, _)| lane)
    }

    /// The card the board treats as selected: the selection when it still
    /// resolves, else the first card. Seeded lazily this way so opening the
    /// board never has to read the store to pick something.
    pub(crate) fn effective_selection(&self, selected: Option<i64>) -> Option<i64> {
        selected
            .filter(|id| self.locate(*id).is_some())
            .or_else(|| self.flattened().first().map(|card| card.id))
    }

    /// Each card's height, per lane. Stacked, every card is its two leading
    /// rows: an 80×24 terminal showed three inbox cards and nothing else,
    /// with the due lane — the one that is a complaint — below the fold.
    /// Two rows apiece gets the inbox and the due lane on one small screen,
    /// and the detail screen has the rest.
    fn card_heights(&self, narrow: bool) -> Vec<Vec<u16>> {
        self.lanes
            .iter()
            .map(|(_, cards)| {
                cards
                    .iter()
                    .map(|card| {
                        if narrow {
                            COMPACT_CARD_ROWS
                        } else {
                            card.rows()
                        }
                    })
                    .collect()
            })
            .collect()
    }

    fn keys(&self) -> Vec<Vec<i64>> {
        self.lanes
            .iter()
            .map(|(_, cards)| cards.iter().map(|card| card.id).collect())
            .collect()
    }

    /// Open items due today or earlier, and inbox items: the two numbers the
    /// dashboard strip reports.
    pub(crate) fn due_count(&self) -> usize {
        self.lane(DocketLane::Due).len()
    }

    pub(crate) fn inbox_count(&self) -> usize {
        self.lane(DocketLane::Inbox).len()
    }
}

/// Bucket the sampled rows into lanes. Pure: the sample carries the store's
/// own "today", so the same rows always make the same board.
///
/// Rows keep the store's order inside a lane (dated open items by due, the
/// rest newest-updated first), except `done`, which is re-cut to its newest
/// ten. Discarded items are not on the board at all.
pub(crate) fn docket_board_model(sample: &DocketSample) -> DocketBoardModel {
    let mut lanes: Vec<(DocketLane, Vec<DocketCard>)> = DocketLane::ALL
        .iter()
        .map(|lane| (*lane, Vec::new()))
        .collect();
    let mut done: Vec<(&str, DocketCard)> = Vec::new();
    for item in &sample.rows {
        let due = due_label(item.due.as_deref(), sample.today);
        let (source, source_full) = source_tags(item.source.as_ref()).unzip();
        let card = DocketCard {
            id: item.id,
            title: item.title.clone(),
            kind: item.kind,
            status: item.status,
            due,
            due_date: item.due.clone(),
            repeat: item.repeat,
            source,
            source_full,
            notes: item.notes.clone(),
        };
        let lane = match item.status {
            DocketStatus::Inbox => DocketLane::Inbox,
            DocketStatus::Open => match (due, item.kind) {
                (DueLabel::Overdue(_) | DueLabel::Today, _) => DocketLane::Due,
                (_, DocketKind::Recurring) => DocketLane::Recurring,
                _ => DocketLane::Slated,
            },
            DocketStatus::Done => {
                done.push((item.updated.as_str(), card));
                continue;
            }
            DocketStatus::Discarded => continue,
        };
        lanes[lane as usize].1.push(card);
    }
    done.sort_by(|a, b| b.0.cmp(a.0).then_with(|| b.1.id.cmp(&a.1.id)));
    lanes[DocketLane::Done as usize].1 = done
        .into_iter()
        .take(DONE_LANE_LIMIT)
        .map(|(_, card)| card)
        .collect();
    DocketBoardModel { lanes }
}

/// The docket board's traversal — the agent board's, over item ids.
pub(crate) fn next_docket_selection(
    model: &DocketBoardModel,
    current: Option<i64>,
    dir: BoardDir,
    narrow: bool,
) -> Option<i64> {
    step_selection(&model.keys(), current, dir, narrow)
}

/// Whether the docket board stacks. Five lanes, always: an empty lane keeps
/// its place so the columns do not shuffle as items move between them.
pub(crate) fn is_docket_narrow(app: &AppState) -> bool {
    is_narrow_with_lanes(app, DocketLane::ALL.len())
}

fn docket_slots_for(app: &AppState, model: &DocketBoardModel) -> Vec<CardSlot> {
    let Some(inner) = inner_area(board_area(app)) else {
        return Vec::new();
    };
    let body = board_body(inner);
    let narrow = is_docket_narrow(app);
    let heights = model.card_heights(narrow);
    if narrow {
        narrow_slots_for(&heights, body).0
    } else {
        wide_slots_for(&heights, body)
    }
}

/// The docket item under a click, if any.
pub(crate) fn docket_card_at(app: &AppState, col: u16, row: u16) -> Option<i64> {
    let model = docket_board_model(&app.docket_sample);
    let slot = docket_slots_for(app, &model)
        .into_iter()
        .find(|slot| rect_contains(slot.rect, col, row))?;
    model.cards(slot.lane).get(slot.row).map(|card| card.id)
}

fn render_docket_lanes(app: &AppState, frame: &mut Frame, model: &DocketBoardModel, body: Rect) {
    let p = &app.palette;
    if body.height == 0 || body.width == 0 {
        return;
    }
    if !app.docket_sample.sampled {
        frame.render_widget(
            Paragraph::new(format!(" reading the docket{}", glyphs::ELLIPSIS))
                .style(Style::default().fg(p.overlay0)),
            Rect::new(body.x, body.y, body.width, 1),
        );
        return;
    }
    let selected = model.effective_selection(app.board.docket_selected);
    let focused = model.lane_of(selected);
    let narrow = is_docket_narrow(app);
    let heights = model.card_heights(narrow);
    if narrow {
        if model.is_empty() {
            frame.render_widget(
                Paragraph::new(" nothing on the docket").style(Style::default().fg(p.overlay0)),
                Rect::new(body.x, body.y, body.width, 1),
            );
            return;
        }
        let (slots, headers) = narrow_slots_for(&heights, body);
        for (y, idx) in headers {
            let (lane, cards) = &model.lanes[idx];
            render_docket_lane_header(
                app,
                frame,
                Rect::new(body.x, y, body.width, 1),
                *lane,
                cards,
                focused == Some(idx),
            );
        }
        for slot in slots {
            if let Some(card) = model.cards(slot.lane).get(slot.row) {
                render_docket_card(app, frame, slot.rect, card, selected == Some(card.id));
            }
        }
        return;
    }
    let rects = lane_rects(body, model.lanes.len());
    for (idx, lane_rect) in rects.iter().enumerate() {
        let (lane, cards) = &model.lanes[idx];
        render_docket_lane_header(
            app,
            frame,
            Rect::new(lane_rect.x, lane_rect.y, lane_rect.width, 1),
            *lane,
            cards,
            focused == Some(idx),
        );
    }
    for slot in wide_slots_for(&heights, body) {
        if let Some(card) = model.cards(slot.lane).get(slot.row) {
            render_docket_card(app, frame, slot.rect, card, selected == Some(card.id));
        }
    }
}

/// A docket lane heading. The due lane's count goes peach when any of it is
/// overdue, with the same `!` the cards carry — the one lane heading that
/// makes a claim, because it is the one lane whose contents are a complaint.
fn render_docket_lane_header(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    lane: DocketLane,
    cards: &[DocketCard],
    focused: bool,
) {
    let p = &app.palette;
    let overdue = cards.iter().filter(|card| card.overdue()).count();
    let color = if focused { p.accent } else { p.overlay0 };
    let count = if overdue > 0 {
        format!(" {} !{overdue}", cards.len())
    } else {
        format!(" {}", cards.len())
    };
    let count_style = if overdue > 0 {
        Style::default().fg(p.peach)
    } else {
        Style::default().fg(p.overlay0)
    };
    let title_width = area.width.saturating_sub(display_width(&count) as u16 + 1) as usize;
    let line = Line::from(vec![
        Span::styled(
            format!(" {}", truncate_end(lane.title(), title_width)),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
        Span::styled(count, count_style),
    ]);
    frame.render_widget(Paragraph::new(line), area);
}

/// One docket card. Same skeleton as an agent card — gutter glyph, bold
/// name, indented facts, a right-pinned trailing fact — so the two boards
/// read as one surface with different nouns.
fn render_docket_card(
    app: &AppState,
    frame: &mut Frame,
    rect: Rect,
    card: &DocketCard,
    selected: bool,
) {
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
    let look = docket_appearance(card.status, card.urgency());
    let marker = if selected { glyphs::MARKER } else { " " };
    let marker_style = if selected {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    let title_style = if selected {
        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.text)
    };
    let dim = Style::default().fg(p.overlay0);
    let content = width.saturating_sub(CARD_RIGHT_MARGIN);
    let indent = " ".repeat(CARD_INDENT);

    // Line 1: marker · glyph · #id · title. The id leads because it is what
    // the CLI verbs take (`shep docket done 3`), and because it is short and
    // the title is the thing that elides.
    let id = format!("#{} ", card.id);
    let title_budget = content
        .saturating_sub(CARD_INDENT)
        .saturating_sub(display_width(&id));
    let title = truncate_end(&card.title, title_budget);
    let glyph_style = if card.overdue() {
        look.style(p).add_modifier(Modifier::BOLD)
    } else {
        look.style(p)
    };
    let line1 = vec![
        Span::styled(marker.to_string(), marker_style),
        Span::styled(look.glyph, glyph_style),
        Span::raw(" "),
        Span::styled(id, dim),
        Span::styled(title, title_style),
    ];
    frame.render_widget(
        Paragraph::new(Line::from(line1)),
        Rect::new(rect.x, rect.y, rect.width, 1),
    );
    if rect.height < 2 {
        return;
    }

    // Line 2: kind · due · repeat. The date takes the glyph's ink when it is
    // the reason the glyph is lit; otherwise it is one more dim fact.
    let due_style = match card.urgency() {
        DocketUrgency::Overdue | DocketUrgency::Today => look.style(p),
        DocketUrgency::Later => dim,
    };
    let mut facts: Vec<Vec<Span>> = vec![
        vec![Span::styled(card.kind.as_str(), dim)],
        vec![Span::styled(card.due.text(), due_style)],
    ];
    if let Some(repeat) = card.repeat {
        facts.push(vec![Span::styled(repeat.as_str(), dim)]);
    }
    let strip = fit_strip(
        facts,
        &Span::styled(glyphs::SEP_SPACED, dim),
        content.saturating_sub(CARD_INDENT),
    );
    let mut line2 = vec![Span::raw(indent.clone())];
    line2.extend(strip);
    frame.render_widget(
        Paragraph::new(Line::from(line2)),
        Rect::new(rect.x, rect.y + 1, rect.width, 1),
    );

    let mut y = rect.y + 2;
    if let Some(source) = &card.source {
        if y >= rect.bottom() {
            return;
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "{indent}{}",
                    truncate_end(source, content.saturating_sub(CARD_INDENT))
                ),
                dim,
            ))),
            Rect::new(rect.x, y, rect.width, 1),
        );
        y += 1;
    }
    if let Some(notes) = card.notes_line() {
        if y >= rect.bottom() {
            return;
        }
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(
                    "{indent}{}",
                    truncate_end(notes, content.saturating_sub(CARD_INDENT))
                ),
                Style::default()
                    .fg(p.overlay0)
                    .add_modifier(Modifier::ITALIC),
            ))),
            Rect::new(rect.x, y, rect.width, 1),
        );
    }
}

/// The selected docket card, or the first when the selection no longer
/// resolves (the item can be discarded from under it).
pub(crate) fn docket_detail_card<'a>(
    app: &AppState,
    model: &'a DocketBoardModel,
) -> Option<&'a DocketCard> {
    let id = model.effective_selection(app.board.docket_selected)?;
    model.flattened().into_iter().find(|card| card.id == id)
}

/// One docket item in full: the card's heading, then every field on its own
/// row, then the notes whole. The verbs are in the footer.
fn render_docket_detail(app: &AppState, frame: &mut Frame, model: &DocketBoardModel, body: Rect) {
    if body.width == 0 || body.height == 0 {
        return;
    }
    let p = &app.palette;
    let dim = Style::default().fg(p.overlay0);
    let Some(card) = docket_detail_card(app, model) else {
        frame.render_widget(
            Paragraph::new("no docket item selected").style(dim),
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

    let look = docket_appearance(card.status, card.urgency());
    row(
        frame,
        &mut y,
        Line::from(vec![
            Span::styled(look.glyph, look.style(p)),
            Span::raw(" "),
            Span::styled(
                truncate_end(&card.title, width.saturating_sub(2)),
                Style::default().fg(p.text).add_modifier(Modifier::BOLD),
            ),
        ]),
    );
    row(
        frame,
        &mut y,
        Line::from(Span::styled(
            format!("#{} {} {}", card.id, glyphs::SEP, look.label),
            dim,
        )),
    );
    row(frame, &mut y, Line::from(""));

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
    let text = Style::default().fg(p.text);
    kv(frame, &mut y, "kind", card.kind.as_str().to_string(), text);
    kv(
        frame,
        &mut y,
        "status",
        card.status.as_str().to_string(),
        text,
    );
    let due = match (&card.due_date, card.due) {
        (Some(date), DueLabel::Undated) => date.clone(),
        (Some(date), label) => format!("{date} {} {}", glyphs::SEP, label.text()),
        (None, _) => glyphs::DASH.to_string(),
    };
    let due_style = match card.urgency() {
        DocketUrgency::Overdue | DocketUrgency::Today => look.style(p),
        DocketUrgency::Later => text,
    };
    kv(frame, &mut y, "due", due, due_style);
    kv(
        frame,
        &mut y,
        "repeat",
        card.repeat
            .map(|repeat| repeat.as_str().to_string())
            .unwrap_or_else(|| glyphs::DASH.to_string()),
        text,
    );
    kv(
        frame,
        &mut y,
        "source",
        card.source_full
            .clone()
            .unwrap_or_else(|| glyphs::DASH.to_string()),
        text,
    );
    row(frame, &mut y, Line::from(""));
    row(frame, &mut y, Line::from(Span::styled("notes", dim)));
    match card
        .notes
        .as_deref()
        .filter(|notes| !notes.trim().is_empty())
    {
        Some(notes) => {
            for line in notes.lines() {
                if y >= bottom {
                    break;
                }
                row(
                    frame,
                    &mut y,
                    Line::from(Span::styled(
                        format!("  {}", truncate_end(line, width.saturating_sub(2))),
                        Style::default().fg(p.subtext0),
                    )),
                );
            }
        }
        None => row(
            frame,
            &mut y,
            Line::from(Span::styled(format!("  {}", glyphs::DASH), dim)),
        ),
    }
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
        // These tests are about the agent lanes; the docket opens by default.
        state.board.view = BoardView::Columns;
        set_state(&mut state, 0, 0, first_root, AgentState::Blocked, true);
        set_state(&mut state, 0, 0, first_second, AgentState::Working, true);
        set_state(&mut state, 1, 0, second_root, AgentState::Idle, false);
        (state, vec![first_root, first_second, second_root])
    }

    #[test]
    fn each_lane_is_a_group_holding_its_own_agents() {
        let (state, panes) = board_state();
        let model = board_model(&state);
        assert_eq!(model.lanes.len(), 2, "one lane per group");
        assert_eq!(model.lanes[0].title, "one");
        assert_eq!(model.lanes[1].title, "two");
        // Group one holds both its agents, blocked before working.
        let first: Vec<PaneId> = model.lanes[0].cards.iter().map(|c| c.pane_id).collect();
        assert_eq!(first, vec![panes[0], panes[1]]);
        let second: Vec<PaneId> = model.lanes[1].cards.iter().map(|c| c.pane_id).collect();
        assert_eq!(second, vec![panes[2]]);
    }

    #[test]
    fn a_group_with_no_agents_keeps_its_lane() {
        let (mut state, _) = board_state();
        state.workspaces.push(Workspace::test_new("three"));
        let model = board_model(&state);
        assert_eq!(model.lanes.len(), 3);
        assert_eq!(model.lanes[2].title, "three");
        assert!(model.lanes[2].cards.is_empty());
    }

    #[test]
    fn summary_buckets_agree_with_attention_priority() {
        // Blocked (0) must be strictly more urgent than done (1), done than
        // working (2), working than idle (3): the tally order is the descending
        // attention_priority order, even though it is no longer the board axis.
        let buckets = [
            (AgentState::Blocked, true),
            (AgentState::Idle, false),
            (AgentState::Working, true),
            (AgentState::Idle, true),
        ];
        for pair in buckets.windows(2) {
            let (sa, la) = pair[0];
            let (sb, lb) = pair[1];
            assert_eq!(summary_bucket(sa, la) + 1, summary_bucket(sb, lb));
            assert!(
                crate::workspace::attention_priority(sa, la)
                    > crate::workspace::attention_priority(sb, lb)
            );
        }
    }

    #[test]
    fn within_a_lane_orders_by_attention_then_recency() {
        // Two done panes (idle+unseen) in the same lane, ordered by seq desc.
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
        assert_eq!(model.lanes[0].cards[0].pane_id, second, "higher seq first");
        assert_eq!(model.lanes[0].cards[1].pane_id, root);
    }

    #[test]
    fn unknown_state_agents_still_get_a_card_and_count_as_idle() {
        let ws = Workspace::test_new("one");
        let root = ws.tabs[0].root_pane;
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        set_state(&mut state, 0, 0, root, AgentState::Unknown, true);
        let model = board_model(&state);
        assert_eq!(model.lanes[0].cards.len(), 1);
        assert_eq!(model.lanes[0].cards[0].pane_id, root);
        assert_eq!(board_summary(&state, &model).idle, 1);
    }

    #[test]
    fn wide_left_right_moves_across_lanes_and_stops_at_edges() {
        let (mut state, panes) = board_state();
        // An empty group between two occupied ones is drawn but not landed on.
        state.workspaces.insert(1, Workspace::test_new("empty"));
        let model = board_model(&state);
        assert!(model.lanes[1].cards.is_empty());

        // Group one (lane 0) -> straight past the empty lane to group two.
        let sel = next_selection(&state, Some(panes[0]), BoardDir::Right, false);
        assert_eq!(sel, Some(panes[2]));
        // Nothing occupied further right: stays put.
        let sel = next_selection(&state, sel, BoardDir::Right, false);
        assert_eq!(sel, Some(panes[2]));
        // Back left, landing on the row-clamped card of group one.
        let sel = next_selection(&state, sel, BoardDir::Left, false);
        assert_eq!(sel, Some(panes[0]));
        // Leftmost lane: nothing further left, stays.
        let sel = next_selection(&state, sel, BoardDir::Left, false);
        assert_eq!(sel, Some(panes[0]));
    }

    #[test]
    fn wide_up_down_wraps_within_a_lane() {
        // One lane with three cards; up/down wrap around.
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
        // Lane order by seq desc: a, b, c.
        let model = board_model(&state);
        let order: Vec<PaneId> = model.lanes[0].cards.iter().map(|c| c.pane_id).collect();
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
    fn narrow_up_down_traverses_flattened_lanes_with_wraparound() {
        let (state, panes) = board_state();
        // Flattened order: group one (blocked, working), then group two (done).
        let sel = next_selection(&state, Some(panes[0]), BoardDir::Down, true);
        assert_eq!(sel, Some(panes[1]));
        let sel = next_selection(&state, sel, BoardDir::Down, true);
        assert_eq!(sel, Some(panes[2]));
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
        let card = &model.lanes[0].cards[0];
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
        let docket = docket_board_model(&state.docket_sample);
        term.draw(|frame| {
            render_dashboard(&state, frame, Rect::new(0, 0, 120, 2), &summary, &docket)
        })
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
        let card = &model.lanes[0].cards[0];
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
        let docket = docket_board_model(&state.docket_sample);
        for width in 20u16..=140 {
            let mut terminal = Terminal::new(TestBackend::new(width, 2)).expect("test terminal");
            terminal
                .draw(|frame| {
                    render_dashboard(&state, frame, Rect::new(0, 0, width, 2), &summary, &docket)
                })
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
        // Two groups fit side by side at 80 columns; four do not. The threshold
        // moves with the number of groups, which is the point of lanes.
        state.view.sidebar_rect = standard;
        state.view.terminal_area = standard;
        assert!(!is_narrow(&state), "two lanes fit in 80 columns");
        state.workspaces.push(Workspace::test_new("three"));
        state.workspaces.push(Workspace::test_new("four"));
        assert!(is_narrow(&state), "four 20-column lanes are too thin");
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
        let card = &model.lanes[0].cards[0];
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
        assert_eq!(model.lanes[0].cards[0].location, "p1");

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
            named: false,
            activity: None,
            activity_lines: Vec::new(),
            summary: None,
            permission_mode: None,
            cost_usd: None,
            lines_added: None,
            lines_removed: None,
            sort_seq: None,
        }
    }

    fn names_for(cards: Vec<BoardCard>) -> Vec<String> {
        let mut model = BoardModel {
            lanes: vec![BoardLane {
                ws_idx: 0,
                title: "group".to_string(),
                cards,
            }],
        };
        assign_distinct_names(&mut model);
        model.lanes[0]
            .cards
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

    /// A name someone gave the agent is the answer already; it must not be
    /// decorated with the placement its unnamed neighbours need.
    #[test]
    fn a_named_agent_keeps_its_name_whole() {
        let mut named = named_card(1, "billing", "shep", Some("master"), "p1");
        named.named = true;
        let names = names_for(vec![
            named,
            named_card(2, "claude", "shep", Some("master"), "docs"),
            named_card(3, "claude", "shep", Some("master"), "board"),
        ]);
        assert_eq!(names[0], "billing");
        // The two that are still just "claude" grow detail as before.
        assert_eq!(names[1], "claude · shep · master · docs");
        assert_eq!(names[2], "claude · shep · master · board");
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

    // -----------------------------------------------------------------------
    // The docket board
    // -----------------------------------------------------------------------

    fn docket_state() -> AppState {
        let mut state = AppState::test_new();
        state.docket_sample = DocketSample::test_fixture();
        state.board.view = BoardView::Docket;
        state.mode = crate::app::state::Mode::Board;
        state
    }

    fn lane_ids(model: &DocketBoardModel, lane: DocketLane) -> Vec<i64> {
        model.lane(lane).iter().map(|card| card.id).collect()
    }

    #[test]
    fn docket_lanes_bucket_by_status_then_date_then_kind() {
        let model = docket_board_model(&DocketSample::test_fixture());
        assert_eq!(lane_ids(&model, DocketLane::Inbox), vec![1, 2, 11]);
        // Overdue and due-today both land in `due`, in the store's order.
        assert_eq!(lane_ids(&model, DocketLane::Due), vec![3, 4]);
        // A dated-later or undated one-off is slated; a recurring item not
        // yet due keeps its own lane.
        assert_eq!(lane_ids(&model, DocketLane::Slated), vec![5, 6]);
        assert_eq!(lane_ids(&model, DocketLane::Recurring), vec![7]);
        // Done is newest-updated first; discarded is not on the board.
        assert_eq!(lane_ids(&model, DocketLane::Done), vec![8, 9]);
        assert!(model.locate(10).is_none());
        assert_eq!(model.due_count(), 2);
        assert_eq!(model.inbox_count(), 3);
    }

    #[test]
    fn the_done_lane_keeps_only_the_newest_ten() {
        let mut sample = DocketSample::test_fixture();
        let template = sample.rows[8].clone();
        for n in 0..15 {
            let mut row = template.clone();
            row.id = 100 + n;
            row.updated = format!("2026-08-{:02}T00:00:00Z", 1 + n);
            sample.rows.push(row);
        }
        let done = lane_ids(&docket_board_model(&sample), DocketLane::Done);
        assert_eq!(done.len(), DONE_LANE_LIMIT);
        assert_eq!(&done[..2], &[8, 9], "the fixture's two are the newest");
        assert_eq!(done[2], 114, "then the synthetic rows, newest first");
    }

    #[test]
    fn due_labels_count_days_from_the_samples_today() {
        let today = Date::parse("2026-09-11");
        assert_eq!(due_label(Some("2026-09-08"), today), DueLabel::Overdue(3));
        assert_eq!(due_label(Some("2026-09-11"), today), DueLabel::Today);
        assert_eq!(due_label(Some("2026-09-16"), today), DueLabel::In(5));
        assert_eq!(due_label(None, today), DueLabel::Undated);
        // Garbage dates and an unsampled today are both "no claim", never
        // an overdue by accident.
        assert_eq!(due_label(Some("soon"), today), DueLabel::Undated);
        assert_eq!(due_label(Some("2026-09-08"), None), DueLabel::Undated);
        assert_eq!(DueLabel::Overdue(3).text(), "overdue 3d");
        assert_eq!(DueLabel::Today.text(), "due today");
        assert_eq!(DueLabel::In(5).text(), "in 5d");
        assert_eq!(DueLabel::Undated.text(), glyphs::DASH);
    }

    #[test]
    fn only_an_open_item_is_overdue() {
        let mut sample = DocketSample::test_fixture();
        // A done item with a past date is not late; it is finished.
        sample.rows[8].due = Some("2026-01-01".into());
        let model = docket_board_model(&sample);
        let done = model.lane(DocketLane::Done)[0].clone();
        assert!(!done.overdue());
        assert_eq!(done.due, DueLabel::Overdue(253));
        let late = model.lane(DocketLane::Due)[0].clone();
        assert!(late.overdue());
    }

    #[test]
    fn a_card_draws_only_the_rows_it_has() {
        let model = docket_board_model(&DocketSample::test_fixture());
        let by_id = |id: i64| {
            model
                .flattened()
                .into_iter()
                .find(|card| card.id == id)
                .cloned()
                .expect("fixture card")
        };
        // Source and notes: four rows.
        assert_eq!(by_id(1).rows(), 4);
        assert_eq!(
            by_id(1).source.as_deref(),
            Some("project_hutch_r1_launcher.md:12")
        );
        // A pane source, no notes: three.
        assert_eq!(by_id(2).rows(), 3);
        assert_eq!(by_id(2).source.as_deref(), Some("pane p3"));
        // Nothing but a title and a kind: two.
        assert_eq!(by_id(5).rows(), 2);
        // Notes but no source: three, and only the first line shows.
        assert_eq!(by_id(3).rows(), 3);
        assert_eq!(
            by_id(1).notes_line(),
            Some("billing is wall-clock, not per call")
        );
    }

    #[test]
    fn docket_selection_moves_like_the_agent_board() {
        let model = docket_board_model(&DocketSample::test_fixture());
        // Nothing selected: the first card.
        assert_eq!(
            next_docket_selection(&model, None, BoardDir::Down, false),
            Some(1)
        );
        // Down wraps within the inbox.
        assert_eq!(
            next_docket_selection(&model, Some(11), BoardDir::Down, false),
            Some(1)
        );
        // Right from row 2 of the inbox clamps to the due lane's last row.
        assert_eq!(
            next_docket_selection(&model, Some(11), BoardDir::Right, false),
            Some(4)
        );
        // Left from the leftmost lane stays put.
        assert_eq!(
            next_docket_selection(&model, Some(1), BoardDir::Left, false),
            Some(1)
        );
        // Narrow: one flattened list, wrapping at both ends.
        assert_eq!(
            next_docket_selection(&model, Some(1), BoardDir::Up, true),
            Some(9)
        );
        assert_eq!(
            next_docket_selection(&model, Some(9), BoardDir::Down, true),
            Some(1)
        );
        // An empty board selects nothing.
        let empty = docket_board_model(&DocketSample::default());
        assert_eq!(
            next_docket_selection(&empty, None, BoardDir::Down, false),
            None
        );
    }

    #[test]
    fn an_empty_lane_is_skipped_over_not_stopped_at() {
        let mut sample = DocketSample::test_fixture();
        sample
            .rows
            .retain(|row| row.status != DocketStatus::Open || row.due.is_none());
        let model = docket_board_model(&sample);
        assert!(model.lane(DocketLane::Due).is_empty());
        // Right from the inbox lands in `slated`, two lanes over.
        assert_eq!(
            next_docket_selection(&model, Some(1), BoardDir::Right, false),
            Some(6)
        );
    }

    #[test]
    fn docket_cards_stack_at_their_own_heights() {
        let model = docket_board_model(&DocketSample::test_fixture());
        let body = Rect::new(0, 0, 150, 30);
        let slots = wide_slots_for(&model.card_heights(false), body);
        let inbox: Vec<(u16, u16)> = slots
            .iter()
            .filter(|slot| slot.lane == 0)
            .map(|slot| (slot.rect.y, slot.rect.height))
            .collect();
        // Header + gap, then a four-row card, a gap, a three-row card, a gap,
        // a two-row card.
        assert_eq!(inbox, vec![(2, 4), (7, 3), (11, 2)]);
        // A card that would not fit whole is not drawn at all.
        let short = Rect::new(0, 0, 150, 9);
        let slots = wide_slots_for(&model.card_heights(false), short);
        assert_eq!(slots.iter().filter(|slot| slot.lane == 0).count(), 1);
    }

    #[test]
    fn docket_card_reads_id_kind_due_source_and_notes() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 40);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 40);
        state.board.docket_selected = Some(3);
        let mut term = Terminal::new(TestBackend::new(150, 40)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        let screen: Vec<String> = (0..40)
            .map(|y| (0..150).map(|x| buffer[(x, y)].symbol()).collect())
            .collect();
        let text = screen.join("\n");
        assert!(text.contains("docket 2 due"), "strip fact: {text}");
        assert!(text.contains("3 inbox"), "strip fact: {text}");
        // The overdue card: `!` in the gutter, the days late on row two, the
        // notes on row three — and the lane heading carries the count.
        let row = screen
            .iter()
            .position(|line| line.contains("sign the four"))
            .expect("overdue card");
        assert!(
            screen[row].contains("! #3 sign the four"),
            "{:?}",
            screen[row]
        );
        assert!(
            screen[row + 1].contains("slated · overdue 3d"),
            "{:?}",
            screen[row + 1]
        );
        assert!(
            screen[row + 2].contains("Nora has the envelope"),
            "{:?}",
            screen[row + 2]
        );
        assert!(text.contains("due 2 !1"), "heading count: {text}");
        // A recurring card names its repeat; a sourced one its file.
        let row = screen
            .iter()
            .position(|line| line.contains("review the Vikunja"))
            .expect("recurring card");
        assert!(
            screen[row + 1].contains("recurring · due today · 1w"),
            "{:?}",
            screen[row + 1]
        );
        assert!(
            screen[row + 2].contains("shiftmayt.mjs:1"),
            "{:?}",
            screen[row + 2]
        );
    }

    #[test]
    fn the_overdue_mark_is_peach_and_bold() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 40);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 40);
        let mut term = Terminal::new(TestBackend::new(150, 40)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        // The card's gutter mark is bold peach; the lane heading's count is
        // peach too, but a count is not a mark.
        let marks = (0..40u16)
            .flat_map(|y| (0..150u16).map(move |x| (x, y)))
            .map(|pos| &buffer[pos])
            .filter(|cell| cell.symbol() == "!" && cell.fg == state.palette.peach)
            .count();
        let bold = (0..40u16)
            .flat_map(|y| (0..150u16).map(move |x| (x, y)))
            .map(|pos| &buffer[pos])
            .filter(|cell| {
                cell.symbol() == "!"
                    && cell.fg == state.palette.peach
                    && cell.modifier.contains(Modifier::BOLD)
            })
            .count();
        assert_eq!(marks, 2, "one card mark and one heading count");
        assert_eq!(bold, 1, "the card mark is bold");
    }

    #[test]
    fn stacked_docket_cards_are_two_rows_each() {
        let model = docket_board_model(&DocketSample::test_fixture());
        assert!(model
            .card_heights(true)
            .iter()
            .flatten()
            .all(|height| *height == COMPACT_CARD_ROWS));
        // Which is what gets the due lane onto an 80×24 screen at all.
        let body = board_body(Rect::new(1, 1, 78, 22));
        let (_, headers) = narrow_slots_for(&model.card_heights(true), body);
        assert!(headers
            .iter()
            .any(|(_, lane)| *lane == DocketLane::Due as usize));
    }

    #[test]
    fn the_docket_stacks_at_eighty_columns() {
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 80, 24);
        state.view.sidebar_rect = Rect::new(0, 0, 80, 24);
        assert!(is_docket_narrow(&state));
        state.view.terminal_area = Rect::new(0, 0, 150, 40);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 40);
        assert!(!is_docket_narrow(&state));
    }

    #[test]
    fn clicking_a_docket_card_finds_its_id() {
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 40);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 40);
        let inner = inner_area(board_area(&state)).expect("inner");
        let body = board_body(inner);
        let model = docket_board_model(&state.docket_sample);
        let slots = wide_slots_for(&model.card_heights(false), body);
        let due_first = slots
            .iter()
            .find(|slot| slot.lane == 1 && slot.row == 0)
            .expect("due lane card");
        assert_eq!(
            docket_card_at(&state, due_first.rect.x + 2, due_first.rect.y + 1),
            Some(3)
        );
        // The gap row between cards is nobody's.
        assert_eq!(
            docket_card_at(&state, due_first.rect.x + 2, due_first.rect.bottom()),
            None
        );
    }

    #[test]
    fn docket_detail_shows_every_field() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 100, 30);
        state.view.sidebar_rect = Rect::new(0, 0, 100, 30);
        state.board.view = BoardView::DocketItem;
        state.board.docket_selected = Some(4);
        let mut term = Terminal::new(TestBackend::new(100, 30)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        let text: String = (0..30)
            .map(|y| {
                (0..100)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        for needle in [
            "review the Vikunja board",
            "#4 · due today",
            "kind         recurring",
            "status       open",
            "due          2026-09-11 · due today",
            "repeat       1w",
            "source       ~/vault/agents/vikunja-docket/dockets/shiftmayt.mjs:1",
            "notes",
            "p slate",
            "esc back to docket",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in:\n{text}");
        }
    }

    #[test]
    fn a_docket_notice_rides_the_footer() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 40);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 40);
        state.board.docket_notice =
            Some("docket item 1 is inbox, only open items can be completed".into());
        let mut term = Terminal::new(TestBackend::new(150, 40)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        term.draw(|frame| render_board_overlay(&state, &runtimes, frame))
            .expect("render");
        let buffer = term.backend().buffer();
        let footer: String = (1..149).map(|x| buffer[(x, 38)].symbol()).collect();
        assert!(footer.contains("! docket item 1 is inbox"), "{footer:?}");
        assert!(footer.trim_end().ends_with("completed"), "{footer:?}");
    }

    #[test]
    #[ignore = "visual preview, run with --nocapture"]
    fn preview_docket_board() {
        use ratatui::{backend::TestBackend, Terminal};
        let mut state = docket_state();
        state.view.terminal_area = Rect::new(0, 0, 150, 34);
        state.view.sidebar_rect = Rect::new(0, 0, 150, 34);
        state.board.docket_selected = Some(3);
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
}
