//! Per-CLI native bridges that point each coding agent at shep's canonical
//! memory files. Installed by `shep memory init`; every bridge is idempotent and
//! either marker-delimited (markdown/exclude) or a key-preserving JSON merge, so
//! it can be re-run, updated, or removed without disturbing the user's own
//! config.
//!
//! Bridges installed (see [`install_bridges`]):
//! 1. claude-code, per-repo — `autoMemoryDirectory` → `<repo>/.shep/memory` in
//!    `<repo>/.claude/settings.json` (JSON merge, preserves other keys).
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
    })
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

    // 1. claude-code per-repo: autoMemoryDirectory JSON merge.
    let claude_settings = paths.repo_root.join(".claude").join("settings.json");
    let auto_memory_dir = super::repo_memory_dir(&paths.repo_root);
    update_json_file(&claude_settings, |content| {
        merge_json_object_keys(
            content,
            &[
                (
                    "autoMemoryDirectory",
                    Value::String(auto_memory_dir.to_string_lossy().into_owned()),
                ),
                ("autoMemoryEnabled", Value::Bool(true)),
            ],
        )
    })?;
    messages.push(format!("claude settings: {}", claude_settings.display()));

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

/// Append `entry` to the `instructions` array (creating it and a `$schema` when
/// the file is new), preserving existing instruction entries and other keys.
/// No-op when `entry` is already present.
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
    let already = array.iter().any(|item| item.as_str() == Some(entry));
    if !already {
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
