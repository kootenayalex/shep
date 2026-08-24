//! Task queue (M4): a local SQLite-backed queue of prompts to dispatch into
//! agent panes, plus the shell command that launches a runtime with a one-shot
//! prompt (heredoc injection, ported from damon-ade's `agent-command.ts`).
//!
//! The store is plain rusqlite at `<state dir>/tasks.db`; `shep task
//! add/list/cancel` edit it directly (no server needed), while dispatch — which
//! must spawn panes — runs server-side via the `task.dispatch` API method and
//! the auto-dispatch hook on agent-state transitions.

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// Queue lifecycle of a task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskState {
    Todo,
    Running,
    Blocked,
    Done,
    Cancelled,
}

impl TaskState {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskState::Todo => "todo",
            TaskState::Running => "running",
            TaskState::Blocked => "blocked",
            TaskState::Done => "done",
            TaskState::Cancelled => "cancelled",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "todo" => TaskState::Todo,
            "running" => TaskState::Running,
            "blocked" => TaskState::Blocked,
            "done" => TaskState::Done,
            "cancelled" => TaskState::Cancelled,
            _ => return None,
        })
    }
}

/// Which agent CLI runs the task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TaskRuntime {
    Claude,
    Opencode,
}

impl TaskRuntime {
    pub fn as_str(self) -> &'static str {
        match self {
            TaskRuntime::Claude => "claude",
            TaskRuntime::Opencode => "opencode",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "claude" => TaskRuntime::Claude,
            "opencode" => TaskRuntime::Opencode,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TaskRecord {
    pub id: i64,
    pub prompt: String,
    pub repo: PathBuf,
    pub runtime: TaskRuntime,
    pub use_worktree: bool,
    pub state: TaskState,
    /// Workspace the task was dispatched into (drives state tracking).
    pub workspace_id: Option<String>,
    /// Exact agent pane the task was assigned to, when applicable.
    pub assigned_pane_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Default on-disk location of the task queue.
pub(crate) fn tasks_db_path() -> PathBuf {
    crate::config::state_dir().join("tasks.db")
}

/// Open (creating if needed) the task store at `path`.
pub(crate) fn open_store(path: &Path) -> io::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).map_err(io_err)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS tasks (
            id INTEGER PRIMARY KEY,
            prompt TEXT NOT NULL,
            repo TEXT NOT NULL,
            runtime TEXT NOT NULL,
            use_worktree INTEGER NOT NULL DEFAULT 0,
            state TEXT NOT NULL DEFAULT 'todo',
            workspace_id TEXT,
            created_at INTEGER NOT NULL,
            updated_at INTEGER NOT NULL
         );",
    )
    .map_err(io_err)?;
    // Tasks databases predate exact agent assignment. Keep old queues usable
    // while recording the pane identity for new assignments.
    let has_pane = conn
        .prepare("PRAGMA table_info(tasks)")
        .map_err(io_err)?
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(io_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err)?
        .iter()
        .any(|name| name == "assigned_pane_id");
    if !has_pane {
        conn.execute("ALTER TABLE tasks ADD COLUMN assigned_pane_id TEXT", [])
            .map_err(io_err)?;
    }
    Ok(conn)
}

pub(crate) fn add_task(
    conn: &Connection,
    prompt: &str,
    repo: &Path,
    runtime: TaskRuntime,
    use_worktree: bool,
    now: i64,
) -> io::Result<i64> {
    conn.execute(
        "INSERT INTO tasks (prompt, repo, runtime, use_worktree, state, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, 'todo', ?5, ?5)",
        rusqlite::params![
            prompt,
            repo.to_string_lossy(),
            runtime.as_str(),
            use_worktree as i64,
            now
        ],
    )
    .map_err(io_err)?;
    Ok(conn.last_insert_rowid())
}

pub(crate) fn list_tasks(conn: &Connection) -> io::Result<Vec<TaskRecord>> {
    let mut statement = conn
        .prepare(
            "SELECT id, prompt, repo, runtime, use_worktree, state, workspace_id,
                    assigned_pane_id, created_at, updated_at
             FROM tasks ORDER BY id",
        )
        .map_err(io_err)?;
    let rows = statement
        .query_map([], row_to_record)
        .map_err(io_err)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(io_err)?;
    Ok(rows)
}

