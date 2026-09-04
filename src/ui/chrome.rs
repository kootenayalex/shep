//! Desktop window chrome: the full-width titlebar and the persistent bottom
//! hint bar. Both draw into rects reserved by `compute_view_internal`; they
//! own no state and take no input. Width-adaptive: segments drop instead of
//! overlapping, and hints truncate from the right.

use ratatui::{
    layout::Rect,
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::glyphs;
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

fn line_width(line: &Line<'_>) -> u16 {
    u16::try_from(line.width()).unwrap_or(u16::MAX)
}

/// Full-width top titlebar: brand + active workspace/tab context + a
/// right-aligned attention slot (update ready, else blocked count).
pub(super) fn render_titlebar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    fill_row(frame, area, p.panel_bg);

    let brand = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);

    let left = Line::from(vec![Span::raw(" "), Span::styled("shep", brand)]);
    let left_w = line_width(&left);
    frame.render_widget(Paragraph::new(left), area);

    // Right slot: one attention item, most urgent wins.
    let right = if app.update_available.is_some() {
        Some(Line::from(vec![
            Span::styled("update ready", brand),
            Span::raw(" "),
        ]))
    } else {
        let blocked = app
            .workspaces
            .iter()
            .filter(|ws| ws.aggregate_state(&app.terminals).0 == AgentState::Blocked)
            .count();
        (blocked > 0).then(|| {
            Line::from(vec![
                Span::styled(
                    format!(
                        "{} {blocked} blocked",
                        super::status::state_glyph(AgentState::Blocked)
                    ),
                    Style::default().fg(p.red).add_modifier(Modifier::BOLD),
                ),
                Span::raw(" "),
            ])
        })
    };
    let right_w = right.as_ref().map(line_width).unwrap_or(0);
    if let Some(right) = right {
        if left_w + right_w + 2 <= area.width {
            let rect = Rect::new(area.x + area.width - right_w, area.y, right_w, 1);
            frame.render_widget(Paragraph::new(right), rect);
        }
    }

    // Center slot: active workspace · tab, only when it fits cleanly.
    let Some(ws) = app.active.and_then(|i| app.workspaces.get(i)) else {
        return;
    };
    let ws_name = ws.display_name_from(&app.terminals, terminal_runtimes);
    let tab_idx = ws.active_tab_index();
    let tab_name = ws
        .tab_display_name(tab_idx)
        .unwrap_or_else(|| (tab_idx + 1).to_string());
    let center = Line::from(vec![
        Span::styled(
            ws_name,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
        Span::styled(glyphs::SEP_SPACED, dim),
        Span::styled(tab_name, Style::default().fg(p.subtext0)),
    ]);
    let center_w = line_width(&center);
    if center_w == 0 || center_w > area.width {
        return;
    }
    let cx = area.x + (area.width - center_w) / 2;
    let fits_left = cx >= area.x + left_w + 2;
    let fits_right = cx + center_w + 2 <= area.x + area.width - right_w;
    if fits_left && fits_right {
        frame.render_widget(Paragraph::new(center), Rect::new(cx, area.y, center_w, 1));
    }
}

/// Persistent bottom hint bar: the prefix chord plus the core prefix-mode
/// actions, derived from the live keybinding config. Recognition over recall.
pub(super) fn render_hint_bar(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }
    let p = &app.palette;
    fill_row(frame, area, p.panel_bg);

    let key = Style::default().fg(p.accent).add_modifier(Modifier::BOLD);
    let dim = Style::default().fg(p.overlay0);

    let prefix = crate::config::format_key_combo((app.prefix_code, app.prefix_mods));
    let mut hints: Vec<(String, &'static str)> = vec![(prefix, "prefix")];
    // Esc leads back to the board from an agent pane, so say so while the user
    // is standing in one — and name the interrupt they gave up to get it.
    if app.escape_returns_to_board_here() {
        hints.push(("esc".to_string(), "board"));
        hints.push(("shift+esc".to_string(), "interrupt"));
    }
    for (bindings, label) in [
        (&app.keybinds.workspace_picker, "groups"),
        (&app.keybinds.new_tab, "tab"),
        (&app.keybinds.help, "keys"),
        (&app.keybinds.detach, "detach"),
    ] {
        if let Some(rhs) = prefix_rhs(bindings) {
            hints.push((rhs, label));
        }
    }

    let mut spans: Vec<Span<'static>> = vec![Span::raw(" ")];
    let mut used: usize = 1;
    for (i, (chord, label)) in hints.into_iter().enumerate() {
        let sep = if i > 0 { 2 } else { 0 };
        let entry_w = sep + chord.chars().count() + 1 + label.chars().count();
        if used + entry_w > usize::from(area.width) {
            break;
        }
        if i > 0 {
            spans.push(Span::styled("  ", dim));
        }
        spans.push(Span::styled(chord, key));
        spans.push(Span::styled(format!(" {label}"), dim));
        used += entry_w;
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}
