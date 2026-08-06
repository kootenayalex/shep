//! Per-CLI native bridges that point each coding agent at shep's canonical
//! memory files. Installed by `shep memory init`; every bridge is idempotent and
//! either marker-delimited (markdown/exclude) or a key-preserving JSON merge, so
//! it can be re-run, updated, or removed without disturbing the user's own
//! config.
//!
//! Bridges installed (see [`install_bridges`]):
//! 1. claude-code, per-repo — `autoMemoryDirectory` → `<repo>/.shep/memory` in
//!    `<repo>/.claude/settings.json` (JSON merge, preserves other keys), plus
//!    lifecycle hooks in the same file: `Stop` → `shep memory reflect-hook`
//!    (one forced memory-review turn) and SessionStart/UserPromptSubmit/Stop/
//!    SessionEnd → `shep memory ingest-event` (FTS5 history sidecar).
//! 2. claude-code, global — a marked block in `~/.claude/CLAUDE.md` importing
//!    `@~/.config/shep/memory/USER.md`.
//! 3. opencode, per-repo — `.shep/memory/MEMORY.md` appended to `instructions`
//!    in `<repo>/opencode.json` (project-relative; the documented form).
//! 4. opencode, global — the absolute `USER.md` path appended to `instructions`
//!    in `~/.config/opencode/opencode.json`. Absolute paths in global
//!    instructions were verified empirically against opencode 1.17.13
//!    (`opencode debug config` resolves them) rather than assumed.
//! 5. git hygiene — `.shep/` appended once to `<repo>/.git/info/exclude`.

use std::io;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

/// Begin marker for markdown marked blocks (claude `CLAUDE.md`).
pub(crate) const BLOCK_BEGIN: &str = "<!-- BEGIN shep memory (managed) -->";
/// End marker for markdown marked blocks.
pub(crate) const BLOCK_END: &str = "<!-- END shep memory (managed) -->";
/// Marker comment preceding the git-exclude entry.
pub(crate) const EXCLUDE_MARKER: &str = "# shep memory (managed)";
/// The path excluded from git in each repo.
pub(crate) const EXCLUDE_ENTRY: &str = ".shep/";

/// Fully-resolved filesystem targets for the bridges. Split out from
/// [`install_bridges`] so installation reads no process globals and is testable
/// against temp dirs without mutating `HOME`/`XDG_CONFIG_HOME`.
#[derive(Debug, Clone)]
pub(crate) struct BridgePaths {
    pub repo_root: PathBuf,
    pub repo_memory: PathBuf,
    pub user_memory: PathBuf,
    /// The `@`-import reference embedded in `~/.claude/CLAUDE.md` (tilde form
    /// when under `HOME`, else absolute).
    pub claude_user_import: String,
    pub claude_global_md: PathBuf,
    pub opencode_global_json: PathBuf,
    /// How hook commands invoke shep (absolute exe path when resolvable, so
    /// hooks work regardless of the agent pane's PATH).
    pub shep_invocation: String,
}

/// Resolve bridge targets from the environment for a live install.
pub(crate) fn resolve_bridge_paths(repo_root: &Path) -> io::Result<BridgePaths> {
    let home = home_dir()?;
    let user_memory = super::user_memory_path();
    Ok(BridgePaths {
        repo_root: repo_root.to_path_buf(),
        repo_memory: super::repo_memory_path(repo_root),
        claude_user_import: format!("@{}", tildify(&user_memory, &home)),
        user_memory,
        claude_global_md: home.join(".claude").join("CLAUDE.md"),
        opencode_global_json: opencode_config_dir(&home)
            .join("opencode")
            .join("opencode.json"),
        shep_invocation: shep_invocation(),
    })
}

/// The command prefix hooks use to run shep: the current executable's absolute
/// path (shell-quoted if it contains whitespace), falling back to plain `shep`.
fn shep_invocation() -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "shep".to_string();
    };
    let path = exe.display().to_string();
    if path.chars().any(char::is_whitespace) {
        format!("\"{path}\"")
    } else {
        path
    }
}

