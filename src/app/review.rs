//! Review flow (M3): inspect a workspace's changes in a pager pane.
//!
//! Panes are real terminals, so the cheapest correct diff view is a new tab in
//! the workspace running `git diff` through a pager — delta when installed,
//! `less -R` otherwise — with a `--stat` header piped in front. For a linked
//! worktree the diff spans the whole branch (merge-base of the base checkout's
//! branch) plus uncommitted work; for a plain workspace it is the working tree
//! against `HEAD`.
//!
//! Command construction is pure ([`review_pager_command`], tested); the pane
//! injection writes into the fresh tab's pty, which the kernel buffers until
//! the shell is up — no readiness race.

use std::path::Path;

use bytes::Bytes;

use crate::workspace::WorktreeSpaceMembership;

impl crate::app::App {
    /// Open a review pager tab for workspace `ws_idx` (focuses it).
    pub(crate) fn open_review_pager(&mut self, ws_idx: usize) {
        if ws_idx >= self.state.workspaces.len() {
            return;
        }
        let ws = &self.state.workspaces[ws_idx];
        let cwd = ws
            .resolved_identity_cwd_from(&self.state.terminals, &self.terminal_runtimes)
            .unwrap_or_else(|| ws.identity_cwd.clone());
        let target = review_diff_target(&cwd, ws.worktree_space());
        let command = review_pager_command(&target, delta_available());

        // Mirror the API tab-create path (production tab creation is
        // server-owned; the convenience wrappers in `creation.rs` are
        // test-only).
        let (rows, cols) = self.state.estimate_pane_size();
        let default_shell = self.state.default_shell.clone();
        let shell_mode = self.state.shell_mode;
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let result = self.state.workspaces[ws_idx].create_tab(
            rows,
            cols,
            cwd,
            scrollback_limit_bytes,
            host_terminal_theme,
            crate::pane::PaneShellConfig::new(&default_shell, shell_mode),
            Vec::new(),
        );
        let (tab_idx, terminal, runtime) = match result {
            Ok(created) => created,
            Err(err) => {
                tracing::warn!(err = %err, "failed to open review pane");
                return;
            }
        };
        self.terminal_runtimes.insert(terminal.id.clone(), runtime);
        self.state.terminals.insert(terminal.id.clone(), terminal);
        let ws = &mut self.state.workspaces[ws_idx];
        ws.tabs[tab_idx].set_custom_name("review".to_string());
        let root_pane = ws.tabs[tab_idx].root_pane;
        self.state.remove_alias_shadowed_by_new_pane(root_pane);
        self.state.switch_workspace_tab(ws_idx, tab_idx);
        self.state.mode = crate::app::state::Mode::Terminal;
        self.emit_tab_created_events(ws_idx, tab_idx);
        self.schedule_session_save();
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, root_pane) else {
            return;
        };
        if let Err(err) = runtime.try_send_bytes(Bytes::from(command)) {
            tracing::warn!(err = %err, "failed to inject review command");
        }
    }
}

