//! Pure, I/O-free core of shep's shared hermes-style memory.
//!
//! A memory file is plain markdown split into two regions by lines that contain
//! only the section-sign delimiter (`§`):
//!
//! ```text
//! <template header>        <- free, never counted against the cap
//! §
//! <entry 1>                <- one fact, may be multiline
//! §
//! <entry 2>
//! §
//! ```
//!
//! ## Cap-accounting rule (exact)
//!
//! The character cap counts **only entry content**: the sum of
//! `char` counts of each entry's trimmed body. It does NOT count the template
//! header, the `§` delimiter lines, or the whitespace between entries. This is
//! what [`MemoryDoc::used_chars`] returns and what every overflow check uses.
//!
//! Overflow never truncates or auto-compacts: an add/replace that would push
//! entry content past the cap fails with [`MemoryError::Overflow`], which
//! reports current usage and instructs consolidation. This is the hermes
//! forcing function that keeps memory curated.

use std::fmt;

/// A line consisting only of this string (after trimming) separates entries.
pub(crate) const DELIMITER: &str = "§";

/// Parsed memory document: a free-form header plus ordered entry bodies.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemoryDoc {
    /// Template header — everything before the first delimiter line. Free.
    header: String,
    /// Entry bodies, trimmed, in file order. Each is counted against the cap.
    entries: Vec<String>,
}

/// Usage stats for a memory file. `used`/`entries` are facts about content;
/// `cap` is the policy limit for the file kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Usage {
    pub used: usize,
    pub cap: usize,
    pub entries: usize,
}

impl Usage {
    /// Percentage of the cap consumed by entry content (0..=100+, saturating at
    /// the true ratio; can exceed 100 only for pre-existing over-cap files).
    pub fn percent(&self) -> u32 {
        if self.cap == 0 {
            return 0;
        }
        // Round to nearest whole percent.
        ((self.used as u64 * 100 + self.cap as u64 / 2) / self.cap as u64) as u32
    }
}

impl fmt::Display for Usage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}/{} chars ({}% full), {} entr{}",
            self.used,
            self.cap,
            self.percent(),
            self.entries,
            if self.entries == 1 { "y" } else { "ies" }
        )
    }
}

/// Errors from mutating a [`MemoryDoc`]. All carry enough context for a helpful
/// CLI message; none mutate the document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum MemoryError {
    /// Add/replace text was empty after trimming.
    Empty,
    /// The new entry is byte-for-byte identical to an existing entry.
    Duplicate,
    /// A substring op matched no entry.
    NoMatch { substring: String },
    /// A substring op matched more than one entry; the candidates are listed so
    /// the caller can disambiguate.
    MultipleMatches {
        substring: String,
        candidates: Vec<String>,
    },
    /// The op would push entry content past the cap. Never truncates.
    Overflow { usage: Usage, incoming: usize },
}

impl fmt::Display for MemoryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MemoryError::Empty => write!(f, "entry text is empty"),
            MemoryError::Duplicate => {
                write!(f, "entry already present (exact duplicate); nothing to do")
            }
            MemoryError::NoMatch { substring } => {
                write!(f, "no entry matches substring {substring:?}")
            }
            MemoryError::MultipleMatches {
                substring,
                candidates,
            } => {
                writeln!(
                    f,
                    "substring {substring:?} matches {} entries; be more specific. candidates:",
                    candidates.len()
                )?;
                for candidate in candidates {
                    writeln!(f, "  - {}", first_line(candidate))?;
                }
                Ok(())
            }
            MemoryError::Overflow { usage, incoming } => write!(
                f,
                "memory full: {usage}; adding {incoming} chars would exceed the {} char cap. \
                 Consolidate existing entries (merge overlapping, drop the stalest) before \
                 adding — shep never auto-compacts.",
                usage.cap
            ),
        }
    }
}

impl std::error::Error for MemoryError {}

impl MemoryDoc {
    /// Parse a raw memory file. Text before the first `§` line is the header;
    /// each region between `§` lines is an entry (trimmed; empty regions
    /// dropped). A file with no delimiter is treated as all-header, no entries.
    pub fn parse(raw: &str) -> Self {
        let mut segments = split_on_delimiter_lines(raw);
        // There is always at least one segment (the header, possibly empty).
        let header = segments.remove(0);
        let entries = segments
            .into_iter()
            .map(|segment| segment.trim().to_string())
            .filter(|entry| !entry.is_empty())
            .collect();
        MemoryDoc { header, entries }
    }

    /// The free-form header region: everything before the first delimiter line.
    /// Never counted against the cap.
    pub fn header(&self) -> &str {
        &self.header
    }

    /// Replace the header region wholesale, leaving entries untouched. Used by
    /// header refresh to upgrade a managed block in place.
    pub fn set_header(&mut self, header: String) {
        self.header = header;
    }

    /// Build a fresh document from a template header with no entries.
    pub fn from_template(header: &str) -> Self {
        MemoryDoc {
            header: header.to_string(),
            entries: Vec::new(),
        }
    }

