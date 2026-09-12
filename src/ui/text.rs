use ratatui::text::Span;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::glyphs;

pub(crate) fn display_width(text: &str) -> usize {
    UnicodeWidthStr::width(text)
}

pub(crate) fn display_width_u16(text: &str) -> u16 {
    display_width(text).min(u16::MAX as usize) as u16
}

pub(crate) fn truncate_end(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return glyphs::ELLIPSIS.to_string();
    }

    let prefix = take_prefix_width(text, max_width.saturating_sub(1));
    format!("{prefix}{}", glyphs::ELLIPSIS)
}

/// Truncate from the front, keeping the tail: `…/vault/dev/shep`.
///
/// The mirror of `truncate_end`, for paths — the leading directories are the
/// interchangeable part, the last component is the one being identified.
pub(crate) fn truncate_start(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width == 0 {
        return String::new();
    }
    if max_width == 1 {
        return glyphs::ELLIPSIS.to_string();
    }

    let suffix = take_suffix_width(text, max_width.saturating_sub(1));
    format!("{}{suffix}", glyphs::ELLIPSIS)
}

pub(crate) fn middle_elide(text: &str, max_width: usize) -> String {
    if display_width(text) <= max_width {
        return text.to_string();
    }
    if max_width <= 1 {
        return glyphs::ELLIPSIS.to_string();
    }

    let content_width = max_width.saturating_sub(1);
    let left_width = content_width / 2;
    let right_width = content_width.saturating_sub(left_width);
    let prefix = take_prefix_width(text, left_width);
    let suffix = take_suffix_width(text, right_width);
    format!("{prefix}{}{suffix}", glyphs::ELLIPSIS)
}

/// The columns a run of spans occupies, as the terminal will count them.
pub(crate) fn spans_width(spans: &[Span<'_>]) -> usize {
    spans.iter().map(|s| display_width(&s.content)).sum()
}

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
pub(crate) fn fit_strip<'a>(
    facts: Vec<Vec<Span<'a>>>,
    sep: &Span<'a>,
    width: usize,
) -> Vec<Span<'a>> {
    let sep_width = display_width(&sep.content);
    let mut out: Vec<Span<'a>> = Vec::new();
    let mut used = 0usize;
    for fact in facts {
        let lead = if out.is_empty() { 0 } else { sep_width };
        if used + lead + spans_width(&fact) > width {
            break;
        }
        used += lead + spans_width(&fact);
        if lead > 0 {
            out.push(sep.clone());
        }
        out.extend(fact);
    }
    out
}

/// `/Users/alex/vault/dev/shep` -> `~/vault/dev/shep`. Cards and titles are
/// narrow and the home prefix is the same on every one of them.
pub(crate) fn contract_home(path: &std::path::Path) -> String {
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

fn take_prefix_width(text: &str, max_width: usize) -> String {
    let mut output = String::new();
    let mut width = 0usize;
    for ch in text.chars() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        output.push(ch);
        width += ch_width;
    }
    output
}

fn take_suffix_width(text: &str, max_width: usize) -> String {
    let mut output = Vec::new();
    let mut width = 0usize;
    for ch in text.chars().rev() {
        let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
        if width + ch_width > max_width {
            break;
        }
        output.push(ch);
        width += ch_width;
    }
    output.into_iter().rev().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncate_end_uses_display_width() {
        let text = truncate_end("提交 shep 的反馈", 15);

        assert_eq!(text, "提交 shep 的反…");
        assert!(display_width(&text) <= 15);
    }

    #[test]
    fn middle_elide_uses_display_width() {
        let text = middle_elide("重构用户认证模块并迁移到统一登录服务", 12);

        assert!(text.contains('…'));
        assert!(display_width(&text) <= 12);
    }

    fn strip_text(spans: &[Span<'_>]) -> String {
        spans.iter().map(|s| s.content.as_ref()).collect()
    }

    /// The facts that do not fit come off whole, from the tail, and the
    /// strip never ends on a separator.
    #[test]
    fn fit_strip_drops_whole_facts_rather_than_clipping_one() {
        let facts = || {
            vec![
                vec![Span::raw("agents 3")],
                vec![Span::raw("done "), Span::raw("2")],
                vec![Span::raw("working 1")],
            ]
        };
        let sep = Span::raw(" · ");
        assert_eq!(
            strip_text(&fit_strip(facts(), &sep, 80)),
            "agents 3 · done 2 · working 1"
        );
        // Exactly the first two: the third needs three more columns.
        assert_eq!(
            strip_text(&fit_strip(facts(), &sep, 17)),
            "agents 3 · done 2"
        );
        // One column short of the second fact drops it whole, not clipped.
        assert_eq!(strip_text(&fit_strip(facts(), &sep, 16)), "agents 3");
        // The order is a prefix: a short third fact never skips ahead of a
        // long second one.
        let gap = vec![
            vec![Span::raw("agents 3")],
            vec![Span::raw("a fact that is much too long")],
            vec![Span::raw("ok")],
        ];
        assert_eq!(strip_text(&fit_strip(gap, &sep, 20)), "agents 3");
        assert!(fit_strip(facts(), &sep, 0).is_empty());
        for width in 0..40 {
            let text = strip_text(&fit_strip(facts(), &sep, width));
            assert!(display_width(&text) <= width, "{width}: {text:?}");
            assert!(!text.ends_with(" · "), "{width}: {text:?}");
        }
    }

    #[test]
    fn spans_width_counts_columns_not_bytes() {
        assert_eq!(spans_width(&[]), 0);
        assert_eq!(spans_width(&[Span::raw("提交"), Span::raw(" shep")]), 4 + 5);
    }

    /// The home prefix folds to `~` only at a path boundary: `/home/alexander`
    /// is not inside `/home/alex`.
    #[test]
    fn contract_home_folds_the_home_prefix_at_a_boundary() {
        let Some(home) = std::env::var_os("HOME") else {
            return;
        };
        let home = std::path::PathBuf::from(home);
        if home.as_os_str().is_empty() {
            return;
        }
        assert_eq!(contract_home(&home), "~");
        assert_eq!(
            contract_home(&home.join("vault/dev/shep")),
            "~/vault/dev/shep"
        );
        let sibling = format!("{}ander/x", home.display());
        assert_eq!(contract_home(std::path::Path::new(&sibling)), sibling);
        assert_eq!(
            contract_home(std::path::Path::new("/tmp/repo")),
            "/tmp/repo"
        );
    }
}