impl crate::app::App {
    /// Ship a linked-worktree workspace: merge its branch into the base
    /// checkout's branch, then hand cleanup to the existing worktree-remove
    /// confirmation flow. Refuses (with a toast, never losing work) when
    /// either checkout is dirty, the branch is detached, or the merge
    /// conflicts — a conflicted merge is aborted in the base checkout.
    /// The merge is a fast local git operation; it runs synchronously.
    pub(crate) fn ship_worktree(&mut self, ws_idx: usize) {
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return;
        };
        let Some(space) = ws
            .worktree_space()
            .filter(|space| space.is_linked_worktree)
            .cloned()
        else {
            self.ship_toast(false, "not a linked-worktree workspace".to_string());
            return;
        };
        match ship_merge(&space) {
            Ok(message) => {
                self.ship_toast(true, message);
                // Auto-cleanup via the existing removal flow (confirmation
                // dialog, async removal, workspace close).
                self.state.request_remove_linked_worktree = Some(ws_idx);
            }
            Err(message) => self.ship_toast(false, message),
        }
    }

    /// Route review feedback to the workspace's agent: type it into the first
    /// pane with a detected agent (fallback: the focused pane) and submit with
    /// Enter, then mark the workspace `changes_requested`.
    pub(crate) fn send_review_feedback(&mut self, ws_idx: usize, feedback: String) {
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return;
        };
        ws.review_state = crate::api::schema::ReviewState::ChangesRequested;
        let target_pane = ws
            .tabs
            .iter()
            .flat_map(|tab| tab.panes.iter())
            .find(|(_, pane)| {
                self.state
                    .terminals
                    .get(&pane.attached_terminal_id)
                    .is_some_and(|terminal| terminal.detected_agent.is_some())
            })
            .map(|(pane_id, _)| *pane_id)
            .or_else(|| ws.focused_pane_id());
        let Some(pane_id) = target_pane else {
            return;
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return;
        };
        if let Err(err) = submit_pane_text(runtime, feedback.trim_end_matches(['\r', '\n'])) {
            tracing::warn!(err = %err, "failed to send review feedback");
        }
    }

    /// Submit `text` to a pane now, or queue it until the agent next goes
    /// idle when `queue` is set and the agent is working/blocked (M5
    /// tab-to-queue).
    ///
    /// Pass the prompt alone: submitting it is this function's job, and it is
    /// [`submit_pane_text`] that decides what the pty actually sees. Queued
    /// text is stored as the prompt and encoded when it is finally delivered,
    /// because whether the pane wants a bracketed paste is a fact about the
    /// pane at delivery time.
    pub(crate) fn send_or_queue_pane_text(
        &mut self,
        ws_idx: usize,
        pane_id: crate::layout::PaneId,
        text: String,
        queue: bool,
    ) -> Result<(), String> {
        use crate::detect::AgentState;
        // Trailing newlines from a client would submit a second, empty time.
        let text = text.trim_end_matches(['\r', '\n']).to_string();
        let busy = queue
            && self
                .state
                .workspaces
                .get(ws_idx)
                .and_then(|ws| ws.pane_state(pane_id))
                .and_then(|pane| self.state.terminals.get(&pane.attached_terminal_id))
                .is_some_and(|terminal| {
                    matches!(terminal.state, AgentState::Working | AgentState::Blocked)
                });
        if busy {
            self.state
                .queued_pane_input
                .entry(pane_id)
                .or_default()
                .push(text);
            return Ok(());
        }
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return Err("pane has no runtime".to_string());
        };
        submit_pane_text(runtime, &text)
    }

    /// Flush any input queued for `pane_id` (called on its transition to
    /// idle). Returns how many messages were delivered.
    pub(crate) fn flush_queued_pane_input(&mut self, pane_id: crate::layout::PaneId) -> usize {
        let Some(queued) = self.state.queued_pane_input.remove(&pane_id) else {
            return 0;
        };
        let Some(ws_idx) = self
            .state
            .workspaces
            .iter()
            .position(|ws| ws.tabs.iter().any(|tab| tab.panes.contains_key(&pane_id)))
        else {
            return 0;
        };
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, pane_id) else {
            return 0;
        };
        let mut delivered = 0;
        for text in queued {
            match submit_pane_text(runtime, &text) {
                Ok(()) => delivered += 1,
                Err(err) => {
                    tracing::warn!(err = %err, "queued input delivery failed");
                    break;
                }
            }
        }
        delivered
    }

    fn ship_toast(&mut self, ok: bool, context: String) {
        use crate::app::state::{ToastKind, ToastNotification};
        self.state.toast = Some(ToastNotification {
            kind: if ok {
                ToastKind::Finished
            } else {
                ToastKind::NeedsAttention
            },
            title: if ok { "✓ shipped" } else { "ship failed" }.to_string(),
            context,
            position: None,
            target: None,
        });
    }
}

