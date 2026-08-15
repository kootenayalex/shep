use ratatui::{
    layout::{Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, Paragraph},
    Frame,
};

use super::text::display_width_u16;
use super::widgets::panel_contrast_fg;
use crate::{
    app::state::{CopyFeedback, Palette, ToastKind, ToastNotification},
    config::{ToastClipboardPosition, ToastShepPosition},
    detect::AgentState,
};

pub(crate) fn copy_feedback_rect(
    area: Rect,
    feedback: &CopyFeedback,
    offset_rows: u16,
    position: ToastClipboardPosition,
) -> Rect {
    if area.width == 0 || area.height == 0 {
        return Rect::default();
    }

    let content_width = feedback.message.len() as u16 + 4;
    let width = content_width.min(area.width);
    let height = 3u16.min(area.height);
    let x = match position {
        ToastClipboardPosition::TopLeft | ToastClipboardPosition::BottomLeft => area.x,
        ToastClipboardPosition::TopCenter | ToastClipboardPosition::BottomCenter => {
            area.x + area.width.saturating_sub(width) / 2
        }
        ToastClipboardPosition::TopRight | ToastClipboardPosition::BottomRight => {
            area.x + area.width.saturating_sub(width)
        }
    };
    let y = match position {
        ToastClipboardPosition::TopLeft
        | ToastClipboardPosition::TopCenter
        | ToastClipboardPosition::TopRight => area.y + offset_rows.min(area.height),
        ToastClipboardPosition::BottomLeft
        | ToastClipboardPosition::BottomCenter
        | ToastClipboardPosition::BottomRight => {
            area.y + area.height.saturating_sub(height + offset_rows)
        }
    };
    Rect::new(x, y, width, height)
}

pub(crate) fn toast_notification_rect(
    area: Rect,
    toast: &ToastNotification,
    offset_for_warning: bool,
    position: ToastShepPosition,
) -> Rect {
    let content_width = display_width_u16(&toast.title)
        .max(display_width_u16(&toast.context))
        .saturating_add(4);
    let width = content_width.saturating_add(2).min(area.width);
    let content_height = if toast.context.is_empty() { 1 } else { 2 };
    let height = (content_height + 2).min(area.height);
    let x = match position {
        ToastShepPosition::TopLeft | ToastShepPosition::BottomLeft => area.x,
        ToastShepPosition::TopRight | ToastShepPosition::BottomRight => {
            area.x + area.width.saturating_sub(width)
        }
    };
    let warning_offset = u16::from(offset_for_warning);
    let y = match position {
        ToastShepPosition::TopLeft | ToastShepPosition::TopRight => {
            area.y + warning_offset.min(area.height)
        }
        ToastShepPosition::BottomLeft | ToastShepPosition::BottomRight => {
            area.y + area.height.saturating_sub(height + warning_offset)
        }
    };
    Rect::new(x, y, width, height)
}

pub(super) fn render_toast_notification(
    frame: &mut Frame,
    area: Rect,
    toast: &ToastNotification,
    offset_for_warning: bool,
    position: ToastShepPosition,
    p: &Palette,
) {
    let dot_color = match toast.kind {
        ToastKind::NeedsAttention => p.red,
        ToastKind::Finished => p.blue,
        ToastKind::UpdateInstalled => p.accent,
    };
    let toast_area = toast_notification_rect(area, toast, offset_for_warning, position);

    frame.render_widget(Clear, toast_area);
    // Severity owns the whole card border (common region), not just the dot.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ratatui::symbols::border::ROUNDED)
        .border_style(Style::default().fg(dot_color))
        .style(Style::default().bg(p.panel_bg));
    let inner = block.inner(toast_area);
    frame.render_widget(block, toast_area);

    if inner.height < 1 {
        return;
    }

    let [title_row, context_row] =
        Layout::vertical([Constraint::Length(1), Constraint::Length(1)]).areas(inner);

    let title = Line::from(vec![
        Span::styled("●", Style::default().fg(dot_color)),
        Span::raw(" "),
        Span::styled(
            &toast.title,
            Style::default().fg(p.text).add_modifier(Modifier::BOLD),
        ),
    ]);
    let context = Line::from(vec![
        Span::styled("  ", Style::default().fg(p.overlay0)),
        Span::styled(&toast.context, Style::default().fg(p.overlay0)),
    ]);

    frame.render_widget(Paragraph::new(title), title_row);
    if !toast.context.is_empty() && inner.height >= 2 {
        frame.render_widget(Paragraph::new(context), context_row);
    }
}

