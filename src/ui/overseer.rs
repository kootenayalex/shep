//! The overseer view: the board the session opens on.
//!
//! Left column, top to bottom: *needs you* (blocked agents, then finished
//! ones nobody has looked at), the *agents* table, the *docket*'s due lane and
//! newest inbox, and one *health* row. Right column: the overseer's narrative
//! (*read of the room*), its *proposals* (inbox items it captured, to keep or
//! drop), and the *chat* with the headless runtime: the newest turns that
//! fit, bottom-anchored over the input line. Above both, one header row: the
//! tick, the brain, the host, and the only button to the overseer's full
//! session.
//!
//! The TUI draws all the structure; the plugin's narrative is prose. Every
//! region is a heading, a *prefix* of its rows, and a blank row, and every
//! row budget is decided in [`overseer_layout`], never in render, so the mouse
//! and the screen agree by construction. Pure throughout: the model and the
//! layout read `&AppState`, and render only draws.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::{Clear, Paragraph},
    Frame,
};

use super::board::{self, BoardDir, DocketCard, DocketLane};
use super::gauge::context_gauge_spans;
use super::glyphs;
use super::sidebar::format_event_age;
use super::status::{agent_icon_for, docket_appearance, state_label, DocketUrgency};
use super::text::{display_width, fit_strip, spans_width, truncate_end};
use crate::api::schema::DocketStatus;
use crate::app::overseer::{
    overseer_runtime_name, ChatRole, ChatTurn, HealthFinding, HealthLevel, ProposalCard,
};
use crate::app::state::{AppState, Palette};
use crate::detect::AgentState;
use crate::layout::PaneId;

/// Below this the two columns stack into one.
pub(crate) const WIDE_MIN_WIDTH: u16 = 120;

/// The left column's width when there are two. Fixed, so the agents table
/// keeps its columns whatever the terminal; the right column takes the rest.
pub(crate) const LEFT_WIDTH: u16 = 74;

/// Columns of air at every row's right edge.
const RIGHT_MARGIN: usize = 1;

/// Where a row's text starts: after the selection marker and the glyph.
const INDENT: usize = 3;

/// The agents table's fixed columns, left to right.
const NAME_COL: usize = 9;
const GROUP_COL: usize = 11;
const BRANCH_COL: usize = 20;
const STATE_COL: usize = 14;

/// The one button on the board. Pinned to the header's right edge and never
/// dropped: it is the only way into the overseer's own session.
const SESSION_BUTTON: &str = "open the overseer's session";

// ---------------------------------------------------------------------------
// Model
// ---------------------------------------------------------------------------

/// One selectable row on the overseer view, by what it points at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OverseerRow {
    NeedsYou(PaneId),
    Agent(PaneId),
    Docket(i64),
    Proposal(i64),
}

