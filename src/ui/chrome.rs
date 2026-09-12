//! Desktop window chrome: the full-width titlebar, the overseer's strip under
//! it, and the persistent bottom hint bar. All three draw into rects reserved
//! by `compute_view_internal` and own no state.
//!
//! Width-adaptive the way `docs/DESIGN-LANGUAGE.md` asks: a strip drops whole
//! facts in a declared order, never part of one. The titlebar's geometry comes
//! from one pure function, [`titlebar_layout`], so the render and the mouse
//! hit-test cannot disagree about where the pill is.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::glyphs;
use super::text::{display_width, spans_width, truncate_end};
use crate::app::state::Palette;
use crate::app::AppState;
use crate::config::ActionKeybinds;
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

fn fill_row(frame: &mut Frame, area: Rect, bg: ratatui::style::Color) {
    let buf = frame.buffer_mut();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            buf[(x, y)].set_style(Style::default().bg(bg));
        }
    }
}

fn prefix_rhs(bindings: &ActionKeybinds) -> Option<String> {
    bindings.prefix_rhs_label()
}

fn width_u16(spans: &[Span<'_>]) -> u16 {
    u16::try_from(spans_width(spans)).unwrap_or(u16::MAX)
}

// ---------------------------------------------------------------------------
// The state tally
// ---------------------------------------------------------------------------

/// How many agents are in each state, for the titlebar. Counts agents, not
/// groups: a group with one blocked agent and two working ones is three
/// facts, and the old `1 blocked` in the corner said one.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct StateTally {
    pub blocked: usize,
    pub working: usize,
    pub done: usize,
    pub idle: usize,
}

pub(crate) fn agent_tally(app: &AppState) -> StateTally {
    let mut tally = StateTally::default();
    for entry in super::sidebar::agent_panel_entries(app) {
        match super::board::summary_bucket(entry.state, entry.seen) {
            0 => tally.blocked += 1,
            1 => tally.done += 1,
            2 => tally.working += 1,
            _ => tally.idle += 1,
        }
    }
    tally
}

// ---------------------------------------------------------------------------
// The titlebar
// ---------------------------------------------------------------------------

/// The two positions of the desktop/board pill.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PillHalf {
    Desktop,
    Board,
}

/// One drawn run of the titlebar: where it goes and what it says.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Slot {
    pub rect: Rect,
    pub line: Line<'static>,
}

/// Everything the titlebar draws, and the two rects the mouse can hit.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TitlebarLayout {
    pub left: Slot,
    /// `group › agent`, only when it fits between the other two.
    pub center: Option<Slot>,
    /// The tally and the pill, at whatever rung of the ladder fits.
    pub right: Option<Slot>,
    /// Zero-sized when the pill did not fit at all.
    pub pill_desktop: Rect,
    pub pill_board: Rect,
}

/// One rung of the right slot's ladder: which facts it still shows. Walked
/// top to bottom until one fits, so what the user sees is always a prefix
/// of the same known order, never a gap-toothed subset.
#[derive(Debug, Clone, Copy)]
struct Rung {
    update: bool,
    blocked: bool,
    working: bool,
    done: bool,
    idle: bool,
    long_pill: bool,
}

const LADDER: [Rung; 7] = [
    // full
    Rung {
        update: true,
        blocked: true,
        working: true,
        done: true,
        idle: true,
        long_pill: true,
    },
    // drop idle
    Rung {
        update: true,
        blocked: true,
        working: true,
        done: true,
        idle: false,
        long_pill: true,
    },
    // drop done
    Rung {
        update: true,
        blocked: true,
        working: true,
        done: false,
        idle: false,
        long_pill: true,
    },
    // pill shortens
    Rung {
        update: true,
        blocked: true,
        working: true,
        done: false,
        idle: false,
        long_pill: false,
    },
    // drop working
    Rung {
        update: true,
        blocked: true,
        working: false,
        done: false,
        idle: false,
        long_pill: false,
    },
    // drop blocked
    Rung {
        update: true,
        blocked: false,
        working: false,
        done: false,
        idle: false,
        long_pill: false,
    },
    // pill alone
    Rung {
        update: false,
        blocked: false,
        working: false,
        done: false,
        idle: false,
        long_pill: false,
    },
];

/// The facts the right slot can show, gathered once so each rung is
/// arithmetic over them.
struct RightFacts {
    update_ready: bool,
    tally: StateTally,
    proposals: usize,
    lit: PillHalf,
    spinner: &'static str,
}