pub(super) fn render_copy_feedback(
    frame: &mut Frame,
    area: Rect,
    feedback: &CopyFeedback,
    offset_rows: u16,
    position: ToastClipboardPosition,
    p: &Palette,
) {
    let feedback_area = copy_feedback_rect(area, feedback, offset_rows, position);
    if feedback_area.is_empty() {
        return;
    }

    frame.render_widget(Clear, feedback_area);
    let block = Block::default()
        .borders(Borders::ALL)
        .border_set(ratatui::symbols::border::ROUNDED)
        .border_style(Style::default().fg(p.green))
        .style(Style::default().bg(p.panel_bg));
    let inner = block.inner(feedback_area);
    frame.render_widget(block, feedback_area);

    if inner.height == 0 {
        return;
    }

    let text = Line::from(vec![
        Span::styled("●", Style::default().fg(p.green).bg(p.panel_bg)),
        Span::raw(" "),
        Span::styled(
            &feedback.message,
            Style::default()
                .fg(p.text)
                .bg(p.panel_bg)
                .add_modifier(Modifier::BOLD),
        ),
    ]);
    frame.render_widget(Paragraph::new(text), inner);
}

pub(super) fn render_config_diagnostic(frame: &mut Frame, area: Rect, message: &str, p: &Palette) {
    let style = Style::default()
        .fg(panel_contrast_fg(p))
        .bg(p.yellow)
        .add_modifier(Modifier::BOLD);

    for (row, line) in message
        .lines()
        .filter(|line| !line.trim().is_empty())
        .take(area.height as usize)
        .enumerate()
    {
        let text = format!(" config warning: {line} ");
        let width = (text.len() as u16).min(area.width);
        let notif_area = Rect::new(
            area.x + area.width.saturating_sub(width),
            area.y + row as u16,
            width,
            1,
        );

        frame.render_widget(Clear, notif_area);
        frame.render_widget(Paragraph::new(Span::styled(text, style)), notif_area);
    }
}

/// Everything a surface needs to draw one agent's state.
///
/// One table so the glyph and the colour cannot drift apart. They used to live
/// in four separate `match`es and did drift: the board card, the sidebar row and
/// the phone each picked their own, and three of the five states ended up
/// sharing a filled `●` that differed only in hue.
pub(super) struct StateAppearance {
    pub glyph: &'static str,
    pub label: &'static str,
    pub ink: StateInk,
}

impl StateAppearance {
    pub fn color(&self, p: &Palette) -> Color {
        self.ink.resolve(p)
    }

    pub fn style(&self, p: &Palette) -> Style {
        Style::default().fg(self.color(p))
    }
}

/// Which palette tier a state draws from. Named rather than resolved so the
/// label and glyph can be asked for without a palette in hand.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum StateInk {
    Stop,
    Working,
    DoneUnseen,
    Settled,
    /// Accepted but not started. Dimmer than working, brighter than absent —
    /// the queue is a backlog, and the eye should land on what is moving.
    Waiting,
    Absent,
}

impl StateInk {
    fn resolve(self, p: &Palette) -> Color {
        match self {
            StateInk::Stop => p.red,
            StateInk::Working => p.yellow,
            StateInk::DoneUnseen => p.blue,
            StateInk::Settled => p.green,
            StateInk::Waiting => p.overlay1,
            StateInk::Absent => p.overlay0,
        }
    }
}

/// The agent-state vocabulary. See `docs/DESIGN-LANGUAGE.md`; the companion
/// implements the same table in `ui/theme/ShepSemantic.kt`.
///
/// `seen` splits idle in two on purpose — an agent that finished while you were
/// away is a different claim from one you have already looked at.
pub(super) fn state_appearance(state: AgentState, seen: bool, tick: u32) -> StateAppearance {
    let (glyph, label, ink) = match (state, seen) {
        (AgentState::Blocked, _) => ("◉", "blocked", StateInk::Stop),
        (AgentState::Working, _) => (super::spinner_frame(tick), "working", StateInk::Working),
        (AgentState::Idle, false) => ("●", "done", StateInk::DoneUnseen),
        // Hollow, not `✓`: that glyph is the approved review badge and nothing
        // else. A state and a badge sharing a mark made both ambiguous.
        (AgentState::Idle, true) => ("○", "idle", StateInk::Settled),
        (AgentState::Unknown, _) => ("·", "idle", StateInk::Absent),
    };
    StateAppearance { glyph, label, ink }
}

/// The task-queue vocabulary. Same shapes, because they mean the same things.
///
/// A ring is stopped, movement is working, filled is finished, hollow is
/// waiting, a speck is nothing. Only "done" takes a different tier from the
/// agent table — settled rather than done-unseen — because a task has no
/// notion of your having looked at it.
///
/// The queue used to draw a filled `●` for all five states and differ only in
/// hue, which is the one thing `docs/DESIGN-LANGUAGE.md` says never to do.
pub(super) fn task_appearance(state: crate::tasks::TaskState, tick: u32) -> StateAppearance {
    use crate::tasks::TaskState;
    let (glyph, label, ink) = match state {
        TaskState::Blocked => ("◉", "blocked", StateInk::Stop),
        TaskState::Running => (super::spinner_frame(tick), "running", StateInk::Working),
        TaskState::Done => ("●", "done", StateInk::Settled),
        TaskState::Todo => ("○", "todo", StateInk::Waiting),
        TaskState::Cancelled => ("·", "cancelled", StateInk::Absent),
    };
    StateAppearance { glyph, label, ink }
}

