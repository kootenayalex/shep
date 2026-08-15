use ratatui::{
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph, Wrap},
    Frame,
};

use super::glyphs;
use super::text::display_width_u16;
use crate::app::state::Palette;

pub(super) fn render_panel_shell(
    frame: &mut Frame,
    area: Rect,
    border_color: Color,
    bg: Color,
) -> Option<Rect> {
    if area.width < 2 || area.height < 2 {
        return None;
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(border_color))
        .border_set(ratatui::symbols::border::ROUNDED)
        .style(Style::default().bg(bg));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    Some(inner)
}

pub(super) fn panel_contrast_fg(p: &Palette) -> Color {
    match p.panel_bg {
        Color::Reset => p.surface_dim,
        color => color,
    }
}

pub(crate) fn centered_popup_rect(area: Rect, popup_w: u16, popup_h: u16) -> Option<Rect> {
    let popup_w = popup_w.min(area.width.saturating_sub(4));
    let popup_h = popup_h.min(area.height.saturating_sub(2));
    if popup_w < 4 || popup_h < 4 {
        return None;
    }

    let popup_x = area.x + (area.width.saturating_sub(popup_w)) / 2;
    let popup_y = area.y + (area.height.saturating_sub(popup_h)) / 2;
    Some(Rect::new(popup_x, popup_y, popup_w, popup_h))
}

/// A modal that could not fit, and what it needed.
///
/// Five modals used to `return` on a rect they could not fill, which drew
/// nothing at all. Pressing a key and seeing the screen not change is
/// indistinguishable from a keybinding that does not exist — and on an 80x24
/// terminal the 88-column announcement modal was already in exactly that
/// state, so the message it existed to deliver simply never arrived.
///
/// This replaces silence with one line saying which modal it was and how much
/// room it wants, which is a thing the reader can act on.
pub(super) fn render_too_small(
    frame: &mut Frame,
    area: Rect,
    label: &str,
    needs: (u16, u16),
    p: &Palette,
) {
    if area.width < 2 || area.height < 1 {
        return;
    }
    // Longest form that fits, rather than one form truncated. A message that
    // ends "— t" is worse than a short one that ends where it means to; and a
    // terminal too small for a modal is by definition too small for prose.
    let candidates = [
        format!(
            " {label} needs {}x{} — this terminal is {}x{} ",
            needs.0, needs.1, area.width, area.height
        ),
        format!(" {label} needs {}x{} ", needs.0, needs.1),
        format!(" needs {}x{} ", needs.0, needs.1),
        " too small ".to_string(),
    ];
    let text = candidates
        .iter()
        .find(|candidate| display_width_u16(candidate) <= area.width)
        .cloned()
        .unwrap_or_else(|| candidates.last().cloned().unwrap_or_default());
    let width = display_width_u16(&text).min(area.width);
    let rect = Rect::new(
        area.x + area.width.saturating_sub(width) / 2,
        area.y + area.height / 2,
        width,
        1,
    );
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(Span::styled(
            text,
            Style::default()
                .fg(panel_contrast_fg(p))
                .bg(p.peach)
                .add_modifier(Modifier::BOLD),
        ))
        .alignment(Alignment::Center),
        rect,
    );
}

/// One modal frame, or one line explaining why there is not one.
///
/// `want` is the size the modal would like; `needs_inner` is what its content
/// genuinely cannot do without. Both live here rather than as a `return` at
/// each of eight call sites, so "too small" is a single policy with a single
/// look — and so that adding a modal cannot reintroduce the silent version.
pub(super) fn render_modal_or_notice(
    frame: &mut Frame,
    area: Rect,
    want: (u16, u16),
    needs_inner: (u16, u16),
    label: &str,
    p: &Palette,
) -> Option<Rect> {
    // Borders cost two columns and two rows on each axis.
    let needs_outer = (
        needs_inner.0.saturating_add(2),
        needs_inner.1.saturating_add(2),
    );
    let inner = centered_popup_rect(area, want.0, want.1)
        .filter(|popup| popup.width >= needs_outer.0 && popup.height >= needs_outer.1)
        .and_then(|popup| render_panel_shell(frame, popup, p.accent, p.panel_bg));
    match inner {
        Some(inner) => Some(inner),
        None => {
            // `centered_popup_rect` insets by 4 columns and 2 rows, so report
            // what the *terminal* has to be rather than what the popup is.
            render_too_small(
                frame,
                area,
                label,
                (
                    needs_outer.0.saturating_add(4),
                    needs_outer.1.saturating_add(2),
                ),
                p,
            );
            None
        }
    }
}