    /// Canonical serialization: header, then each entry framed by `§` lines. A
    /// document with no entries still emits one delimiter so the entries region
    /// is visible. `parse(render(doc))` preserves the entries.
    pub fn render(&self) -> String {
        let mut out = self.header.trim_end().to_string();
        out.push('\n');
        for entry in &self.entries {
            out.push('\n');
            out.push_str(DELIMITER);
            out.push_str("\n\n");
            out.push_str(entry);
            out.push('\n');
        }
        if self.entries.is_empty() {
            out.push('\n');
            out.push_str(DELIMITER);
            out.push('\n');
        }
        out
    }

    pub fn entries(&self) -> &[String] {
        &self.entries
    }

    /// Total entry-content characters — the cap-accounted quantity.
    pub fn used_chars(&self) -> usize {
        self.entries.iter().map(|entry| entry.chars().count()).sum()
    }

    pub fn usage(&self, cap: usize) -> Usage {
        Usage {
            used: self.used_chars(),
            cap,
            entries: self.entries.len(),
        }
    }

    /// Append a new entry. Fails on empty text, an exact duplicate, or if the
    /// resulting entry content would exceed `cap`.
    pub fn add(&mut self, text: &str, cap: usize) -> Result<(), MemoryError> {
        let entry = text.trim();
        if entry.is_empty() {
            return Err(MemoryError::Empty);
        }
        if self.entries.iter().any(|existing| existing == entry) {
            return Err(MemoryError::Duplicate);
        }
        let incoming = entry.chars().count();
        let projected = self.used_chars() + incoming;
        if projected > cap {
            return Err(MemoryError::Overflow {
                usage: self.usage(cap),
                incoming,
            });
        }
        self.entries.push(entry.to_string());
        Ok(())
    }

    /// Replace the single entry containing `old_substring` with `new_text`.
    /// Errors on zero or multiple matches, empty replacement, a duplicate, or
    /// overflow. Overflow is checked against the content total after removing
    /// the old entry and adding the new one.
    pub fn replace(
        &mut self,
        old_substring: &str,
        new_text: &str,
        cap: usize,
    ) -> Result<(), MemoryError> {
        let entry = new_text.trim();
        if entry.is_empty() {
            return Err(MemoryError::Empty);
        }
        let index = self.single_match(old_substring)?;
        if self
            .entries
            .iter()
            .enumerate()
            .any(|(other, existing)| other != index && existing == entry)
        {
            return Err(MemoryError::Duplicate);
        }
        let outgoing = self.entries[index].chars().count();
        let incoming = entry.chars().count();
        let projected = self.used_chars() - outgoing + incoming;
        if projected > cap {
            return Err(MemoryError::Overflow {
                usage: self.usage(cap),
                incoming: incoming.saturating_sub(outgoing),
            });
        }
        self.entries[index] = entry.to_string();
        Ok(())
    }

    /// Remove the single entry containing `substring`. Errors on zero or
    /// multiple matches.
    pub fn remove(&mut self, substring: &str) -> Result<(), MemoryError> {
        let index = self.single_match(substring)?;
        self.entries.remove(index);
        Ok(())
    }

    /// Resolve a substring to exactly one entry index, or a descriptive error.
    fn single_match(&self, substring: &str) -> Result<usize, MemoryError> {
        let matches: Vec<usize> = self
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| entry.contains(substring))
            .map(|(index, _)| index)
            .collect();
        match matches.as_slice() {
            [] => Err(MemoryError::NoMatch {
                substring: substring.to_string(),
            }),
            [index] => Ok(*index),
            many => Err(MemoryError::MultipleMatches {
                substring: substring.to_string(),
                candidates: many.iter().map(|&i| self.entries[i].clone()).collect(),
            }),
        }
    }
}