/// One state's mark, without a palette.
///
/// For the places that draw a state's glyph beside a *count* rather than
/// beside a name — a title-bar badge, a mobile summary. Those used to write
/// the literal `●` for blocked, which is the done-unseen mark: a red `●` is a
/// glyph and a colour from two different rows of the table.
///
/// `seen` is false, so `Idle` reads as "done" — which is the right default for
/// a badge counting agents that finished while you were elsewhere.
pub(super) fn state_glyph(state: AgentState) -> &'static str {
    state_appearance(state, false, 0).glyph
}

pub(super) fn agent_icon(
    state: AgentState,
    seen: bool,
    tick: u32,
    p: &Palette,
) -> (&'static str, Style) {
    let it = state_appearance(state, seen, tick);
    (it.glyph, it.style(p))
}

/// The review-lifecycle badge, or `None` when there is nothing to say.
///
/// Kept beside the state table because badges and states share a palette and
/// used to fight over it: `◆` was yellow, which is the working tier, so a space
/// waiting for review looked like a space that was running. `✓` is the approved
/// badge and nothing else — see `docs/DESIGN-LANGUAGE.md`.
pub(super) fn review_badge(
    state: crate::api::schema::ReviewState,
    p: &Palette,
) -> Option<(&'static str, Style)> {
    use crate::api::schema::ReviewState;
    let (glyph, color) = match state {
        ReviewState::None => return None,
        ReviewState::NeedsReview => ("\u{25c6}", p.mauve),
        ReviewState::ChangesRequested => ("\u{21ba}", p.peach),
        ReviewState::Approved => ("\u{2713}", p.green),
    };
    Some((glyph, Style::default().fg(color)))
}

pub(super) fn state_label(state: AgentState, seen: bool) -> &'static str {
    state_appearance(state, seen, 0).label
}

