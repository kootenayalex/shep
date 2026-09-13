//! What an agent says about its own session, read from the agent's own records.
//!
//! shep used to answer "what is this agent doing" by scraping the bottom of its
//! screen (`crate::detect::extract_activity_lines`). That is parsing someone
//! else's TUI: every card ended up quoting Claude's key hints or its model
//! banner, because those are genuinely the last lines on the screen.
//!
//! Claude Code already writes the real answer down. Its transcript carries a
//! short generated title, the name `/rename` set, the permission mode, and the
//! session's cost and churn — and shep is handed that file's path by the
//! `SessionStart` hook it installs. Reading it is both cheaper and true.
//!
//! Three rules hold here, all of them the same rule `activity_lines` follows:
//!
//! 1. **These are a display hint.** Never detection evidence, never a substitute
//!    for an agent's own reported state, and never load-bearing.
//! 2. **Every failure is silence.** A missing file, a partial line written while
//!    we were reading, an agent shep has no manifest for — all of them are
//!    `None`, not an error. The file is being appended to as we read it.
//! 3. **The read is bounded.** Transcripts run to megabytes and there is one per
//!    pane; the reader seeks to the end and takes a window, never the whole file.

pub(crate) mod manifest;

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub(crate) use manifest::reload as reload_manifests;

/// What an agent reports about its own session. Every field is optional: a
/// fresh session has no title yet, and most agents have no manifest at all.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct SessionFacts {
    /// A short human summary of the session — what shows in the agent's own
    /// session list. This is what a board card wants on its summary row.
    pub title: Option<String>,
    /// The name the user gave the session inside the agent (Claude's
    /// `/rename`). shep adopts this for an agent it has no name of its own for.
    pub name: Option<String>,
    /// `normal` / `plan` / `acceptEdits` / `bypassPermissions`.
    pub permission_mode: Option<String>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

impl SessionFacts {
    /// Whether the session file said nothing we asked for. Used by tests and by
    /// callers deciding whether a refresh found anything worth redrawing.
    #[allow(dead_code)] // Read by tests; kept as the natural companion to `read`.
    pub(crate) fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    /// The permission mode when it is one worth telling somebody about.
    ///
    /// `normal` is the default and `acceptEdits` is the setting most sessions
    /// spend their life in; reporting either would put a badge on every agent,
    /// which is the same as putting one on none. `plan` and `bypassPermissions`
    /// are the two that change what you should expect the agent to do.
    pub(crate) fn notable_permission_mode(&self) -> Option<&str> {
        match self.permission_mode.as_deref()? {
            mode @ ("plan" | "bypassPermissions") => Some(mode),
            _ => None,
        }
    }
}

/// Read `path` as `agent`'s session record and return whatever it says.
///
/// Returns the default (all `None`) for an agent with no manifest, a file that
/// cannot be read, or a file that says nothing we asked for.
pub(crate) fn read(agent: &str, path: &Path) -> SessionFacts {
    let Some(manifest) = manifest::manifest_for(agent) else {
        return SessionFacts::default();
    };
    match manifest.format {
        manifest::ManifestFormat::JsonlTail => {
            let Some(text) = read_tail(path, manifest.tail_bytes()) else {
                return SessionFacts::default();
            };
            facts_from_tail(&manifest, &text)
        }
    }
}

/// Whether shep knows how to read `agent`'s session file.
///
/// The one caller is the report path, which uses this to decide whether to
/// store a path an agent handed it. Everything else can just call [`read`] and
/// get silence.
pub(crate) fn reads_session_files_for(agent: &str) -> bool {
    manifest::manifest_for(agent).is_some()
}

/// The last `limit` bytes of `path`, dropping a leading partial line.
///
/// Seeking from the end can land mid-line, and mid-line for JSON is garbage, so
/// everything before the first newline in the window is discarded. A file
/// shorter than the window is returned whole, and in that case there is no
/// partial line to drop — hence the `read_from_start` flag rather than blindly
/// cutting at the first newline, which would eat a short file's only record.
fn read_tail(path: &Path, limit: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let read_from_start = len <= limit;
    if !read_from_start {
        file.seek(SeekFrom::Start(len - limit)).ok()?;
    }
    let mut buf = Vec::with_capacity(limit.min(len) as usize);
    file.take(limit).read_to_end(&mut buf).ok()?;
    // The window can cut a multi-byte character as easily as a line.
    let text = String::from_utf8_lossy(&buf).into_owned();
    if read_from_start {
        return Some(text);
    }
    // A window with no newline at all is one enormous partial line.
    text.find('\n').map(|idx| text[idx + 1..].to_string())
}