/// Split raw text into segments on lines that are exactly the delimiter. The
/// first segment is always present (the header). Delimiter lines are consumed.
fn split_on_delimiter_lines(raw: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for line in raw.split_inclusive('\n') {
        if line.trim_end_matches(['\n', '\r']).trim() == DELIMITER {
            segments.push(std::mem::take(&mut current));
        } else {
            current.push_str(line);
        }
    }
    segments.push(current);
    segments
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text).trim()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CAP: usize = 100;
    const HEADER: &str = "# Memory\n\nWrite one fact per entry.";

    fn doc() -> MemoryDoc {
        MemoryDoc::from_template(HEADER)
    }

    #[test]
    fn add_appends_trimmed_entry() {
        let mut doc = doc();
        doc.add("  hello world  ", CAP).unwrap();
        assert_eq!(doc.entries(), &["hello world".to_string()]);
        assert_eq!(doc.used_chars(), "hello world".len());
    }

    #[test]
    fn add_rejects_empty() {
        let mut doc = doc();
        assert_eq!(doc.add("   \n  ", CAP), Err(MemoryError::Empty));
    }

    #[test]
    fn add_rejects_exact_duplicate() {
        let mut doc = doc();
        doc.add("fact", CAP).unwrap();
        assert_eq!(doc.add("  fact  ", CAP), Err(MemoryError::Duplicate));
        assert_eq!(doc.entries().len(), 1);
    }

    #[test]
    fn add_overflow_reports_usage_and_does_not_mutate() {
        let mut doc = doc();
        doc.add(&"a".repeat(60), CAP).unwrap();
        let err = doc.add(&"b".repeat(50), CAP).unwrap_err();
        match err {
            MemoryError::Overflow { usage, incoming } => {
                assert_eq!(usage.used, 60);
                assert_eq!(usage.cap, CAP);
                assert_eq!(incoming, 50);
            }
            other => panic!("expected overflow, got {other:?}"),
        }
        // No truncation, no partial add.
        assert_eq!(doc.entries().len(), 1);
        assert_eq!(doc.used_chars(), 60);
    }

    #[test]
    fn add_exactly_at_cap_succeeds() {
        let mut doc = doc();
        doc.add(&"x".repeat(CAP), CAP).unwrap();
        assert_eq!(doc.used_chars(), CAP);
        assert_eq!(doc.usage(CAP).percent(), 100);
    }

    #[test]
    fn replace_swaps_single_match() {
        let mut doc = doc();
        doc.add("apple pie", CAP).unwrap();
        doc.add("banana bread", CAP).unwrap();
        doc.replace("apple", "cherry tart", CAP).unwrap();
        assert_eq!(
            doc.entries(),
            &["cherry tart".to_string(), "banana bread".to_string()]
        );
    }

    #[test]
    fn replace_errors_on_no_match() {
        let mut doc = doc();
        doc.add("apple", CAP).unwrap();
        assert_eq!(
            doc.replace("zzz", "new", CAP),
            Err(MemoryError::NoMatch {
                substring: "zzz".to_string()
            })
        );
    }

    #[test]
    fn replace_errors_on_multiple_matches_listing_candidates() {
        let mut doc = doc();
        doc.add("shared token one", CAP).unwrap();
        doc.add("shared token two", CAP).unwrap();
        match doc.replace("shared", "new", CAP).unwrap_err() {
            MemoryError::MultipleMatches {
                substring,
                candidates,
            } => {
                assert_eq!(substring, "shared");
                assert_eq!(candidates.len(), 2);
            }
            other => panic!("expected multiple matches, got {other:?}"),
        }
    }

    #[test]
    fn replace_overflow_is_checked_after_removing_old() {
        let mut doc = doc();
        doc.add(&"a".repeat(90), CAP).unwrap();
        // Replacing the 90-char entry with 100 chars: 100 total, fits.
        doc.replace("aaa", &"b".repeat(100), CAP).unwrap();
        assert_eq!(doc.used_chars(), 100);
        // Replacing 100 with 101 overflows.
        assert!(matches!(
            doc.replace("bbb", &"c".repeat(101), CAP),
            Err(MemoryError::Overflow { .. })
        ));
    }

    #[test]
    fn remove_deletes_single_match() {
        let mut doc = doc();
        doc.add("keep me", CAP).unwrap();
        doc.add("delete me", CAP).unwrap();
        doc.remove("delete").unwrap();
        assert_eq!(doc.entries(), &["keep me".to_string()]);
    }

    #[test]
    fn remove_errors_on_no_and_multiple_matches() {
        let mut doc = doc();
        doc.add("alpha", CAP).unwrap();
        doc.add("alpine", CAP).unwrap();
        assert!(matches!(
            doc.remove("zzz"),
            Err(MemoryError::NoMatch { .. })
        ));
        assert!(matches!(
            doc.remove("alp"),
            Err(MemoryError::MultipleMatches { .. })
        ));
    }

    #[test]
    fn multiline_entries_round_trip_through_render_and_parse() {
        let mut doc = doc();
        doc.add("line one\nline two\nline three", CAP).unwrap();
        doc.add("second entry", CAP).unwrap();
        let rendered = doc.render();
        let reparsed = MemoryDoc::parse(&rendered);
        assert_eq!(reparsed.entries(), doc.entries());
    }

    #[test]
    fn empty_doc_renders_a_visible_delimiter_and_round_trips() {
        let rendered = doc().render();
        assert!(rendered.contains("\n§\n"));
        let reparsed = MemoryDoc::parse(&rendered);
        assert!(reparsed.entries().is_empty());
    }

    #[test]
    fn cap_accounting_ignores_header_and_delimiters() {
        // Header is large; only entry content counts.
        let big_header = "x".repeat(10_000);
        let mut doc = MemoryDoc::from_template(&big_header);
        doc.add("tiny", CAP).unwrap();
        assert_eq!(doc.used_chars(), 4);
        // Render includes header + delimiters, but usage stays 4.
        let reparsed = MemoryDoc::parse(&doc.render());
        assert_eq!(reparsed.used_chars(), 4);
    }

    #[test]
    fn used_chars_counts_unicode_scalars_not_bytes() {
        let mut doc = doc();
        doc.add("café ☕", CAP).unwrap();
        assert_eq!(doc.used_chars(), 6);
    }

    #[test]
    fn usage_percent_rounds() {
        let usage = Usage {
            used: 1100,
            cap: 2200,
            entries: 3,
        };
        assert_eq!(usage.percent(), 50);
    }
}