pub(crate) fn get_task(conn: &Connection, id: i64) -> io::Result<Option<TaskRecord>> {
    Ok(list_tasks(conn)?.into_iter().find(|task| task.id == id))
}

/// Oldest queued task, if any.
pub(crate) fn next_todo(conn: &Connection) -> io::Result<Option<TaskRecord>> {
    Ok(list_tasks(conn)?
        .into_iter()
        .find(|task| task.state == TaskState::Todo))
}

pub(crate) fn set_task_state(
    conn: &Connection,
    id: i64,
    state: TaskState,
    workspace_id: Option<&str>,
    pane_id: Option<&str>,
    now: i64,
) -> io::Result<()> {
    match (workspace_id, pane_id) {
        (Some(workspace_id), Some(pane_id)) => conn.execute(
            "UPDATE tasks SET state = ?2, workspace_id = ?3, assigned_pane_id = ?4,
             updated_at = ?5 WHERE id = ?1",
            rusqlite::params![id, state.as_str(), workspace_id, pane_id, now],
        ),
        (Some(_workspace_id), None) => conn.execute(
            "UPDATE tasks SET state = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, state.as_str(), now],
        ),
        (None, _) => conn.execute(
            "UPDATE tasks SET state = ?2, updated_at = ?3 WHERE id = ?1",
            rusqlite::params![id, state.as_str(), now],
        ),
    }
    .map_err(io_err)?;
    Ok(())
}

/// Cancel a task that has not finished. Returns whether a row changed.
pub(crate) fn cancel_task(conn: &Connection, id: i64, now: i64) -> io::Result<bool> {
    let changed = conn
        .execute(
            "UPDATE tasks SET state = 'cancelled', updated_at = ?2
             WHERE id = ?1 AND state IN ('todo', 'running', 'blocked')",
            rusqlite::params![id, now],
        )
        .map_err(io_err)?;
    Ok(changed > 0)
}

/// Remove one task outright. Returns whether a row was deleted.
///
/// Cancelling leaves a tombstone in the list, which is right while a task is
/// still interesting and wrong once it is not. Deleting is the sweep: the queue
/// is a working surface, not an audit log.
pub(crate) fn delete_task(conn: &Connection, id: i64) -> io::Result<bool> {
    let changed = conn
        .execute("DELETE FROM tasks WHERE id = ?1", rusqlite::params![id])
        .map_err(io_err)?;
    Ok(changed > 0)
}

/// Delete every finished task (`done` / `cancelled`). Returns the count removed.
/// Open loops — todo, running, blocked — are never swept.
pub(crate) fn clear_finished(conn: &Connection) -> io::Result<usize> {
    let changed = conn
        .execute("DELETE FROM tasks WHERE state IN ('done', 'cancelled')", [])
        .map_err(io_err)?;
    Ok(changed)
}

/// Hand an open task to an agent session that is already running.
///
/// Dispatch spawns a pane; assignment does not — the caller has already sent
/// the prompt into a live pane, and this records where it went. Marking it
/// `running` against that workspace is what lets the existing agent-state
/// tracker carry it to `blocked`/`done` exactly as a dispatched task.
pub(crate) fn assign_task(
    conn: &Connection,
    id: i64,
    workspace_id: &str,
    pane_id: &str,
    now: i64,
) -> io::Result<bool> {
    let changed = conn
        .execute(
            "UPDATE tasks SET state = 'running', workspace_id = ?2, assigned_pane_id = ?3,
             updated_at = ?4 WHERE id = ?1 AND state IN ('todo', 'blocked')",
            rusqlite::params![id, workspace_id, pane_id, now],
        )
        .map_err(io_err)?;
    Ok(changed > 0)
}