/// The right slot at one rung: its spans, plus each pill half's column offset
/// and width within the slot.
struct RightRun {
    spans: Vec<Span<'static>>,
    desktop: (u16, u16),
    board: (u16, u16),
}

fn tally_fact(
    glyph: &str,
    count: usize,
    color: ratatui::style::Color,
    bold: bool,
) -> Span<'static> {
    let mut style = Style::default().fg(color);
    if bold {
        style = style.add_modifier(Modifier::BOLD);
    }
    Span::styled(format!("{glyph} {count}"), style)
}

fn right_run(facts: &RightFacts, rung: Rung, p: &Palette) -> RightRun {
    let brand = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let gap = Span::raw("  ");
    let mut spans: Vec<Span<'static>> = Vec::new();
    let push = |span: Span<'static>, spans: &mut Vec<Span<'static>>| {
        if !spans.is_empty() {
            spans.push(gap.clone());
        }
        spans.push(span);
    };
    if rung.update && facts.update_ready {
        push(Span::styled("update ready", brand), &mut spans);
    }
    let t = facts.tally;
    if rung.blocked && t.blocked > 0 {
        push(
            tally_fact(
                super::status::state_glyph(AgentState::Blocked),
                t.blocked,
                p.red,
                true,
            ),
            &mut spans,
        );
    }
    if rung.working && t.working > 0 {
        push(
            tally_fact(facts.spinner, t.working, p.yellow, false),
            &mut spans,
        );
    }
    if rung.done && t.done > 0 {
        push(
            tally_fact(
                super::status::state_glyph(AgentState::Idle),
                t.done,
                p.blue,
                false,
            ),
            &mut spans,
        );
    }
    if rung.idle && t.idle > 0 {
        push(
            tally_fact(
                super::status::state_appearance(AgentState::Idle, true, 0).glyph,
                t.idle,
                p.green,
                false,
            ),
            &mut spans,
        );
    }
    if !spans.is_empty() {
        spans.push(gap);
    }

    // The pill: two adjacent cell groups, the lit half in the focus tier.
    let lit = Style::default()
        .fg(p.panel_bg)
        .bg(p.accent)
        .add_modifier(Modifier::BOLD);
    let unlit = Style::default().fg(p.subtext0).bg(p.surface1);
    let count = Style::default().fg(p.teal).bg(p.surface1);
    let desktop_word = if rung.long_pill { "desktop" } else { "desk" };
    let desktop_start = width_u16(&spans);
    let desktop_style = if facts.lit == PillHalf::Desktop {
        lit
    } else {
        unlit
    };
    spans.push(Span::styled(format!(" {desktop_word} "), desktop_style));
    let board_start = width_u16(&spans);
    if facts.lit == PillHalf::Board {
        spans.push(Span::styled(" board ", lit));
    } else {
        spans.push(Span::styled(" board", unlit));
        if facts.proposals > 0 {
            spans.push(Span::styled(format!(" {}", facts.proposals), count));
        }
        spans.push(Span::styled(" ", unlit));
    }
    let board_end = width_u16(&spans);
    spans.push(Span::raw(" "));
    RightRun {
        spans,
        desktop: (desktop_start, board_start - desktop_start),
        board: (board_start, board_end - board_start),
    }
}

