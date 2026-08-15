//! Every non-ASCII mark the UI draws, in one place.
//!
//! Shep's marks were spread across twelve files as bare string literals — the
//! same `▕` appearing in three, `↵` in fourteen places across three — with no
//! way to find them all and nothing to stop a thirteenth from inventing a
//! fourteenth. That is fine right up until you need to answer a question about
//! the *set*: does anything here break in a terminal without a Nerd Font, does
//! anything change width when ambiguous glyphs are configured wide, and could
//! this render at all over a serial console.
//!
//! Collecting them makes those answerable, and makes an ASCII-safe mode a
//! change to this file rather than an archaeology project across the UI.
//!
//! **Every glyph here is one column wide.** That is not a nicety: a terminal
//! cell is one column, and a mark that measures two shifts everything after it
//! on the row. `glyphs_are_one_column` pins it, and East-Asian-Ambiguous marks
//! are called out where they are used, because a reader with a CJK-configured
//! terminal gets two columns for them.
//!
//! The *state* vocabulary is not here. It lives in `status.rs` beside the
//! colours it is meaningless without — see `docs/DESIGN-LANGUAGE.md`.

// ── Selection and focus ─────────────────────────────────────────────────────

/// The selection marker down the left edge of a row. Half a cell, filled.
pub(super) const MARKER: &str = "▌";

/// The same mark on the right edge — a scrollbar thumb, a rule.
pub(super) const RULE_RIGHT: &str = "▐";

/// A quarter-cell rule: quieter than [RULE_RIGHT], for a gutter rather than a
/// scrollbar.
pub(super) const RULE_LEFT: &str = "▕";

/// An eighth-cell rule. The quietest vertical mark shep draws.
pub(super) const RULE_THIN: &str = "▏";

// ── Disclosure and direction ────────────────────────────────────────────────

/// A collapsed node. Points at what opening it would reveal.
pub(super) const COLLAPSED: &str = "▸";

/// An expanded node.
pub(super) const EXPANDED: &str = "▾";

/// "Goes to" — a breadcrumb separator, a mapping.
pub(super) const LEADS_TO: &str = "›";

/// The same, with more weight: an explicit destination.
pub(super) const ARROW_RIGHT: &str = "→";

/// The return key, in a hint.
pub(super) const ENTER: &str = "↵";

/// A reset, a re-run.
pub(super) const RESET: &str = "↻";

/// Movement, in a keybinding hint.
pub(super) const UP_DOWN: &str = "↑↓";

/// Whole key-hint clusters.
///
/// These are `&'static str` because the hint tables are `&[(&str, &str)]` and
/// a `format!` cannot live there — and because the cluster is the unit a
/// reader recognises, not the arrows inside it.
pub(super) const KEYS_ARROWS: &str = "hjkl/↑↓←→";
pub(super) const KEYS_VERTICAL: &str = "jk/↑↓";
pub(super) const KEYS_VERTICAL_SLASHED: &str = "j/k/↑↓";
pub(super) const KEYS_WHEEL: &str = "wheel ↑↓";

/// Ahead of upstream, behind upstream.
pub(super) const AHEAD: &str = "↑";
pub(super) const BEHIND: &str = "↓";

// ── Marks ───────────────────────────────────────────────────────────────────

/// Truncation. One column, and one character — `...` costs three.
pub(super) const ELLIPSIS: &str = "…";

/// A list bullet.
pub(super) const BULLET: &str = "•";

/// A filled dot used as a *badge*, not as a state: "this section has news",
/// "this toast is a warning". The state vocabulary's `●` lives in `status.rs`
/// and means done-unseen; this one carries whatever colour it is given.
pub(super) const DOT: &str = "●";

/// Between two facts on one line.
///
/// Bare when it joins parts of one identifier (`t3·p2`); [SEP_SPACED] when it
/// separates phrases; [SEP_WIDE] on a strip where the eye needs more help
/// finding the boundaries.
pub(super) const SEP: &str = "·";
pub(super) const SEP_SPACED: &str = " · ";
pub(super) const SEP_WIDE: &str = "  ·  ";

/// A badge dot with its trailing space, for the `&'static str` slots that
/// cannot take a `format!`.
pub(super) const DOT_SPACED: &str = "● ";
/// A tick with its leading space: "this is the current one".
pub(super) const TICK_MARKER: &str = " ✓";
/// A checkbox, on and off. The off state is deliberately the same width.
pub(super) const CHECKED: &str = "[✓]";
pub(super) const UNCHECKED: &str = "[ ]";
/// A list's selection cursor, spaced to sit in its own column.
pub(super) const COLLAPSED_MARKER: &str = " ▸ ";

/// A separator between two facts on one line, with spaces around it.
pub(super) const DASH: &str = "—";

/// A range or a placeholder for a missing number.
pub(super) const EN_DASH: &str = "–";

/// Yes, done, approved. See `status.rs` — it means *approved* and nothing else.
pub(super) const TICK: &str = "✓";

/// Input waiting for an agent to go idle.
///
/// East-Asian-Ambiguous: two columns on a CJK-configured terminal. Kept
/// because it is the one mark that reads as "queued" without a legend, and
/// because it only ever appears at the end of a badge where a second column
/// costs nothing.
pub(super) const QUEUED: &str = "⇥";

// ── Meters ──────────────────────────────────────────────────────────────────

/// Block eighths, empty through full.
///
/// A bar drawn in whole cells can only be as precise as its width — an
/// eight-cell gauge has nine states, so 12% and 24% look identical. These give
/// each cell eight, which is the difference between a gauge that reports and
/// one that rounds.
pub(super) const EIGHTHS: [&str; 9] = [" ", "▏", "▎", "▍", "▌", "▋", "▊", "▉", "█"];

