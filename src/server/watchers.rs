//! Filesystem watchers (M4): `~/.config/shep/watchers.toml` maps directories
//! to prompt templates; a file created in a watched dir enqueues a task
//! (`{file}` substituted with the new file's absolute path). Ported from
//! damon-ade's `scheduler/watcher.ts`.
//!
//! Watchers only ENQUEUE — dispatch stays with `shep task dispatch` or the
//! `[tasks] auto_dispatch` hook, so a watched drop never spawns panes behind
//! the user's back. All failures are log-and-continue: a broken watchers file
//! or unwatchable dir must never take the server down.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use notify::Watcher;

use crate::tasks::TaskRuntime;

/// One `[[watchers]]` entry, resolved and validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct WatcherEntry {
    pub dir: PathBuf,
    /// Prompt template; `{file}` is replaced with the created file's path.
    pub prompt: String,
    pub runtime: TaskRuntime,
    /// Repo the task targets; defaults to the watched dir's enclosing git
    /// repo, falling back to the watched dir itself.
    pub repo: PathBuf,
    pub use_worktree: bool,
}

/// Default config location: `<config dir>/watchers.toml`.
pub(crate) fn watchers_config_path() -> PathBuf {
    crate::config::config_dir().join("watchers.toml")
}

/// Parse a watchers config. Invalid entries are skipped with a diagnostic in
/// the returned list; a missing file is simply zero watchers.
pub(crate) fn parse_watchers_config(raw: &str) -> (Vec<WatcherEntry>, Vec<String>) {
    let mut entries = Vec::new();
    let mut diagnostics = Vec::new();
    let value: toml::Value = match raw.parse() {
        Ok(value) => value,
        Err(err) => {
            diagnostics.push(format!("watchers.toml: {err}"));
            return (entries, diagnostics);
        }
    };
    let Some(list) = value.get("watchers").and_then(|w| w.as_array()) else {
        return (entries, diagnostics);
    };
    for (index, item) in list.iter().enumerate() {
        let Some(dir) = item.get("dir").and_then(|v| v.as_str()) else {
            diagnostics.push(format!("watchers[{index}]: missing `dir`"));
            continue;
        };
        let Some(prompt) = item.get("prompt").and_then(|v| v.as_str()) else {
            diagnostics.push(format!("watchers[{index}]: missing `prompt`"));
            continue;
        };
        let runtime = match item.get("runtime").and_then(|v| v.as_str()) {
            None => TaskRuntime::Claude,
            Some(raw) => match TaskRuntime::parse(raw) {
                Some(runtime) => runtime,
                None => {
                    diagnostics.push(format!("watchers[{index}]: unknown runtime `{raw}`"));
                    continue;
                }
            },
        };
        let dir = crate::worktree::expand_tilde_path(dir);
        let repo = match item.get("repo").and_then(|v| v.as_str()) {
            Some(repo) => crate::worktree::expand_tilde_path(repo),
            None => crate::memory::enclosing_git_repo(&dir).unwrap_or_else(|| dir.clone()),
        };
        entries.push(WatcherEntry {
            dir,
            prompt: prompt.to_string(),
            runtime,
            repo,
            use_worktree: item
                .get("worktree")
                .and_then(|v| v.as_bool())
                .unwrap_or(false),
        });
    }
    (entries, diagnostics)
}

/// Substitute `{file}` in a prompt template.
pub(crate) fn prompt_for(template: &str, file: &Path) -> String {
    template.replace("{file}", &file.to_string_lossy())
}

/// Files a watcher should ignore: hidden files and common partial-write
/// artifacts (editors and sync tools create-then-rename these).
pub(crate) fn should_ignore(path: &Path) -> bool {
    let Some(name) = path.file_name().map(|name| name.to_string_lossy()) else {
        return true;
    };
    name.starts_with('.')
        || name.ends_with('~')
        || name.ends_with(".tmp")
        || name.ends_with(".swp")
        || name.ends_with(".part")
        || name.ends_with(".crdownload")
}