/// The active group and its focused agent, for the centre slot.
fn active_group_and_agent(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Option<(String, String)> {
    let ws_idx = app.active?;
    let ws = app.workspaces.get(ws_idx)?;
    let group = match terminal_runtimes {
        Some(runtimes) => ws.display_name_from(&app.terminals, runtimes),
        None => ws.display_name_from(&app.terminals, &TerminalRuntimeRegistry::new()),
    };
    let focused = ws.focused_pane_id();
    let agent = super::sidebar::agent_panel_entries(app)
        .into_iter()
        .find(|entry| entry.ws_idx == ws_idx && Some(entry.pane_id) == focused)
        .and_then(|entry| entry.agent_label)
        .filter(|label| !label.trim().is_empty())
        .or_else(|| {
            let tab_idx = ws.active_tab_index();
            ws.tab_display_name(tab_idx, &app.terminals)
        })
        .unwrap_or_else(|| (ws.active_tab_index() + 1).to_string());
    Some((group, agent))
}

fn center_line(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Option<Line<'static>> {
    let p = &app.palette;
    if app.board_underlay() {
        return Some(Line::from(Span::styled(
            format!("{} overseer", glyphs::OVERSEER),
            Style::default().fg(p.mauve).add_modifier(Modifier::BOLD),
        )));
    }
    let (group, agent) = active_group_and_agent(app, terminal_runtimes)?;
    Some(Line::from(vec![
        Span::styled(
            group,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            format!(" {} ", glyphs::LEADS_TO),
            Style::default().fg(p.overlay0),
        ),
        Span::styled(agent, Style::default().fg(p.subtext0)),
    ]))
}

/// Where everything on the titlebar goes. Pure: the render draws it and the
/// mouse asks it, so the pill is clickable exactly where it is drawn.
///
/// `terminal_runtimes` only sharpens the group's name (a live cwd); the
/// geometry of the right slot — the part the mouse cares about — does not
/// depend on it, so the hit-test passes `None`.
pub(crate) fn titlebar_layout(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
    area: Rect,
) -> TitlebarLayout {
    let p = &app.palette;
    let brand = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let left_spans = vec![Span::raw(" "), Span::styled("shep", brand)];
    let left_w = width_u16(&left_spans);
    let left = Slot {
        rect: Rect::new(area.x, area.y, left_w.min(area.width), area.height.min(1)),
        line: Line::from(left_spans),
    };

    let facts = RightFacts {
        update_ready: app.update_available.is_some(),
        tally: agent_tally(app),
        proposals: crate::app::overseer::proposals(&app.docket_sample).len(),
        lit: if app.board_underlay() {
            PillHalf::Board
        } else {
            PillHalf::Desktop
        },
        spinner: super::spinner_frame(app.spinner_tick),
    };
    // The centre is fixed text; the right slot walks its ladder until both
    // fit — and, on a terminal too narrow for that, walks it again without
    // the centre rather than draw nothing on the right.
    let center_line = center_line(app, terminal_runtimes);
    let center_w = center_line
        .as_ref()
        .map(|line| u16::try_from(line.width()).unwrap_or(u16::MAX))
        .filter(|w| *w > 0 && *w <= area.width);
    let center_fits = |right_w: u16| {
        let Some(center_w) = center_w else {
            return false;
        };
        let cx = area.x + (area.width - center_w) / 2;
        cx >= area.x + left_w + 2 && cx + center_w + 2 <= area.x + area.width - right_w
    };
    let runs: Vec<RightRun> = LADDER
        .iter()
        .map(|rung| right_run(&facts, *rung, p))
        .collect();
    let fits_right = |run: &RightRun| left_w + width_u16(&run.spans) + 2 <= area.width;
    let chosen = runs
        .iter()
        .find(|run| fits_right(run) && center_fits(width_u16(&run.spans)))
        .or_else(|| runs.iter().find(|run| fits_right(run)));

    let mut right = None;
    let mut pill_desktop = Rect::default();
    let mut pill_board = Rect::default();
    if let Some(run) = chosen {
        let right_w = width_u16(&run.spans);
        let x = area.x + area.width - right_w;
        pill_desktop = Rect::new(x + run.desktop.0, area.y, run.desktop.1, 1);
        pill_board = Rect::new(x + run.board.0, area.y, run.board.1, 1);
        right = Some(Slot {
            rect: Rect::new(x, area.y, right_w, 1),
            line: Line::from(run.spans.clone()),
        });
    }
    let right_w = right.as_ref().map(|slot| slot.rect.width).unwrap_or(0);

    let center = center_line.and_then(|line| {
        let center_w = center_w?;
        let cx = area.x + (area.width - center_w) / 2;
        center_fits(right_w).then(|| Slot {
            rect: Rect::new(cx, area.y, center_w, 1),
            line,
        })
    });

    TitlebarLayout {
        left,
        center,
        right,
        pill_desktop,
        pill_board,
    }
}

fn rect_contains(rect: Rect, col: u16, row: u16) -> bool {
    rect.width > 0
        && rect.height > 0
        && col >= rect.x
        && col < rect.x + rect.width
        && row >= rect.y
        && row < rect.y + rect.height
}

/// Which half of the pill, if any, sits under `(col, row)`.
pub(crate) fn titlebar_pill_at(app: &AppState, col: u16, row: u16) -> Option<PillHalf> {
    let area = app.view.titlebar_rect;
    if area.height == 0 || !rect_contains(area, col, row) {
        return None;
    }
    let layout = titlebar_layout(app, None, area);
    if rect_contains(layout.pill_desktop, col, row) {
        Some(PillHalf::Desktop)
    } else if rect_contains(layout.pill_board, col, row) {
        Some(PillHalf::Board)
    } else {
        None
    }
}

/// Full-width top titlebar: brand, `group › agent`, and the state tally
/// beside the desktop/board pill.
pub(super) fn render_titlebar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    fill_row(frame, area, app.palette.panel_bg);
    let layout = titlebar_layout(app, Some(terminal_runtimes), area);
    frame.render_widget(Paragraph::new(layout.left.line), layout.left.rect);
    if let Some(right) = layout.right {
        frame.render_widget(Paragraph::new(right.line), right.rect);
    }
    if let Some(center) = layout.center {
        frame.render_widget(Paragraph::new(center.line), center.rect);
    }
}