/// Fill `width` cells to `fraction`, to the nearest eighth of a cell.
///
/// `track` is what an unfilled cell draws — `" "` for a bar that relies on a
/// background colour, `"░"` for one drawn in a single span where the track has
/// to be visible in the ink itself.
pub(super) fn bar(fraction: f32, width: usize, track: &str) -> String {
    if width == 0 {
        return String::new();
    }
    let eighths = (fraction.clamp(0.0, 1.0) * (width * 8) as f32).round() as usize;
    let full = eighths / 8;
    let remainder = eighths % 8;
    let mut out = String::with_capacity(width * 3);
    for _ in 0..full.min(width) {
        out.push_str(EIGHTHS[8]);
    }
    if full < width && remainder > 0 {
        out.push_str(EIGHTHS[remainder]);
    }
    let drawn = full.min(width) + usize::from(full < width && remainder > 0);
    for _ in drawn..width {
        out.push_str(track);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::text::display_width;

    /// One cell, one column. A mark that measures two shifts the whole row.
    #[test]
    fn glyphs_are_one_column() {
        let single = [
            MARKER,
            RULE_RIGHT,
            RULE_LEFT,
            RULE_THIN,
            COLLAPSED,
            EXPANDED,
            LEADS_TO,
            ARROW_RIGHT,
            ENTER,
            RESET,
            AHEAD,
            BEHIND,
            ELLIPSIS,
            BULLET,
            DASH,
            EN_DASH,
            TICK,
            QUEUED,
        ];
        for glyph in single {
            assert_eq!(display_width(glyph), 1, "{glyph:?} is not one column");
        }
        assert_eq!(display_width(UP_DOWN), 2, "{UP_DOWN:?}");
        for eighth in EIGHTHS {
            assert_eq!(display_width(eighth), 1, "{eighth:?} is not one column");
        }
    }

    /// A bar is exactly as wide as it was asked to be, whatever the fraction —
    /// otherwise it pushes whatever follows it off the end of the row.
    #[test]
    fn a_bar_is_the_width_it_was_asked_for() {
        for width in [0usize, 1, 4, 8, 20] {
            for step in 0..=40 {
                let fraction = step as f32 / 40.0;
                assert_eq!(
                    display_width(&bar(fraction, width, " ")),
                    width,
                    "bar({fraction}, {width})"
                );
            }
        }
    }

    /// The point of eighths: two fractions a whole cell apart cannot look the
    /// same. A four-cell bar used to have five states; it now has thirty-three.
    #[test]
    fn eighths_resolve_what_whole_cells_cannot() {
        let width = 4;
        let distinct: std::collections::HashSet<String> =
            (0..=32).map(|n| bar(n as f32 / 32.0, width, " ")).collect();
        assert_eq!(distinct.len(), 33);
    }

    /// Clamped, not wrapped: a percentage over 100 draws a full bar rather
    /// than an empty one.
    #[test]
    fn out_of_range_fractions_clamp() {
        assert_eq!(bar(-1.0, 4, " "), "    ");
        assert_eq!(bar(2.0, 4, " "), "████");
    }

    /// A named mark is defined once.
    ///
    /// Every glyph in this file, plus the state vocabulary in `status.rs`, is
    /// banned as a literal everywhere else in `src/ui`. That is the whole
    /// point of collecting them: a `▌` typed into a thirteenth file is a
    /// second definition, and second definitions are how `yellow` came to mean
    /// both *working* and *needs review*.
    ///
    /// Box drawing is deliberately **not** covered. It is structural rather
    /// than semantic, ratatui owns a canonical set of it, and an ASCII-safe
    /// mode would swap ratatui's border set rather than these constants.
    #[test]
    fn a_named_mark_is_defined_once() {
        let named = [
            MARKER,
            RULE_RIGHT,
            RULE_LEFT,
            RULE_THIN,
            COLLAPSED,
            EXPANDED,
            LEADS_TO,
            ARROW_RIGHT,
            ENTER,
            RESET,
            ELLIPSIS,
            BULLET,
            DOT,
            EN_DASH,
            TICK,
            QUEUED,
            SEP,
            // The state vocabulary, from `status.rs`.
            "◉",
            "○",
            "⠋",
        ];
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/ui");
        let mut offenders = Vec::new();
        let mut scanned = 0usize;
        let mut walk = vec![root];
        while let Some(dir) = walk.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk.push(path);
                    continue;
                }
                let name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or_default();
                // glyphs.rs and status.rs are the two homes; snapshot.rs is a
                // fixture full of deliberate literals.
                if !name.ends_with(".rs")
                    || matches!(name, "glyphs.rs" | "status.rs" | "snapshot.rs")
                {
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                scanned += 1;
                // Production only — test fixtures name glyphs on purpose.
                let body = match text.find("\nmod tests") {
                    Some(cut) => &text[..cut],
                    None => &text[..],
                };
                for (number, line) in body.lines().enumerate() {
                    let trimmed = line.trim_start();
                    if trimmed.starts_with("//") {
                        continue;
                    }
                    for glyph in named {
                        // Only inside a string literal, so a comment or an
                        // identifier does not trip it.
                        if line
                            .split('"')
                            .skip(1)
                            .step_by(2)
                            .any(|lit| lit.contains(glyph))
                        {
                            offenders.push(format!("{name}:{}: {glyph}", number + 1));
                        }
                    }
                }
            }
        }
        assert!(scanned > 10, "scanned only {scanned} files — wrong path?");
        assert!(
            offenders.is_empty(),
            "these marks have a name in ui/glyphs.rs or ui/status.rs; use it:\n  {}",
            offenders.join("\n  ")
        );
    }
}
