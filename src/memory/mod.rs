//! Shep's shared, hermes-style memory: bounded, agent-curated plain-markdown
//! files that coordinate Alex's concurrent claude-code / opencode sessions.
//!
//! Two canonical files:
//! - global user profile: `~/.config/shep/memory/USER.md` (cap [`USER_CAP`]);
//! - per-repo shared memory: `<repo>/.shep/memory/MEMORY.md` (cap [`REPO_CAP`]).
//!
//! The pure entry model lives in [`core`]; the per-CLI native bridges that point
//! each agent at these files live in [`bridges`]. This module holds the paths,
//! templates, and the thin file load/create/write layer.

pub(crate) mod bridges;
pub(crate) mod core;
pub(crate) mod history;
pub(crate) mod reflect;

use std::io;
use std::path::{Path, PathBuf};

pub(crate) use core::{MemoryDoc, MemoryError};

/// Character cap for the global user profile. Counts entry content only (see
/// [`core`] for the exact rule).
pub(crate) const USER_CAP: usize = 1_375;
/// Character cap for per-repo shared memory. Counts entry content only.
pub(crate) const REPO_CAP: usize = 2_200;

/// Which canonical memory file an operation targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MemoryKind {
    /// `~/.config/shep/memory/USER.md`.
    User,
    /// `<repo>/.shep/memory/MEMORY.md`.
    Repo,
}

impl MemoryKind {
    pub fn cap(self) -> usize {
        match self {
            MemoryKind::User => USER_CAP,
            MemoryKind::Repo => REPO_CAP,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            MemoryKind::User => "user",
            MemoryKind::Repo => "repo",
        }
    }

    fn template(self) -> String {
        match self {
            MemoryKind::User => user_template(),
            MemoryKind::Repo => repo_template(),
        }
    }
}

/// Absolute path to the global user-profile file. Honors `XDG_CONFIG_HOME` via
/// [`crate::config::config_dir`], so tests point it at a temp dir.
pub(crate) fn user_memory_path() -> PathBuf {
    crate::config::config_dir().join("memory").join("USER.md")
}

/// Directory holding the per-repo memory (the dir claude-code auto-memory and
/// the opencode/claude bridges point at).
pub(crate) fn repo_memory_dir(repo_root: &Path) -> PathBuf {
    repo_root.join(".shep").join("memory")
}

/// Absolute path to a repo's shared memory file.
pub(crate) fn repo_memory_path(repo_root: &Path) -> PathBuf {
    repo_memory_dir(repo_root).join("MEMORY.md")
}

/// Walk up from `start` to the enclosing git repo root (first ancestor with a
/// `.git` entry, file or directory). `None` if `start` is not inside a repo.
pub(crate) fn enclosing_git_repo(start: &Path) -> Option<PathBuf> {
    let mut current: PathBuf = if start.is_dir() {
        start.to_path_buf()
    } else {
        start.parent()?.to_path_buf()
    };
    loop {
        if current.join(".git").exists() {
            return Some(current);
        }
        if !current.pop() {
            return None;
        }
    }
}

/// Resolve the repo root for a `--repo` flag (or cwd when absent) by finding the
/// enclosing git repo. Returns a helpful error when there is none.
pub(crate) fn resolve_repo_root(explicit: Option<&Path>) -> io::Result<PathBuf> {
    let start = match explicit {
        Some(path) => path.to_path_buf(),
        None => std::env::current_dir()?,
    };
    enclosing_git_repo(&start).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!(
                "{} is not inside a git repository; pass --repo <path> to a repo",
                start.display()
            ),
        )
    })
}

/// Percent of the repo-memory cap in use for the repo enclosing `cwd`. `None`
/// outside a git repo or before any memory file exists (nothing to nudge
/// about). Cheap: one small-file read; safe to call from refresh jobs.
pub(crate) fn repo_usage_percent(cwd: &Path) -> Option<u8> {
    let root = enclosing_git_repo(cwd)?;
    let raw = std::fs::read_to_string(repo_memory_path(&root)).ok()?;
    let doc = MemoryDoc::parse(&raw);
    Some(doc.usage(REPO_CAP).percent().min(u8::MAX as u32) as u8)
}