/// The scroll half of a scrollable modal's footer.
///
/// Keys first and always, wheel only when shep is actually receiving one.
/// Three modals used to advertise `wheel ↑↓` — two of them advertised *nothing
/// else* — so with `mouse_capture = false` their footers named the one control
/// that does nothing and stayed silent about `jk/↑↓`, `pgup/pgdn`, `home` and
/// `end`, all of which have always worked.
pub(super) fn scroll_hint_spans<'a>(mouse_capture: bool, p: &Palette) -> Vec<Span<'a>> {
    let label = Style::default().fg(p.overlay0);
    let key = Style::default().fg(p.text);
    let mut spans = vec![
        Span::styled(" scroll ", label),
        Span::styled(glyphs::KEYS_VERTICAL, key),
    ];
    if mouse_capture {
        spans.push(Span::styled(" or ", label));
        spans.push(Span::styled(glyphs::KEYS_WHEEL, key));
    }
    spans.push(Span::styled(glyphs::SEP_WIDE, label));
    spans.push(Span::styled("jump", label));
    // No trailing space: SEP_WIDE brings its own, and the two together made
    // this gap one column wider than every other gap on the row.
    spans.push(Span::styled(" pgup / pgdn", key));
    spans
}

pub(super) fn render_modal_header(frame: &mut Frame, area: Rect, title: &str, p: &Palette) {
    let line = Line::from(vec![Span::styled(
        title,
        Style::default().fg(p.text).add_modifier(Modifier::BOLD),
    )]);
    frame.render_widget(Paragraph::new(line), area);
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct ModalStackAreas {
    pub header: Rect,
    pub content: Rect,
    pub footer: Option<Rect>,
    pub actions: Option<Rect>,
}

pub(crate) fn modal_stack_areas(
    inner: Rect,
    header_height: u16,
    footer_height: u16,
    actions_height: u16,
    gap: u16,
) -> ModalStackAreas {
    #[derive(Clone, Copy)]
    enum Slot {
        Header,
        Content,
        Footer,
        Actions,
    }

    let mut constraints = Vec::new();
    let mut slots = Vec::new();
    let mut push = |slot: Slot, constraint: Constraint| {
        if !slots.is_empty() {
            constraints.push(Constraint::Length(gap));
        }
        constraints.push(constraint);
        slots.push(slot);
    };

    push(Slot::Header, Constraint::Length(header_height));
    push(Slot::Content, Constraint::Min(0));
    if footer_height > 0 {
        push(Slot::Footer, Constraint::Length(footer_height));
    }
    if actions_height > 0 {
        push(Slot::Actions, Constraint::Length(actions_height));
    }

    let areas = Layout::vertical(constraints).split(inner);
    let mut header = Rect::default();
    let mut content = Rect::default();
    let mut footer = None;
    let mut actions = None;

    for (slot, area) in slots.into_iter().zip(areas.iter().step_by(2).copied()) {
        match slot {
            Slot::Header => header = area,
            Slot::Content => content = area,
            Slot::Footer => footer = Some(area),
            Slot::Actions => actions = Some(area),
        }
    }

    ModalStackAreas {
        header,
        content,
        footer,
        actions,
    }
}

pub(crate) fn action_button_text(hint: Option<&str>, label: &str) -> String {
    match hint {
        Some(hint) => format!(" {hint} {label} "),
        None => format!(" {label} "),
    }
}

pub(crate) fn action_button_width(hint: Option<&str>, label: &str) -> u16 {
    action_button_text(hint, label).chars().count() as u16
}

pub(crate) struct ActionButtonSpec<'a> {
    pub hint: Option<&'a str>,
    pub label: &'a str,
}