/// Install (or refresh) all bridges. Idempotent: re-running produces the same
/// files. Returns one human-readable message per bridge for the CLI.
pub(crate) fn install_bridges(paths: &BridgePaths) -> io::Result<Vec<String>> {
    let mut messages = Vec::new();

    // Canonical files first, so every bridge points at something real.
    super::load_or_create(&paths.user_memory, super::MemoryKind::User)?;
    super::load_or_create(&paths.repo_memory, super::MemoryKind::Repo)?;
    messages.push(format!(
        "memory files ready: {} + {}",
        paths.user_memory.display(),
        paths.repo_memory.display()
    ));

    // Files created before the read protocol existed keep their original header
    // forever (`load_or_create` only templates on absence), so init doubles as
    // the header upgrade path. Entries are untouched either way.
    let mut refreshed = Vec::new();
    for (path, label) in [(&paths.user_memory, "user"), (&paths.repo_memory, "repo")] {
        if super::refresh_header(path)? {
            refreshed.push(label);
        }
    }
    if !refreshed.is_empty() {
        messages.push(format!("read protocol added to: {}", refreshed.join(" + ")));
    }

    // 1. claude-code per-repo: autoMemoryDirectory + lifecycle hooks, one merge.
    let claude_settings = paths.repo_root.join(".claude").join("settings.json");
    let auto_memory_dir = super::repo_memory_dir(&paths.repo_root);
    update_json_file(&claude_settings, |content| {
        let merged = merge_json_object_keys(
            content,
            &[
                (
                    "autoMemoryDirectory",
                    Value::String(auto_memory_dir.to_string_lossy().into_owned()),
                ),
                ("autoMemoryEnabled", Value::Bool(true)),
            ],
        )?;
        merge_claude_hooks(&merged, &paths.shep_invocation)
    })?;
    messages.push(format!(
        "claude settings (auto-memory + hooks): {}",
        claude_settings.display()
    ));

    // 2. claude-code global: marked @import block in ~/.claude/CLAUDE.md.
    update_text_file(&paths.claude_global_md, |content| {
        upsert_marked_block(content, BLOCK_BEGIN, BLOCK_END, &paths.claude_user_import)
    })?;
    messages.push(format!(
        "claude CLAUDE.md: {}",
        paths.claude_global_md.display()
    ));

    // 3. opencode per-repo: project-relative instructions entry.
    let opencode_repo = paths.repo_root.join("opencode.json");
    update_json_file(&opencode_repo, |content| {
        merge_opencode_instructions(content, ".shep/memory/MEMORY.md")
    })?;
    messages.push(format!("opencode config: {}", opencode_repo.display()));

    // 4. opencode global: absolute USER.md instructions entry.
    let user_abs = paths.user_memory.to_string_lossy();
    update_json_file(&paths.opencode_global_json, |content| {
        merge_opencode_instructions(content, &user_abs)
    })?;
    messages.push(format!(
        "opencode global: {}",
        paths.opencode_global_json.display()
    ));

    // 5. git hygiene: exclude .shep/ (skip silently if not a git dir).
    let git_dir = paths.repo_root.join(".git");
    if git_dir.is_dir() {
        let exclude = git_dir.join("info").join("exclude");
        update_text_file(&exclude, |content| {
            append_excluded_once(content, EXCLUDE_MARKER, EXCLUDE_ENTRY)
        })?;
        messages.push(format!("git exclude: {}", exclude.display()));
    }

    Ok(messages)
}

/// Read-modify-write a text file through a pure transform, creating parents.
fn update_text_file(path: &Path, transform: impl FnOnce(&str) -> String) -> io::Result<()> {
    let content = read_to_string_or_empty(path)?;
    let updated = transform(&content);
    write_with_parents(path, &updated)
}

/// Read-modify-write a JSON file through a fallible pure transform.
fn update_json_file(
    path: &Path,
    transform: impl FnOnce(&str) -> io::Result<String>,
) -> io::Result<()> {
    let content = read_to_string_or_empty(path)?;
    let updated = transform(&content)?;
    write_with_parents(path, &updated)
}

fn read_to_string_or_empty(path: &Path) -> io::Result<String> {
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(content),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(String::new()),
        Err(err) => Err(err),
    }
}

fn write_with_parents(path: &Path, content: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content)
}

/// Insert or replace a marked block delimited by `begin`/`end`. If a block
/// already exists it is replaced in place (idempotent update); otherwise the
/// block is appended. `body` is the single reference line inside the block.
pub(crate) fn upsert_marked_block(content: &str, begin: &str, end: &str, body: &str) -> String {
    let block = format!("{begin}\n{body}\n{end}");
    if let (Some(start), Some(end_idx)) = (content.find(begin), content.find(end)) {
        if end_idx >= start {
            let end_of_block = end_idx + end.len();
            let mut result = String::with_capacity(content.len());
            result.push_str(&content[..start]);
            result.push_str(&block);
            result.push_str(&content[end_of_block..]);
            return result;
        }
    }
    let mut result = content.trim_end_matches('\n').to_string();
    if !result.is_empty() {
        result.push_str("\n\n");
    }
    result.push_str(&block);
    result.push('\n');
    result
}

/// Merge scalar keys into a top-level JSON object, preserving every other key.
/// Empty/whitespace content starts from `{}`. Errors if the existing content is
/// not a JSON object.
pub(crate) fn merge_json_object_keys(content: &str, keys: &[(&str, Value)]) -> io::Result<String> {
    let mut root = parse_json_object(content)?;
    for (key, value) in keys {
        root.insert((*key).to_string(), value.clone());
    }
    Ok(serialize_json_object(&root))
}