/// Start watching every configured dir. The returned watcher must be kept
/// alive for the server's lifetime; `None` when there is nothing to watch.
pub(crate) fn spawn_watchers(entries: Vec<WatcherEntry>) -> Option<notify::RecommendedWatcher> {
    if entries.is_empty() {
        return None;
    }
    let watch_dirs: Vec<PathBuf> = entries.iter().map(|entry| entry.dir.clone()).collect();
    let mut seen: HashSet<PathBuf> = HashSet::new();
    let handler = move |result: Result<notify::Event, notify::Error>| {
        let event = match result {
            Ok(event) => event,
            Err(err) => {
                tracing::warn!(err = %err, "fs watcher error");
                return;
            }
        };
        if !matches!(event.kind, notify::EventKind::Create(_)) {
            return;
        }
        for path in event.paths {
            if should_ignore(&path) || !path.is_file() || !seen.insert(path.clone()) {
                continue;
            }
            let Some(entry) = entries.iter().find(|entry| path.starts_with(&entry.dir)) else {
                continue;
            };
            enqueue_for_file(entry, &path);
        }
    };
    let mut watcher = match notify::recommended_watcher(handler) {
        Ok(watcher) => watcher,
        Err(err) => {
            tracing::warn!(err = %err, "failed to create fs watcher");
            return None;
        }
    };
    let mut watching = 0usize;
    for dir in watch_dirs {
        if let Err(err) = std::fs::create_dir_all(&dir) {
            tracing::warn!(dir = %dir.display(), err = %err, "cannot create watched dir");
            continue;
        }
        match watcher.watch(&dir, notify::RecursiveMode::NonRecursive) {
            Ok(()) => {
                watching += 1;
                tracing::info!(dir = %dir.display(), "watching for task drops");
            }
            Err(err) => tracing::warn!(dir = %dir.display(), err = %err, "cannot watch dir"),
        }
    }
    (watching > 0).then_some(watcher)
}

fn enqueue_for_file(entry: &WatcherEntry, path: &Path) {
    let prompt = prompt_for(&entry.prompt, path);
    let result = crate::tasks::open_store(&crate::tasks::tasks_db_path()).and_then(|conn| {
        crate::tasks::add_task(
            &conn,
            &prompt,
            &entry.repo,
            entry.runtime,
            entry.use_worktree,
            crate::tasks::unix_now(),
        )
    });
    match result {
        Ok(id) => {
            tracing::info!(task_id = id, file = %path.display(), "watcher enqueued task")
        }
        Err(err) => tracing::warn!(err = %err, file = %path.display(), "watcher enqueue failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_entries_and_skips_invalid_ones() {
        let raw = r#"
[[watchers]]
dir = "/drop/a"
prompt = "process {file}"

[[watchers]]
dir = "/drop/b"
prompt = "review {file}"
runtime = "opencode"
repo = "/repo"
worktree = true

[[watchers]]
prompt = "no dir"

[[watchers]]
dir = "/drop/c"
prompt = "bad runtime"
runtime = "codex"
"#;
        let (entries, diagnostics) = parse_watchers_config(raw);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].dir, PathBuf::from("/drop/a"));
        assert_eq!(entries[0].runtime, TaskRuntime::Claude);
        // No repo configured and /drop/a is no git repo: falls back to the dir.
        assert_eq!(entries[0].repo, PathBuf::from("/drop/a"));
        assert!(!entries[0].use_worktree);
        assert_eq!(entries[1].runtime, TaskRuntime::Opencode);
        assert_eq!(entries[1].repo, PathBuf::from("/repo"));
        assert!(entries[1].use_worktree);
        assert_eq!(diagnostics.len(), 2, "{diagnostics:?}");
    }

    #[test]
    fn broken_toml_is_a_diagnostic_not_a_crash() {
        let (entries, diagnostics) = parse_watchers_config("not [ valid");
        assert!(entries.is_empty());
        assert_eq!(diagnostics.len(), 1);
    }

    #[test]
    fn prompt_substitution_and_ignore_rules() {
        assert_eq!(
            prompt_for("handle {file} now", Path::new("/d/x.md")),
            "handle /d/x.md now"
        );
        assert!(should_ignore(Path::new("/d/.hidden")));
        assert!(should_ignore(Path::new("/d/save.tmp")));
        assert!(should_ignore(Path::new("/d/file~")));
        assert!(!should_ignore(Path::new("/d/task.md")));
    }
}
