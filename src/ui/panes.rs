use ratatui::{
    layout::Rect,
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
    Frame,
};

use super::gauge::context_gauge_spans;
use super::glyphs;
use super::scrollbar::{render_pane_scrollbar, should_show_scrollbar};
use super::text::{contract_home, display_width, spans_width, truncate_end, truncate_start};
use super::widgets::panel_contrast_fg;
use crate::app::state::Palette;
use crate::app::{AppState, Mode};
use crate::layout::PaneInfo;
use crate::terminal::{TerminalRuntime, TerminalRuntimeRegistry};

pub(crate) fn pane_is_scrolled_back(rt: &TerminalRuntime) -> bool {
    rt.scroll_metrics()
        .is_some_and(|metrics| metrics.offset_from_bottom > 0)
}

/// The least room the working directory gets in a pane title before it
/// comes off the title whole. `~/…/shep` still names the repo.
const MIN_CWD_WIDTH: usize = 12;

/// The least of the agent's name the title keeps before the branch beside
/// it goes: `claude · fix/stripe-webhook` on a wide pane, `claude · fix/st…`
/// on a narrower one, `claude` alone on a narrow one.
const MIN_LABEL_WIDTH: usize = 12;

/// The least of a branch worth printing beside the agent; under this the
/// branch drops rather than reading `f…`.
const MIN_BRANCH_WIDTH: usize = 6;

/// A pane's title, in two halves: ` agent · branch ` at the left of the top
/// border and ` cwd · <gauge> NN% ` pinned to its right.
///
/// `width` is the room between the border's corners. The right half is a
/// ladder — the directory comes off before the gauge, and the gauge before
/// the title stops saying which agent this is — and the left half truncates
/// to whatever the right leaves: the branch first, down to a floor, then the
/// branch goes and the name truncates.
///
/// The state is not here. It used to trail as ` · blocked`, in the state's
/// colour; the border ring is already that colour and the sidebar row already
/// says the word, and a pane title saying it a third time cost the columns
/// the branch now has.
pub(crate) fn pane_title_parts<'a>(
    label: &str,
    branch: Option<&str>,
    cwd: Option<&str>,
    percent: Option<u8>,
    width: u16,
    focused: bool,
    p: &Palette,
) -> (Vec<Span<'a>>, Vec<Span<'a>>) {
    let label = label.trim();
    let width = width as usize;
    if label.is_empty() || width == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut label_style = Style::default().fg(if focused { p.accent } else { p.overlay0 });
    let mut branch_style = Style::default().fg(p.mauve);
    if focused {
        label_style = label_style.add_modifier(Modifier::BOLD);
        branch_style = branch_style.add_modifier(Modifier::BOLD);
    }
    let dim = Style::default().fg(p.overlay0);

    let branch = branch.map(str::trim).filter(|branch| !branch.is_empty());
    let sep_width = display_width(glyphs::SEP_SPACED);
    // The right half yields to the name on the left, and the directory
    // yields to a short branch too: a directory is a place, the branch says
    // which line of work this is, and a title that named the place but not
    // the work read as the wrong pane. The gauge outranks the branch — on a
    // narrow pane the group row beside it already names the branch, and the
    // gauge is nowhere else.
    let name_floor = display_width(label).min(MIN_LABEL_WIDTH) + 2;
    let branch_floor = if branch.is_some() {
        sep_width + MIN_BRANCH_WIDTH
    } else {
        0
    };
    let right_budget = width.saturating_sub(name_floor + branch_floor);
    let gauge_budget = width.saturating_sub(name_floor);
    let gauge: Vec<Span<'a>> = percent
        .map(|percent| context_gauge_spans(percent, p))
        .unwrap_or_default();
    let cwd = cwd
        .map(str::trim)
        .filter(|cwd| !cwd.is_empty())
        .map(|cwd| contract_home(std::path::Path::new(cwd)));
    let mut right: Vec<Span<'a>> = Vec::new();
    // Rung one: cwd and gauge. The path truncates from the front to what the
    // gauge leaves it, down to a floor that still shows the last directory.
    if let Some(cwd) = cwd.as_deref() {
        let inner = right_budget.saturating_sub(2);
        let cwd_budget = if gauge.is_empty() {
            inner
        } else {
            inner.saturating_sub(spans_width(&gauge) + sep_width)
        }
        .max(MIN_CWD_WIDTH.min(inner));
        let shown = truncate_start(cwd, cwd_budget);
        let mut candidate = vec![Span::raw(" "), Span::styled(shown, dim)];
        if !gauge.is_empty() {
            candidate.push(Span::styled(glyphs::SEP_SPACED, dim));
            candidate.extend(gauge.iter().cloned());
        }
        candidate.push(Span::raw(" "));
        if spans_width(&candidate) <= right_budget {
            right = candidate;
        }
    }
    // Rung two: the gauge alone.
    if right.is_empty() && !gauge.is_empty() && spans_width(&gauge) + 2 <= gauge_budget {
        right.push(Span::raw(" "));
        right.extend(gauge);
        right.push(Span::raw(" "));
    }

    let left_budget = width.saturating_sub(spans_width(&right));
    let inner = left_budget.saturating_sub(2);
    let mut left: Vec<Span<'a>> = vec![Span::raw(" ")];
    let label_width = display_width(label);
    match branch {
        Some(branch) if inner >= label_width + sep_width + MIN_BRANCH_WIDTH => {
            left.push(Span::styled(label.to_string(), label_style));
            left.push(Span::styled(glyphs::SEP_SPACED, dim));
            left.push(Span::styled(
                truncate_end(branch, inner - label_width - sep_width),
                branch_style,
            ));
        }
        _ => {
            if inner == 0 {
                return (Vec::new(), right);
            }
            left.push(Span::styled(truncate_end(label, inner), label_style));
        }
    }
    left.push(Span::raw(" "));
    (left, right)
}

fn stable_terminal_inner_rect(pane_inner: Rect) -> Rect {
    if pane_inner.width <= 4 {
        return pane_inner;
    }

    Rect::new(
        pane_inner.x,
        pane_inner.y,
        pane_inner.width.saturating_sub(1),
        pane_inner.height,
    )
}