/// Merge shep's lifecycle-hook commands into a claude `settings.json`:
/// `Stop` gets `<shep> memory reflect-hook` + `<shep> memory ingest-event`;
/// SessionStart/UserPromptSubmit/SessionEnd get ingest-event. Existing user
/// hooks (any matcher group whose commands are not ours) are preserved
/// untouched, and re-running is a no-op.
///
/// Ours are recognized by shep *subcommand*, not by the full command string:
/// the string embeds whichever binary ran `memory init` (see
/// [`shep_invocation`]), so exact-string matching made a hook installed by a
/// second build look foreign and appended a duplicate beside it. Init is
/// therefore self-healing — a stale hook is retargeted in place and extra
/// copies are dropped.
pub(crate) fn merge_claude_hooks(content: &str, shep_bin: &str) -> io::Result<String> {
    let mut root = parse_json_object(content)?;
    let hooks = root
        .entry("hooks".to_string())
        .or_insert_with(|| Value::Object(Map::new()));
    let hooks_map = hooks
        .as_object_mut()
        .ok_or_else(|| io::Error::other("claude `hooks` must be a JSON object"))?;
    for (event, kinds) in HOOK_EVENTS {
        let groups = hooks_map
            .entry(event.to_string())
            .or_insert_with(|| Value::Array(Vec::new()))
            .as_array_mut()
            .ok_or_else(|| {
                io::Error::other(format!("claude hooks.{event} must be a JSON array"))
            })?;
        for kind in kinds {
            let command = format!("{shep_bin} {kind}");
            if !collapse_shep_hooks(groups, kind, &command) {
                groups.push(serde_json::json!({
                    "hooks": [{"type": "command", "command": command}]
                }));
            }
        }
    }
    Ok(serialize_json_object(&root))
}

/// The shep subcommand a hook command invokes, ignoring the binary path that
/// runs it (`/abs/path/shep memory ingest-event` and a bare
/// `shep memory ingest-event` are the same hook). `None` for anything that is
/// not one of ours.
fn shep_hook_kind(command: &str) -> Option<&'static str> {
    ["memory ingest-event", "memory reflect-hook"]
        .into_iter()
        .find(|kind| {
            command
                .strip_suffix(kind)
                .is_some_and(|prefix| prefix.is_empty() || prefix.ends_with(' '))
        })
}

/// Point shep's `kind` hook at `command`, keeping the first occurrence and
/// deleting extras left behind by installs from other binary paths. Returns
/// whether one was found, so the caller knows whether to append a fresh group.
/// Only the command string is rewritten — a group holding a user hook beside
/// ours keeps the user hook and its position.
fn collapse_shep_hooks(groups: &mut Vec<Value>, kind: &str, command: &str) -> bool {
    let mut found = false;
    for group in groups.iter_mut() {
        let Some(hooks) = group.get_mut("hooks").and_then(Value::as_array_mut) else {
            continue;
        };
        hooks.retain_mut(|hook| {
            let Some(existing) = hook.get("command").and_then(Value::as_str) else {
                return true;
            };
            if shep_hook_kind(existing) != Some(kind) {
                return true;
            }
            if found {
                return false;
            }
            found = true;
            if existing != command {
                hook["command"] = Value::String(command.to_string());
            }
            true
        });
    }
    // Drop only groups we emptied; groups still carrying user hooks stay put.
    groups.retain(|group| {
        group
            .get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|hooks| !hooks.is_empty())
    });
    found
}

/// The claude events shep installs hooks on, and which subcommands each gets.
const HOOK_EVENTS: [(&str, &[&str]); 4] = [
    ("SessionStart", &["memory ingest-event"]),
    ("UserPromptSubmit", &["memory ingest-event"]),
    ("Stop", &["memory ingest-event", "memory reflect-hook"]),
    ("SessionEnd", &["memory ingest-event"]),
];

/// A problem with the installed claude hooks, as found by [`audit_claude_hooks`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum HookIssue {
    /// Shep installs this hook, but it is not in the settings file.
    Missing { event: String, kind: String },
    /// The hook's binary path no longer exists. Claude runs the command,
    /// the exec fails, and nothing is reported — the hook is silently dead.
    Unreachable { event: String, command: String },
    /// More than one hook for the same subcommand, so it runs more than once
    /// per event (each copy writing its own row).
    Duplicate {
        event: String,
        kind: String,
        count: usize,
    },
}

impl std::fmt::Display for HookIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing { event, kind } => write!(f, "{event}: no `{kind}` hook installed"),
            Self::Unreachable { event, command } => {
                write!(
                    f,
                    "{event}: hook binary is gone, so it silently does nothing: {command}"
                )
            }
            Self::Duplicate { event, kind, count } => {
                write!(
                    f,
                    "{event}: `{kind}` installed {count}x, so it runs {count} times per event"
                )
            }
        }
    }
}