// ---------------------------------------------------------------------------
// The overseer strip
// ---------------------------------------------------------------------------

/// The strip's one line: `✦ overseer · <sentence>` with the tick time pinned
/// to the right edge. Narrowing drops the word `overseer` first (the mark
/// still says who is speaking), then elides the sentence; the time stays.
pub(crate) fn strip_line(
    sentence: &str,
    time: Option<&str>,
    width: usize,
    p: &Palette,
) -> Line<'static> {
    let mark = Style::default().fg(p.mauve).add_modifier(Modifier::BOLD);
    let sep = Style::default().fg(p.overlay0);
    let body = Style::default().fg(p.text);
    let clock = Style::default().fg(p.overlay0);

    let time_text = time.map(|t| format!("{t} ")).unwrap_or_default();
    let time_w = display_width(&time_text);
    // One column of air between the sentence and the clock.
    let reserve = if time_w > 0 { time_w + 1 } else { 1 };

    let long_lead: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(format!("{} overseer", glyphs::OVERSEER), mark),
        Span::styled(glyphs::SEP_SPACED, sep),
    ];
    let short_lead: Vec<Span<'static>> = vec![
        Span::raw(" "),
        Span::styled(glyphs::OVERSEER, mark),
        Span::raw(" "),
    ];
    let sentence_w = display_width(sentence);
    let lead = if spans_width(&long_lead) + sentence_w + reserve <= width {
        long_lead
    } else {
        short_lead
    };
    let lead_w = spans_width(&lead);
    let room = width.saturating_sub(lead_w + reserve);
    let shown = if sentence_w <= room {
        sentence.to_string()
    } else {
        truncate_end(sentence, room)
    };
    let mut spans = lead;
    spans.push(Span::styled(shown, body));
    let used = spans_width(&spans);
    let pad = width.saturating_sub(used + time_w);
    if pad > 0 {
        spans.push(Span::raw(" ".repeat(pad)));
    }
    if time_w > 0 && used + time_w <= width {
        spans.push(Span::styled(time_text, clock));
    }
    Line::from(spans)
}

/// The overseer's one line under the titlebar. No buttons: clicking it opens
/// the board, and that is the strip's whole interaction.
pub(super) fn render_overseer_strip(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    fill_row(frame, area, p.surface0);
    let Some(sentence) = app.overseer.sample.first_sentence() else {
        return;
    };
    let line = strip_line(
        &sentence,
        app.overseer.sample.tick_at.as_deref(),
        usize::from(area.width),
        p,
    );
    frame.render_widget(Paragraph::new(line), area);
}

// ---------------------------------------------------------------------------
// The hint bar
// ---------------------------------------------------------------------------

