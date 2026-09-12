//! The context-window gauge: `███▍░░ 62%`, one place, so the board card, the
//! agent detail screen and the pane title all draw the same meter.
//!
//! Rendered as a bar because the number alone doesn't read at a glance — the
//! thing worth seeing across eight cards is *which agent is nearly full*.
//!
//! Drawn in eighths. Six whole cells give seven states for a hundred and one
//! percentages, so 60% and 74% were the same picture; eighths give the same
//! six columns forty-nine, which is the difference between a gauge that
//! reports and one that rounds.
//!
//! The number is right-aligned in its own three columns. The gauge is pinned to
//! the card's right edge, so an unpadded `100%` would drag the bar one column
//! left of every other card's — a shifted bar in a column of bars reads as a
//! different measurement.
//!
//! The bar is drawn cell by cell rather than as one string, and the boundary
//! cell carries the fill as foreground over the track as background. As plain
//! text the partial cell showed the panel through it and the bar read as
//! broken — a gap between the fill and the track — which every cell-exact
//! snapshot passed, because every cell was right. It took looking at pixels.

use ratatui::{
    style::{Color, Style},
    text::Span,
};

use super::glyphs;
use crate::app::state::Palette;

/// Cells in the bar, before the ` NN%` that follows it.
pub(crate) const GAUGE_WIDTH: usize = 6;

/// The point at which a context window is worth noticing.
const WARM_PERCENT: u8 = 80;

/// The gauge's ink. Peach — the warning tier — once the window is nearly
/// full, and otherwise the dim metadata tier. Never red or yellow: those are
/// *blocked* and *working*, and a meter is neither (`docs/DESIGN-LANGUAGE.md`).
pub(crate) fn gauge_color(percent: u8, p: &Palette) -> Color {
    if percent >= WARM_PERCENT {
        p.peach
    } else {
        p.overlay0
    }
}

/// A tiny inline gauge for the context window: `███▍░░ 62%`.
pub(crate) fn context_gauge_spans(percent: u8, p: &Palette) -> Vec<Span<'static>> {
    let percent = percent.min(100);
    let color = gauge_color(percent, p);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text::display_width;

    fn context_gauge(percent: u8) -> String {
        context_gauge_spans(percent, &Palette::shep())
            .iter()
            .map(|s| s.content.as_ref())
            .collect()
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
        // Over a hundred clamps rather than wrapping or overflowing the bar.
        assert_eq!(context_gauge(250), context_gauge(100));
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
        assert_eq!(widths.into_iter().next(), Some(GAUGE_WIDTH + 5));
    }

    /// The meter takes the warning tier and nothing hotter: red is a blocked
    /// agent and yellow is a working one, and a context window is neither.
    #[test]
    fn the_gauge_warms_to_peach_at_eighty_and_never_takes_a_state_tier() {
        let p = Palette::shep();
        assert_eq!(gauge_color(0, &p), p.overlay0);
        assert_eq!(gauge_color(79, &p), p.overlay0);
        assert_eq!(gauge_color(80, &p), p.peach);
        assert_eq!(gauge_color(100, &p), p.peach);
        for percent in 0..=100u8 {
            let color = gauge_color(percent, &p);
            assert_ne!(color, p.red, "{percent}% took the stop tier");
            assert_ne!(color, p.yellow, "{percent}% took the working tier");
        }
        // The number carries the same ink as the bar, so the two read as one
        // measurement.
        let spans = context_gauge_spans(85, &p);
        let number = spans.last().expect("the percentage");
        assert_eq!(number.style.fg, Some(p.peach));
        assert_eq!(spans[0].style.fg, Some(p.peach));
    }
}
