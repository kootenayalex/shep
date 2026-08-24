//! Server-side task dispatch (M4): pop a queued task, spawn a workspace (or a
//! fresh linked worktree) at its repo, and launch the runtime with the prompt
//! injected via heredoc into the new pane's pty.

use std::path::{Path, PathBuf};

use bytes::Bytes;

use crate::tasks::{self, TaskRecord, TaskState};
use crate::workspace::WorktreeSpaceMembership;

impl crate::app::App {
    /// Dispatch `explicit` (or the oldest queued task). Returns the task and
    /// the internal workspace id it landed in.
    pub(crate) fn dispatch_task(
        &mut self,
        explicit: Option<i64>,
    ) -> Result<(TaskRecord, String), String> {
        let conn = tasks::open_store(&tasks::tasks_db_path()).map_err(|err| err.to_string())?;
        let task = match explicit {
            Some(id) => tasks::get_task(&conn, id)
                .map_err(|err| err.to_string())?
                .ok_or_else(|| format!("task {id} not found"))?,
            None => tasks::next_todo(&conn)
                .map_err(|err| err.to_string())?
                .ok_or_else(|| "no queued tasks".to_string())?,
        };
        if task.state != TaskState::Todo {
            return Err(format!(
                "task {} is {}, not todo",
                task.id,
                task.state.as_str()
            ));
        }
        if !task.repo.is_dir() {
            return Err(format!("task repo {} does not exist", task.repo.display()));
        }

        let (cwd, membership) = if task.use_worktree {
            let (checkout, membership) = create_task_worktree(&task.repo, task.id)?;
            (checkout, Some(membership))
        } else {
            (task.repo.clone(), None)
        };

        // Spawn unfocused: auto-dispatch must never steal the user's focus.
        let ws_idx = self
            .create_workspace_with_options(cwd, false)
            .map_err(|err| err.to_string())?;
        {
            let ws = &mut self.state.workspaces[ws_idx];
            ws.custom_name = Some(format!("task {}", task.id));
            if membership.is_some() {
                ws.worktree_space = membership;
            }
        }
        self.emit_workspace_open_events(ws_idx);

        let command = tasks::launch_command(
            self.state.tasks_config.runtime_command(task.runtime),
            &task.prompt,
            task.id,
        );
        let root_pane = self.state.workspaces[ws_idx].tabs[0].root_pane;
        let public_pane_id = self
            .public_pane_id(ws_idx, root_pane)
            .ok_or_else(|| "dispatched pane has no public id".to_string())?;
        let Some(runtime) = self.lookup_runtime_sender(ws_idx, root_pane) else {
            return Err("dispatched pane has no runtime".to_string());
        };
        runtime
            .try_send_bytes(Bytes::from(command))
            .map_err(|err| err.to_string())?;

        let workspace_id = self.state.workspaces[ws_idx].id.clone();
        tasks::set_task_state(
            &conn,
            task.id,
            TaskState::Running,
            Some(&workspace_id),
            Some(&public_pane_id),
            tasks::unix_now(),
        )
        .map_err(|err| err.to_string())?;
        tracing::info!(task_id = task.id, workspace_id = %workspace_id, "dispatched task");
        Ok((task, workspace_id))
    }
}

/// Create a linked worktree for a task: branch `task/<id>` checked out beside
/// the repo at `../<repo>-worktrees/task-<id>`.
fn create_task_worktree(
    repo: &Path,
    task_id: i64,
) -> Result<(PathBuf, WorktreeSpaceMembership), String> {
    let repo_name = repo
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "repo".to_string());
    let branch = format!("task/{task_id}");
    let checkout = repo
        .parent()
        .unwrap_or(repo)
        .join(format!("{repo_name}-worktrees"))
        .join(format!("task-{task_id}"));
    if checkout.exists() {
        return Err(format!(
            "worktree checkout {} already exists",
            checkout.display()
        ));
    }
    let output = std::process::Command::new("git")
        .arg("-C")
        .arg(repo)
        .arg("worktree")
        .arg("add")
        .arg("-b")
        .arg(&branch)
        .arg(&checkout)
        .output()
        .map_err(|err| err.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).trim().to_string());
    }
    let key = crate::workspace::git_space_metadata(repo)
        .map(|meta| meta.key)
        .unwrap_or_else(|| repo.to_string_lossy().into_owned());
    Ok((
        checkout.clone(),
        WorktreeSpaceMembership {
            key,
            label: branch,
            repo_root: repo.to_path_buf(),
            checkout_path: checkout,
            is_linked_worktree: true,
        },
    ))
}