/// Write `text` into a pane as a submitted prompt: the text, then Enter.
///
/// The two go out as **separate writes**, and the text goes as a *paste* when
/// the pane negotiated bracketed paste. Concatenating them — `{text}\r` in one
/// buffer, which is what this used to do — hands an agent CLI a single burst of
/// bytes ending in a carriage return, and the readline libraries they are built
/// on read a burst like that as pasted content: the `\r` becomes a newline
/// *inside* the input box instead of the keypress that submits it. Claude does
/// exactly this, which is why prompts sent from the phone sometimes landed as
/// an extra blank line and sat there unsent. Bracketing says "this part is
/// text" explicitly, so the Enter after the paste-end marker is unambiguous.
///
/// `\r`, not `\n`: Enter at the pty layer is a carriage return, and agent CLIs
/// submit on it.
fn submit_pane_text(runtime: &crate::terminal::TerminalRuntime, text: &str) -> Result<(), String> {
    let encoded = crate::app::api_helpers::encode_api_text(runtime, text);
    runtime
        .try_send_bytes(Bytes::from(encoded))
        .map_err(|err| err.to_string())?;
    runtime
        .try_send_bytes(Bytes::from_static(b"\r"))
        .map_err(|err| err.to_string())
}

/// The merge half of ship, separated from workspace state for testing against
/// real temp repos. Returns a human summary on success.
pub(crate) fn ship_merge(space: &WorktreeSpaceMembership) -> Result<String, String> {
    if !git_status_clean(&space.checkout_path)? {
        return Err("worktree has uncommitted changes — commit or stash first".to_string());
    }
    let branch = git_stdout(&space.checkout_path, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok_or_else(|| "cannot resolve the worktree branch".to_string())?;
    if branch == "HEAD" {
        return Err("worktree is on a detached HEAD".to_string());
    }
    let base = git_stdout(&space.repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
        .ok_or_else(|| "cannot resolve the base checkout branch".to_string())?;
    if base == branch {
        return Err(format!("worktree is already on the base branch {base}"));
    }
    if !git_status_clean(&space.repo_root)? {
        return Err(format!(
            "base checkout ({base}) has uncommitted changes — commit or stash first"
        ));
    }
    if let Err(err) = git_run(&space.repo_root, &["merge", "--no-edit", &branch]) {
        // Leave the base checkout the way we found it.
        let _ = git_run(&space.repo_root, &["merge", "--abort"]);
        return Err(format!("merge of {branch} into {base} failed: {err}"));
    }
    Ok(format!("merged {branch} into {base}"))
}

/// Whether `git status --porcelain` is empty in `dir`.
fn git_status_clean(dir: &Path) -> Result<bool, String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(["status", "--porcelain", "--untracked-files=no"])
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    Ok(output.stdout.is_empty())
}

/// Run a git command in `dir`, mapping failure to its stderr.
fn git_run(dir: &Path, args: &[&str]) -> Result<(), String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .map_err(|err| err.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&output.stderr).trim().to_string())
    }
}

/// What `git diff` is run against. Linked worktrees review the whole branch
/// (merge-base with the base checkout's current branch) plus uncommitted work;
/// everything else reviews the working tree against `HEAD`. Any git failure
/// falls back to `HEAD` — a too-narrow diff beats a broken pane.
pub(crate) fn review_diff_target(cwd: &Path, worktree: Option<&WorktreeSpaceMembership>) -> String {
    let head = || "HEAD".to_string();
    let Some(space) = worktree.filter(|space| space.is_linked_worktree) else {
        return head();
    };
    let Some(base_branch) = git_stdout(&space.repo_root, &["rev-parse", "--abbrev-ref", "HEAD"])
    else {
        return head();
    };
    git_stdout(cwd, &["merge-base", &base_branch, "HEAD"]).unwrap_or_else(head)
}

/// The review diff for a workspace as text, for non-pager clients (the Android
/// companion's `workspace.diff`): the resolved target ref, the `--stat` summary,
/// and the full unified diff. The diff is capped so a giant change can't flood a
/// phone; any git failure yields empty strings (nothing to review).
pub(crate) fn workspace_review_diff(
    cwd: &Path,
    worktree: Option<&WorktreeSpaceMembership>,
) -> (String, String, String) {
    const MAX_DIFF_BYTES: usize = 64 * 1024;
    let target = review_diff_target(cwd, worktree);
    let stat = git_stdout(cwd, &["diff", "--stat", &target]).unwrap_or_default();
    // Untrimmed: diff formatting (leading context spaces) is significant.
    let mut diff = git_stdout_untrimmed(cwd, &["diff", &target]).unwrap_or_default();
    if diff.len() > MAX_DIFF_BYTES {
        // Truncate on a char boundary, then append a notice.
        let mut cut = MAX_DIFF_BYTES;
        while cut > 0 && !diff.is_char_boundary(cut) {
            cut -= 1;
        }
        diff.truncate(cut);
        diff.push_str("\n… diff truncated — open on desktop for the full change …\n");
    }
    (target, stat, diff)
}