/// Walk the tail **backwards**, and resolve each fact by manifest order.
///
/// Two rules are at work and they are not the same rule:
///
/// - **Recency**, within one record kind. These records are rewritten in full
///   whenever they change, so the file holds thousands of superseded copies and
///   only the last of each kind is current. Walking backwards makes the first
///   hit the right one, and lets the walk stop early once everything is
///   answered.
/// - **Manifest order**, between record kinds that answer the same field. A
///   title the user typed outranks the one the agent generated for itself — and
///   it has to outrank it *regardless of position*, because the generated one
///   is rewritten constantly and so is almost always the later record. Taking
///   the most recent value per block first and folding the blocks in order
///   afterwards is what keeps those two rules from fighting.
fn facts_from_tail(manifest: &manifest::SessionFactsManifest, text: &str) -> SessionFacts {
    let mut per_block: Vec<SessionFacts> = vec![SessionFacts::default(); manifest.facts.len()];
    let wanted = manifest.wanted_records();
    for line in text.lines().rev() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // Nearly every line is an `assistant` or `user` record tens of
        // kilobytes long. Rejecting those on a substring test rather than by
        // parsing them is the whole performance story of this function.
        if !wanted.iter().any(|record| line.contains(record)) {
            continue;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            // A line still being written, or one truncated by the window.
            continue;
        };
        let Some(kind) = value.get(&manifest.discriminator).and_then(|v| v.as_str()) else {
            continue;
        };
        for (idx, block) in manifest.facts.iter().enumerate() {
            if block.record != kind {
                continue;
            }
            let slot = &mut per_block[idx];
            take_str(&mut slot.title, block.title.as_deref(), &value);
            take_str(&mut slot.name, block.name.as_deref(), &value);
            take_str(
                &mut slot.permission_mode,
                block.permission_mode.as_deref(),
                &value,
            );
            take_f64(&mut slot.cost_usd, block.cost_usd.as_deref(), &value);
            take_u64(&mut slot.lines_added, block.lines_added.as_deref(), &value);
            take_u64(
                &mut slot.lines_removed,
                block.lines_removed.as_deref(),
                &value,
            );
        }
        if complete(manifest, &per_block) {
            break;
        }
    }

    let mut facts = SessionFacts::default();
    for block in per_block {
        facts.title = facts.title.or(block.title);
        facts.name = facts.name.or(block.name);
        facts.permission_mode = facts.permission_mode.or(block.permission_mode);
        facts.cost_usd = facts.cost_usd.or(block.cost_usd);
        facts.lines_added = facts.lines_added.or(block.lines_added);
        facts.lines_removed = facts.lines_removed.or(block.lines_removed);
    }
    facts
}

/// Whether every key every block declares has been answered, so the backwards
/// walk can stop.
///
/// This asks per block, not per field: a manifest where two blocks answer
/// `title` is not finished the moment one of them does, because the other may
/// be the higher-precedence one and still be waiting further back in the file.
fn complete(manifest: &manifest::SessionFactsManifest, per_block: &[SessionFacts]) -> bool {
    manifest.facts.iter().zip(per_block).all(|(block, found)| {
        let answered = |key: &Option<String>, slot: bool| key.is_none() || slot;
        answered(&block.title, found.title.is_some())
            && answered(&block.name, found.name.is_some())
            && answered(&block.permission_mode, found.permission_mode.is_some())
            && answered(&block.cost_usd, found.cost_usd.is_some())
            && answered(&block.lines_added, found.lines_added.is_some())
            && answered(&block.lines_removed, found.lines_removed.is_some())
    })
}

/// Fill `slot` from `key` unless it is already answered.
///
/// "Already answered wins" is what implements the recency half of the rule: the
/// walk runs backwards, so the first value seen for a block is its most recent
/// one and every earlier copy is ignored.
fn take_str(slot: &mut Option<String>, key: Option<&str>, value: &serde_json::Value) {
    if slot.is_some() {
        return;
    }
    let Some(key) = key else { return };
    if let Some(found) = value.get(key).and_then(|v| v.as_str()) {
        let found = found.trim();
        if !found.is_empty() {
            *slot = Some(found.to_string());
        }
    }
}

fn take_f64(slot: &mut Option<f64>, key: Option<&str>, value: &serde_json::Value) {
    if slot.is_some() {
        return;
    }
    let Some(key) = key else { return };
    if let Some(found) = value.get(key).and_then(serde_json::Value::as_f64) {
        if found.is_finite() {
            *slot = Some(found);
        }
    }
}

fn take_u64(slot: &mut Option<u64>, key: Option<&str>, value: &serde_json::Value) {
    if slot.is_some() {
        return;
    }
    let Some(key) = key else { return };
    if let Some(found) = value.get(key).and_then(serde_json::Value::as_u64) {
        *slot = Some(found);
    }
}

#[cfg(test)]
mod tests;