/// Check the claude hooks in a `settings.json` body. `binary_exists` resolves
/// whether a hook's binary is still there (injected so this stays a pure
/// function over the JSON).
///
/// This exists because a hook whose binary path went stale — a rebuilt,
/// moved, or cleaned target directory — is indistinguishable from a working
/// one from the outside: claude reports nothing and the session proceeds
/// normally. That is how the history sidecar came to sit dead for three weeks.
pub(crate) fn audit_claude_hooks(
    content: &str,
    binary_exists: impl Fn(&Path) -> bool,
) -> Vec<HookIssue> {
    let mut issues = Vec::new();
    let Ok(root) = parse_json_object(content) else {
        return issues;
    };
    let hooks = root.get("hooks").and_then(Value::as_object);
    for (event, kinds) in HOOK_EVENTS {
        let groups = hooks
            .and_then(|hooks| hooks.get(event))
            .and_then(Value::as_array);
        let commands: Vec<&str> = groups
            .into_iter()
            .flatten()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter_map(|hook| hook.get("command").and_then(Value::as_str))
            .collect();
        for kind in kinds {
            let ours: Vec<&&str> = commands
                .iter()
                .filter(|command| shep_hook_kind(command) == Some(*kind))
                .collect();
            match ours.len() {
                0 => issues.push(HookIssue::Missing {
                    event: event.to_string(),
                    kind: (*kind).to_string(),
                }),
                1 => {}
                count => issues.push(HookIssue::Duplicate {
                    event: event.to_string(),
                    kind: (*kind).to_string(),
                    count,
                }),
            }
            for command in ours {
                if let Some(binary) = hook_binary(command) {
                    if !binary_exists(Path::new(binary)) {
                        issues.push(HookIssue::Unreachable {
                            event: event.to_string(),
                            command: (*command).to_string(),
                        });
                    }
                }
            }
        }
    }
    issues
}

/// The binary path a hook command runs, when it is an absolute path we can
/// check. `None` for a bare `shep …`, which resolves through `PATH` and cannot
/// be verified from here.
fn hook_binary(command: &str) -> Option<&str> {
    let (binary, _) = command.split_once(' ')?;
    let binary = binary.trim_matches('"');
    binary.starts_with('/').then_some(binary)
}

/// Which canonical memory file an opencode `instructions` entry points at, for
/// any shep config dir (`shep`, `shep-dev`, a repo's `.shep`). Same rationale
/// as [`shep_hook_kind`]: the absolute path varies by build, so matching the
/// exact string appended a second entry instead of correcting the first.
fn shep_instruction_kind(entry: &str) -> Option<&'static str> {
    if !entry.contains("shep") {
        return None;
    }
    ["memory/USER.md", "memory/MEMORY.md"]
        .into_iter()
        .find(|suffix| entry.ends_with(suffix))
}

/// Append `entry` to the `instructions` array (creating it and a `$schema` when
/// the file is new), preserving existing instruction entries and other keys.
/// No-op when `entry` is already present; an entry pointing at the same shep
/// memory file under a different config dir is corrected in place, keeping its
/// position, rather than joined by a second one.
pub(crate) fn merge_opencode_instructions(content: &str, entry: &str) -> io::Result<String> {
    let mut root = parse_json_object(content)?;
    if !root.contains_key("$schema") {
        root.insert(
            "$schema".to_string(),
            Value::String("https://opencode.ai/config.json".to_string()),
        );
    }
    let instructions = root
        .entry("instructions".to_string())
        .or_insert_with(|| Value::Array(Vec::new()));
    let array = instructions.as_array_mut().ok_or_else(|| {
        io::Error::other("opencode `instructions` must be a JSON array".to_string())
    })?;
    let kind = shep_instruction_kind(entry);
    let mut found = false;
    array.retain_mut(|item| {
        let Some(existing) = item.as_str() else {
            return true;
        };
        let ours = match kind {
            Some(kind) => shep_instruction_kind(existing) == Some(kind),
            None => existing == entry,
        };
        if !ours {
            return true;
        }
        if found {
            return false;
        }
        found = true;
        *item = Value::String(entry.to_string());
        true
    });
    if !found {
        array.push(Value::String(entry.to_string()));
    }
    Ok(serialize_json_object(&root))
}

fn parse_json_object(content: &str) -> io::Result<Map<String, Value>> {
    if content.trim().is_empty() {
        return Ok(Map::new());
    }
    let value: Value = serde_json::from_str(content)
        .map_err(|err| io::Error::other(format!("invalid JSON: {err}")))?;
    match value {
        Value::Object(map) => Ok(map),
        _ => Err(io::Error::other("expected a top-level JSON object")),
    }
}

fn serialize_json_object(map: &Map<String, Value>) -> String {
    let mut out = serde_json::to_string_pretty(&Value::Object(map.clone()))
        .unwrap_or_else(|_| "{}".to_string());
    out.push('\n');
    out
}

/// Append the exclude `entry` (preceded by `marker`) once. No-op if a line
/// equal to `entry` is already present.
pub(crate) fn append_excluded_once(content: &str, marker: &str, entry: &str) -> String {
    if content.lines().any(|line| line.trim() == entry) {
        return content.to_string();
    }
    let mut result = content.to_string();
    if !result.is_empty() && !result.ends_with('\n') {
        result.push('\n');
    }
    result.push_str(marker);
    result.push('\n');
    result.push_str(entry);
    result.push('\n');
    result
}

fn home_dir() -> io::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|home| !home.as_os_str().is_empty())
        .ok_or_else(|| io::Error::other("HOME is not set"))
}

/// opencode reads config from `$XDG_CONFIG_HOME` or `~/.config`.
fn opencode_config_dir(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|dir| !dir.as_os_str().is_empty())
        .unwrap_or_else(|| home.join(".config"))
}