pub(crate) fn pane_inner_rect(area: Rect, borders: Borders) -> Rect {
    if borders.is_empty() {
        area
    } else {
        Block::default().borders(borders).inner(area)
    }
}

fn ranges_overlap(a_start: u16, a_len: u16, b_start: u16, b_len: u16) -> bool {
    a_start < b_start.saturating_add(b_len) && b_start < a_start.saturating_add(a_len)
}

fn pane_to_right<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let right = info.rect.x.saturating_add(info.rect.width);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.x == right
            && ranges_overlap(
                info.rect.y,
                info.rect.height,
                other.rect.y,
                other.rect.height,
            )
    })
}

fn pane_below<'a>(info: &PaneInfo, panes: &'a [PaneInfo]) -> Option<&'a PaneInfo> {
    let bottom = info.rect.y.saturating_add(info.rect.height);
    panes.iter().find(|other| {
        other.id != info.id
            && other.rect.y == bottom
            && ranges_overlap(info.rect.x, info.rect.width, other.rect.x, other.rect.width)
    })
}

fn shrink_for_one_cell_gap(size: u16) -> u16 {
    if size > 1 {
        size - 1
    } else {
        size
    }
}

pub(crate) fn apply_pane_chrome(
    panes: Vec<PaneInfo>,
    pane_borders: bool,
    pane_gaps: bool,
) -> Vec<PaneInfo> {
    let multi_pane = panes.len() > 1;
    panes
        .iter()
        .cloned()
        .map(|mut info| {
            let right_neighbor = multi_pane.then(|| pane_to_right(&info, &panes)).flatten();
            let below_neighbor = multi_pane.then(|| pane_below(&info, &panes)).flatten();

            if multi_pane && pane_gaps && !pane_borders {
                if right_neighbor.is_some() {
                    info.rect.width = shrink_for_one_cell_gap(info.rect.width);
                }
                if below_neighbor.is_some() {
                    info.rect.height = shrink_for_one_cell_gap(info.rect.height);
                }
            }

            info.borders = if !multi_pane || !pane_borders {
                Borders::NONE
            } else {
                let mut borders = Borders::ALL;
                if !pane_gaps {
                    if right_neighbor.is_some() {
                        borders.remove(Borders::RIGHT);
                    }
                    if below_neighbor.is_some() {
                        borders.remove(Borders::BOTTOM);
                    }
                }
                borders
            };
            info
        })
        .collect()
}

fn runtime_for_tab_pane<'a>(
    terminal_runtimes: &'a TerminalRuntimeRegistry,
    tab: &'a crate::workspace::Tab,
    pane_id: crate::layout::PaneId,
) -> Option<(&'a crate::terminal::TerminalId, &'a TerminalRuntime)> {
    let terminal_id = tab.terminal_id(pane_id)?;
    #[cfg(test)]
    if let Some(runtime) = tab.runtimes.get(&pane_id) {
        return Some((terminal_id, runtime));
    }
    terminal_runtimes
        .get(terminal_id)
        .map(|runtime| (terminal_id, runtime))
}

fn stable_scrollbar_gutter(rt: &TerminalRuntime, pane_inner: Rect) -> (Rect, Option<Rect>) {
    let inner_rect = stable_terminal_inner_rect(pane_inner);
    if inner_rect == pane_inner {
        return (inner_rect, None);
    }
    let gutter = Rect::new(
        pane_inner.x + pane_inner.width.saturating_sub(1),
        pane_inner.y,
        1,
        pane_inner.height,
    );
    let scrollbar_rect = rt
        .scroll_metrics()
        .filter(|metrics| should_show_scrollbar(*metrics))
        .map(|_| gutter);

    (inner_rect, scrollbar_rect)
}