/// Why an agent is in the *needs you* region.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum NeedsYouWhy {
    /// Blocked on something only the person can answer: the last thing its
    /// screen said, or what it says it is doing, or just the state word.
    Blocked { prompt: String },
    /// Finished while nobody was looking: what it last said, and the commits
    /// waiting to be pushed when the group knows.
    DoneUnseen { said: String, ahead: Option<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct NeedsYouRow {
    pub pane_id: PaneId,
    pub ws_idx: usize,
    pub name: String,
    pub group: String,
    pub location: String,
    pub state: AgentState,
    pub seen: bool,
    pub manual_state: Option<crate::api::schema::PaneManualState>,
    pub age: Option<String>,
    pub why: NeedsYouWhy,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct AgentRow {
    pub pane_id: PaneId,
    pub ws_idx: usize,
    pub name: String,
    pub group: String,
    pub branch: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub manual_state: Option<crate::api::schema::PaneManualState>,
    pub age: Option<String>,
    pub context_percent: Option<u8>,
}

/// What the header row says, already reduced to strings.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct HeaderFacts {
    pub tick_at: Option<String>,
    /// `3m`, from the brain stamp's age.
    pub brain_age: Option<String>,
    /// `[plugins.overseer] runtime`, when the config names one.
    pub runtime: Option<String>,
    pub load: Option<(u16, usize)>,
    pub memory_percent: Option<u8>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct OverseerModel {
    pub header: HeaderFacts,
    pub needs_you: Vec<NeedsYouRow>,
    pub agents: Vec<AgentRow>,
    /// The due lane whole, then the newest inbox items. The layout cuts a
    /// prefix, so the due lane is what survives a short screen.
    pub docket: Vec<DocketCard>,
    pub docket_due: usize,
    pub docket_overdue: usize,
    pub docket_inbox: usize,
    pub health: Vec<HealthFinding>,
    pub narrative: Vec<String>,
    pub proposals: Vec<ProposalCard>,
    /// The chat's tail, oldest first; the region shows the newest that fit.
    pub chat: Vec<ChatTurn>,
}

/// What the header knows, from the samples already in state.
pub(crate) fn header_facts(app: &AppState) -> HeaderFacts {
    let sample = &app.overseer.sample;
    let vitals = app.dashboard_sample.vitals;
    HeaderFacts {
        tick_at: sample.tick_at.clone(),
        brain_age: sample
            .brain_age(std::time::SystemTime::now())
            .map(format_event_age),
        runtime: overseer_runtime_name(app),
        load: vitals.load_percent.zip(vitals.cores),
        memory_percent: vitals.memory_percent,
    }
}

/// Build the view's model. Agents come from the live board model — fresher
/// than anything the plugin wrote — in board order; the docket, health and
/// narrative from the samples; proposals from the docket's situation-sourced
/// inbox.
pub(crate) fn overseer_model(app: &AppState) -> OverseerModel {
    let board_model = board::board_model(app);
    let mut needs_you = Vec::new();
    let mut agents = Vec::new();
    for card in board_model.flattened() {
        let age = board::card_age(app, card);
        agents.push(AgentRow {
            pane_id: card.pane_id,
            ws_idx: card.ws_idx,
            name: card.agent_label.clone(),
            group: card.workspace_label.clone(),
            branch: card.branch.clone(),
            state: card.state,
            seen: card.seen,
            manual_state: card.manual_state.clone(),
            age: age.clone(),
            context_percent: card.context_percent,
        });
        // The last thing its screen said, else what it says it is doing,
        // else just the state word.
        let said = card
            .activity
            .clone()
            .or_else(|| card.summary.clone())
            .unwrap_or_else(|| state_label(card.state, card.seen).to_string());
        let why = match (card.state, card.seen) {
            (AgentState::Blocked, _) => NeedsYouWhy::Blocked { prompt: said },
            (AgentState::Idle, false) => NeedsYouWhy::DoneUnseen {
                said,
                ahead: app
                    .workspaces
                    .get(card.ws_idx)
                    .and_then(|ws| ws.cached_git_ahead_behind)
                    .map(|(ahead, _)| ahead),
            },
            _ => continue,
        };
        needs_you.push(NeedsYouRow {
            pane_id: card.pane_id,
            ws_idx: card.ws_idx,
            name: card.agent_label.clone(),
            group: card.workspace_label.clone(),
            location: card.location.clone(),
            state: card.state,
            seen: card.seen,
            manual_state: card.manual_state.clone(),
            age,
            why,
        });
    }
    // Blocked first, then done-unseen; stable, so board order holds inside
    // each.
    needs_you.sort_by_key(|row| matches!(row.why, NeedsYouWhy::DoneUnseen { .. }));

    let docket_model = board::docket_board_model(&app.docket_sample);
    let due = docket_model.lane(DocketLane::Due);
    let inbox = docket_model.lane(DocketLane::Inbox);
    let proposals = crate::app::overseer::proposals(&app.docket_sample);
    let mut docket: Vec<DocketCard> = due.to_vec();
    // The store lists inbox items newest-updated first already. The ones the
    // overseer captured have a region of their own and are not listed twice.
    docket.extend(
        inbox
            .iter()
            .filter(|card| !proposals.iter().any(|p| p.id == card.id))
            .cloned(),
    );

    OverseerModel {
        header: header_facts(app),
        needs_you,
        agents,
        docket,
        docket_due: due.len(),
        docket_overdue: due.iter().filter(|card| card.overdue()).count(),
        docket_inbox: inbox.len(),
        health: app.overseer.sample.health.clone(),
        narrative: app.overseer.sample.narrative_lines(),
        proposals,
        chat: app.overseer.chat.clone(),
    }
}

impl OverseerModel {
    /// Every selectable row, in traversal order: needs you, agents, docket,
    /// proposals.
    pub(crate) fn rows(&self) -> Vec<OverseerRow> {
        let mut rows = Vec::new();
        rows.extend(
            self.needs_you
                .iter()
                .map(|r| OverseerRow::NeedsYou(r.pane_id)),
        );
        rows.extend(self.agents.iter().map(|r| OverseerRow::Agent(r.pane_id)));
        rows.extend(self.docket.iter().map(|r| OverseerRow::Docket(r.id)));
        rows.extend(self.proposals.iter().map(|r| OverseerRow::Proposal(r.id)));
        rows
    }

    /// The row the view treats as selected: the selection when it still
    /// resolves, else the first row.
    pub(crate) fn effective_selection(&self, sel: Option<OverseerRow>) -> Option<OverseerRow> {
        let rows = self.rows();
        sel.filter(|row| rows.contains(row))
            .or_else(|| rows.first().copied())
    }

    /// The row after moving `dir` from `sel`: up and down walk the flat order
    /// and wrap; left and right go nowhere.
    pub(crate) fn next(&self, sel: Option<OverseerRow>, dir: BoardDir) -> Option<OverseerRow> {
        let rows = self.rows();
        if rows.is_empty() {
            return None;
        }
        let Some(current) = sel.and_then(|row| rows.iter().position(|r| *r == row)) else {
            return rows.first().copied();
        };
        let next = match dir {
            BoardDir::Up => (current + rows.len() - 1) % rows.len(),
            BoardDir::Down => (current + 1) % rows.len(),
            BoardDir::Left | BoardDir::Right => current,
        };
        rows.get(next).copied()
    }

    /// The `(workspace index, pane)` a needs-you or agent row points at.
    pub(crate) fn pane_target(&self, row: OverseerRow) -> Option<(usize, PaneId)> {
        match row {
            OverseerRow::NeedsYou(pane) => self
                .needs_you
                .iter()
                .find(|r| r.pane_id == pane)
                .map(|r| (r.ws_idx, r.pane_id)),
            OverseerRow::Agent(pane) => self
                .agents
                .iter()
                .find(|r| r.pane_id == pane)
                .map(|r| (r.ws_idx, r.pane_id)),
            OverseerRow::Docket(_) | OverseerRow::Proposal(_) => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Layout
// ---------------------------------------------------------------------------

/// A region on screen: its heading row and the rows under it, plus how many
/// of its entries fit. `Rect::default()` everywhere when the region was
/// dropped.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct RegionRect {
    pub heading: Rect,
    pub body: Rect,
    /// Entries drawn: a prefix of the model's list.
    pub entries: usize,
}

impl RegionRect {
    pub(crate) fn shown(&self) -> bool {
        self.heading.height > 0
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct OverseerLayout {
    pub wide: bool,
    pub header: Rect,
    pub session_button: Rect,
    /// The divider column between the two, zero-width when stacked.
    pub divider: Rect,
    pub needs_you: RegionRect,
    pub agents: RegionRect,
    pub docket: RegionRect,
    pub health: RegionRect,
    pub room: RegionRect,
    pub proposals: RegionRect,
    pub chat: RegionRect,
    pub chat_input: Rect,
    /// The narrative wrapped to the room's width; `room.entries` of them draw.
    pub room_lines: Vec<String>,
    pub row_hits: Vec<(Rect, OverseerRow)>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Region {
    NeedsYou,
    Agents,
    Docket,
    Health,
    Room,
    Proposals,
    Chat,
}

/// What a region asks of the allocator.
#[derive(Debug, Clone, Copy)]
struct RegionSpec {
    region: Region,
    entry_rows: u16,
    /// Entries it would draw with unlimited room.
    entries: usize,
    /// Entries it is worth drawing at all.
    min_entries: usize,
    /// Which regions keep their minimum first when the column is short.
    keep_rank: u8,
    /// Which regions grow toward their full height first.
    grow_rank: u8,
}

fn spec_cost(spec: &RegionSpec, entries: usize) -> usize {
    1 + entries * usize::from(spec.entry_rows) + 1
}

/// Fit regions into `height` rows. Every kept region is its heading, a prefix
/// of its entries and a blank row; the last region's blank row may hang off
/// the bottom. Regions keep their minimum by `keep_rank`, dropping whole when
/// even that does not fit, then grow by `grow_rank`. Returns `(region,
/// entries)` in the declared order.
fn fit_column(specs: &[RegionSpec], height: u16) -> Vec<(Region, usize)> {
    let budget = usize::from(height) + 1;
    let mut kept: Vec<(usize, usize)> = Vec::new(); // (spec index, entries)
    let mut used = 0usize;
    let mut by_keep: Vec<usize> = (0..specs.len()).collect();
    by_keep.sort_by_key(|idx| specs[*idx].keep_rank);
    for idx in by_keep {
        let spec = &specs[idx];
        let min = spec.entries.min(spec.min_entries);
        let cost = spec_cost(spec, min);
        if used + cost <= budget {
            used += cost;
            kept.push((idx, min));
        }
    }
    let mut by_grow: Vec<usize> = (0..kept.len()).collect();
    by_grow.sort_by_key(|k| specs[kept[*k].0].grow_rank);
    for k in by_grow {
        let (idx, have) = kept[k];
        let spec = &specs[idx];
        let room = (budget - used) / usize::from(spec.entry_rows.max(1));
        let extra = spec.entries.saturating_sub(have).min(room);
        kept[k].1 = have + extra;
        used += extra * usize::from(spec.entry_rows);
    }
    kept.sort_by_key(|(idx, _)| *idx);
    kept.into_iter()
        .map(|(idx, entries)| (specs[idx].region, entries))
        .collect()
}

/// Lay a fitted column out from `column.y` down, returning each region's
/// rects. The blank row after a region is the gap before the next heading.
fn place_column(
    fitted: &[(Region, usize)],
    specs: &[RegionSpec],
    column: Rect,
) -> Vec<(Region, RegionRect)> {
    let mut y = column.y;
    let bottom = column.bottom();
    let mut out = Vec::new();
    for (region, entries) in fitted {
        let Some(spec) = specs.iter().find(|spec| spec.region == *region) else {
            continue;
        };
        if y >= bottom {
            break;
        }
        let rows = (*entries as u16).saturating_mul(spec.entry_rows);
        let heading = Rect::new(column.x, y, column.width, 1);
        let body = Rect::new(
            column.x,
            y + 1,
            column.width,
            rows.min(bottom.saturating_sub(y + 1)),
        );
        out.push((
            *region,
            RegionRect {
                heading,
                body,
                entries: *entries,
            },
        ));
        y = y.saturating_add(1 + rows + 1);
    }
    out
}

/// Break one line of prose into lines no wider than `width`, on spaces. A
/// word wider than the line is cut rather than lost.
pub(crate) fn wrap_words(text: &str, width: usize) -> Vec<String> {
    if width == 0 {
        return Vec::new();
    }
    let mut lines = Vec::new();
    let mut current = String::new();
    for word in text.split_whitespace() {
        let word_width = display_width(word);
        let current_width = display_width(&current);
        if current.is_empty() {
            if word_width <= width {
                current.push_str(word);
            } else {
                current = truncate_end(word, width);
            }
        } else if current_width + 1 + word_width <= width {
            current.push(' ');
            current.push_str(word);
        } else {
            lines.push(std::mem::take(&mut current));
            current = if word_width <= width {
                word.to_string()
            } else {
                truncate_end(word, width)
            };
        }
    }
    if !current.is_empty() {
        lines.push(current);
    }
    lines
}

/// The text width inside a column: the marker gutter on the left and the
/// margin on the right.
fn text_width(column: Rect) -> usize {
    usize::from(column.width).saturating_sub(2 + RIGHT_MARGIN)
}

/// Where everything on the view goes, for `area` (the board's whole surface,
/// header row included).
// `_app` is the seam the mouse and the render share: the layout is a fact
// about the model and the area today, and stays callable from both if it
// ever needs more.
pub(crate) fn overseer_layout(
    _app: &AppState,
    model: &OverseerModel,
    area: Rect,
) -> OverseerLayout {
    let mut layout = OverseerLayout {
        wide: area.width >= WIDE_MIN_WIDTH,
        header: Rect::new(area.x, area.y, area.width, area.height.min(1)),
        ..Default::default()
    };
    layout.session_button = session_button_rect(layout.header);
    if area.height < 2 || area.width == 0 {
        return layout;
    }
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    let (left, right) = if layout.wide {
        let left = Rect::new(body.x, body.y, LEFT_WIDTH, body.height);
        layout.divider = Rect::new(body.x + LEFT_WIDTH, body.y, 1, body.height);
        let right_x = body.x + LEFT_WIDTH + 1;
        let right = Rect::new(
            right_x,
            body.y,
            body.right().saturating_sub(right_x),
            body.height,
        );
        (left, Some(right))
    } else {
        (body, None)
    };

    let room_width = text_width(right.unwrap_or(left));
    layout.room_lines = model
        .narrative
        .iter()
        .flat_map(|line| wrap_words(line, room_width))
        .collect();

    let left_specs = [
        RegionSpec {
            region: Region::NeedsYou,
            entry_rows: 2,
            entries: model.needs_you.len(),
            min_entries: 1,
            keep_rank: 1,
            grow_rank: 0,
        },
        RegionSpec {
            region: Region::Agents,
            entry_rows: 1,
            entries: model.agents.len().max(1),
            min_entries: 1,
            keep_rank: 2,
            grow_rank: 1,
        },
        RegionSpec {
            region: Region::Docket,
            entry_rows: 1,
            entries: model.docket.len().max(1),
            min_entries: 1,
            keep_rank: 3,
            grow_rank: 5,
        },
        RegionSpec {
            region: Region::Health,
            entry_rows: 1,
            entries: usize::from(!model.health.is_empty()),
            min_entries: 1,
            keep_rank: 0,
            grow_rank: 2,
        },
    ];
    let right_specs = [
        RegionSpec {
            region: Region::Room,
            entry_rows: 1,
            entries: layout.room_lines.len().max(1),
            min_entries: 2,
            keep_rank: 4,
            grow_rank: 4,
        },
        RegionSpec {
            region: Region::Proposals,
            entry_rows: 2,
            entries: model.proposals.len(),
            min_entries: 1,
            keep_rank: 5,
            grow_rank: 3,
        },
        RegionSpec {
            region: Region::Chat,
            entry_rows: 1,
            entries: usize::MAX / 4,
            min_entries: 1,
            keep_rank: 6,
            grow_rank: 6,
        },
    ];
    // A region with nothing to say is not a region: no `needs you 0`, no
    // `proposals 0`, and no health row before the overseer has spoken.
    let wanted = |spec: &RegionSpec| spec.entries > 0;
    let placed: Vec<(Region, RegionRect)> = match right {
        Some(right) => {
            let left_specs: Vec<RegionSpec> = left_specs.iter().copied().filter(wanted).collect();
            let right_specs: Vec<RegionSpec> = right_specs.iter().copied().filter(wanted).collect();
            let mut placed = place_column(&fit_column(&left_specs, left.height), &left_specs, left);
            placed.extend(place_column(
                &fit_column(&right_specs, right.height),
                &right_specs,
                right,
            ));
            placed
        }
        None => {
            let specs: Vec<RegionSpec> = left_specs
                .iter()
                .chain(right_specs.iter())
                .copied()
                .filter(wanted)
                .collect();
            place_column(&fit_column(&specs, left.height), &specs, left)
        }
    };
    for (region, rect) in placed {
        match region {
            Region::NeedsYou => layout.needs_you = rect,
            Region::Agents => layout.agents = rect,
            Region::Docket => layout.docket = rect,
            Region::Health => layout.health = rect,
            Region::Room => layout.room = rect,
            Region::Proposals => layout.proposals = rect,
            Region::Chat => {
                layout.chat = rect;
                if rect.body.height > 0 {
                    layout.chat_input =
                        Rect::new(rect.body.x, rect.body.bottom() - 1, rect.body.width, 1);
                }
            }
        }
    }

    // Hit rects, one per drawn entry.
    let hits = |rect: RegionRect, rows: u16, keys: &mut dyn Iterator<Item = OverseerRow>| {
        let mut out = Vec::new();
        for (i, key) in keys.take(rect.entries).enumerate() {
            let y = rect.body.y + (i as u16) * rows;
            if y + rows <= rect.body.bottom() {
                out.push((Rect::new(rect.body.x, y, rect.body.width, rows), key));
            }
        }
        out
    };
    layout.row_hits.extend(hits(
        layout.needs_you,
        2,
        &mut model
            .needs_you
            .iter()
            .map(|r| OverseerRow::NeedsYou(r.pane_id)),
    ));
    layout.row_hits.extend(hits(
        layout.agents,
        1,
        &mut model.agents.iter().map(|r| OverseerRow::Agent(r.pane_id)),
    ));
    layout.row_hits.extend(hits(
        layout.docket,
        1,
        &mut model.docket.iter().map(|r| OverseerRow::Docket(r.id)),
    ));
    layout.row_hits.extend(hits(
        layout.proposals,
        2,
        &mut model.proposals.iter().map(|r| OverseerRow::Proposal(r.id)),
    ));
    layout
}

/// The session button's cell range on the header row: the label plus a
/// space either side, pinned to the right edge.
fn session_button_rect(header: Rect) -> Rect {
    if header.height == 0 {
        return Rect::default();
    }
    let width = (display_width(SESSION_BUTTON) as u16 + 4).min(header.width);
    Rect::new(header.right() - width, header.y, width, 1)
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    col >= rect.x && col < rect.right() && row >= rect.y && row < rect.bottom()
}

fn layout_for(app: &AppState) -> OverseerLayout {
    let model = overseer_model(app);
    overseer_layout(app, &model, board::board_area(app))
}

/// The row under a pointer position, if any.
pub(crate) fn row_at(app: &AppState, col: u16, row: u16) -> Option<OverseerRow> {
    layout_for(app)
        .row_hits
        .into_iter()
        .find(|(rect, _)| rect_contains(*rect, col, row))
        .map(|(_, key)| key)
}

/// Whether a pointer position is on the session button.
pub(crate) fn session_button_at(app: &AppState, col: u16, row: u16) -> bool {
    rect_contains(session_button_rect(board_header_rect(app)), col, row)
}

/// Whether a pointer position is on the chat input line.
pub(crate) fn chat_input_at(app: &AppState, col: u16, row: u16) -> bool {
    rect_contains(layout_for(app).chat_input, col, row)
}

fn board_header_rect(app: &AppState) -> Rect {
    let area = board::board_area(app);
    Rect::new(area.x, area.y, area.width, area.height.min(1))
}

// ---------------------------------------------------------------------------
// Render
// ---------------------------------------------------------------------------

fn fill_bg(frame: &mut Frame, rect: Rect, color: ratatui::style::Color) {
    let buf = frame.buffer_mut();
    for y in rect.top()..rect.bottom() {
        for x in rect.left()..rect.right() {
            buf[(x, y)].set_style(Style::default().bg(color));
        }
    }
}

fn draw_line(frame: &mut Frame, rect: Rect, y: u16, spans: Vec<Span<'static>>) {
    if y >= rect.bottom() || rect.width == 0 {
        return;
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)),
        Rect::new(rect.x, y, rect.width, 1),
    );
}

/// Spaces that push a trailing fact out to the content edge; at least one.
fn pin(width: usize, used: usize, trailing: usize) -> String {
    " ".repeat(
        width
            .saturating_sub(RIGHT_MARGIN)
            .saturating_sub(used)
            .saturating_sub(trailing)
            .max(1),
    )
}

/// The one header line: the overseer's clock, the host, and the button.
///
/// `tick hh:mm · brain Nm ago · <runtime> headless` then `shep <ver> · load
/// N % of C · mem N %` in overlay0, and ` ▸ open the overseer's session `
/// pinned right on surface1. Facts come off whole, in this order: the
/// runtime, the brain, load and memory, the tick. The button never drops.
pub(super) fn render_overseer_header(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    let facts = header_facts(app);
    let button = session_button_rect(area);
    let strip_width = usize::from(area.width)
        .saturating_sub(usize::from(button.width))
        .saturating_sub(2);
    let strip = header_strip(&facts, p, strip_width);
    let mut spans = vec![Span::raw(" ")];
    spans.extend(strip);
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
    if button.width >= 4 {
        let inner = usize::from(button.width) - 2;
        let label = truncate_end(&format!("{} {SESSION_BUTTON}", glyphs::COLLAPSED), inner);
        frame.render_widget(
            Paragraph::new(Line::from(Span::styled(
                format!(" {label:<inner$} "),
                Style::default().fg(p.text).bg(p.surface1),
            ))),
            button,
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HeaderFact {
    Tick,
    Brain,
    Runtime,
    Shep,
    LoadMem,
}

/// The header's facts at `width`: the fullest rung of the ladder that fits.
fn header_strip(facts: &HeaderFacts, p: &Palette, width: usize) -> Vec<Span<'static>> {
    use HeaderFact::*;
    let dim = Style::default().fg(p.overlay0);
    let text = Style::default().fg(p.subtext0);
    let key = Style::default().fg(p.text);
    let fact = |which: HeaderFact| -> Option<Vec<Span<'static>>> {
        match which {
            Tick => facts
                .tick_at
                .as_ref()
                .map(|at| vec![Span::styled("tick ", text), Span::styled(at.clone(), key)]),
            Brain => facts.brain_age.as_ref().map(|age| {
                vec![
                    Span::styled("brain ", text),
                    Span::styled(format!("{age} ago"), key),
                ]
            }),
            Runtime => facts
                .runtime
                .as_ref()
                .map(|name| vec![Span::styled(format!("{name} headless"), text)]),
            Shep => Some(vec![
                Span::styled("shep ", dim),
                Span::styled(env!("CARGO_PKG_VERSION"), dim),
            ]),
            LoadMem => {
                let mut spans = Vec::new();
                if let Some((percent, cores)) = facts.load {
                    spans.push(Span::styled(format!("load {percent} % of {cores}"), dim));
                }
                if let Some(mem) = facts.memory_percent {
                    if !spans.is_empty() {
                        spans.push(Span::styled(glyphs::SEP_WIDE, dim));
                    }
                    spans.push(Span::styled(format!("mem {mem} %"), dim));
                }
                (!spans.is_empty()).then_some(spans)
            }
        }
    };
    // Display order is fixed; each rung takes one more fact away.
    const ORDER: [HeaderFact; 5] = [Tick, Brain, Runtime, Shep, LoadMem];
    const LADDER: [&[HeaderFact]; 6] = [
        &[Tick, Brain, Runtime, Shep, LoadMem],
        &[Tick, Brain, Shep, LoadMem],
        &[Tick, Shep, LoadMem],
        &[Tick, Shep],
        &[Shep],
        &[],
    ];
    let sep = Span::styled(glyphs::SEP_WIDE, Style::default().fg(p.surface1));
    for rung in LADDER {
        let present: Vec<Vec<Span<'static>>> = ORDER
            .iter()
            .filter(|which| rung.contains(which))
            .filter_map(|which| fact(*which))
            .collect();
        let total: usize = present.iter().map(|f| spans_width(f)).sum::<usize>()
            + present.len().saturating_sub(1) * display_width(&sep.content);
        if total <= width {
            return fit_strip(present, &sep, width);
        }
    }
    Vec::new()
}

/// A region heading: bold text, plain counts, an overlay1 hint after.
fn heading_spans(
    p: &Palette,
    color: ratatui::style::Color,
    title: &str,
    counts: &str,
    hint: &str,
    width: usize,
) -> Vec<Span<'static>> {
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(
            title.to_string(),
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        ),
    ];
    let mut used = 1 + display_width(title);
    if !counts.is_empty() {
        spans.push(Span::styled(
            format!(" {counts}"),
            Style::default().fg(p.text),
        ));
        used += 1 + display_width(counts);
    }
    if !hint.is_empty() && used + 2 + display_width(hint) <= width {
        spans.push(Span::styled(
            format!("  {hint}"),
            Style::default().fg(p.overlay1),
        ));
    }
    spans
}

/// The selection's marker in the gutter of its first row. Drawn after the
/// rows, whose leading space would otherwise paint over it; the `surface0`
/// behind them is painted first and survives, since a bare span patches no
/// background.
fn mark_selected(frame: &mut Frame, p: &Palette, rect: Rect) {
    frame.render_widget(
        Paragraph::new(Line::from(Span::styled(
            glyphs::MARKER,
            Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
        ))),
        Rect::new(rect.x, rect.y, 1.min(rect.width), 1),
    );
}

pub(super) fn render_overseer(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    frame.render_widget(Clear, area);
    let model = overseer_model(app);
    let layout = overseer_layout(app, &model, area);
    render_overseer_header(app, frame, layout.header);
    if layout.divider.width > 0 {
        let divider = Style::default().fg(p.surface1);
        for y in layout.divider.top()..layout.divider.bottom() {
            frame.render_widget(
                Paragraph::new(Line::from(Span::styled("│", divider))),
                Rect::new(layout.divider.x, y, 1, 1),
            );
        }
    }
    let selected = model.effective_selection(app.board.overseer_selected);
    let selected_rect = layout
        .row_hits
        .iter()
        .find(|(_, row)| Some(*row) == selected)
        .map(|(rect, _)| *rect);
    if let Some(rect) = selected_rect {
        fill_bg(frame, rect, p.surface0);
    }
    render_needs_you(app, frame, &model, layout.needs_you);
    render_agents(app, frame, &model, layout.agents);
    render_docket(app, frame, &model, layout.docket);
    render_health(app, frame, &model, layout.health);
    render_room(app, frame, &model, &layout);
    render_proposals(app, frame, &model, layout.proposals);
    render_chat(app, frame, &model, &layout);
    if let Some(rect) = selected_rect {
        mark_selected(frame, p, rect);
    }
}

fn render_needs_you(app: &AppState, frame: &mut Frame, model: &OverseerModel, rect: RegionRect) {
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    frame.render_widget(
        Paragraph::new(Line::from(heading_spans(
            p,
            p.text,
            "needs you",
            &model.needs_you.len().to_string(),
            "",
            width,
        ))),
        rect.heading,
    );
    let dim = Style::default().fg(p.overlay0);
    let hint = Style::default().fg(p.overlay1);
    for (i, row) in model.needs_you.iter().take(rect.entries).enumerate() {
        let y = rect.body.y + (i as u16) * 2;
        let (glyph, glyph_style) = agent_icon_for(
            row.state,
            row.seen,
            row.manual_state.as_ref(),
            app.spinner_tick,
            p,
        );
        let state_style =
            Style::default().fg(super::status::state_label_color(row.state, row.seen, p));
        let mut trailing = state_label(row.state, row.seen).to_string();
        if let Some(age) = &row.age {
            trailing.push(' ');
            trailing.push_str(age);
        }
        let loc = if row.location.is_empty() {
            String::new()
        } else {
            format!("  {}", row.location)
        };
        let trailing_width = display_width(&trailing) + display_width(&loc);
        let name = format!("{}{}{}", row.name, glyphs::SEP_SPACED, row.group);
        let name = truncate_end(
            &name,
            width.saturating_sub(INDENT + RIGHT_MARGIN + trailing_width + 1),
        );
        let used = INDENT + display_width(&name);
        draw_line(
            frame,
            rect.body,
            y,
            vec![
                Span::raw(" "),
                Span::styled(glyph, glyph_style),
                Span::raw(" "),
                Span::styled(
                    name,
                    Style::default().fg(p.text).add_modifier(Modifier::BOLD),
                ),
                Span::raw(pin(width, used, trailing_width)),
                Span::styled(trailing, state_style),
                Span::styled(loc, dim),
            ],
        );
        // Line 2: the prompt or what happened, and what enter does about it.
        let (detail, right): (String, Vec<Span<'static>>) = match &row.why {
            NeedsYouWhy::Blocked { prompt } => (
                prompt.clone(),
                vec![Span::styled(
                    format!("enter {} answer it", glyphs::ARROW_RIGHT),
                    hint,
                )],
            ),
            NeedsYouWhy::DoneUnseen {
                said,
                ahead: Some(n),
            } if *n > 0 => (
                said.clone(),
                vec![
                    Span::styled(
                        format!("{}{n}", glyphs::AHEAD),
                        Style::default().fg(p.green),
                    ),
                    Span::styled(" not pushed", hint),
                ],
            ),
            NeedsYouWhy::DoneUnseen { said, .. } => (
                said.clone(),
                vec![Span::styled(
                    format!("enter {} look", glyphs::ARROW_RIGHT),
                    hint,
                )],
            ),
        };
        let right_width = spans_width(&right);
        let detail = truncate_end(
            &detail,
            width.saturating_sub(INDENT + RIGHT_MARGIN + right_width + 2),
        );
        let used = INDENT + display_width(&detail);
        let mut spans = vec![
            Span::raw(" ".repeat(INDENT)),
            Span::styled(detail, Style::default().fg(p.subtext0)),
            Span::raw(pin(width, used, right_width)),
        ];
        spans.extend(right);
        draw_line(frame, rect.body, y + 1, spans);
    }
}

/// Which of the table's columns a width can hold. Branch goes first, then
/// the gauge, then the group; the name and the state stay.
struct AgentCols {
    group: bool,
    branch: bool,
    gauge: bool,
}

fn agent_cols(width: usize) -> AgentCols {
    AgentCols {
        group: width >= 42,
        branch: width >= 72,
        gauge: width >= 54,
    }
}

fn render_agents(app: &AppState, frame: &mut Frame, model: &OverseerModel, rect: RegionRect) {
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    frame.render_widget(
        Paragraph::new(Line::from(heading_spans(
            p,
            p.text,
            "agents",
            &model.agents.len().to_string(),
            "",
            width,
        ))),
        rect.heading,
    );
    if model.agents.is_empty() {
        draw_line(
            frame,
            rect.body,
            rect.body.y,
            vec![Span::styled(
                format!("{}no agents running", " ".repeat(INDENT)),
                Style::default().fg(p.overlay0),
            )],
        );
        return;
    }
    let cols = agent_cols(width);
    for (i, row) in model.agents.iter().take(rect.entries).enumerate() {
        let y = rect.body.y + i as u16;
        let (glyph, glyph_style) = agent_icon_for(
            row.state,
            row.seen,
            row.manual_state.as_ref(),
            app.spinner_tick,
            p,
        );
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(glyph, glyph_style),
            Span::raw(" "),
            Span::styled(
                format!("{:<NAME_COL$}", truncate_end(&row.name, NAME_COL)),
                Style::default().fg(p.text),
            ),
        ];
        let mut used = INDENT + NAME_COL;
        if cols.group {
            spans.push(Span::styled(
                format!(" {:<GROUP_COL$}", truncate_end(&row.group, GROUP_COL)),
                Style::default().fg(p.subtext0),
            ));
            used += 1 + GROUP_COL;
        }
        if cols.branch {
            spans.push(Span::styled(
                format!(
                    " {:<BRANCH_COL$}",
                    truncate_end(row.branch.as_deref().unwrap_or(""), BRANCH_COL)
                ),
                Style::default().fg(p.mauve),
            ));
            used += 1 + BRANCH_COL;
        }
        let mut state = state_label(row.state, row.seen).to_string();
        if let Some(age) = &row.age {
            state.push(' ');
            state.push_str(age);
        }
        spans.push(Span::styled(
            format!(" {:<STATE_COL$}", truncate_end(&state, STATE_COL)),
            Style::default().fg(super::status::state_label_color(row.state, row.seen, p)),
        ));
        used += 1 + STATE_COL;
        if cols.gauge {
            if let Some(percent) = row.context_percent {
                let gauge = context_gauge_spans(percent, p);
                spans.push(Span::raw(pin(width, used, spans_width(&gauge))));
                spans.extend(gauge);
            }
        }
        draw_line(frame, rect.body, y, spans);
    }
}

fn render_docket(app: &AppState, frame: &mut Frame, model: &OverseerModel, rect: RegionRect) {
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    let mut heading = vec![
        Span::raw(" "),
        Span::styled(
            "docket",
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled("  due ", Style::default().fg(p.overlay1)),
        Span::styled(model.docket_due.to_string(), Style::default().fg(p.text)),
    ];
    if model.docket_overdue > 0 {
        heading.push(Span::styled(
            format!(" !{}", model.docket_overdue),
            Style::default().fg(p.peach),
        ));
    }
    heading.push(Span::styled(
        format!("{}inbox ", glyphs::SEP_WIDE),
        Style::default().fg(p.overlay1),
    ));
    heading.push(Span::styled(
        model.docket_inbox.to_string(),
        Style::default().fg(p.text),
    ));
    frame.render_widget(Paragraph::new(Line::from(heading)), rect.heading);
    let dim = Style::default().fg(p.overlay0);
    if model.docket.is_empty() {
        draw_line(
            frame,
            rect.body,
            rect.body.y,
            vec![Span::styled(
                format!("{}nothing due, inbox empty", " ".repeat(INDENT)),
                dim,
            )],
        );
        return;
    }
    for (i, card) in model.docket.iter().take(rect.entries).enumerate() {
        let y = rect.body.y + i as u16;
        let look = docket_appearance(card.status, card.urgency());
        let glyph_style = if card.overdue() {
            look.style(p).add_modifier(Modifier::BOLD)
        } else {
            look.style(p)
        };
        let (trailing, trailing_style) = if card.status == DocketStatus::Inbox {
            ("inbox".to_string(), dim)
        } else {
            let mut text = card.due.text();
            if let Some(repeat) = card.repeat {
                text.push_str(glyphs::SEP_SPACED);
                text.push_str(repeat.as_str());
            }
            let style = match card.urgency() {
                DocketUrgency::Overdue | DocketUrgency::Today => look.style(p),
                DocketUrgency::Later => dim,
            };
            (text, style)
        };
        let trailing_width = display_width(&trailing);
        let id = format!("#{} ", card.id);
        let title = truncate_end(
            &card.title,
            width.saturating_sub(INDENT + display_width(&id) + RIGHT_MARGIN + trailing_width + 1),
        );
        let used = INDENT + display_width(&id) + display_width(&title);
        draw_line(
            frame,
            rect.body,
            y,
            vec![
                Span::raw(" "),
                Span::styled(look.glyph, glyph_style),
                Span::raw(" "),
                Span::styled(id, dim),
                Span::styled(title, Style::default().fg(p.text)),
                Span::raw(pin(width, used, trailing_width)),
                Span::styled(trailing, trailing_style),
            ],
        );
    }
}

/// The health row's facts, ok ones first as they read, but the first to go.
fn health_facts(findings: &[HealthFinding], p: &Palette) -> Vec<Vec<Span<'static>>> {
    findings
        .iter()
        .map(|finding| match finding.level {
            HealthLevel::Ok => vec![
                Span::styled(glyphs::TICK, Style::default().fg(p.green)),
                Span::styled(
                    format!(" {}", finding.check),
                    Style::default().fg(p.subtext0),
                ),
            ],
            HealthLevel::Warn => vec![Span::styled(
                format!("{} {} {}", glyphs::WARNING, finding.check, finding.detail)
                    .trim_end()
                    .to_string(),
                Style::default().fg(p.peach),
            )],
            // A failed check stops you the way a blocked agent does: the
            // stop tier's own glyph and ink.
            HealthLevel::Fail => vec![Span::styled(
                format!(
                    "{} {} {}",
                    super::status::state_glyph(AgentState::Blocked),
                    finding.check,
                    finding.detail
                )
                .trim_end()
                .to_string(),
                Style::default().fg(p.red),
            )],
        })
        .collect()
}

fn render_health(app: &AppState, frame: &mut Frame, model: &OverseerModel, rect: RegionRect) {
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    frame.render_widget(
        Paragraph::new(Line::from(heading_spans(
            p, p.text, "health", "", "", width,
        ))),
        rect.heading,
    );
    let sep = Span::raw("  ");
    let budget = width.saturating_sub(1 + RIGHT_MARGIN);
    // Ok findings come off first, last one first, and only then does the
    // strip drop whole findings from its end: a warning outranks a tick.
    let ok_total = model
        .health
        .iter()
        .filter(|f| f.level == HealthLevel::Ok)
        .count();
    let mut ok_keep = ok_total;
    let strip = loop {
        let mut ok_seen = 0usize;
        let ordered: Vec<HealthFinding> = model
            .health
            .iter()
            .filter(|f| {
                if f.level != HealthLevel::Ok {
                    return true;
                }
                ok_seen += 1;
                ok_seen <= ok_keep
            })
            .cloned()
            .collect();
        let facts = health_facts(&ordered, p);
        let total: usize =
            facts.iter().map(|f| spans_width(f)).sum::<usize>() + facts.len().saturating_sub(1) * 2;
        if total <= budget || ok_keep == 0 {
            break fit_strip(facts, &sep, budget);
        }
        ok_keep -= 1;
    };
    let mut spans = vec![Span::raw(" ")];
    spans.extend(strip);
    draw_line(frame, rect.body, rect.body.y, spans);
}

fn render_room(app: &AppState, frame: &mut Frame, model: &OverseerModel, layout: &OverseerLayout) {
    let rect = layout.room;
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let mut heading = vec![
        Span::raw(" "),
        Span::styled(
            format!("{} read of the room", glyphs::OVERSEER),
            Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
        ),
    ];
    if let Some(at) = &model.header.tick_at {
        heading.push(Span::styled(
            format!("{}{at}", glyphs::SEP_WIDE),
            Style::default().fg(p.overlay0),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(heading)), rect.heading);
    if layout.room_lines.is_empty() {
        draw_line(
            frame,
            rect.body,
            rect.body.y,
            vec![Span::styled(
                "  the overseer has not spoken yet",
                Style::default().fg(p.overlay0),
            )],
        );
        return;
    }
    for (i, line) in layout.room_lines.iter().take(rect.entries).enumerate() {
        draw_line(
            frame,
            rect.body,
            rect.body.y + i as u16,
            vec![Span::styled(
                format!("  {line}"),
                Style::default().fg(p.subtext0),
            )],
        );
    }
}

fn render_proposals(app: &AppState, frame: &mut Frame, model: &OverseerModel, rect: RegionRect) {
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    frame.render_widget(
        Paragraph::new(Line::from(heading_spans(
            p,
            p.teal,
            "proposals",
            &model.proposals.len().to_string(),
            &format!("{} inbox only, you dispose", glyphs::SEP),
            width,
        ))),
        rect.heading,
    );
    let dim = Style::default().fg(p.overlay0);
    let verbs = vec![
        Span::styled(
            "a",
            Style::default().fg(p.green).add_modifier(Modifier::BOLD),
        ),
        Span::styled(" keep  ", dim),
        Span::styled("x", Style::default().fg(p.red).add_modifier(Modifier::BOLD)),
        Span::styled(" drop", dim),
    ];
    let verbs_width = spans_width(&verbs);
    let look = docket_appearance(DocketStatus::Inbox, DocketUrgency::Later);
    for (i, card) in model.proposals.iter().take(rect.entries).enumerate() {
        let y = rect.body.y + (i as u16) * 2;
        let id = format!("#{} ", card.id);
        let title = truncate_end(
            &card.title,
            width.saturating_sub(INDENT + display_width(&id) + RIGHT_MARGIN + verbs_width + 2),
        );
        let used = INDENT + display_width(&id) + display_width(&title);
        let mut spans = vec![
            Span::raw(" "),
            Span::styled(look.glyph, look.style(p)),
            Span::raw(" "),
            Span::styled(id, dim),
            Span::styled(title, Style::default().fg(p.text)),
            Span::raw(pin(width, used, verbs_width)),
        ];
        spans.extend(verbs.iter().cloned());
        draw_line(frame, rect.body, y, spans);
        let second = card
            .reference
            .as_ref()
            .map(|r| format!("from  {r}"))
            .or_else(|| card.notes_line.clone())
            .unwrap_or_default();
        draw_line(
            frame,
            rect.body,
            y + 1,
            vec![Span::styled(
                format!(
                    "{}{}",
                    " ".repeat(INDENT),
                    truncate_end(&second, width.saturating_sub(INDENT + RIGHT_MARGIN))
                ),
                dim,
            )],
        );
    }
}

/// Where a chat row's text starts: ` you  ` or ` ✦    `, then the age in
/// three cells and two spaces.
const CHAT_TEXT_COL: usize = 1 + 3 + 2 + 3 + 2;

/// The chat heading's hint, whole facts dropping until it fits: the answer
/// time goes first, the runtime second, the promise last.
fn chat_hint(runtime: Option<&str>, room: usize) -> String {
    let s = glyphs::SEP;
    let runtime_fact = runtime.map(|name| format!("{name} headless"));
    let ladder: [Vec<Option<&str>>; 4] = [
        vec![
            runtime_fact.as_deref(),
            Some("answers in a few seconds"),
            Some("never types into a pane"),
        ],
        vec![runtime_fact.as_deref(), Some("never types into a pane")],
        vec![Some("never types into a pane")],
        vec![],
    ];
    ladder
        .iter()
        .map(|facts| {
            facts
                .iter()
                .flatten()
                .map(|fact| format!("{s} {fact}"))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .find(|hint| hint.is_empty() || 2 + display_width(hint) <= room)
        .unwrap_or_default()
}

/// One screen row of the chat: a turn's first line carries who and when,
/// the rest hang under the text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChatRow {
    pub role: ChatRole,
    /// `Some` on a turn's first row: its age, `6m`.
    pub age: Option<String>,
    pub text: String,
}

/// The newest turns that fit in `budget` rows, wrapped to `text_width`,
/// oldest first. A turn is shown whole or not at all, so the oldest visible
/// turn never starts mid-sentence.
pub(crate) fn chat_rows(
    turns: &[ChatTurn],
    text_width: usize,
    budget: usize,
    now: u64,
) -> Vec<ChatRow> {
    let mut rows: Vec<ChatRow> = Vec::new();
    for turn in turns.iter().rev() {
        let mut lines = wrap_words(turn.text.trim(), text_width.max(1));
        if lines.is_empty() {
            lines.push(String::new());
        }
        if rows.len() + lines.len() > budget {
            break;
        }
        let age = format_event_age(std::time::Duration::from_secs(now.saturating_sub(turn.at)));
        let mut turn_rows: Vec<ChatRow> = lines
            .into_iter()
            .enumerate()
            .map(|(i, text)| ChatRow {
                role: turn.role,
                age: (i == 0).then(|| age.clone()),
                text,
            })
            .collect();
        turn_rows.extend(rows);
        rows = turn_rows;
    }
    rows
}

fn render_chat(app: &AppState, frame: &mut Frame, model: &OverseerModel, layout: &OverseerLayout) {
    let rect = layout.chat;
    if !rect.shown() {
        return;
    }
    let p = &app.palette;
    let width = usize::from(rect.body.width);
    let hint = chat_hint(
        model.header.runtime.as_deref(),
        width.saturating_sub(1 + display_width("chat") + RIGHT_MARGIN),
    );
    frame.render_widget(
        Paragraph::new(Line::from(heading_spans(
            p, p.text, "chat", "", &hint, width,
        ))),
        rect.heading,
    );
    if layout.chat_input.height == 0 {
        return;
    }
    let chat = &app.overseer;
    let dim = Style::default().fg(p.overlay0);

    // The turns, bottom-anchored above the pending row and the input.
    let pending_rows = usize::from(chat.chat_pending);
    let budget = usize::from(rect.body.height)
        .saturating_sub(1)
        .saturating_sub(pending_rows);
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let rows = chat_rows(
        &model.chat,
        width.saturating_sub(CHAT_TEXT_COL + RIGHT_MARGIN),
        budget,
        now,
    );
    let first_y = layout.chat_input.y - (pending_rows + rows.len()) as u16;
    for (i, row) in rows.iter().enumerate() {
        let mut spans = vec![Span::raw(" ")];
        match row.age.as_deref() {
            Some(age) => {
                let (who, who_style) = match row.role {
                    ChatRole::You => (
                        "you".to_string(),
                        Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
                    ),
                    ChatRole::Overseer => (
                        format!("{}  ", glyphs::OVERSEER),
                        Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
                    ),
                };
                spans.push(Span::styled(who, who_style));
                spans.push(Span::styled(format!("  {age:>3}  "), dim));
            }
            None => spans.push(Span::raw(" ".repeat(CHAT_TEXT_COL - 1))),
        }
        let text_style = match row.role {
            ChatRole::You => Style::default().fg(p.text),
            ChatRole::Overseer => Style::default().fg(p.subtext0),
        };
        spans.push(Span::styled(row.text.clone(), text_style));
        draw_line(frame, rect.body, first_y + i as u16, spans);
    }
    if chat.chat_pending {
        draw_line(
            frame,
            rect.body,
            layout.chat_input.y - 1,
            vec![
                Span::raw(" "),
                Span::styled(
                    format!(
                        "{} thinking{}",
                        super::spinner_frame(app.spinner_tick),
                        glyphs::ELLIPSIS
                    ),
                    Style::default().fg(p.yellow),
                ),
            ],
        );
    }

    // The input line: `› text▮` with the keys, `› ask the overseer` without.
    let mut spans = vec![
        Span::raw(" "),
        Span::styled(glyphs::LEADS_TO, Style::default().fg(p.overlay1)),
        Span::raw(" "),
    ];
    let room = width.saturating_sub(3 + 1 + RIGHT_MARGIN);
    if chat.chat_focused {
        // The tail of a long line, so the cursor is always on screen.
        let mut text = chat.chat_input.as_str();
        while display_width(text) > room {
            let mut chars = text.chars();
            chars.next();
            text = chars.as_str();
        }
        spans.push(Span::styled(text.to_string(), Style::default().fg(p.text)));
        spans.push(Span::styled(glyphs::CURSOR, Style::default().fg(p.accent)));
    } else if chat.chat_input.is_empty() {
        spans.push(Span::styled("ask the overseer", dim));
    } else {
        spans.push(Span::styled(
            truncate_end(&chat.chat_input, room),
            Style::default().fg(p.text),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), layout.chat_input);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::{BoardView, Mode};
    use ratatui::{backend::TestBackend, Terminal};

    fn overseer_state(width: u16, height: u16) -> AppState {
        let mut state = crate::ui::snapshot::fixture::session();
        state.mode = Mode::Board;
        state.board.view = BoardView::Overseer;
        state.view.terminal_area = Rect::new(0, 1, width, height - 2);
        state.view.sidebar_rect = Rect::new(0, 1, 0, height - 2);
        state
    }

    fn screen(state: &AppState, width: u16, height: u16) -> Vec<String> {
        let mut term = Terminal::new(TestBackend::new(width, height)).expect("terminal");
        term.draw(|frame| render_overseer(state, frame, Rect::new(0, 0, width, height)))
            .expect("render");
        let buffer = term.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_string()
            })
            .collect()
    }

    #[test]
    fn needs_you_lists_blocked_with_prompt_then_done_unseen_with_push_count() {
        let state = overseer_state(160, 45);
        let model = overseer_model(&state);
        assert_eq!(model.needs_you.len(), 2);
        let blocked = &model.needs_you[0];
        assert_eq!(
            (blocked.name.as_str(), blocked.group.as_str()),
            ("claude", "workmayt")
        );
        assert_eq!(blocked.state, AgentState::Blocked);
        assert_eq!(
            blocked.why,
            NeedsYouWhy::Blocked {
                prompt: "may I run the stripe integration tests?".into()
            }
        );
        let done = &model.needs_you[1];
        assert_eq!(done.group, "emberline");
        // emberline's group has no ahead count; workmayt's would be 2.
        assert_eq!(
            done.why,
            NeedsYouWhy::DoneUnseen {
                said: "pushed 3 commits to master".into(),
                ahead: None
            }
        );
        // With an ahead count the second line says so.
        let mut ahead = overseer_state(160, 45);
        ahead.workspaces[1].cached_git_ahead_behind = Some((3, 0));
        let rows = screen(&ahead, 160, 45);
        assert!(
            rows.iter()
                .any(|r| r.contains("pushed 3 commits to master") && r.contains("↑3 not pushed")),
            "{rows:#?}"
        );
        // A blocked agent with nothing on its screen falls back to its summary,
        // then the state word.
        let mut quiet = overseer_state(160, 45);
        let blocked_pane = overseer_model(&quiet).needs_you[0].pane_id;
        let tid = quiet.workspaces[0]
            .terminal_id(blocked_pane)
            .expect("terminal")
            .clone();
        let t = quiet.terminals.get_mut(&tid).expect("terminal");
        t.set_activity_lines(Vec::new());
        let model = overseer_model(&quiet);
        assert_eq!(
            model.needs_you[0].why,
            NeedsYouWhy::Blocked {
                prompt: "Now the refund path's idempotency key.".into()
            }
        );
    }

    #[test]
    fn agents_table_matches_board_model_order() {
        let state = overseer_state(160, 45);
        let model = overseer_model(&state);
        let board = board::board_model(&state);
        assert_eq!(
            model.agents.iter().map(|a| a.pane_id).collect::<Vec<_>>(),
            board
                .flattened()
                .iter()
                .map(|c| c.pane_id)
                .collect::<Vec<_>>()
        );
        assert_eq!(model.agents.len(), 5);
        assert_eq!(
            model.agents[0].branch.as_deref(),
            Some("fix/stripe-webhook")
        );
        assert_eq!(model.agents[0].context_percent, Some(72));
    }

    #[test]
    fn docket_region_is_due_in_full_then_newest_inbox() {
        let state = overseer_state(160, 45);
        let model = overseer_model(&state);
        let docket = board::docket_board_model(&state.docket_sample);
        let due: Vec<i64> = docket.lane(DocketLane::Due).iter().map(|c| c.id).collect();
        let inbox: Vec<i64> = docket
            .lane(DocketLane::Inbox)
            .iter()
            .map(|c| c.id)
            .collect();
        let ids: Vec<i64> = model.docket.iter().map(|c| c.id).collect();
        assert_eq!(&ids[..due.len()], &due[..]);
        // Inbox items in board order, minus the overseer's own proposals,
        // which have a region of their own.
        let proposals: Vec<i64> = model.proposals.iter().map(|c| c.id).collect();
        let expected_inbox: Vec<i64> = inbox
            .iter()
            .copied()
            .filter(|id| !proposals.contains(id))
            .collect();
        assert_eq!(&ids[due.len()..], &expected_inbox[..]);
        assert!(!expected_inbox.contains(&12));
        assert_eq!(model.docket_due, 2);
        assert_eq!(model.docket_overdue, 1);
        assert_eq!(model.docket_inbox, inbox.len());
        // A short screen keeps the due lane and cuts the inbox.
        let layout = overseer_layout(&state, &model, Rect::new(0, 0, 160, 16));
        assert!(layout.docket.entries >= due.len().min(layout.docket.entries));
        assert!(layout.docket.entries < model.docket.len());
    }

    #[test]
    fn selection_walks_regions_in_order_and_wraps() {
        let state = overseer_state(160, 45);
        let model = overseer_model(&state);
        let rows = model.rows();
        assert!(matches!(rows[0], OverseerRow::NeedsYou(_)));
        let first_agent = rows
            .iter()
            .position(|r| matches!(r, OverseerRow::Agent(_)))
            .expect("agents");
        let first_docket = rows
            .iter()
            .position(|r| matches!(r, OverseerRow::Docket(_)))
            .expect("docket");
        let first_proposal = rows
            .iter()
            .position(|r| matches!(r, OverseerRow::Proposal(_)))
            .expect("proposals");
        assert!(first_agent < first_docket && first_docket < first_proposal);
        assert_eq!(model.effective_selection(None), Some(rows[0]));
        assert_eq!(model.next(None, BoardDir::Down), Some(rows[0]));
        assert_eq!(model.next(Some(rows[0]), BoardDir::Down), Some(rows[1]));
        assert_eq!(
            model.next(Some(rows[0]), BoardDir::Up),
            rows.last().copied(),
            "wraps"
        );
        assert_eq!(
            model.next(rows.last().copied(), BoardDir::Down),
            Some(rows[0])
        );
        assert_eq!(model.next(Some(rows[1]), BoardDir::Left), Some(rows[1]));
        assert_eq!(
            model.effective_selection(Some(OverseerRow::Docket(9999))),
            Some(rows[0]),
            "a stale selection falls back to the first row"
        );
    }

    #[test]
    fn layout_stacks_below_120_and_regions_are_prefixes() {
        let state = overseer_state(160, 45);
        let model = overseer_model(&state);
        let wide = overseer_layout(&state, &model, Rect::new(0, 0, 160, 45));
        assert!(wide.wide);
        assert_eq!(wide.divider.x, LEFT_WIDTH);
        assert_eq!(wide.needs_you.body.width, LEFT_WIDTH);
        assert!(wide.room.heading.x > LEFT_WIDTH);
        assert!(wide.chat.shown());
        assert_eq!(wide.chat_input.y, 44);
        // Every region draws a prefix of its list, never a subset.
        assert_eq!(wide.needs_you.entries, model.needs_you.len());
        assert_eq!(wide.agents.entries, model.agents.len());
        assert_eq!(wide.proposals.entries, model.proposals.len());
        assert!(wide.docket.entries <= model.docket.len());

        let narrow = overseer_layout(&state, &model, Rect::new(0, 0, 119, 45));
        assert!(!narrow.wide);
        assert_eq!(narrow.divider.width, 0);
        assert_eq!(narrow.room.heading.x, 0);
        assert!(narrow.room.heading.y > narrow.health.heading.y);
        assert!(narrow.chat.heading.y > narrow.proposals.heading.y);

        // Too short for everything: docket goes before agents before needs
        // you, and health survives.
        let short = overseer_layout(&state, &model, Rect::new(0, 0, 160, 12));
        assert!(short.health.shown(), "{short:?}");
        assert!(short.needs_you.shown(), "{short:?}");
        assert!(short.agents.shown(), "{short:?}");
        assert!(!short.docket.shown(), "{short:?}");
        let shorter = overseer_layout(&state, &model, Rect::new(0, 0, 160, 8));
        assert!(shorter.health.shown());
        assert!(shorter.needs_you.shown());
        assert!(!shorter.agents.shown());
    }

    #[test]
    fn row_at_agrees_with_layout() {
        let mut state = overseer_state(160, 45);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 160, 45));
        let model = overseer_model(&state);
        let layout = overseer_layout(&state, &model, board::board_area(&state));
        for (rect, row) in &layout.row_hits {
            assert_eq!(row_at(&state, rect.x + 2, rect.y), Some(*row));
            assert_eq!(
                row_at(&state, rect.x + 2, rect.bottom() - 1),
                Some(*row),
                "every row of a two-row entry hits it"
            );
        }
        // A heading is nobody's.
        assert_eq!(
            row_at(&state, layout.agents.heading.x + 2, layout.agents.heading.y),
            None
        );
        assert!(!chat_input_at(
            &state,
            layout.chat_input.x,
            layout.chat_input.y - 1
        ));
        assert!(chat_input_at(
            &state,
            layout.chat_input.x + 3,
            layout.chat_input.y
        ));
    }

    #[test]
    fn session_button_is_pinned_right() {
        let mut state = overseer_state(160, 45);
        crate::ui::compute_view(&mut state, Rect::new(0, 0, 160, 45));
        let area = board::board_area(&state);
        let button = session_button_rect(Rect::new(area.x, area.y, area.width, 1));
        assert_eq!(button.right(), area.right());
        assert!(session_button_at(&state, button.x + 1, area.y));
        assert!(!session_button_at(&state, button.x - 1, area.y));
        assert!(!session_button_at(&state, button.x + 1, area.y + 1));
        let rows = screen(&state, 160, 3);
        assert!(
            rows[0].ends_with("▸ open the overseer's session"),
            "{:?}",
            rows[0]
        );
    }

    #[test]
    fn header_drops_facts_in_order_and_keeps_the_button() {
        let p = Palette::shep();
        let facts = HeaderFacts {
            tick_at: Some("07:08".into()),
            brain_age: Some("3m".into()),
            runtime: Some("claude".into()),
            load: Some((38, 12)),
            memory_percent: Some(64),
        };
        let text = |width: usize| -> String {
            header_strip(&facts, &p, width)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        let full = text(200);
        assert_eq!(
            full,
            format!(
                "tick 07:08  ·  brain 3m ago  ·  claude headless  ·  shep {}  ·  load 38 % of 12  ·  mem 64 %",
                env!("CARGO_PKG_VERSION")
            )
        );
        let mut seen = vec![full.clone()];
        for width in (0..full.len()).rev() {
            let now = text(width);
            if seen.last() != Some(&now) {
                seen.push(now);
            }
        }
        let expect_order = ["claude headless", "brain", "load", "tick", "shep"];
        for (step, gone) in expect_order.iter().enumerate() {
            assert!(
                seen.iter().skip(step + 1).all(|s| !s.contains(gone)),
                "{gone} should be gone after rung {step}: {seen:?}"
            );
        }
        assert_eq!(seen.last().map(String::as_str), Some(""));

        // The button survives every width the header can draw at.
        let mut state = overseer_state(160, 45);
        state.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"claude\"").expect("table"),
        );
        for width in [40u16, 60, 80, 120, 160] {
            let rows = screen(&state, width, 3);
            assert!(
                rows[0].contains("overseer's session"),
                "width {width}: {:?}",
                rows[0]
            );
        }
    }

    #[test]
    fn wrap_words_breaks_on_spaces_and_cuts_long_words() {
        assert_eq!(
            wrap_words("one two three four", 9),
            vec!["one two", "three", "four"]
        );
        assert_eq!(wrap_words("abcdefghij", 4), vec!["abc…"]);
        assert!(wrap_words("x", 0).is_empty());
    }

    #[test]
    fn health_row_drops_ok_checks_before_warnings() {
        let mut state = overseer_state(160, 45);
        state.view.terminal_area = Rect::new(0, 0, 60, 30);
        state.view.sidebar_rect = Rect::new(0, 0, 60, 30);
        let rows = screen(&state, 60, 30);
        let health = rows
            .iter()
            .position(|r| r.trim() == "health")
            .expect("health heading");
        let row = &rows[health + 1];
        assert!(row.contains("⚠ disk 9.8 G free"), "{row:?}");
        assert!(row.contains("⚠ err-log"), "{row:?}");
        assert!(!row.contains("✓ push"), "ok checks go first: {row:?}");
    }

    #[test]
    fn a_proposal_row_reads_keep_and_drop_and_paints_the_selection() {
        let mut state = overseer_state(160, 45);
        state.board.overseer_selected = Some(OverseerRow::Proposal(12));
        let rows = screen(&state, 160, 45);
        let card = rows
            .iter()
            .find(|r| r.contains("#12 ask claude about the stripe retry budget"))
            .expect("proposal card");
        assert!(card.contains("a keep  x drop"), "{card:?}");
        assert!(card.contains(glyphs::MARKER), "{card:?}");
    }

    #[test]
    fn chat_shows_the_newest_turns_that_fit() {
        let now = 10_000;
        let turns: Vec<ChatTurn> = (0..6)
            .map(|i| ChatTurn {
                at: now - (6 - i) * 60,
                role: if i % 2 == 0 {
                    ChatRole::You
                } else {
                    ChatRole::Overseer
                },
                text: if i == 3 {
                    "a long answer that wraps onto a second line at this width".into()
                } else {
                    format!("turn {i}")
                },
            })
            .collect();
        // Turn 3 costs two rows at width 30; everything else one.
        let rows = chat_rows(&turns, 30, 100, now);
        assert_eq!(rows.len(), 7);
        assert_eq!(rows[0].text, "turn 0");
        assert_eq!(rows[0].age.as_deref(), Some("6m"));
        assert_eq!(rows[3].age.as_deref(), Some("3m"));
        assert_eq!(rows[4].age, None, "a continuation row hangs, unlabelled");
        assert_eq!(rows[4].role, ChatRole::Overseer);

        // Three rows: the newest two turns; turn 3 would need two more.
        let rows = chat_rows(&turns, 30, 3, now);
        assert_eq!(
            rows.iter().map(|r| r.text.as_str()).collect::<Vec<_>>(),
            vec!["turn 4", "turn 5"],
            "a turn is whole or absent"
        );
        // Four rows: turn 3 fits whole.
        let rows = chat_rows(&turns, 30, 4, now);
        assert_eq!(rows.len(), 4);
        assert_eq!(rows[0].age.as_deref(), Some("3m"));
        assert!(chat_rows(&turns, 30, 0, now).is_empty());
        assert!(chat_rows(&[], 30, 5, now).is_empty());
    }

    #[test]
    fn chat_heading_drops_facts_in_order() {
        assert_eq!(
            chat_hint(Some("claude"), 80),
            "· claude headless · answers in a few seconds · never types into a pane"
        );
        assert_eq!(
            chat_hint(Some("claude"), 50),
            "· claude headless · never types into a pane"
        );
        assert_eq!(chat_hint(Some("claude"), 40), "· never types into a pane");
        assert_eq!(chat_hint(Some("claude"), 10), "");
        assert_eq!(
            chat_hint(None, 80),
            "· answers in a few seconds · never types into a pane"
        );
    }

    #[test]
    fn chat_region_draws_turns_pending_and_the_input() {
        let mut state = overseer_state(160, 45);
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        state.overseer.chat = crate::app::overseer::OverseerState::test_chat_fixture(now);
        state.overseer.chat_pending = true;
        state.overseer.chat_focused = true;
        state.overseer.chat_input = "ok".into();
        let rows = screen(&state, 160, 45);
        let input = rows.last().expect("input row");
        assert!(input.ends_with("› ok▮"), "{input:?}");
        let pending = &rows[rows.len() - 2];
        assert!(pending.contains("thinking…"), "{pending:?}");
        let follow_up = &rows[rows.len() - 3];
        assert!(
            follow_up.contains("you   3m  is the disk warning urgent?"),
            "{follow_up:?}"
        );
        assert!(
            rows.iter()
                .any(|r| r.contains("✦     5m  answer workmayt's claude")),
            "{rows:#?}"
        );
        // Unfocused with a draft: the draft stays, the cursor goes.
        state.overseer.chat_focused = false;
        let rows = screen(&state, 160, 45);
        assert!(rows.last().expect("input").ends_with("› ok"));
        state.overseer.chat_input.clear();
        let rows = screen(&state, 160, 45);
        assert!(rows.last().expect("input").ends_with("› ask the overseer"));
    }

    #[test]
    #[ignore = "visual preview, run with --nocapture"]
    fn preview_overseer() {
        let state = overseer_state(160, 40);
        for row in screen(&state, 160, 40) {
            println!("{row}");
        }
    }

    #[test]
    fn overseer_model_skips_system_workspaces() {
        let app = AppState::test_with_system_workspace();
        let system = AppState::TEST_SYSTEM_WS;
        let model = overseer_model(&app);
        assert!(!model.agents.iter().any(|row| row.ws_idx == system));
        assert!(!model.needs_you.iter().any(|row| row.ws_idx == system));
        assert_eq!(model.agents.len(), 2);
    }
}