/// Persistent bottom hint bar: the prefix chord plus the few keys worth a
/// glance, derived from the live keybinding config. Recognition over recall.
pub(super) fn render_hint_bar(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    fill_row(frame, area, p.panel_bg);

    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);

    let switch = app.keybinds.switch_view.label();
    let mut hints: Vec<(String, &'static str)> = Vec::new();
    if app.board_underlay() {
        use crate::app::state::BoardView;
        if let Some(chord) = switch {
            hints.push((chord, "desktop"));
        }
        // Each view advertises only the keys that do something on it; the
        // detail screens say what esc does — back, not close — so the board
        // is always one step away.
        let view_hints: &[(&str, &'static str)] = match app.board.view {
            // While the chat has the keys, only the chat's keys are true.
            BoardView::Overseer if app.overseer.chat_focused => {
                &[("enter", "send"), ("esc", "back")]
            }
            BoardView::Overseer => &[
                ("enter", "focus"),
                ("jk", "move"),
                ("a", "keep"),
                ("x", "drop"),
                ("tab", "chat"),
                ("?", "keys"),
            ],
            BoardView::Columns => &[
                ("enter", "focus"),
                ("i", "inspect"),
                ("hjkl", "move"),
                ("<>", "move group"),
                ("a", "docket"),
            ],
            BoardView::Docket => &[
                ("i", "inspect"),
                ("n", "new"),
                ("p", "slate"),
                ("r", "recur"),
                ("d", "done"),
                ("x", "discard"),
                ("a", "overseer"),
            ],
            BoardView::Agent => &[("enter", "attach"), ("esc", "back")],
            BoardView::DocketItem => &[
                ("p", "slate"),
                ("r", "recur"),
                ("d", "done"),
                ("x", "discard"),
                ("esc", "back"),
            ],
        };
        hints.extend(
            view_hints
                .iter()
                .map(|(chord, label)| (chord.to_string(), *label)),
        );
    } else {
        let prefix = crate::config::format_key_combo((app.prefix_code, app.prefix_mods));
        hints.push((prefix, "prefix"));
        // Esc leads back to the board from an agent pane, so say so while the
        // user is standing in one — and name the interrupt they gave up to
        // get it.
        if app.escape_returns_to_board_here() {
            hints.push(("esc".to_string(), "board"));
            hints.push(("shift+esc".to_string(), "interrupt"));
        } else if app.escape_interrupts_here() {
            // This host sends a bare Esc, so there is no shift+esc to offer:
            // Esc stays the interrupt and the board is a key of its own.
            hints.push(("esc".to_string(), "interrupt"));
            if let Some(rhs) = prefix_rhs(&app.keybinds.board) {
                hints.push((rhs, "board"));
            }
        }
        if let Some(chord) = switch {
            hints.push((chord, "board"));
        }
        for (bindings, label) in [
            (&app.keybinds.help, "keys"),
            (&app.keybinds.detach, "detach"),
        ] {
            if let Some(rhs) = prefix_rhs(bindings) {
                hints.push((rhs, label));
            }
        }
    }

    // A refused docket verb says why, in the store's words, on the bar's
    // right edge; it is gone on the next key. The hints yield to it from
    // the right.
    let notice = app
        .board
        .docket_notice
        .as_deref()
        .filter(|_| app.board_underlay())
        .map(|notice| format!("! {notice}"));
    let notice_width = notice
        .as_ref()
        .map(|text| display_width(text) + 2)
        .unwrap_or(0);
    let hint_width = usize::from(area.width).saturating_sub(notice_width);

    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used: usize = 1;
    for (i, (chord, label)) in hints.into_iter().enumerate() {
        let sep = if i > 0 { 2 } else { 0 };
        let entry_w = sep + chord.chars().count() + 1 + label.chars().count();
        if used + entry_w > hint_width {
            break;
        }
        if i > 0 {
            spans.push(Span::styled("  ", dim));
        }
        spans.push(Span::styled(chord, key));
        spans.push(Span::styled(format!(" {label}"), dim));
        used += entry_w;
    }
    if let Some(notice) = notice {
        let room = usize::from(area.width).saturating_sub(used + 2);
        let text = truncate_end(&notice, room);
        let pad = usize::from(area.width)
            .saturating_sub(used)
            .saturating_sub(display_width(&text) + 1);
        spans.push(Span::raw(" ".repeat(pad)));
        spans.push(Span::styled(text, Style::default().fg(p.peach)));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::state::Mode;

    fn fixture() -> AppState {
        let mut state = super::super::snapshot::fixture::session();
        state.mode = Mode::Terminal;
        state
    }

    fn text(line: &Line<'_>) -> String {
        line.spans.iter().map(|s| s.content.as_ref()).collect()
    }

    fn right_text(app: &AppState, width: u16) -> String {
        titlebar_layout(app, None, Rect::new(0, 0, width, 1))
            .right
            .map(|slot| text(&slot.line))
            .unwrap_or_default()
    }

    #[test]
    fn tally_counts_agents_not_groups() {
        let app = fixture();
        assert_eq!(
            agent_tally(&app),
            StateTally {
                blocked: 1,
                working: 2,
                done: 1,
                idle: 1,
            }
        );
        let right = right_text(&app, 200);
        assert_eq!(right, "◉ 1  ⠹ 2  ● 1  ○ 1   desktop  board 2  ");
    }

    #[test]
    fn right_slot_drops_idle_then_done_then_shortens_pill() {
        let mut app = fixture();
        // No active group, so no centre: only `left + right + 2` gates and
        // the table below is the ladder itself.
        app.active = None;
        let full = "◉ 1  ⠹ 2  ● 1  ○ 1   desktop  board 2  ";
        let no_idle = "◉ 1  ⠹ 2  ● 1   desktop  board 2  ";
        let no_done = "◉ 1  ⠹ 2   desktop  board 2  ";
        let short = "◉ 1  ⠹ 2   desk  board 2  ";
        let no_working = "◉ 1   desk  board 2  ";
        let no_blocked = " desk  board 2  ";
        let full_w = display_width(full) as u16 + 7;
        let cases = [
            (full_w, full),
            (full_w - 1, no_idle),
            (display_width(no_idle) as u16 + 7, no_idle),
            (display_width(no_idle) as u16 + 6, no_done),
            (display_width(no_done) as u16 + 6, short),
            (display_width(short) as u16 + 6, no_working),
            (display_width(no_working) as u16 + 6, no_blocked),
            (display_width(no_blocked) as u16 + 6, ""),
        ];
        for (width, want) in cases {
            assert_eq!(right_text(&app, width), want, "at width {width}");
        }
    }

    /// The centre is a fact too, and it outranks the tail of the tally: on
    /// a standard 80 columns the ladder drops idle and done so that
    /// `group › agent` still fits between the brand and the pill.
    #[test]
    fn centre_outranks_the_tail_of_the_tally() {
        let app = fixture();
        let layout = titlebar_layout(&app, None, Rect::new(0, 0, 80, 1));
        assert_eq!(
            text(&layout.center.expect("centre").line),
            "workmayt › claude"
        );
        assert_eq!(
            text(&layout.right.expect("right").line),
            "◉ 1  ⠹ 2   desktop  board 2  "
        );
        // Too narrow for both: the centre goes, the right slot stays and
        // keeps walking its own ladder.
        let layout = titlebar_layout(&app, None, Rect::new(0, 0, 40, 1));
        assert_eq!(layout.center, None);
        assert_eq!(
            text(&layout.right.expect("right").line),
            "◉ 1  ⠹ 2   desktop  board 2  "
        );
        let layout = titlebar_layout(&app, None, Rect::new(0, 0, 35, 1));
        assert_eq!(
            text(&layout.right.expect("right").line),
            "◉ 1  ⠹ 2   desk  board 2  "
        );
    }

    #[test]
    fn strip_not_reserved_without_a_narrative() {
        let mut app = fixture();
        let area = Rect::new(0, 0, 120, 40);
        crate::ui::compute_view(&mut app, area);
        assert_eq!(app.view.titlebar_rect, Rect::new(0, 0, 120, 1));
        assert_eq!(app.view.overseer_strip_rect, Rect::new(0, 1, 120, 1));
        assert_eq!(app.view.sidebar_rect.y, 2, "everything below shifts a row");

        app.overseer.sample.narrative = None;
        crate::ui::compute_view(&mut app, area);
        assert_eq!(app.view.overseer_strip_rect, Rect::default());
        assert_eq!(app.view.sidebar_rect.y, 1);

        // Off by config, and off on the board, even with a narrative.
        app.overseer.sample = crate::app::overseer::OverseerSample::test_fixture();
        app.overseer_strip = false;
        crate::ui::compute_view(&mut app, area);
        assert_eq!(app.view.overseer_strip_rect, Rect::default());
        app.overseer_strip = true;
        app.mode = Mode::Board;
        crate::ui::compute_view(&mut app, area);
        assert_eq!(app.view.overseer_strip_rect, Rect::default());
    }

    /// Render never opens the overseer's dir: everything it draws comes from
    /// the sample, and the sample is only refreshed from the scheduled tick.
    #[test]
    fn render_never_touches_the_state_dir() {
        let mut app = fixture();
        let dir = crate::app::overseer::test_state_dir();
        app.overseer.state_dir = dir.clone();
        for (w, h) in [(200, 55), (80, 24)] {
            let _ =
                crate::server::render_stream::render_virtual(&mut app, Rect::new(0, 0, w, h), true);
        }
        app.mode = Mode::Board;
        let _ =
            crate::server::render_stream::render_virtual(&mut app, Rect::new(0, 0, 120, 40), true);
        assert!(!dir.exists(), "render created {}", dir.display());
    }

    #[test]
    fn update_ready_outranks_the_tally() {
        let mut app = fixture();
        app.update_available = Some("9.9.9".to_string());
        let right = right_text(&app, 200);
        assert!(right.starts_with("update ready  ◉ 1"), "{right:?}");
        // Narrow enough to lose every fact but the pill, and the pill wins
        // over the update: the last rung is the pill alone.
        let right = right_text(&app, 24);
        assert_eq!(right, " desk  board 2  ");
        // One column narrower than the pill alone and the slot is empty.
        assert_eq!(right_text(&app, 22), "");
    }

    #[test]
    fn pill_lights_the_current_view() {
        let mut app = fixture();
        let area = Rect::new(0, 0, 200, 1);
        let layout = titlebar_layout(&app, None, area);
        let right = layout.right.expect("right slot");
        let lit = |line: &Line<'_>, word: &str| {
            line.spans
                .iter()
                .find(|s| s.content.contains(word))
                .map(|s| s.style.bg == Some(app.palette.accent))
                .expect("pill half")
        };
        assert!(lit(&right.line, "desktop"));
        assert!(!lit(&right.line, "board"));
        assert_eq!(
            text(&layout.center.expect("centre").line),
            "workmayt › claude"
        );

        app.mode = Mode::Board;
        let layout = titlebar_layout(&app, None, area);
        let right = layout.right.expect("right slot");
        assert!(!lit(&right.line, "desktop"));
        assert!(lit(&right.line, "board"));
        assert_eq!(text(&layout.center.expect("centre").line), "✦ overseer");
        // The lit board half does not count what it is already showing.
        assert!(text(&right.line).ends_with(" desktop  board  "));
    }

    #[test]
    fn board_half_counts_waiting_proposals() {
        let mut app = fixture();
        let layout = titlebar_layout(&app, None, Rect::new(0, 0, 200, 1));
        let right = layout.right.expect("right slot");
        let count = right
            .line
            .spans
            .iter()
            .find(|s| s.content == " 2")
            .expect("count span");
        assert_eq!(count.style.fg, Some(app.palette.teal));

        app.docket_sample.rows.clear();
        assert_eq!(
            right_text(&app, 200),
            "◉ 1  ⠹ 2  ● 1  ○ 1   desktop  board  "
        );
    }

    #[test]
    fn the_pill_is_clickable_where_it_is_drawn() {
        let mut app = fixture();
        app.view.titlebar_rect = Rect::new(0, 0, 120, 1);
        let layout = titlebar_layout(&app, None, app.view.titlebar_rect);
        let d = layout.pill_desktop;
        let b = layout.pill_board;
        assert_eq!(d.x + d.width, b.x, "halves are adjacent");
        assert_eq!(titlebar_pill_at(&app, d.x, 0), Some(PillHalf::Desktop));
        assert_eq!(titlebar_pill_at(&app, b.x, 0), Some(PillHalf::Board));
        assert_eq!(
            titlebar_pill_at(&app, b.x + b.width - 1, 0),
            Some(PillHalf::Board)
        );
        assert_eq!(titlebar_pill_at(&app, b.x + b.width, 0), None);
        assert_eq!(titlebar_pill_at(&app, 0, 0), None);
        assert_eq!(titlebar_pill_at(&app, d.x, 1), None);
    }

    #[test]
    fn strip_drops_the_word_then_elides() {
        let p = Palette::shep();
        let sentence = "workmayt's claude has been blocked 2m on a permission prompt.";
        let full = text(&strip_line(sentence, Some("07:08"), 100, &p));
        assert!(full.starts_with(" ✦ overseer · workmayt's claude"));
        assert!(full.ends_with(" 07:08 "));
        assert_eq!(display_width(&full), 100);

        // Too narrow for the word: the mark stays, the sentence is whole.
        let width = display_width(sentence) + 3 + 7;
        let narrow = text(&strip_line(sentence, Some("07:08"), width, &p));
        assert!(narrow.starts_with(" ✦ workmayt's"), "{narrow:?}");
        assert!(narrow.contains("prompt. 07:08 "));
        assert_eq!(display_width(&narrow), width);

        // Narrower still: the sentence elides, the time stays.
        let elided = text(&strip_line(sentence, Some("07:08"), 40, &p));
        assert!(elided.starts_with(" ✦ workmayt's claude"), "{elided:?}");
        assert!(elided.contains("…"));
        assert!(elided.ends_with(" 07:08 "));
        assert_eq!(display_width(&elided), 40);

        // No tick time: nothing pinned right.
        let untimed = text(&strip_line(sentence, None, 100, &p));
        assert!(untimed.trim_end().ends_with("prompt."));
    }

    #[test]
    fn hint_bar_names_the_switch_per_mode() {
        let mut app = fixture();
        app.hint_bar = true;
        let area = Rect::new(0, 0, 80, 24);
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let row = |buffer: &ratatui::buffer::Buffer| {
            (0..80)
                .map(|x| buffer[(x, 23)].symbol().to_string())
                .collect::<String>()
        };
        let bottom = row(&buffer);
        assert!(
            bottom.contains("ctrl+b prefix  ctrl+alt+b board  ? keys  q detach"),
            "{bottom:?}"
        );
        assert!(!bottom.contains("groups"));

        app.mode = Mode::Board;
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let bottom = row(&buffer);
        assert!(
            bottom.contains(
                "ctrl+alt+b desktop  enter focus  jk move  a keep  x drop  tab chat  ? keys"
            ),
            "{bottom:?}"
        );
        app.board.view = crate::app::state::BoardView::Docket;
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let bottom = row(&buffer);
        assert!(
            bottom.contains(
                "ctrl+alt+b desktop  i inspect  n new  p slate  r recur  d done  x discard"
            ),
            "{bottom:?}"
        );
        app.board.view = crate::app::state::BoardView::Agent;
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let bottom = row(&buffer);
        assert!(bottom.contains("enter attach  esc back"), "{bottom:?}");
    }

    /// A refused docket verb rides the hint bar's right edge, in peach, and
    /// the hints yield to it from the right.
    #[test]
    fn a_docket_notice_rides_the_hint_bar() {
        let mut app = fixture();
        app.hint_bar = true;
        app.mode = Mode::Board;
        app.board.view = crate::app::state::BoardView::Docket;
        app.board.docket_notice =
            Some("docket item 1 is inbox, only open items can be completed".into());
        let area = Rect::new(0, 0, 150, 40);
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let bottom: String = (0..150).map(|x| buffer[(x, 39)].symbol()).collect();
        assert!(bottom.contains("! docket item 1 is inbox"), "{bottom:?}");
        assert!(bottom.trim_end().ends_with("completed"), "{bottom:?}");
        let cell = buffer[(149 - 1, 39)].style();
        assert_eq!(cell.fg, Some(app.palette.peach));
        // Narrow: the hints give way before the notice is lost.
        let area = Rect::new(0, 0, 90, 24);
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let bottom: String = (0..90).map(|x| buffer[(x, 23)].symbol()).collect();
        assert!(bottom.contains("! docket item 1"), "{bottom:?}");
        assert!(!bottom.contains("a overseer"), "{bottom:?}");
    }

    /// The board is a screen, not an overlay: nothing of the desktop shows
    /// under it — and a modal it opened keeps it that way.
    #[test]
    fn the_board_replaces_the_desktop_and_stays_under_its_modals() {
        let mut app = fixture();
        app.mode = Mode::Board;
        let area = Rect::new(0, 0, 120, 40);
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let text: String = (0..40)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(
            !text.contains("+ new group"),
            "sidebar drawn under the board:\n{text}"
        );
        assert!(text.contains("needs you"), "{text}");
        app.board.suspended = true;
        app.mode = Mode::KeybindHelp;
        let (buffer, _) = crate::server::render_stream::render_virtual(&mut app, area, true);
        let text: String = (0..40)
            .map(|y| {
                (0..120)
                    .map(|x| buffer[(x, y)].symbol())
                    .collect::<String>()
                    + "\n"
            })
            .collect();
        assert!(!text.contains("+ new group"), "{text}");
        assert!(text.contains("open the overseer's session"), "{text}");
        assert!(text.contains("keybind"), "help drawn over it: {text}");
    }
}