/// Resize every visible runtime in a tab to the geometry it would receive if the tab were selected.
pub(super) fn resize_tab_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    tab: &crate::workspace::Tab,
    area: Rect,
    cell_size: crate::kitty_graphics::HostCellSize,
) {
    let multi_pane = tab.layout.pane_count() > 1;

    if tab.zoomed {
        let focused_id = tab.layout.focused();
        if let Some((terminal_id, rt)) = runtime_for_tab_pane(terminal_runtimes, tab, focused_id) {
            let borders = if multi_pane && app.pane_borders {
                Borders::ALL
            } else {
                Borders::NONE
            };
            let pane_inner = pane_inner_rect(area, borders);
            let inner_rect = stable_terminal_inner_rect(pane_inner);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return;
    }

    for info in apply_pane_chrome(tab.layout.panes(area), app.pane_borders, app.pane_gaps) {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        if let Some((terminal_id, rt)) = runtime_for_tab_pane(terminal_runtimes, tab, info.id) {
            let inner_rect = stable_terminal_inner_rect(pane_inner);
            if !app.direct_attach_resize_locks.contains(terminal_id) {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
    }
}

/// Compute pane layout info and optionally resize pane runtimes to match.
pub(super) fn compute_pane_infos(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    area: Rect,
    resize_panes: bool,
    cell_size: crate::kitty_graphics::HostCellSize,
) -> Vec<PaneInfo> {
    let Some(ws_idx) = app.active else {
        return Vec::new();
    };
    let Some(ws) = app.workspaces.get(ws_idx) else {
        return Vec::new();
    };

    let multi_pane = ws.layout.pane_count() > 1;

    if ws.zoomed {
        let focused_id = ws.layout.focused();
        let borders = if multi_pane && app.pane_borders {
            Borders::ALL
        } else {
            Borders::NONE
        };
        let pane_inner = pane_inner_rect(area, borders);
        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, focused_id) {
            (inner_rect, scrollbar_rect) = stable_scrollbar_gutter(rt, pane_inner);
            if resize_panes
                && ws.terminal_id(focused_id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }
        return vec![PaneInfo {
            id: focused_id,
            rect: area,
            inner_rect,
            scrollbar_rect,
            borders,
            is_focused: true,
        }];
    }

    let mut pane_infos = apply_pane_chrome(ws.layout.panes(area), app.pane_borders, app.pane_gaps);

    for info in &mut pane_infos {
        let pane_inner = pane_inner_rect(info.rect, info.borders);

        let mut inner_rect = pane_inner;
        let mut scrollbar_rect = None;
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            (inner_rect, scrollbar_rect) = stable_scrollbar_gutter(rt, pane_inner);
            if resize_panes
                && ws.terminal_id(info.id).is_some_and(|terminal_id| {
                    !app.direct_attach_resize_locks.contains(terminal_id)
                })
            {
                rt.resize(
                    inner_rect.height,
                    inner_rect.width,
                    cell_size.width_px,
                    cell_size.height_px,
                );
            }
        }

        info.inner_rect = inner_rect;
        info.scrollbar_rect = scrollbar_rect;
    }

    pane_infos
}

pub(super) fn render_panes(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let Some(ws_idx) = app.active else {
        render_empty(app, frame, area);
        return;
    };
    let Some(ws) = app.workspaces.get(ws_idx) else {
        render_empty(app, frame, area);
        return;
    };

    let multi_pane = ws.layout.pane_count() > 1;
    let terminal_active = app.mode == Mode::Terminal;

    for info in &app.view.pane_infos {
        if let Some(rt) = app.runtime_for_pane_in_workspace(terminal_runtimes, ws_idx, info.id) {
            let show_cursor = info.is_focused
                && terminal_active
                && !pane_is_scrolled_back(rt)
                && app.pane_exposes_host_cursor(ws_idx, info.id);
            rt.render(frame, info.inner_rect, show_cursor);
            render_pane_scrollbar(app, frame, info, rt);

            let should_dim = !info.is_focused && multi_pane && !terminal_active;
            if should_dim {
                let inner = info.inner_rect;
                let buf = frame.buffer_mut();
                for y in inner.y..inner.y + inner.height {
                    for x in inner.x..inner.x + inner.width {
                        let cell = &mut buf[(x, y)];
                        cell.set_style(cell.style().add_modifier(Modifier::DIM));
                    }
                }
            }

            render_selection_highlight(
                &app.selection,
                frame,
                info.id,
                info.inner_rect,
                rt.scroll_metrics(),
                &app.palette,
                app.host_terminal_theme,
            );
            render_copy_mode_cursor(app, frame, info);
        }
    }

    render_pane_borders(app, ws, frame);
}

#[derive(Clone, Copy, Default)]
struct LineCell {
    up: bool,
    down: bool,
    left: bool,
    right: bool,
}

/// State-border-ring color for a pane running a recognized agent, paired with
/// its attention priority for tie-breaking on shared border cells. Reuses the
/// sidebar's state color scheme (blocked/working/done/idle). `None` when rings
/// are disabled, the pane has no recognized agent, or its state is unknown.
fn pane_ring_color(
    app: &AppState,
    ws: &crate::workspace::Workspace,
    pane_id: crate::layout::PaneId,
) -> Option<(u8, Color)> {
    let pane = ws.pane_state(pane_id)?;
    let terminal = app.terminals.get(&pane.attached_terminal_id)?;
    terminal
        .effective_known_agent()
        .or(terminal.detected_agent)?;
    if terminal.state == crate::detect::AgentState::Unknown {
        return None;
    }
    Some((
        crate::workspace::attention_priority(terminal.state, pane.seen),
        super::status::state_label_color(terminal.state, pane.seen, &app.palette),
    ))
}

fn render_pane_borders(app: &AppState, ws: &crate::workspace::Workspace, frame: &mut Frame) {
    if !app.pane_borders
        || app
            .view
            .pane_infos
            .iter()
            .all(|info| info.borders.is_empty())
    {
        return;
    }

    let mut cells = std::collections::HashMap::<(u16, u16), LineCell>::new();
    for info in &app.view.pane_infos {
        add_pane_border_cells(&mut cells, info);
    }
    add_split_border_cells(app, &mut cells);

    // Precompute state-ring colors once per pane. This only recolors glyphs that
    // are already drawn, so it stays cheap over mosh (no extra redraws).
    let ring_colors: std::collections::HashMap<crate::layout::PaneId, (u8, Color)> =
        if app.state_border_rings {
            app.view
                .pane_infos
                .iter()
                .filter_map(|info| pane_ring_color(app, ws, info.id).map(|ring| (info.id, ring)))
                .collect()
        } else {
            std::collections::HashMap::new()
        };

    let buf = frame.buffer_mut();
    let area = buf.area;
    for ((x, y), line) in cells {
        if x < area.x
            || x >= area.x.saturating_add(area.width)
            || y < area.y
            || y >= area.y.saturating_add(area.height)
        {
            continue;
        }
        let focused = app
            .view
            .pane_infos
            .iter()
            .any(|info| info.is_focused && line_touches_pane(x, y, info, app.pane_gaps));
        let symbol = line_cell_symbol(line);
        if symbol.is_empty() {
            continue;
        }
        let cell = &mut buf[(x, y)];
        cell.set_symbol(symbol);
        // Focused pane keeps the accent highlight so the active pane is obvious;
        // other panes get a state ring when one applies, else the dim default.
        let color = if focused {
            app.palette.accent
        } else if !ring_colors.is_empty() {
            app.view
                .pane_infos
                .iter()
                .filter(|info| line_touches_pane(x, y, info, app.pane_gaps))
                .filter_map(|info| ring_colors.get(&info.id).copied())
                .max_by_key(|(priority, _)| *priority)
                .map(|(_, ring_color)| ring_color)
                .unwrap_or(app.palette.overlay0)
        } else {
            app.palette.overlay0
        };
        cell.set_style(Style::default().fg(color));
    }

    render_pane_border_titles(app, ws, frame);
}

fn add_split_border_cells(
    app: &AppState,
    cells: &mut std::collections::HashMap<(u16, u16), LineCell>,
) {
    if app.pane_gaps {
        return;
    }

    for split in &app.view.split_borders {
        match split.direction {
            ratatui::layout::Direction::Horizontal => {
                let x = split.pos;
                let end = split.area.y.saturating_add(split.area.height);
                for y in split.area.y..=end {
                    if !cells.contains_key(&(x, y)) {
                        continue;
                    }
                    let left = x
                        .checked_sub(1)
                        .and_then(|left_x| cells.get(&(left_x, y)))
                        .is_some_and(|cell| cell.left || cell.right);
                    let right = cells
                        .get(&(x.saturating_add(1), y))
                        .is_some_and(|cell| cell.left || cell.right);
                    let cell = cells.entry((x, y)).or_default();
                    cell.up |= y > split.area.y;
                    cell.down |= y + 1 < end;
                    cell.left |= left;
                    cell.right |= right;
                }
            }
            ratatui::layout::Direction::Vertical => {
                let y = split.pos;
                let end = split.area.x.saturating_add(split.area.width);
                for x in split.area.x..=end {
                    if !cells.contains_key(&(x, y)) {
                        continue;
                    }
                    let up = y
                        .checked_sub(1)
                        .and_then(|up_y| cells.get(&(x, up_y)))
                        .is_some_and(|cell| cell.up || cell.down);
                    let down = cells
                        .get(&(x, y.saturating_add(1)))
                        .is_some_and(|cell| cell.up || cell.down);
                    let cell = cells.entry((x, y)).or_default();
                    cell.left |= x > split.area.x;
                    cell.right |= x + 1 < end;
                    cell.up |= up;
                    cell.down |= down;
                }
            }
        }
    }
}

fn add_pane_border_cells(
    cells: &mut std::collections::HashMap<(u16, u16), LineCell>,
    info: &PaneInfo,
) {
    let rect = info.rect;
    if rect.width == 0 || rect.height == 0 {
        return;
    }
    let right = rect.x.saturating_add(rect.width).saturating_sub(1);
    let bottom = rect.y.saturating_add(rect.height).saturating_sub(1);

    if info.borders.contains(Borders::TOP) {
        for x in rect.x..=right {
            let cell = cells.entry((x, rect.y)).or_default();
            cell.left |= x > rect.x;
            cell.right |= x < right;
        }
    }
    if info.borders.contains(Borders::BOTTOM) {
        for x in rect.x..=right {
            let cell = cells.entry((x, bottom)).or_default();
            cell.left |= x > rect.x;
            cell.right |= x < right;
        }
    }
    if info.borders.contains(Borders::LEFT) {
        for y in rect.y..=bottom {
            let cell = cells.entry((rect.x, y)).or_default();
            cell.up |= y > rect.y;
            cell.down |= y < bottom;
        }
    }
    if info.borders.contains(Borders::RIGHT) {
        for y in rect.y..=bottom {
            let cell = cells.entry((right, y)).or_default();
            cell.up |= y > rect.y;
            cell.down |= y < bottom;
        }
    }
}

fn line_touches_pane(x: u16, y: u16, info: &PaneInfo, pane_gaps: bool) -> bool {
    let rect = info.rect;
    if rect.width == 0 || rect.height == 0 {
        return false;
    }
    let right = rect.x.saturating_add(rect.width).saturating_sub(1);
    let bottom = rect.y.saturating_add(rect.height).saturating_sub(1);
    let in_rows = y >= rect.y && y <= bottom;
    let in_cols = x >= rect.x && x <= right;
    let own_border =
        (in_rows && (x == rect.x || x == right)) || (in_cols && (y == rect.y || y == bottom));

    if pane_gaps {
        return own_border;
    }

    let shared_right = rect.x.saturating_add(rect.width);
    let shared_bottom = rect.y.saturating_add(rect.height);
    own_border
        || (in_rows && x == shared_right)
        || (in_cols && y == shared_bottom)
        || (x == shared_right && y == shared_bottom)
}

fn render_pane_border_titles(app: &AppState, ws: &crate::workspace::Workspace, frame: &mut Frame) {
    let buf = frame.buffer_mut();
    let area = buf.area;
    let branch = ws.branch();
    for info in &app.view.pane_infos {
        if !info.borders.contains(Borders::TOP) || info.rect.width <= 4 {
            continue;
        }
        let pane = ws.pane_state(info.id);
        let Some(terminal) = pane.and_then(|pane| app.terminals.get(&pane.attached_terminal_id))
        else {
            continue;
        };
        let Some(label) = terminal.border_label(app.show_agent_labels_on_pane_borders) else {
            continue;
        };
        let y = info.rect.y;
        if y < area.y || y >= area.y.saturating_add(area.height) {
            continue;
        }
        let start_x = info.rect.x.saturating_add(1);
        let end_x = info
            .rect
            .x
            .saturating_add(info.rect.width)
            .saturating_sub(1)
            .min(area.x.saturating_add(area.width));
        if start_x >= end_x {
            continue;
        }
        let cwd = terminal.cwd.to_string_lossy();
        let (left, right) = pane_title_parts(
            &label,
            branch.as_deref(),
            Some(cwd.as_ref()),
            terminal.context_percent,
            end_x.saturating_sub(start_x),
            info.is_focused,
            &app.palette,
        );
        buf.set_line(start_x, y, &Line::from(left), end_x.saturating_sub(start_x));
        // The right half runs up to the corner: its own trailing space is the
        // one column in that every strip keeps at its right edge, the mirror
        // of the space after `╭` on the left.
        let right_width = u16::try_from(spans_width(&right)).unwrap_or(u16::MAX);
        let right_x = end_x.saturating_sub(right_width);
        if right_width > 0 && right_x >= start_x {
            buf.set_line(right_x, y, &Line::from(right), right_width);
        }
    }
}

fn line_cell_symbol(line: LineCell) -> &'static str {
    match (line.up, line.down, line.left, line.right) {
        (true, true, true, true) => "┼",
        (true, true, true, false) => "┤",
        (true, true, false, true) => "├",
        (true, false, true, true) => "┴",
        (false, true, true, true) => "┬",
        (true, true, false, false) | (true, false, false, false) | (false, true, false, false) => {
            "│"
        }
        (false, false, true, true) | (false, false, true, false) | (false, false, false, true) => {
            "─"
        }
        (false, true, false, true) => "╭",
        (false, true, true, false) => "╮",
        (true, false, false, true) => "╰",
        (true, false, true, false) => "╯",
        _ => "",
    }
}

fn render_copy_mode_cursor(app: &AppState, frame: &mut Frame, info: &PaneInfo) {
    if app.mode != Mode::Copy {
        return;
    }
    let Some(copy_mode) = app.copy_mode else {
        return;
    };
    if copy_mode.pane_id != info.id
        || copy_mode.cursor_row >= info.inner_rect.height
        || copy_mode.cursor_col >= info.inner_rect.width
    {
        return;
    }

    let x = info.inner_rect.x + copy_mode.cursor_col;
    let y = info.inner_rect.y + copy_mode.cursor_row;
    let cell = &mut frame.buffer_mut()[(x, y)];
    cell.set_style(
        Style::default()
            .fg(panel_contrast_fg(&app.palette))
            .bg(app.palette.accent)
            .add_modifier(Modifier::BOLD),
    );
}

fn render_selection_highlight(
    selection: &Option<crate::selection::Selection>,
    frame: &mut Frame,
    pane_id: crate::layout::PaneId,
    inner: Rect,
    scroll_metrics: Option<crate::pane::ScrollMetrics>,
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) {
    if let Some(sel) = selection {
        if sel.is_visible() && sel.pane_id == pane_id {
            let buf = frame.buffer_mut();
            let style = automatic_selection_style(p, host_theme);
            for y in 0..inner.height {
                for x in 0..inner.width {
                    if sel.contains(y, x, scroll_metrics) {
                        let cell = &mut buf[(inner.x + x, inner.y + y)];
                        cell.set_style(style);
                    }
                }
            }
        }
    }
}

type Rgb = (u8, u8, u8);

fn automatic_selection_style(
    p: &Palette,
    host_theme: crate::terminal_theme::TerminalTheme,
) -> Style {
    let bg = automatic_selection_bg(p, host_theme);
    Style::reset().fg(selection_fg_for_bg(bg, p)).bg(bg)
}

fn automatic_selection_bg(p: &Palette, host_theme: crate::terminal_theme::TerminalTheme) -> Color {
    let Some(background) = host_theme.background.map(terminal_theme_to_rgb) else {
        return selection_palette_background(p);
    };

    let target = if relative_luminance(background) < 0.5 {
        (255, 255, 255)
    } else {
        (0, 0, 0)
    };
    let selected = mix_rgb(background, target, 0.28);
    Color::Rgb(selected.0, selected.1, selected.2)
}

fn selection_palette_background(p: &Palette) -> Color {
    if p.panel_bg == Color::Reset {
        p.surface_dim
    } else {
        p.panel_bg
    }
}

fn terminal_theme_to_rgb(color: crate::terminal_theme::RgbColor) -> Rgb {
    (color.r, color.g, color.b)
}

fn selection_fg_for_bg(bg: Color, p: &Palette) -> Color {
    color_to_rgb(bg)
        .map(|bg| {
            if relative_luminance(bg) < 0.5 {
                Color::White
            } else {
                Color::Black
            }
        })
        .unwrap_or_else(|| panel_contrast_fg(p))
}

fn mix_rgb(base: Rgb, target: Rgb, amount: f32) -> Rgb {
    fn channel(base: u8, target: u8, amount: f32) -> u8 {
        (f32::from(base) + (f32::from(target) - f32::from(base)) * amount).round() as u8
    }
    (
        channel(base.0, target.0, amount),
        channel(base.1, target.1, amount),
        channel(base.2, target.2, amount),
    )
}

fn relative_luminance(color: Rgb) -> f32 {
    fn channel(value: u8) -> f32 {
        let value = f32::from(value) / 255.0;
        if value <= 0.03928 {
            value / 12.92
        } else {
            ((value + 0.055) / 1.055).powf(2.4)
        }
    }
    0.2126 * channel(color.0) + 0.7152 * channel(color.1) + 0.0722 * channel(color.2)
}

fn color_to_rgb(color: Color) -> Option<Rgb> {
    match color {
        Color::Reset => None,
        Color::Black => Some((0, 0, 0)),
        Color::Red => Some((128, 0, 0)),
        Color::Green => Some((0, 128, 0)),
        Color::Yellow => Some((128, 128, 0)),
        Color::Blue => Some((0, 0, 128)),
        Color::Magenta => Some((128, 0, 128)),
        Color::Cyan => Some((0, 128, 128)),
        Color::Gray => Some((192, 192, 192)),
        Color::DarkGray => Some((128, 128, 128)),
        Color::LightRed => Some((255, 0, 0)),
        Color::LightGreen => Some((0, 255, 0)),
        Color::LightYellow => Some((255, 255, 0)),
        Color::LightBlue => Some((0, 0, 255)),
        Color::LightMagenta => Some((255, 0, 255)),
        Color::LightCyan => Some((0, 255, 255)),
        Color::White => Some((255, 255, 255)),
        Color::Rgb(r, g, b) => Some((r, g, b)),
        Color::Indexed(_) => None,
    }
}

fn render_empty(app: &AppState, frame: &mut Frame, area: Rect) {
    let p = &app.palette;
    let lines = vec![
        Line::from(""),
        Line::from(""),
        Line::from(Span::styled(
            "  No groups yet",
            Style::default().fg(p.overlay0),
        )),
        Line::from(""),
        Line::from(Span::styled(
            "  A group is one project context.",
            Style::default().fg(p.overlay1),
        )),
        Line::from(Span::styled(
            "  Its root pane (top-left) sets the default repo or folder name.",
            Style::default().fg(p.overlay1),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("  Press ", Style::default().fg(p.overlay0)),
            Span::styled(
                app.keybinds
                    .new_workspace
                    .label()
                    .unwrap_or_else(|| "unset".to_string()),
                Style::default().fg(p.accent).add_modifier(Modifier::BOLD),
            ),
            Span::styled(" to create one", Style::default().fg(p.overlay0)),
        ]),
    ];
    frame.render_widget(
        Paragraph::new(lines).block(
            Block::default()
                .borders(Borders::ALL)
                .border_set(ratatui::symbols::border::ROUNDED)
                .border_style(Style::default().fg(p.surface_dim)),
        ),
        area,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::detect::AgentState;
    use crate::layout::PaneId;
    use crate::selection::Selection;
    use crate::terminal::TerminalRuntime;
    use crate::terminal::TerminalState;
    use crate::workspace::Workspace;

    fn title_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn title_at(
        label: &str,
        branch: Option<&str>,
        cwd: Option<&str>,
        percent: Option<u8>,
        width: u16,
    ) -> (String, String) {
        let p = Palette::shep();
        let (left, right) = pane_title_parts(label, branch, cwd, percent, width, false, &p);
        (title_text(&left), title_text(&right))
    }

    #[test]
    fn pane_title_trims_and_truncates_the_label() {
        assert_eq!(title_at(" claude ", None, None, None, 16).0, " claude ");
        assert_eq!(
            title_at("", None, None, None, 16),
            (String::new(), String::new())
        );
        assert_eq!(title_at("abcdef", None, None, None, 6).0, " abc… ");
        assert_eq!(
            title_at("abcdef", None, None, None, 0),
            (String::new(), String::new())
        );
        let p = Palette::shep();
        let (left, _) = pane_title_parts("claude", None, None, None, 20, true, &p);
        assert_eq!(left[1].style.fg, Some(p.accent));
        assert!(left[1].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn pane_title_truncates_cjk_by_display_width() {
        let (left, _) = title_at("1 模块组织（已定）", None, None, None, 10);

        assert_eq!(left, " 1 模块… ");
        assert!(display_width(left.as_str()) <= 10);
    }

    /// Wide: ` agent · branch ` on the left and ` cwd · gauge ` on the right.
    /// Narrowing drops the directory first, then the gauge, and only then
    /// does the branch give way — the name outlives everything.
    #[test]
    fn pane_title_drops_cwd_before_the_gauge() {
        let full = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            80,
        );
        let gauge = title_text(&context_gauge_spans(72, &Palette::shep()));
        assert_eq!(full.0, " claude · fix/stripe-webhook ");
        assert_eq!(full.1, format!(" ~/vault/dev/workmayt · {gauge} "));

        // Too narrow for the whole path: it truncates from the front, and
        // the branch beside the name shrinks to its floor rather than going.
        let squeezed = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            50,
        );
        assert_eq!(squeezed.1, format!(" …ult/dev/workmayt · {gauge} "));
        assert_eq!(squeezed.0, " claude · fix/s… ");

        // Under the path's floor it comes off whole, the gauge stays, and the
        // branch has its room back.
        let no_cwd = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            40,
        );
        assert_eq!(no_cwd.1, format!(" {gauge} "));
        assert_eq!(no_cwd.0, " claude · fix/stripe-webh… ");

        // Narrower still, the gauge outranks the branch: the group row in
        // the sidebar names the branch, and the gauge is nowhere else.
        let gauge_only = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            24,
        );
        assert_eq!(gauge_only.1, format!(" {gauge} "));
        assert_eq!(gauge_only.0, " claude ");

        // Then the gauge goes and the branch has the room, down to its floor.
        let name_only = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            17,
        );
        assert_eq!(name_only.1, "");
        assert_eq!(name_only.0, " claude · fix/s… ");

        // Under the branch's floor the branch drops whole.
        let bare = title_at(
            "claude",
            Some("fix/stripe-webhook"),
            Some("~/vault/dev/workmayt"),
            Some(72),
            12,
        );
        assert_eq!(bare, (" claude ".to_string(), String::new()));

        // Every width fits, and the halves never overlap.
        for width in 0..90u16 {
            let (left, right) = title_at(
                "claude",
                Some("fix/stripe-webhook"),
                Some("~/vault/dev/workmayt"),
                Some(72),
                width,
            );
            assert!(
                display_width(&left) + display_width(&right) <= width as usize,
                "{width}: {left:?} {right:?}"
            );
        }
    }

    /// The border ring and the sidebar row carry the state; the title does
    /// not say it a third time.
    #[test]
    fn pane_title_has_no_state_suffix() {
        let (mut app, ws, _) = titled_pane_app(80, AgentState::Blocked);
        app.show_agent_labels_on_pane_borders = true;
        let row = top_border_row(&app, &ws, 80);
        assert!(row.contains(" claude · fix/stripe-webhook "), "{row:?}");
        assert!(!row.contains("blocked"), "{row:?}");
        assert!(!row.contains("· blocked"), "{row:?}");
    }

    /// The right half's own trailing space is the one column in from the
    /// corner, the mirror of the space after `╭`.
    #[test]
    fn pane_title_gauge_ends_one_column_in() {
        let width = 60u16;
        let (mut app, ws, _) = titled_pane_app(width, AgentState::Idle);
        app.show_agent_labels_on_pane_borders = true;
        let row = top_border_row(&app, &ws, width);
        let cells = row_cells(&app, &ws, width);
        let last = usize::from(width) - 1;
        assert_eq!(cells[last], "╮", "{row:?}");
        assert_eq!(cells[last - 1], " ", "{row:?}");
        assert_eq!(cells[last - 2], "%", "{row:?}");
        assert_eq!(cells[0], "╭", "{row:?}");
        assert_eq!(cells[1], " ", "{row:?}");
        assert_eq!(cells[2], "c", "{row:?}");
    }

    fn titled_pane_app(
        width: u16,
        state: AgentState,
    ) -> (AppState, Workspace, crate::terminal::TerminalId) {
        let mut app = AppState::test_new();
        app.mode = Mode::Terminal;
        app.view.terminal_area = Rect::new(0, 0, width, 3);
        let mut ws = Workspace::test_new("test");
        ws.cached_git_branch = Some("fix/stripe-webhook".into());
        let pane_id = ws.tabs[0].root_pane;
        app.view.pane_infos = vec![PaneInfo {
            id: pane_id,
            rect: Rect::new(0, 0, width, 3),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: true,
        }];
        let terminal_id = ws.tabs[0].panes[&pane_id].attached_terminal_id.clone();
        let mut terminal_state =
            TerminalState::new(terminal_id.clone(), "/tmp/vault/dev/workmayt".into());
        terminal_state.detected_agent = Some(crate::detect::Agent::Claude);
        terminal_state.agent_name = Some("claude".into());
        terminal_state.state = state;
        terminal_state.context_percent = Some(72);
        app.terminals.insert(terminal_id.clone(), terminal_state);
        (app, ws, terminal_id)
    }

    fn row_cells(app: &AppState, ws: &Workspace, width: u16) -> Vec<String> {
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(width, 3)).unwrap();
        terminal
            .draw(|frame| render_pane_borders(app, ws, frame))
            .unwrap();
        let buffer = terminal.backend().buffer();
        (0..width)
            .map(|x| buffer[(x, 0)].symbol().to_string())
            .collect()
    }

    fn top_border_row(app: &AppState, ws: &Workspace, width: u16) -> String {
        row_cells(app, ws, width).concat()
    }

    #[test]
    fn pane_border_renderer_places_adjacent_cjk_by_display_width() {
        let mut app = AppState::test_new();
        app.mode = Mode::Terminal;
        app.view.terminal_area = Rect::new(0, 0, 12, 3);
        let ws = Workspace::test_new("test");
        let pane_id = ws.tabs[0].root_pane;
        app.view.pane_infos = vec![PaneInfo {
            id: pane_id,
            rect: Rect::new(0, 0, 12, 3),
            inner_rect: Rect::default(),
            scrollbar_rect: None,
            borders: Borders::ALL,
            is_focused: false,
        }];

        let terminal_id = ws.tabs[0].panes[&pane_id].attached_terminal_id.clone();
        let mut terminal_state = TerminalState::new(terminal_id.clone(), "/tmp".into());
        terminal_state.set_manual_label("1 模块组织（已定）".into());
        app.terminals.insert(terminal_id, terminal_state);

        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(12, 3)).unwrap();
        terminal
            .draw(|frame| render_pane_borders(&app, &ws, frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(4, 0)].symbol(), "模");
        assert_eq!(buffer[(5, 0)].symbol(), " ");
        assert_eq!(buffer[(6, 0)].symbol(), "块");
    }

    #[test]
    fn default_horizontal_split_uses_one_shared_divider_column() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            true,
            false,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect.x + left.rect.width, right.rect.x);
        assert!(!left.borders.contains(Borders::RIGHT));
        assert!(right.borders.contains(Borders::LEFT));
    }

    #[test]
    fn default_vertical_split_uses_one_shared_divider_row() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let bottom = workspace.test_split(ratatui::layout::Direction::Vertical);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            true,
            false,
        );
        let top = infos.iter().find(|info| info.id == root).unwrap();
        let bottom = infos.iter().find(|info| info.id == bottom).unwrap();

        assert_eq!(top.rect.y + top.rect.height, bottom.rect.y);
        assert!(!top.borders.contains(Borders::BOTTOM));
        assert!(bottom.borders.contains(Borders::TOP));
    }

    #[test]
    fn pane_gaps_keep_independent_bordered_panes() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            true,
            true,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect.x + left.rect.width, right.rect.x);
        assert_eq!(left.borders, Borders::ALL);
        assert_eq!(right.borders, Borders::ALL);
    }

    #[test]
    fn borderless_pane_gaps_add_one_empty_cell_between_panes() {
        let mut workspace = Workspace::test_new("test");
        let root = workspace.tabs[0].root_pane;
        let right = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.tabs[0].layout.focus_pane(root);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            false,
            true,
        );
        let left = infos.iter().find(|info| info.id == root).unwrap();
        let right = infos.iter().find(|info| info.id == right).unwrap();

        assert_eq!(left.rect, Rect::new(0, 0, 49, 20));
        assert_eq!(right.rect, Rect::new(50, 0, 50, 20));
        assert!(left.borders.is_empty());
        assert!(right.borders.is_empty());
    }

    #[test]
    fn disabled_pane_borders_make_inner_rect_equal_visual_rect() {
        let mut workspace = Workspace::test_new("test");
        workspace.test_split(ratatui::layout::Direction::Horizontal);

        let infos = apply_pane_chrome(
            workspace.tabs[0].layout.panes(Rect::new(0, 0, 100, 20)),
            false,
            false,
        );

        for info in infos {
            assert!(info.borders.is_empty());
            assert_eq!(pane_inner_rect(info.rect, info.borders), info.rect);
        }
    }

    #[test]
    fn global_pane_border_renderer_composes_junctions_and_focus_style() {
        let mut app = AppState::test_new();
        app.mode = Mode::Terminal;
        app.view.terminal_area = Rect::new(0, 0, 4, 4);
        app.view.pane_infos = vec![
            PaneInfo {
                id: PaneId::from_raw(1),
                rect: Rect::new(0, 0, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT,
                is_focused: true,
            },
            PaneInfo {
                id: PaneId::from_raw(2),
                rect: Rect::new(2, 0, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT | Borders::RIGHT,
                is_focused: false,
            },
            PaneInfo {
                id: PaneId::from_raw(3),
                rect: Rect::new(0, 2, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::TOP | Borders::LEFT | Borders::BOTTOM,
                is_focused: false,
            },
            PaneInfo {
                id: PaneId::from_raw(4),
                rect: Rect::new(2, 2, 2, 2),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: false,
            },
        ];
        app.view.split_borders = vec![
            crate::layout::SplitBorder {
                pos: 2,
                direction: ratatui::layout::Direction::Horizontal,
                ratio: 0.5,
                area: Rect::new(0, 0, 4, 4),
                path: vec![],
            },
            crate::layout::SplitBorder {
                pos: 2,
                direction: ratatui::layout::Direction::Vertical,
                ratio: 0.5,
                area: Rect::new(0, 0, 4, 4),
                path: vec![false],
            },
        ];
        let ws = Workspace::test_new("test");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 4)).unwrap();

        terminal
            .draw(|frame| render_pane_borders(&app, &ws, frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(2, 2)].symbol(), "┼");
        assert_eq!(buffer[(2, 2)].style().fg, Some(app.palette.accent));
        assert_eq!(buffer[(2, 1)].symbol(), "│");
        assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.accent));
    }

    #[test]
    fn gapped_pane_focus_does_not_color_neighbor_border() {
        let mut app = AppState::test_new();
        app.mode = Mode::Terminal;
        app.pane_gaps = true;
        app.view.terminal_area = Rect::new(0, 0, 4, 3);
        app.view.pane_infos = vec![
            PaneInfo {
                id: PaneId::from_raw(1),
                rect: Rect::new(0, 0, 2, 3),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: true,
            },
            PaneInfo {
                id: PaneId::from_raw(2),
                rect: Rect::new(2, 0, 2, 3),
                inner_rect: Rect::default(),
                scrollbar_rect: None,
                borders: Borders::ALL,
                is_focused: false,
            },
        ];
        let ws = Workspace::test_new("test");
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(4, 3)).unwrap();

        terminal
            .draw(|frame| render_pane_borders(&app, &ws, frame))
            .unwrap();

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(1, 1)].style().fg, Some(app.palette.accent));
        assert_eq!(buffer[(2, 1)].style().fg, Some(app.palette.overlay0));
    }

    #[tokio::test]
    async fn pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[tokio::test]
    async fn zoomed_pane_scrollbar_gutter_is_reserved_before_scrollback_exists() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        workspace.zoomed = true;
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[tokio::test]
    async fn zoomed_multi_pane_keeps_border_space() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let focused_pane = workspace.test_split(ratatui::layout::Direction::Horizontal);
        workspace.zoomed = true;
        workspace.tabs[0].runtimes.insert(
            focused_pane,
            TerminalRuntime::test_with_scrollback_bytes(40, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.id, focused_pane);
        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, Rect::new(11, 4, 37, 6));
    }

    #[tokio::test]
    async fn tiny_pane_does_not_reserve_scrollbar_gutter() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(4, 8, 1024, b"ready\n"),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 4, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, None);
        assert_eq!(info.inner_rect, area);
    }

    #[tokio::test]
    async fn pane_scrollbar_reserves_last_column_from_terminal_area() {
        let mut app = AppState::test_new();
        let mut workspace = Workspace::test_new("test");
        let root_pane = workspace.tabs[0].root_pane;
        workspace.tabs[0].runtimes.insert(
            root_pane,
            TerminalRuntime::test_with_scrollback_bytes(
                40,
                8,
                1024,
                b"one\ntwo\nthree\nfour\nfive\nsix\nseven\neight\nnine\nten\n",
            ),
        );
        app.workspaces = vec![workspace];
        app.active = Some(0);

        let area = Rect::new(10, 3, 40, 8);
        let terminal_runtimes = TerminalRuntimeRegistry::new();
        let infos = compute_pane_infos(
            &app,
            &terminal_runtimes,
            area,
            false,
            crate::kitty_graphics::HostCellSize::default(),
        );
        let info = &infos[0];

        assert_eq!(info.rect, area);
        assert_eq!(info.scrollbar_rect, Some(Rect::new(49, 3, 1, 8)));
        assert_eq!(info.inner_rect, Rect::new(10, 3, 39, 8));
    }

    #[test]
    fn selection_highlight_uses_one_uniform_style() {
        let palette = Palette::catppuccin();
        let host_theme = crate::terminal_theme::TerminalTheme {
            foreground: None,
            background: Some(crate::terminal_theme::RgbColor {
                r: 12,
                g: 14,
                b: 16,
            }),
        };
        let expected_style = automatic_selection_style(&palette, host_theme);
        let selection = Some(Selection::range(PaneId::from_raw(1), 0, 0, 2, None));
        let backend = ratatui::backend::TestBackend::new(4, 1);
        let mut terminal = ratatui::Terminal::new(backend).unwrap();

        terminal
            .draw(|frame| {
                let buf = frame.buffer_mut();
                buf[(0, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(10, 220, 120))
                        .bg(Color::Black),
                );
                buf[(1, 0)].set_style(
                    Style::default()
                        .fg(Color::Rgb(220, 180, 40))
                        .bg(Color::DarkGray)
                        .add_modifier(Modifier::BOLD),
                );
                buf[(2, 0)].set_style(Style::default().fg(Color::Blue).bg(Color::Reset));
                render_selection_highlight(
                    &selection,
                    frame,
                    PaneId::from_raw(1),
                    Rect::new(0, 0, 4, 1),
                    None,
                    &palette,
                    host_theme,
                );
            })
            .unwrap();

        let buffer = terminal.backend().buffer();
        let first = buffer[(0, 0)].style();
        let second = buffer[(1, 0)].style();
        let third = buffer[(2, 0)].style();

        assert_eq!(first.fg, expected_style.fg);
        assert_eq!(second.fg, expected_style.fg);
        assert_eq!(third.fg, expected_style.fg);
        assert_eq!(first.bg, expected_style.bg);
        assert_eq!(second.bg, expected_style.bg);
        assert_eq!(third.bg, expected_style.bg);
        assert_eq!(first.add_modifier, expected_style.add_modifier);
        assert_eq!(second.add_modifier, expected_style.add_modifier);
        assert_eq!(third.add_modifier, expected_style.add_modifier);
        assert!(!second.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn automatic_selection_background_uses_host_background() {
        let bg = automatic_selection_bg(
            &Palette::terminal(),
            crate::terminal_theme::TerminalTheme {
                foreground: Some(crate::terminal_theme::RgbColor {
                    r: 230,
                    g: 230,
                    b: 230,
                }),
                background: Some(crate::terminal_theme::RgbColor {
                    r: 12,
                    g: 14,
                    b: 16,
                }),
            },
        );

        let Color::Rgb(r, g, b) = bg else {
            panic!("selection background should resolve to rgb");
        };
        assert!(relative_luminance((r, g, b)) > relative_luminance((12, 14, 16)));
    }
}