/// Render `path` as `~/rest` when under `home`, else its absolute form.
fn tildify(path: &Path, home: &Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tests::temp_dir;

    #[test]
    fn upsert_marked_block_appends_when_absent() {
        let out = upsert_marked_block("# CLAUDE\n\nexisting\n", BLOCK_BEGIN, BLOCK_END, "@x");
        assert!(out.contains("existing"));
        assert!(out.contains(BLOCK_BEGIN));
        assert!(out.contains("@x"));
        assert!(out.contains(BLOCK_END));
    }

    #[test]
    fn upsert_marked_block_replaces_existing_block_idempotently() {
        let first = upsert_marked_block("keep\n", BLOCK_BEGIN, BLOCK_END, "@old");
        let second = upsert_marked_block(&first, BLOCK_BEGIN, BLOCK_END, "@new");
        // Body updated, no duplicate block, unrelated content preserved.
        assert_eq!(second.matches(BLOCK_BEGIN).count(), 1);
        assert!(second.contains("@new"));
        assert!(!second.contains("@old"));
        assert!(second.contains("keep"));
        // Re-running with the same body is a fixed point.
        let third = upsert_marked_block(&second, BLOCK_BEGIN, BLOCK_END, "@new");
        assert_eq!(third, second);
    }

    #[test]
    fn merge_json_object_keys_preserves_unrelated_keys() {
        let content = r#"{"model":"opus","hooks":{"Stop":[]}}"#;
        let out = merge_json_object_keys(
            content,
            &[
                ("autoMemoryDirectory", Value::String("/m".to_string())),
                ("autoMemoryEnabled", Value::Bool(true)),
            ],
        )
        .unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["model"], "opus");
        assert_eq!(value["hooks"]["Stop"], serde_json::json!([]));
        assert_eq!(value["autoMemoryDirectory"], "/m");
        assert_eq!(value["autoMemoryEnabled"], true);
    }

    #[test]
    fn merge_json_object_keys_starts_from_empty() {
        let out = merge_json_object_keys("", &[("autoMemoryEnabled", Value::Bool(true))]).unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["autoMemoryEnabled"], true);
    }

    #[test]
    fn merge_json_object_keys_rejects_non_object() {
        assert!(merge_json_object_keys("[1,2]", &[]).is_err());
    }

    /// Whether any matcher group carries `command` verbatim.
    fn hook_command_present(groups: &[Value], command: &str) -> bool {
        groups.iter().any(|group| {
            group
                .get("hooks")
                .and_then(Value::as_array)
                .is_some_and(|hooks| {
                    hooks
                        .iter()
                        .any(|hook| hook.get("command").and_then(Value::as_str) == Some(command))
                })
        })
    }

    /// How many hooks across all groups invoke shep's `kind` subcommand.
    fn shep_hook_count(groups: &[Value], kind: &str) -> usize {
        groups
            .iter()
            .filter_map(|group| group.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter(|hook| {
                hook.get("command")
                    .and_then(Value::as_str)
                    .and_then(shep_hook_kind)
                    == Some(kind)
            })
            .count()
    }

    #[test]
    fn merge_claude_hooks_installs_all_events_from_empty() {
        let out = merge_claude_hooks("", "shep").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        for event in ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"] {
            assert!(
                hook_command_present(
                    value["hooks"][event].as_array().unwrap(),
                    "shep memory ingest-event"
                ),
                "{event} missing ingest hook"
            );
        }
        assert!(hook_command_present(
            value["hooks"]["Stop"].as_array().unwrap(),
            "shep memory reflect-hook"
        ));
        // Correct claude hooks shape: matcher group wrapping a command entry.
        assert_eq!(
            value["hooks"]["SessionEnd"][0]["hooks"][0]["type"],
            "command"
        );
    }

    #[test]
    fn merge_claude_hooks_is_idempotent_and_preserves_user_hooks() {
        let existing = r#"{"hooks":{"Stop":[{"hooks":[{"type":"command","command":"my-bell.sh"}]}]},"model":"opus"}"#;
        let once = merge_claude_hooks(existing, "shep").unwrap();
        let twice = merge_claude_hooks(&once, "shep").unwrap();
        assert_eq!(once, twice, "second merge must be a no-op");
        let value: Value = serde_json::from_str(&twice).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        // User's hook first, ours appended, nothing duplicated.
        assert!(hook_command_present(stop, "my-bell.sh"));
        assert!(hook_command_present(stop, "shep memory reflect-hook"));
        assert!(hook_command_present(stop, "shep memory ingest-event"));
        assert_eq!(stop.len(), 3);
        assert_eq!(value["model"], "opus");
    }

    #[test]
    fn merge_claude_hooks_retargets_a_hook_installed_by_another_binary() {
        // init from one binary, then another: the second must correct the
        // first's command rather than install a rival set beside it.
        let once = merge_claude_hooks("", "/old/path/shep").unwrap();
        let twice = merge_claude_hooks(&once, "/new/path/shep").unwrap();
        let value: Value = serde_json::from_str(&twice).unwrap();
        for event in ["SessionStart", "UserPromptSubmit", "Stop", "SessionEnd"] {
            let groups = value["hooks"][event].as_array().unwrap();
            assert_eq!(
                shep_hook_count(groups, "memory ingest-event"),
                1,
                "{event} should carry exactly one ingest hook"
            );
            assert!(hook_command_present(
                groups,
                "/new/path/shep memory ingest-event"
            ));
            assert!(!hook_command_present(
                groups,
                "/old/path/shep memory ingest-event"
            ));
        }
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(shep_hook_count(stop, "memory reflect-hook"), 1);
        assert!(hook_command_present(
            stop,
            "/new/path/shep memory reflect-hook"
        ));
    }

    #[test]
    fn merge_claude_hooks_collapses_hooks_from_three_binaries() {
        // The live `.claude/settings.json` shape this fix exists for: shep
        // installed from the installed binary, target/debug, and target/release.
        let mut content = String::from("{}");
        for path in ["/a/shep", "/b/shep", "/c/shep"] {
            content = merge_claude_hooks_appending(&content, path);
        }
        let before: Value = serde_json::from_str(&content).unwrap();
        assert_eq!(
            before["hooks"]["Stop"].as_array().unwrap().len(),
            6,
            "fixture should reproduce the triple-installed state"
        );

        let healed = merge_claude_hooks(&content, "/c/shep").unwrap();
        let value: Value = serde_json::from_str(&healed).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        assert_eq!(shep_hook_count(stop, "memory ingest-event"), 1);
        assert_eq!(shep_hook_count(stop, "memory reflect-hook"), 1);
        assert_eq!(stop.len(), 2, "emptied groups must be dropped");
        // And it stays collapsed.
        assert_eq!(merge_claude_hooks(&healed, "/c/shep").unwrap(), healed);
    }

    /// The pre-fix merge: append unconditionally, keyed on the exact command.
    /// Builds the duplicated fixture the fix has to clean up.
    fn merge_claude_hooks_appending(content: &str, shep_bin: &str) -> String {
        let mut root: Map<String, Value> = serde_json::from_str(content).unwrap();
        let hooks = root
            .entry("hooks".to_string())
            .or_insert_with(|| Value::Object(Map::new()));
        let hooks_map = hooks.as_object_mut().unwrap();
        for (event, commands) in [
            ("SessionStart", vec!["memory ingest-event"]),
            ("UserPromptSubmit", vec!["memory ingest-event"]),
            ("Stop", vec!["memory ingest-event", "memory reflect-hook"]),
            ("SessionEnd", vec!["memory ingest-event"]),
        ] {
            let groups = hooks_map
                .entry(event.to_string())
                .or_insert_with(|| Value::Array(Vec::new()))
                .as_array_mut()
                .unwrap();
            for command in commands {
                groups.push(serde_json::json!({
                    "hooks": [{"type": "command", "command": format!("{shep_bin} {command}")}]
                }));
            }
        }
        serialize_json_object(&root)
    }

    #[test]
    fn merge_claude_hooks_retarget_preserves_a_user_hook_sharing_the_group() {
        // A user hook sitting in the same group as ours must survive the
        // rewrite with its command and position intact.
        let existing = r#"{"hooks":{"Stop":[{"matcher":"*","hooks":[{"type":"command","command":"my-bell.sh"},{"type":"command","command":"/old/shep memory ingest-event"}]}]}}"#;
        let out = merge_claude_hooks(existing, "/new/shep").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        let stop = value["hooks"]["Stop"].as_array().unwrap();
        let group = &stop[0]["hooks"];
        assert_eq!(group[0]["command"], "my-bell.sh");
        assert_eq!(group[1]["command"], "/new/shep memory ingest-event");
        assert_eq!(stop[0]["matcher"], "*", "matcher must be preserved");
        assert_eq!(shep_hook_count(stop, "memory ingest-event"), 1);
    }

    #[test]
    fn shep_hook_kind_matches_any_binary_path_but_not_foreign_commands() {
        assert_eq!(
            shep_hook_kind("/usr/local/bin/shep memory ingest-event"),
            Some("memory ingest-event")
        );
        assert_eq!(
            shep_hook_kind("shep memory reflect-hook"),
            Some("memory reflect-hook")
        );
        assert_eq!(shep_hook_kind("my-bell.sh"), None);
        // A command merely ending in the same words without a separator.
        assert_eq!(shep_hook_kind("notmemory ingest-event"), None);
    }

    #[test]
    fn audit_reports_a_healthy_install_as_clean() {
        let content = merge_claude_hooks("", "/bin/shep").unwrap();
        assert_eq!(audit_claude_hooks(&content, |_| true), Vec::new());
    }

    #[test]
    fn audit_flags_a_hook_whose_binary_is_gone() {
        // The silent-death case: claude runs the command, exec fails, nothing
        // is reported, and the sidecar quietly stops being fed.
        let content = merge_claude_hooks("", "/gone/shep").unwrap();
        let issues = audit_claude_hooks(&content, |path| path != Path::new("/gone/shep"));
        assert_eq!(issues.len(), 5, "one per installed hook, got {issues:?}");
        assert!(issues.iter().all(|issue| matches!(
            issue,
            HookIssue::Unreachable { command, .. } if command.starts_with("/gone/shep")
        )));
    }

    #[test]
    fn audit_flags_duplicates_and_missing_hooks() {
        let duplicated =
            merge_claude_hooks_appending(&merge_claude_hooks_appending("{}", "/a/shep"), "/b/shep");
        let issues = audit_claude_hooks(&duplicated, |_| true);
        assert!(issues.iter().any(|issue| matches!(
            issue,
            HookIssue::Duplicate { event, kind, count }
                if event == "Stop" && kind == "memory ingest-event" && *count == 2
        )));

        let empty = audit_claude_hooks("{}", |_| true);
        assert_eq!(empty.len(), 5, "every hook missing, got {empty:?}");
        assert!(empty
            .iter()
            .all(|issue| matches!(issue, HookIssue::Missing { .. })));
    }

    #[test]
    fn audit_ignores_a_path_less_hook_it_cannot_verify() {
        // A bare `shep …` resolves through PATH; we must not claim it is dead.
        let content = merge_claude_hooks("", "shep").unwrap();
        assert_eq!(audit_claude_hooks(&content, |_| false), Vec::new());
    }

    #[test]
    fn merge_opencode_instructions_corrects_a_stale_shep_config_dir() {
        // The shep-dev leak: a debug build appended its own path beside the
        // real one. Re-running from the release build must fix, not duplicate.
        let content = r#"{"instructions":["AGENTS.md","/home/a/.config/shep-dev/memory/USER.md","OTHER.md"]}"#;
        let out =
            merge_opencode_instructions(content, "/home/a/.config/shep/memory/USER.md").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        let instructions = value["instructions"].as_array().unwrap();
        assert_eq!(instructions.len(), 3, "corrected in place, not appended");
        assert_eq!(instructions[0], "AGENTS.md");
        assert_eq!(instructions[1], "/home/a/.config/shep/memory/USER.md");
        assert_eq!(instructions[2], "OTHER.md", "order must be preserved");
    }

    #[test]
    fn merge_opencode_instructions_collapses_duplicate_shep_entries() {
        let content = r#"{"instructions":["/a/shep/memory/USER.md","/b/shep-dev/memory/USER.md"]}"#;
        let out = merge_opencode_instructions(content, "/c/shep/memory/USER.md").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        let instructions = value["instructions"].as_array().unwrap();
        assert_eq!(instructions.len(), 1);
        assert_eq!(instructions[0], "/c/shep/memory/USER.md");
    }

    #[test]
    fn merge_opencode_instructions_leaves_a_repo_entry_alone_when_adding_the_user_one() {
        // The two shep entries are different kinds; neither may displace the
        // other, since a global config can legitimately carry both.
        let content = r#"{"instructions":[".shep/memory/MEMORY.md"]}"#;
        let out = merge_opencode_instructions(content, "/abs/shep/memory/USER.md").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        let instructions = value["instructions"].as_array().unwrap();
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0], ".shep/memory/MEMORY.md");
        assert_eq!(instructions[1], "/abs/shep/memory/USER.md");
    }

    #[test]
    fn merge_claude_hooks_rejects_malformed_hooks() {
        assert!(merge_claude_hooks(r#"{"hooks":[]}"#, "shep").is_err());
        assert!(merge_claude_hooks(r#"{"hooks":{"Stop":{}}}"#, "shep").is_err());
    }

    #[test]
    fn merge_opencode_instructions_appends_once_and_preserves() {
        let content = r#"{"instructions":["AGENTS.md"],"theme":"dark"}"#;
        let once = merge_opencode_instructions(content, ".shep/memory/MEMORY.md").unwrap();
        let twice = merge_opencode_instructions(&once, ".shep/memory/MEMORY.md").unwrap();
        assert_eq!(once, twice, "second merge must be a no-op");
        let value: Value = serde_json::from_str(&twice).unwrap();
        let instructions = value["instructions"].as_array().unwrap();
        assert_eq!(instructions.len(), 2);
        assert_eq!(instructions[0], "AGENTS.md");
        assert_eq!(instructions[1], ".shep/memory/MEMORY.md");
        assert_eq!(value["theme"], "dark");
    }

    #[test]
    fn merge_opencode_instructions_creates_schema_and_array_when_new() {
        let out = merge_opencode_instructions("", "/abs/USER.md").unwrap();
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["$schema"], "https://opencode.ai/config.json");
        assert_eq!(value["instructions"][0], "/abs/USER.md");
    }

    #[test]
    fn append_excluded_once_is_idempotent() {
        let first = append_excluded_once("target/\n", EXCLUDE_MARKER, EXCLUDE_ENTRY);
        let second = append_excluded_once(&first, EXCLUDE_MARKER, EXCLUDE_ENTRY);
        assert_eq!(first, second);
        assert_eq!(first.matches(EXCLUDE_ENTRY).count(), 1);
        assert!(first.contains("target/"));
    }

    #[test]
    fn tildify_prefers_home_relative() {
        let home = Path::new("/home/alex");
        assert_eq!(
            tildify(Path::new("/home/alex/.config/shep/memory/USER.md"), home),
            "~/.config/shep/memory/USER.md"
        );
        assert_eq!(tildify(Path::new("/etc/x"), home), "/etc/x");
    }

    /// End-to-end idempotency: install twice against temp targets, assert stable
    /// files, preserved unrelated keys, and no touching of real `HOME`.
    #[test]
    fn install_bridges_is_idempotent_and_preserves_unrelated_config() {
        let base = temp_dir("install");
        let repo = base.join("repo");
        std::fs::create_dir_all(repo.join(".git").join("info")).unwrap();
        // Pre-existing config that must survive the merges.
        std::fs::create_dir_all(repo.join(".claude")).unwrap();
        std::fs::write(
            repo.join(".claude").join("settings.json"),
            r#"{"model":"opus"}"#,
        )
        .unwrap();
        std::fs::write(
            repo.join("opencode.json"),
            r#"{"instructions":["AGENTS.md"]}"#,
        )
        .unwrap();
        std::fs::write(repo.join(".git").join("info").join("exclude"), "target/\n").unwrap();

        let paths = BridgePaths {
            repo_root: repo.clone(),
            repo_memory: crate::memory::repo_memory_path(&repo),
            user_memory: base.join("config/shep/memory/USER.md"),
            claude_user_import: "@~/.config/shep/memory/USER.md".to_string(),
            claude_global_md: base.join("home/.claude/CLAUDE.md"),
            opencode_global_json: base.join("home/.config/opencode/opencode.json"),
            shep_invocation: "shep".to_string(),
        };

        let first = install_bridges(&paths).unwrap();
        assert!(!first.is_empty());
        let before = snapshot(&paths);
        // Second run must not change anything.
        install_bridges(&paths).unwrap();
        assert_eq!(before, snapshot(&paths), "install must be idempotent");

        // claude settings merged, model preserved.
        let claude: Value = serde_json::from_str(
            &std::fs::read_to_string(repo.join(".claude/settings.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(claude["model"], "opus");
        assert_eq!(
            claude["autoMemoryDirectory"],
            crate::memory::repo_memory_dir(&repo)
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(claude["autoMemoryEnabled"], true);
        // Lifecycle hooks installed alongside auto-memory.
        assert!(hook_command_present(
            claude["hooks"]["Stop"].as_array().unwrap(),
            "shep memory reflect-hook"
        ));
        assert!(hook_command_present(
            claude["hooks"]["UserPromptSubmit"].as_array().unwrap(),
            "shep memory ingest-event"
        ));

        // opencode per-repo instructions preserved + appended once.
        let opencode: Value =
            serde_json::from_str(&std::fs::read_to_string(repo.join("opencode.json")).unwrap())
                .unwrap();
        let instructions = opencode["instructions"].as_array().unwrap();
        assert_eq!(instructions[0], "AGENTS.md");
        assert!(instructions.contains(&Value::String(".shep/memory/MEMORY.md".to_string())));

        // opencode global got the absolute USER.md path.
        let opencode_global: Value =
            serde_json::from_str(&std::fs::read_to_string(&paths.opencode_global_json).unwrap())
                .unwrap();
        assert!(opencode_global["instructions"]
            .as_array()
            .unwrap()
            .contains(&Value::String(
                paths.user_memory.to_string_lossy().into_owned()
            )));

        // claude global marked block, single copy.
        let claude_md = std::fs::read_to_string(&paths.claude_global_md).unwrap();
        assert_eq!(claude_md.matches(BLOCK_BEGIN).count(), 1);
        assert!(claude_md.contains("@~/.config/shep/memory/USER.md"));

        // git exclude appended once, prior entry kept.
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(exclude.contains("target/"));
        assert_eq!(exclude.matches(EXCLUDE_ENTRY).count(), 1);

        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn install_bridges_skips_git_exclude_outside_repo() {
        let base = temp_dir("no-git-install");
        let repo = base.join("plain");
        std::fs::create_dir_all(&repo).unwrap();
        let paths = BridgePaths {
            repo_root: repo.clone(),
            repo_memory: crate::memory::repo_memory_path(&repo),
            user_memory: base.join("config/shep/memory/USER.md"),
            claude_user_import: "@/x/USER.md".to_string(),
            claude_global_md: base.join("home/.claude/CLAUDE.md"),
            opencode_global_json: base.join("home/.config/opencode/opencode.json"),
            shep_invocation: "shep".to_string(),
        };
        let messages = install_bridges(&paths).unwrap();
        assert!(!messages.iter().any(|m| m.contains("git exclude")));
        assert!(!repo.join(".git").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    fn snapshot(paths: &BridgePaths) -> Vec<(PathBuf, String)> {
        let files = [
            paths.repo_root.join(".claude/settings.json"),
            paths.repo_root.join("opencode.json"),
            paths.repo_root.join(".git/info/exclude"),
            paths.claude_global_md.clone(),
            paths.opencode_global_json.clone(),
            paths.user_memory.clone(),
            paths.repo_memory.clone(),
        ];
        files
            .into_iter()
            .filter_map(|path| {
                std::fs::read_to_string(&path)
                    .ok()
                    .map(|content| (path, content))
            })
            .collect()
    }
}