/// Load a memory file, creating it from its template on first use. The template
/// header is written once; entry content accrues under the cap thereafter.
pub(crate) fn load_or_create(path: &Path, kind: MemoryKind) -> io::Result<MemoryDoc> {
    match std::fs::read_to_string(path) {
        Ok(raw) => Ok(MemoryDoc::parse(&raw)),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            let doc = MemoryDoc::from_template(&kind.template());
            write_doc(path, &doc)?;
            Ok(doc)
        }
        Err(err) => Err(err),
    }
}

/// Begin marker for the managed read-protocol block inside a memory header.
pub(crate) const READ_BEGIN: &str = "<!-- BEGIN shep read protocol (managed) -->";
/// End marker for the managed read-protocol block.
pub(crate) const READ_END: &str = "<!-- END shep read protocol (managed) -->";

/// Bring an existing memory file's header up to date with the current managed
/// read protocol. Returns whether the file changed.
///
/// [`load_or_create`] only writes a template when the file is absent, so a file
/// created before the read protocol existed would never learn about
/// `shep memory search` — and the header is the only text guaranteed to reach
/// every agent session. This is the upgrade path for those files.
///
/// The rewrite is confined to the header (everything before the first delimiter
/// line) and, within it, to the marked block — so entries are byte-identical
/// afterwards and hand-written header notes survive. Absent file is not an
/// error: there is simply nothing to refresh.
pub(crate) fn refresh_header(path: &Path) -> io::Result<bool> {
    let raw = match std::fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(err) => return Err(err),
    };
    let mut doc = MemoryDoc::parse(&raw);
    let refreshed = bridges::upsert_marked_block(
        doc.header(),
        READ_BEGIN,
        READ_END,
        read_protocol().trim_end(),
    );
    if refreshed == doc.header() {
        return Ok(false);
    }
    doc.set_header(refreshed);
    write_doc(path, &doc)?;
    Ok(true)
}

/// Serialize a document to disk, creating parent directories as needed.
pub(crate) fn write_doc(path: &Path, doc: &MemoryDoc) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, doc.render())
}

/// Template header for the global user profile. Ends before the first `§` line
/// (added by [`MemoryDoc::render`]); MUST NOT contain a lone `§` line itself.
fn user_template() -> String {
    format!(
        "# User profile (shep shared memory)\n\
         <!-- Managed via `shep memory` and by your agents. Shared across all shep\n\
              sessions. Who the human is: name, role, timezone, tech preferences,\n\
              communication style, hard always/never rules. -->\n\
         \n\
         {}\n\
         \n\
         {}\n\
         \n\
         Cap: {USER_CAP} characters of ENTRY CONTENT (this header does not count).\n",
        writeback_protocol(),
        marked_read_protocol()
    )
}

/// Template header for per-repo shared memory.
fn repo_template() -> String {
    format!(
        "# Project memory (shep shared memory)\n\
         <!-- Managed via `shep memory` and by your agents. Shared across all shep\n\
              sessions on this repo. Stable facts: environment, conventions,\n\
              build/test commands, tool quirks, lessons learned. -->\n\
         \n\
         {}\n\
         \n\
         {}\n\
         \n\
         Cap: {REPO_CAP} characters of ENTRY CONTENT (this header does not count).\n",
        writeback_protocol(),
        marked_read_protocol()
    )
}

/// The read protocol wrapped in its managed markers, as it appears in a header.
fn marked_read_protocol() -> String {
    format!("{READ_BEGIN}\n{}{READ_END}", read_protocol())
}