/// The running task assigned to `pane_id`.
///
/// The workspace fallback upgrades queues created before exact assignment was
/// added; once their state changes, the caller writes the exact pane id.
pub(crate) fn task_for_pane(
    conn: &Connection,
    pane_id: &str,
    workspace_id: &str,
) -> io::Result<Option<TaskRecord>> {
    Ok(list_tasks(conn)?.into_iter().find(|task| {
        (task.assigned_pane_id.as_deref() == Some(pane_id)
            || (task.assigned_pane_id.is_none()
                && task.workspace_id.as_deref() == Some(workspace_id)))
            && matches!(task.state, TaskState::Running | TaskState::Blocked)
    }))
}

/// One-shot launch line for a task: `<runtime cmd> "$(cat <<'DELIM' ... )"`.
/// The heredoc keeps the prompt verbatim (no shell interpolation inside a
/// quoted delimiter); the delimiter is grown until it collides with nothing in
/// the prompt. Leading space keeps the line out of shell history.
pub(crate) fn launch_command(runtime_command: &str, prompt: &str, task_id: i64) -> String {
    let mut delimiter = format!("SHEP_TASK_{task_id}");
    while prompt.lines().any(|line| line.trim() == delimiter) {
        delimiter.push('_');
    }
    format!(" {runtime_command} \"$(cat <<'{delimiter}'\n{prompt}\n{delimiter}\n)\"\n")
}

