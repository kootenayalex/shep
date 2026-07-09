//! `shep task` — the M4 task queue. `add`/`list`/`cancel` are local file
//! operations on `<state dir>/tasks.db` (agents can queue work from any pane,
//! server up or not); `dispatch` talks to the server, which spawns the pane.

use std::path::PathBuf;

use crate::tasks::{self, TaskRuntime};

pub(super) fn run_task_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_task_help();
        return Ok(2);
    };
    match subcommand {
        "add" => task_add(&args[1..]),
        "list" => task_list(&args[1..]),
        "cancel" => task_cancel(&args[1..]),
        "dispatch" => task_dispatch(&args[1..]),
        "help" | "--help" | "-h" => {
            print_task_help();
            Ok(0)
        }
        other => {
            eprintln!("unknown task subcommand: {other}");
            print_task_help();
            Ok(2)
        }
    }
}

fn task_add(args: &[String]) -> std::io::Result<i32> {
    let mut prompt: Option<String> = None;
    let mut repo: Option<PathBuf> = None;
    let mut runtime = TaskRuntime::Claude;
    let mut use_worktree = false;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--repo" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --repo");
                    return Ok(2);
                };
                repo = Some(PathBuf::from(value));
                index += 2;
            }
            "--runtime" => {
                let Some(value) = args.get(index + 1) else {
                    eprintln!("missing value for --runtime");
                    return Ok(2);
                };
                let Some(parsed) = TaskRuntime::parse(value) else {
                    eprintln!("invalid --runtime {value} (claude|opencode)");
                    return Ok(2);
                };
                runtime = parsed;
                index += 2;
            }
            "--worktree" => {
                use_worktree = true;
                index += 1;
            }
            flag if flag.starts_with("--") => {
                eprintln!("unknown option: {flag}");
                return Ok(2);
            }
            positional => {
                if prompt.is_some() {
                    eprintln!("task add takes exactly one prompt argument");
                    return Ok(2);
                }
                prompt = Some(positional.to_string());
                index += 1;
            }
        }
    }
    let Some(prompt) = prompt.filter(|prompt| !prompt.trim().is_empty()) else {
        eprintln!("usage: shep task add \"<prompt>\" [--repo <path>] [--runtime claude|opencode] [--worktree]");
        return Ok(2);
    };
    let repo = crate::memory::resolve_repo_root(repo.as_deref())?;
    let conn = tasks::open_store(&tasks::tasks_db_path())?;
    let id = tasks::add_task(
        &conn,
        &prompt,
        &repo,
        runtime,
        use_worktree,
        tasks::unix_now(),
    )?;
    println!(
        "queued task {id} [{}{}] in {}",
        runtime.as_str(),
        if use_worktree { ", worktree" } else { "" },
        repo.display()
    );
    Ok(0)
}

fn task_list(args: &[String]) -> std::io::Result<i32> {
    if !args.is_empty() {
        eprintln!("usage: shep task list");
        return Ok(2);
    }
    let db = tasks::tasks_db_path();
    if !db.exists() {
        println!("no tasks");
        return Ok(0);
    }
    let conn = tasks::open_store(&db)?;
    let records = tasks::list_tasks(&conn)?;
    if records.is_empty() {
        println!("no tasks");
        return Ok(0);
    }
    println!("{:>4}  {:<9}  {:<8}  prompt", "id", "state", "runtime");
    for task in records {
        let mut prompt = task.prompt.replace('\n', " ");
        if prompt.chars().count() > 60 {
            prompt = prompt.chars().take(59).collect::<String>() + "…";
        }
        println!(
            "{:>4}  {:<9}  {:<8}  {}{}",
            task.id,
            task.state.as_str(),
            task.runtime.as_str(),
            prompt,
            task.workspace_id
                .as_deref()
                .map(|ws| format!("  [{ws}]"))
                .unwrap_or_default()
        );
    }
    Ok(0)
}

fn task_cancel(args: &[String]) -> std::io::Result<i32> {
    let [id] = args else {
        eprintln!("usage: shep task cancel <id>");
        return Ok(2);
    };
    let Ok(id) = id.parse::<i64>() else {
        eprintln!("invalid task id: {id}");
        return Ok(2);
    };
    let conn = tasks::open_store(&tasks::tasks_db_path())?;
    if tasks::cancel_task(&conn, id, tasks::unix_now())? {
        println!("cancelled task {id}");
        Ok(0)
    } else {
        eprintln!("task {id} not found or already finished");
        Ok(1)
    }
}

fn task_dispatch(args: &[String]) -> std::io::Result<i32> {
    let task_id = match args {
        [] => None,
        [id] => match id.parse::<i64>() {
            Ok(id) => Some(id),
            Err(_) => {
                eprintln!("invalid task id: {id}");
                return Ok(2);
            }
        },
        _ => {
            eprintln!("usage: shep task dispatch [<id>]");
            return Ok(2);
        }
    };
    super::runtime::task_dispatch(crate::api::schema::TaskDispatchParams { task_id })
}

fn print_task_help() {
    eprintln!("shep task commands:");
    eprintln!(
        "  shep task add \"<prompt>\" [--repo <path>] [--runtime claude|opencode] [--worktree]"
    );
    eprintln!("  shep task list                         queue with states");
    eprintln!("  shep task cancel <id>                  cancel an unfinished task");
    eprintln!("  shep task dispatch [<id>]              dispatch now (needs a running server)");
}

#[cfg(test)]
mod tests {
    // Argument handling is exercised through the pure store tests in
    // `crate::tasks`; the parsing here is deliberately thin. One smoke test
    // keeps the usage strings honest.
    #[test]
    fn runtime_parse_matches_help_text() {
        use crate::tasks::TaskRuntime;
        assert_eq!(TaskRuntime::parse("claude"), Some(TaskRuntime::Claude));
        assert_eq!(TaskRuntime::parse("opencode"), Some(TaskRuntime::Opencode));
        assert_eq!(TaskRuntime::parse("codex"), None);
    }
}