/// Compact read protocol embedded in every template header.
///
/// The write-back protocol tells an agent how to record a fact; without this an
/// agent has no way to learn that recorded facts can be looked up again.
/// `shep memory search` has existed and been tested for months while nothing in
/// any agent's context ever mentioned it. Kept to roughly the length of the
/// write-back protocol: the header is uncounted against the cap, but it is not
/// free of the reader's attention.
fn read_protocol() -> String {
    "## Read protocol\n\
     These entries are not everything that is known. `shep memory search \"<terms>\"`\n\
     also searches prior sessions' prompts and replies, which are not in your context.\n\
     - SEARCH FIRST when: about to re-derive an environment, build, or convention\n\
       fact; about to ask the human something they may have answered before;\n\
       picking up work in a part of this repo you have not seen this session.\n\
     - Returns matching entries plus history snippets with the match in [brackets].\n\
       Terms are ANDed and matched as words, not meaning — retry with fewer or\n\
       different words. A miss is cheap and expected; skipping the search is not.\n"
        .to_string()
}

/// Compact write-back protocol embedded in every template header. Adapted from
/// damon-ade's `WRITEBACK_PROTOCOL` (itself ported from the Hermes memory tool).
/// Kept short: the header is context-free (uncounted), but must stay tight so
/// the file reads well.
fn writeback_protocol() -> String {
    "## Write-back protocol\n\
     Entries are separated by a line containing only the section sign. Edit with\n\
     `shep memory add/replace/remove` (substring match). One fact per entry,\n\
     present tense, absolute dates, no secrets.\n\
     - SAVE proactively: preferences & corrections, then stable environment/\n\
       convention facts. SKIP trivia, task progress, log dumps, one-off paths.\n\
     - WHEN FULL: shep errors instead of auto-compacting. Consolidate — merge\n\
       overlapping entries and drop the stalest in one edit, then add.\n"
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn templates_contain_no_lone_delimiter_line() {
        // A lone `§` line in the header would be misread as the entries boundary.
        for template in [user_template(), repo_template()] {
            assert!(
                !template.lines().any(|line| line.trim() == core::DELIMITER),
                "template header must not contain a lone delimiter line"
            );
        }
    }

    #[test]
    fn templates_carry_the_read_protocol() {
        for template in [user_template(), repo_template()] {
            assert!(template.contains("Read protocol"));
            assert!(template.contains("shep memory search"));
            assert!(template.contains(READ_BEGIN) && template.contains(READ_END));
        }
    }

    #[test]
    fn refresh_header_adds_read_protocol_and_leaves_entries_intact() {
        let dir = crate::memory::tests::temp_dir("refresh-adds");
        let path = dir.join("MEMORY.md");
        // A file as it existed before the read protocol: write-back header only.
        let legacy = format!(
            "# Project memory (shep shared memory)\n\n{}\n\nCap: {REPO_CAP} characters.\n",
            writeback_protocol()
        );
        let mut doc = MemoryDoc::from_template(&legacy);
        doc.add("build with just check", REPO_CAP).unwrap();
        doc.add("trunk is master", REPO_CAP).unwrap();
        write_doc(&path, &doc).unwrap();
        let entries_before = doc.entries().to_vec();

        assert!(refresh_header(&path).unwrap(), "legacy header must change");

        let after = MemoryDoc::parse(&std::fs::read_to_string(&path).unwrap());
        assert!(after.header().contains("Read protocol"));
        assert!(after.header().contains("shep memory search"));
        // The whole point: upgrading the header must not touch a single entry.
        assert_eq!(after.entries(), entries_before.as_slice());
        // The pre-existing write-back protocol survives alongside the new block.
        assert!(after.header().contains("Write-back protocol"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refresh_header_is_idempotent_and_preserves_hand_written_notes() {
        let dir = crate::memory::tests::temp_dir("refresh-idempotent");
        let path = dir.join("MEMORY.md");
        let mut doc = MemoryDoc::from_template(&repo_template());
        doc.add("only entry", REPO_CAP).unwrap();
        write_doc(&path, &doc).unwrap();
        // A note the human added to the header by hand.
        let raw = std::fs::read_to_string(&path).unwrap();
        let hand_edited = raw.replace(
            "# Project memory (shep shared memory)",
            "# Project memory (shep shared memory)\n\nNOTE: ask Alex before pruning.",
        );
        std::fs::write(&path, &hand_edited).unwrap();

        // A file already carrying the current block is left alone entirely.
        assert!(!refresh_header(&path).unwrap());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), hand_edited);

        // And the marked block never duplicates.
        let after = std::fs::read_to_string(&path).unwrap();
        assert_eq!(after.matches(READ_BEGIN).count(), 1);
        assert!(after.contains("NOTE: ask Alex before pruning."));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn refresh_header_works_on_a_full_file_and_ignores_a_missing_one() {
        let dir = crate::memory::tests::temp_dir("refresh-full");
        let path = dir.join("MEMORY.md");
        // Nothing to refresh is not an error.
        assert!(!refresh_header(&path).unwrap());

        // A file at its cap must still be upgradable: the header is uncounted,
        // so refresh must never surface an Overflow.
        let legacy = format!("# Project memory\n\n{}\n", writeback_protocol());
        let mut doc = MemoryDoc::from_template(&legacy);
        doc.add(&"x".repeat(REPO_CAP), REPO_CAP).unwrap();
        write_doc(&path, &doc).unwrap();
        assert_eq!(doc.usage(REPO_CAP).used, REPO_CAP);

        assert!(refresh_header(&path).unwrap());
        let after = MemoryDoc::parse(&std::fs::read_to_string(&path).unwrap());
        assert_eq!(after.usage(REPO_CAP).used, REPO_CAP);
        assert!(after.header().contains("Read protocol"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fresh_file_parses_to_zero_entries_and_keeps_header() {
        for kind in [MemoryKind::User, MemoryKind::Repo] {
            let doc = MemoryDoc::from_template(&kind.template());
            let reparsed = MemoryDoc::parse(&doc.render());
            assert!(reparsed.entries().is_empty(), "{}", kind.label());
            assert!(doc.render().contains("Write-back protocol"));
            assert!(doc.render().contains(&kind.cap().to_string()));
        }
    }

    #[test]
    fn load_or_create_writes_template_then_reads_entries() {
        let dir = crate::memory::tests::temp_dir("load-create");
        let path = dir.join(".shep/memory/MEMORY.md");
        // First call creates from template.
        let created = load_or_create(&path, MemoryKind::Repo).unwrap();
        assert!(created.entries().is_empty());
        assert!(path.exists());

        // Mutate and persist.
        let mut doc = created;
        doc.add("build with just check", MemoryKind::Repo.cap())
            .unwrap();
        write_doc(&path, &doc).unwrap();

        // Second call reads the persisted entry, header intact.
        let reloaded = load_or_create(&path, MemoryKind::Repo).unwrap();
        assert_eq!(reloaded.entries(), &["build with just check".to_string()]);
        assert!(std::fs::read_to_string(&path)
            .unwrap()
            .contains("Write-back protocol"));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enclosing_git_repo_finds_root_from_subdir() {
        let dir = crate::memory::tests::temp_dir("git-root");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        let sub = dir.join("a").join("b");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(
            enclosing_git_repo(&sub).and_then(|p| p.canonicalize().ok()),
            dir.canonicalize().ok()
        );
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn repo_usage_percent_reads_enclosing_repo_memory() {
        let dir = temp_dir("usage-pct");
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        // No memory file yet: nothing to report.
        assert_eq!(repo_usage_percent(&dir), None);

        let path = repo_memory_path(&dir);
        let mut doc = load_or_create(&path, MemoryKind::Repo).unwrap();
        doc.add(&"x".repeat(REPO_CAP / 2), REPO_CAP).unwrap();
        write_doc(&path, &doc).unwrap();
        assert_eq!(repo_usage_percent(&dir), Some(50));
        // Works from a subdirectory too (uses the enclosing repo).
        let sub = dir.join("nested");
        std::fs::create_dir_all(&sub).unwrap();
        assert_eq!(repo_usage_percent(&sub), Some(50));
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn enclosing_git_repo_none_outside_repo() {
        let dir = crate::memory::tests::temp_dir("no-git");
        std::fs::create_dir_all(&dir).unwrap();
        assert_eq!(enclosing_git_repo(&dir), None);
        std::fs::remove_dir_all(&dir).ok();
    }

    /// Unique temp dir under the system temp root; unit tests clean up after
    /// themselves. Shared by this module and [`bridges`] tests.
    pub(crate) fn temp_dir(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("shep-memory-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }
}