fn row_to_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<TaskRecord> {
    let runtime: String = row.get(3)?;
    let state: String = row.get(5)?;
    let repo: String = row.get(2)?;
    Ok(TaskRecord {
        id: row.get(0)?,
        prompt: row.get(1)?,
        repo: PathBuf::from(repo),
        runtime: TaskRuntime::parse(&runtime).unwrap_or(TaskRuntime::Claude),
        use_worktree: row.get::<_, i64>(4)? != 0,
        state: TaskState::parse(&state).unwrap_or(TaskState::Todo),
        workspace_id: row.get(6)?,
        assigned_pane_id: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

pub(crate) fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn io_err(err: rusqlite::Error) -> io::Error {
    io::Error::other(format!("tasks db: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_store(tag: &str) -> (PathBuf, Connection) {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let dir =
            std::env::temp_dir().join(format!("shep-tasks-{tag}-{}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let conn = open_store(&dir.join("tasks.db")).unwrap();
        (dir, conn)
    }

    #[test]
    fn add_list_roundtrip_and_queue_order() {
        let (dir, conn) = temp_store("roundtrip");
        let first = add_task(
            &conn,
            "fix login",
            Path::new("/repo"),
            TaskRuntime::Claude,
            false,
            10,
        )
        .unwrap();
        let second = add_task(
            &conn,
            "add tests",
            Path::new("/repo"),
            TaskRuntime::Opencode,
            true,
            20,
        )
        .unwrap();
        let tasks = list_tasks(&conn).unwrap();
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, first);
        assert_eq!(tasks[0].prompt, "fix login");
        assert_eq!(tasks[0].runtime, TaskRuntime::Claude);
        assert!(!tasks[0].use_worktree);
        assert_eq!(tasks[1].runtime, TaskRuntime::Opencode);
        assert!(tasks[1].use_worktree);
        // FIFO: oldest todo first.
        assert_eq!(next_todo(&conn).unwrap().unwrap().id, first);
        let _ = second;
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn state_transitions_and_workspace_linkage() {
        let (dir, conn) = temp_store("state");
        let id = add_task(&conn, "p", Path::new("/r"), TaskRuntime::Claude, false, 1).unwrap();
        set_task_state(&conn, id, TaskState::Running, Some("w1"), Some("w1:p1"), 2).unwrap();
        let task = get_task(&conn, id).unwrap().unwrap();
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(task.workspace_id.as_deref(), Some("w1"));
        assert_eq!(task.updated_at, 2);
        assert_eq!(task_for_pane(&conn, "w1:p1", "w1").unwrap().unwrap().id, id);
        // State-only update keeps the linkage.
        set_task_state(&conn, id, TaskState::Blocked, None, None, 3).unwrap();
        assert_eq!(
            get_task(&conn, id)
                .unwrap()
                .unwrap()
                .workspace_id
                .as_deref(),
            Some("w1")
        );
        assert!(next_todo(&conn).unwrap().is_none());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn cancel_only_touches_unfinished_tasks() {
        let (dir, conn) = temp_store("cancel");
        let id = add_task(&conn, "p", Path::new("/r"), TaskRuntime::Claude, false, 1).unwrap();
        assert!(cancel_task(&conn, id, 2).unwrap());
        assert_eq!(
            get_task(&conn, id).unwrap().unwrap().state,
            TaskState::Cancelled
        );
        // Already cancelled: no-op.
        assert!(!cancel_task(&conn, id, 3).unwrap());
        let done = add_task(&conn, "q", Path::new("/r"), TaskRuntime::Claude, false, 4).unwrap();
        set_task_state(&conn, done, TaskState::Done, None, None, 5).unwrap();
        assert!(!cancel_task(&conn, done, 6).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn delete_removes_one_row_and_clear_sweeps_only_finished() {
        let (dir, conn) = temp_store("sweep");
        let todo = add_task(&conn, "a", Path::new("/r"), TaskRuntime::Claude, false, 1).unwrap();
        let done = add_task(&conn, "b", Path::new("/r"), TaskRuntime::Claude, false, 2).unwrap();
        let cancelled =
            add_task(&conn, "c", Path::new("/r"), TaskRuntime::Claude, false, 3).unwrap();
        let running = add_task(&conn, "d", Path::new("/r"), TaskRuntime::Claude, false, 4).unwrap();
        set_task_state(&conn, done, TaskState::Done, None, None, 5).unwrap();
        cancel_task(&conn, cancelled, 6).unwrap();
        set_task_state(
            &conn,
            running,
            TaskState::Running,
            Some("w1"),
            Some("w1:p1"),
            7,
        )
        .unwrap();

        // Clear sweeps the two finished rows and leaves the open loops alone.
        assert_eq!(clear_finished(&conn).unwrap(), 2);
        let left: Vec<i64> = list_tasks(&conn).unwrap().iter().map(|t| t.id).collect();
        assert_eq!(left, vec![todo, running]);

        // Delete is unconditional — an open task can be removed outright.
        assert!(delete_task(&conn, running).unwrap());
        assert!(!delete_task(&conn, running).unwrap());
        assert_eq!(list_tasks(&conn).unwrap().len(), 1);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn assign_links_an_open_task_to_a_live_workspace() {
        let (dir, conn) = temp_store("assign");
        let id = add_task(&conn, "p", Path::new("/r"), TaskRuntime::Claude, false, 1).unwrap();
        assert!(assign_task(&conn, id, "w3", "w3:p1", 2).unwrap());
        let task = get_task(&conn, id).unwrap().unwrap();
        assert_eq!(task.state, TaskState::Running);
        assert_eq!(task.workspace_id.as_deref(), Some("w3"));
        // The existing tracker can now find it and carry it to done.
        assert_eq!(task_for_pane(&conn, "w3:p1", "w3").unwrap().unwrap().id, id);
        // Finished tasks are not reassignable.
        set_task_state(&conn, id, TaskState::Done, None, None, 3).unwrap();
        assert!(!assign_task(&conn, id, "w4", "w4:p1", 4).unwrap());
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn launch_command_wraps_prompt_in_collision_free_heredoc() {
        let out = launch_command("claude", "do the thing\nwith care", 7);
        assert!(out.starts_with(" claude \"$(cat <<'SHEP_TASK_7'\n"));
        assert!(out.contains("do the thing\nwith care\n"));
        assert!(out.ends_with("SHEP_TASK_7\n)\"\n"));

        // A prompt containing the delimiter forces a longer one.
        let hostile = "before\nSHEP_TASK_7\nafter";
        let out = launch_command("claude", hostile, 7);
        assert!(out.contains("<<'SHEP_TASK_7_'"));
        assert!(out.trim_end().ends_with("SHEP_TASK_7_\n)\""));
    }
}