/// stdout of a git command in `dir` without trimming (preserves diff whitespace),
/// `None` on failure or empty output.
fn git_stdout_untrimmed(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    (!stdout.is_empty()).then_some(stdout)
}

/// The line typed into the review pane. Leading space keeps it out of shell
/// history; the trailing newline submits it. `--stat` first so the pager opens
/// on the summary.
pub(crate) fn review_pager_command(diff_target: &str, use_delta: bool) -> String {
    if use_delta {
        format!(
            " clear; {{ git diff --stat '{diff_target}'; echo; git diff '{diff_target}'; }} | delta --paging=always\n"
        )
    } else {
        format!(
            " clear; {{ git diff --color=always --stat '{diff_target}'; echo; git diff --color=always '{diff_target}'; }} | less -R\n"
        )
    }
}

/// Whether `delta` is on PATH.
fn delta_available() -> bool {
    let Some(path) = std::env::var_os("PATH") else {
        return false;
    };
    std::env::split_paths(&path).any(|dir| dir.join("delta").is_file())
}

/// Trimmed stdout of a git command in `dir`, `None` on any failure or empty
/// output.
fn git_stdout(dir: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!stdout.is_empty()).then_some(stdout)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_dir(tag: &str) -> PathBuf {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let path =
            std::env::temp_dir().join(format!("shep-review-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&path).unwrap();
        path
    }

    fn run_git(dir: &Path, args: &[&str]) {
        let status = std::process::Command::new("git")
            .arg("-C")
            .arg(dir)
            .args(args)
            .env("GIT_AUTHOR_NAME", "t")
            .env("GIT_AUTHOR_EMAIL", "t@t")
            .env("GIT_COMMITTER_NAME", "t")
            .env("GIT_COMMITTER_EMAIL", "t@t")
            .status()
            .unwrap();
        assert!(status.success(), "git {args:?} failed in {}", dir.display());
    }

    #[test]
    fn workspace_review_diff_reports_uncommitted_change() {
        let repo = temp_dir("wsdiff");
        run_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-q", "-m", "base"]);
        // Uncommitted edit: the working tree diff against HEAD should show it.
        std::fs::write(repo.join("f.txt"), "one\ntwo\n").unwrap();
        let (target, stat, diff) = workspace_review_diff(&repo, None);
        assert_eq!(target, "HEAD");
        assert!(stat.contains("f.txt"), "stat: {stat}");
        assert!(diff.contains("+two"), "diff: {diff}");
        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn workspace_review_diff_empty_on_clean_tree() {
        let repo = temp_dir("wsdiff-clean");
        run_git(&repo, &["init", "-q"]);
        std::fs::write(repo.join("f.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-q", "-m", "base"]);
        let (_target, stat, diff) = workspace_review_diff(&repo, None);
        assert!(stat.is_empty() && diff.is_empty(), "clean tree has no diff");
        std::fs::remove_dir_all(&repo).ok();
    }

    #[test]
    fn pager_command_shapes() {
        let less = review_pager_command("HEAD", false);
        assert!(less.starts_with(' '), "must stay out of shell history");
        assert!(less.ends_with('\n'), "must submit itself");
        assert!(less.contains("--color=always"));
        assert!(less.contains("--stat 'HEAD'"));
        assert!(less.contains("less -R"));

        let delta = review_pager_command("abc123", true);
        assert!(delta.contains("delta --paging=always"));
        assert!(delta.contains("diff 'abc123'"));
        // delta colorizes itself; forcing git color would garble it.
        assert!(!delta.contains("--color=always"));
    }

    #[test]
    fn diff_target_is_head_without_linked_worktree() {
        let dir = temp_dir("plain");
        assert_eq!(review_diff_target(&dir, None), "HEAD");
        let membership = WorktreeSpaceMembership {
            key: "k".into(),
            label: "l".into(),
            repo_root: dir.clone(),
            checkout_path: dir.clone(),
            is_linked_worktree: false,
        };
        assert_eq!(review_diff_target(&dir, Some(&membership)), "HEAD");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn diff_target_uses_merge_base_for_linked_worktree() {
        let base = temp_dir("wt");
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "one"]);
        let checkout = base.join("wt");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                checkout.to_str().unwrap(),
            ],
        );
        // Advance the feature branch so merge-base != HEAD of the worktree.
        std::fs::write(checkout.join("b.txt"), "two\n").unwrap();
        run_git(&checkout, &["add", "."]);
        run_git(&checkout, &["commit", "-m", "two"]);

        let membership = WorktreeSpaceMembership {
            key: "k".into(),
            label: "feature".into(),
            repo_root: repo.clone(),
            checkout_path: checkout.clone(),
            is_linked_worktree: true,
        };
        let target = review_diff_target(&checkout, Some(&membership));
        let expected = git_stdout(&checkout, &["merge-base", "main", "HEAD"]).unwrap();
        assert_eq!(target, expected);
        assert_ne!(
            target,
            git_stdout(&checkout, &["rev-parse", "HEAD"]).unwrap(),
            "merge-base must trail the feature commit"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    fn worktree_fixture(tag: &str) -> (PathBuf, WorktreeSpaceMembership) {
        let base = temp_dir(tag);
        let repo = base.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        run_git(&repo, &["init", "-b", "main"]);
        std::fs::write(repo.join("a.txt"), "one\n").unwrap();
        run_git(&repo, &["add", "."]);
        run_git(&repo, &["commit", "-m", "one"]);
        let checkout = base.join("wt");
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                "feature",
                checkout.to_str().unwrap(),
            ],
        );
        std::fs::write(checkout.join("b.txt"), "two\n").unwrap();
        run_git(&checkout, &["add", "."]);
        run_git(&checkout, &["commit", "-m", "two"]);
        let membership = WorktreeSpaceMembership {
            key: "k".into(),
            label: "feature".into(),
            repo_root: repo,
            checkout_path: checkout,
            is_linked_worktree: true,
        };
        (base, membership)
    }

    #[test]
    fn ship_merge_fast_forwards_clean_worktree() {
        let (base, membership) = worktree_fixture("ship-ok");
        let message = ship_merge(&membership).unwrap();
        assert_eq!(message, "merged feature into main");
        // The base checkout now contains the worktree commit.
        assert!(membership.repo_root.join("b.txt").exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ship_merge_refuses_dirty_worktree() {
        let (base, membership) = worktree_fixture("ship-dirty");
        std::fs::write(membership.checkout_path.join("b.txt"), "edited\n").unwrap();
        let err = ship_merge(&membership).unwrap_err();
        assert!(err.contains("uncommitted"), "{err}");
        assert!(
            !membership.repo_root.join("b.txt").exists(),
            "merge must not have happened"
        );
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ship_merge_aborts_on_conflict_and_reports() {
        let (base, membership) = worktree_fixture("ship-conflict");
        // Conflicting change on main.
        std::fs::write(membership.repo_root.join("b.txt"), "main-side\n").unwrap();
        run_git(&membership.repo_root, &["add", "."]);
        run_git(&membership.repo_root, &["commit", "-m", "conflict"]);
        let err = ship_merge(&membership).unwrap_err();
        assert!(err.contains("failed"), "{err}");
        // Base checkout left clean (merge aborted).
        assert!(git_status_clean(&membership.repo_root).unwrap());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn diff_target_falls_back_to_head_on_git_failure() {
        let dir = temp_dir("no-repo");
        let membership = WorktreeSpaceMembership {
            key: "k".into(),
            label: "l".into(),
            repo_root: dir.join("missing"),
            checkout_path: dir.clone(),
            is_linked_worktree: true,
        };
        assert_eq!(review_diff_target(&dir, Some(&membership)), "HEAD");
        std::fs::remove_dir_all(&dir).ok();
    }
}