pub(crate) fn action_button_row_rects(
    area: Rect,
    buttons: &[ActionButtonSpec<'_>],
    gap: u16,
    row_offset: u16,
) -> Vec<Rect> {
    let widths: Vec<u16> = buttons
        .iter()
        .map(|button| action_button_width(button.hint, button.label))
        .collect();
    centered_button_row(area, &widths, gap, row_offset)
}

pub(super) fn render_action_button(
    frame: &mut Frame,
    rect: Rect,
    hint: Option<&str>,
    label: &str,
    style: Style,
) {
    frame.render_widget(
        Paragraph::new(action_button_text(hint, label))
            .style(style)
            .alignment(Alignment::Center),
        rect,
    );
}

pub(crate) fn render_modal_description(frame: &mut Frame, area: Rect, text: &str, style: Style) {
    frame.render_widget(
        Paragraph::new(format!(" {text}"))
            .style(style)
            .wrap(Wrap { trim: false }),
        area,
    );
}

pub(crate) fn modal_choice_rows(area: Rect, count: usize, row_height: u16) -> Vec<Rect> {
    let mut rows = Vec::with_capacity(count);
    let mut y = area.y;
    for _ in 0..count {
        if y >= area.y + area.height {
            break;
        }
        let remaining = area.y + area.height - y;
        let height = row_height.min(remaining);
        rows.push(Rect::new(area.x, y, area.width, height));
        y = y.saturating_add(row_height);
    }
    rows
}

pub(crate) fn render_modal_choice_list<T>(
    frame: &mut Frame,
    area: Rect,
    title: &str,
    description: &str,
    options: &[(&str, T)],
    current_value: T,
    selected_idx: usize,
    p: &Palette,
    row_height: u16,
) where
    T: Copy + PartialEq,
{
    let [desc_area, _, list_area] = Layout::vertical([
        Constraint::Length(2),
        Constraint::Length(1),
        Constraint::Min(2),
    ])
    .areas::<3>(area);

    render_modal_description(
        frame,
        desc_area,
        description,
        Style::default().fg(p.overlay1),
    );

    let rows = modal_choice_rows(list_area, options.len(), row_height);
    for (idx, ((label, value), row)) in options.iter().zip(rows.iter()).enumerate() {
        let is_active = *value == current_value;
        let is_selected = idx == selected_idx;
        let marker = if is_active { glyphs::TICK_MARKER } else { "" };
        let style = if is_selected {
            Style::default()
                .bg(p.surface0)
                .fg(p.text)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.subtext0)
        };
        frame.render_widget(
            Paragraph::new(format!(" {title}: {label}{marker}"))
                .style(style)
                .wrap(Wrap { trim: false }),
            *row,
        );
    }
}

pub(super) fn centered_button_row(
    inner: Rect,
    widths: &[u16],
    gap: u16,
    row_offset: u16,
) -> Vec<Rect> {
    let total_w = widths
        .iter()
        .copied()
        .sum::<u16>()
        .saturating_add(gap.saturating_mul(widths.len().saturating_sub(1) as u16));
    let mut x = inner.x + inner.width.saturating_sub(total_w) / 2;
    let y = inner.y + row_offset.min(inner.height.saturating_sub(1));
    widths
        .iter()
        .map(|w| {
            let rect = Rect::new(
                x,
                y,
                (*w).min(inner.width.saturating_sub(x.saturating_sub(inner.x))),
                1,
            );
            x = x.saturating_add(*w).saturating_add(gap);
            rect
        })
        .collect()
}