pub(super) fn state_label_color(state: AgentState, seen: bool, p: &Palette) -> Color {
    state_appearance(state, seen, 0).color(p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{ToastClipboardPosition, ToastShepPosition};

    /// The state table from `docs/DESIGN-LANGUAGE.md`, spelled out.
    ///
    /// The companion has the same table in `ShepSemanticTest.kt`. If you change
    /// one, change the other and the doc — a phone that disagrees with the
    /// desktop about what yellow means is the bug this pins.
    /// The queue's labels are the wire format, so a typo here would rename a
    /// state in the UI while the server kept calling it something else.
    #[test]
    fn task_labels_are_the_wire_format() {
        use crate::tasks::TaskState;
        for state in [
            TaskState::Todo,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Done,
            TaskState::Cancelled,
        ] {
            assert_eq!(task_appearance(state, 0).label, state.as_str());
        }
    }

    /// Colour is never the only channel — the queue used to draw one filled
    /// dot for all five states.
    #[test]
    fn every_task_state_has_its_own_glyph() {
        use crate::tasks::TaskState;
        let glyphs: Vec<&str> = [
            TaskState::Todo,
            TaskState::Running,
            TaskState::Blocked,
            TaskState::Done,
            TaskState::Cancelled,
        ]
        .into_iter()
        .map(|s| task_appearance(s, 0).glyph)
        .collect();
        let unique: std::collections::HashSet<&&str> = glyphs.iter().collect();
        assert_eq!(unique.len(), glyphs.len(), "{glyphs:?}");
    }

    #[test]
    fn state_vocabulary_matches_the_design_language() {
        let p = Palette::shep();
        let row = |state, seen| {
            let it = state_appearance(state, seen, 0);
            (it.glyph, it.label, it.color(&p))
        };
        assert_eq!(row(AgentState::Blocked, true), ("◉", "blocked", p.red));
        assert_eq!(row(AgentState::Blocked, false), ("◉", "blocked", p.red));
        // Frame 0 of the spinner; the glyph animates, the label and ink do not.
        assert_eq!(row(AgentState::Working, true), ("⠋", "working", p.yellow));
        assert_eq!(row(AgentState::Idle, false), ("●", "done", p.blue));
        assert_eq!(row(AgentState::Idle, true), ("○", "idle", p.green));
        assert_eq!(row(AgentState::Unknown, true), ("·", "idle", p.overlay0));
    }

    /// Every state must be told apart without colour — a monochrome themed icon
    /// and a colour-blind reader both have to work. Three board-card states
    /// once shared a filled `●` and differed only in hue.
    #[test]
    fn every_state_has_its_own_glyph() {
        let mut glyphs = vec![
            state_appearance(AgentState::Blocked, true, 0).glyph,
            state_appearance(AgentState::Working, true, 0).glyph,
            state_appearance(AgentState::Idle, false, 0).glyph,
            state_appearance(AgentState::Idle, true, 0).glyph,
            state_appearance(AgentState::Unknown, true, 0).glyph,
        ];
        let total = glyphs.len();
        glyphs.sort_unstable();
        glyphs.dedup();
        assert_eq!(glyphs.len(), total, "states share a glyph: {glyphs:?}");
    }

    #[test]
    fn review_badges_do_not_borrow_a_state_colour() {
        use crate::api::schema::ReviewState;
        let p = Palette::shep();
        assert!(review_badge(ReviewState::None, &p).is_none());
        let ink = |state| {
            review_badge(state, &p)
                .expect("badge should render")
                .1
                .fg
                .expect("badge should have a foreground")
        };
        // Needs-review must not be yellow: that is the working tier.
        assert_eq!(ink(ReviewState::NeedsReview), p.mauve);
        assert_ne!(ink(ReviewState::NeedsReview), p.yellow);
        assert_eq!(ink(ReviewState::ChangesRequested), p.peach);
        assert_eq!(ink(ReviewState::Approved), p.green);
    }

    /// `✓` belongs to the approved badge alone. It used to be idle's glyph too.
    #[test]
    fn the_approved_tick_is_not_also_a_state() {
        use crate::api::schema::ReviewState;
        let p = Palette::shep();
        let tick = review_badge(ReviewState::Approved, &p)
            .expect("approved renders")
            .0;
        for (state, seen) in [
            (AgentState::Blocked, true),
            (AgentState::Working, true),
            (AgentState::Idle, false),
            (AgentState::Idle, true),
            (AgentState::Unknown, true),
        ] {
            assert_ne!(state_appearance(state, seen, 0).glyph, tick);
        }
    }

    fn toast() -> ToastNotification {
        ToastNotification {
            kind: ToastKind::Finished,
            title: "done".to_string(),
            context: "workspace".to_string(),
            position: None,
            target: None,
        }
    }

    fn feedback() -> CopyFeedback {
        CopyFeedback {
            message: "copied to clipboard".to_string(),
        }
    }

    #[test]
    fn toast_rect_uses_configured_corner() {
        let area = Rect::new(10, 20, 100, 40);
        let toast = toast();

        let top_left = toast_notification_rect(area, &toast, false, ToastShepPosition::TopLeft);
        assert_eq!(top_left.x, area.x);
        assert_eq!(top_left.y, area.y);

        let top_right = toast_notification_rect(area, &toast, false, ToastShepPosition::TopRight);
        assert_eq!(top_right.x + top_right.width, area.x + area.width);
        assert_eq!(top_right.y, area.y);

        let bottom_left =
            toast_notification_rect(area, &toast, false, ToastShepPosition::BottomLeft);
        assert_eq!(bottom_left.x, area.x);
        assert_eq!(bottom_left.y + bottom_left.height, area.y + area.height);

        let bottom_right =
            toast_notification_rect(area, &toast, false, ToastShepPosition::BottomRight);
        assert_eq!(bottom_right.x + bottom_right.width, area.x + area.width);
        assert_eq!(bottom_right.y + bottom_right.height, area.y + area.height);
    }

    #[test]
    fn toast_rect_uses_display_width_for_cjk_labels() {
        let area = Rect::new(0, 0, 100, 20);
        let toast = ToastNotification {
            kind: ToastKind::NeedsAttention,
            title: "重构用户认证模块".to_string(),
            context: "提交 shep 的反馈".to_string(),
            position: None,
            target: None,
        };

        let rect = toast_notification_rect(area, &toast, false, ToastShepPosition::TopRight);

        let expected_content_width =
            display_width_u16(&toast.title).max(display_width_u16(&toast.context)) + 6;
        assert_eq!(rect.width, expected_content_width);
        assert_eq!(rect.x + rect.width, area.x + area.width);
    }

    #[test]
    fn copy_feedback_rect_uses_configured_position() {
        let area = Rect::new(10, 20, 100, 40);
        let feedback = feedback();

        let top_center = copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::TopCenter);
        assert_eq!(top_center.y, area.y);
        assert_eq!(
            top_center.x,
            area.x + area.width.saturating_sub(top_center.width) / 2
        );

        let bottom_center =
            copy_feedback_rect(area, &feedback, 0, ToastClipboardPosition::BottomCenter);
        assert_eq!(bottom_center.y + bottom_center.height, area.y + area.height);
        assert_eq!(
            bottom_center.x,
            area.x + area.width.saturating_sub(bottom_center.width) / 2
        );
    }
}
