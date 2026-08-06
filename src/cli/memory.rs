//! `shep memory` — read and curate shep's shared hermes-style memory.
//!
//! This subcommand is a **local file operation**: unlike most of the `shep`
//! CLI, it does NOT connect to the server socket or route through the JSON API.
//! Memory lives in plain files (`~/.config/shep/memory/USER.md` and
//! `<repo>/.shep/memory/MEMORY.md`); editing them needs no running server and
//! must work from inside any agent pane's Bash, including before a session is
//! attached. Keeping it socket-free also means an agent can curate memory even
//! when the server is busy or down. See `docs/VISION.md` §M2.

use std::path::{Path, PathBuf};

use crate::memory::{self, MemoryDoc, MemoryError, MemoryKind};

/// Which file a mutating op targets. `Repo(None)` means the git repo enclosing
/// the current directory.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Target {
    User,
    Repo(Option<PathBuf>),
}

/// Parsed `shep memory` invocation. Parsing is separated from execution so the
/// argument plumbing is unit-testable without touching the filesystem.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Command {
    Show {
        user: bool,
        repo: Option<PathBuf>,
    },
    Add {
        target: Target,
        text: String,
    },
    Replace {
        target: Target,
        old: String,
        new: String,
    },
    Remove {
        target: Target,
        substring: String,
    },
    Status {
        repo: Option<PathBuf>,
    },
    Init {
        repo: Option<PathBuf>,
    },
    /// claude-code Stop-hook plumbing (reads hook JSON from stdin). Hidden from
    /// the main help; wired by `shep memory init`.
    ReflectHook,
    /// Lifecycle-hook plumbing: record one hook payload (stdin JSON) into the
    /// FTS5 history sidecar. Wired by `shep memory init`.
    IngestEvent,
    Search {
        query: String,
        limit: usize,
    },
}

pub(super) fn run_memory_command(args: &[String]) -> std::io::Result<i32> {
    let command = match parse(args) {
        Ok(Some(command)) => command,
        Ok(None) => {
            print_help();
            return Ok(0);
        }
        Err(message) => {
            eprintln!("{message}");
            print_help();
            return Ok(2);
        }
    };
    execute(command)
}

/// Parse `args` (everything after `memory`). `Ok(None)` requests help.
fn parse(args: &[String]) -> Result<Option<Command>, String> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        return Ok(None);
    };
    let rest = &args[1..];
    match subcommand {
        "help" | "--help" | "-h" => Ok(None),
        "show" => {
            let flags = Flags::parse(rest)?;
            flags.expect_positionals(0)?;
            Ok(Some(Command::Show {
                user: flags.user,
                repo: flags.repo,
            }))
        }
        "add" => {
            let flags = Flags::parse(rest)?;
            let text = flags.expect_positionals(1)?[0].to_string();
            Ok(Some(Command::Add {
                target: flags.target()?,
                text,
            }))
        }
        "replace" => {
            let flags = Flags::parse(rest)?;
            let positionals = flags.expect_positionals(2)?;
            Ok(Some(Command::Replace {
                target: flags.target()?,
                old: positionals[0].to_string(),
                new: positionals[1].to_string(),
            }))
        }
        "remove" => {
            let flags = Flags::parse(rest)?;
            let substring = flags.expect_positionals(1)?[0].to_string();
            Ok(Some(Command::Remove {
                target: flags.target()?,
                substring,
            }))
        }
        "status" => {
            let flags = Flags::parse(rest)?;
            flags.reject_user("status")?;
            flags.expect_positionals(0)?;
            Ok(Some(Command::Status { repo: flags.repo }))
        }
        "init" => {
            let flags = Flags::parse(rest)?;
            flags.reject_user("init")?;
            flags.expect_positionals(0)?;
            Ok(Some(Command::Init { repo: flags.repo }))
        }
        "reflect-hook" => {
            // Hook plumbing: reads the Stop payload from stdin, takes no flags or
            // positionals. Anything else is a wiring mistake we surface.
            let flags = Flags::parse(rest)?;
            flags.reject_user("reflect-hook")?;
            flags.expect_positionals(0)?;
            Ok(Some(Command::ReflectHook))
        }
        "ingest-event" => {
            let flags = Flags::parse(rest)?;
            flags.reject_user("ingest-event")?;
            flags.expect_positionals(0)?;
            Ok(Some(Command::IngestEvent))
        }
        "search" => {
            let flags = Flags::parse(rest)?;
            flags.reject_user("search")?;
            if flags.positionals.is_empty() {
                return Err("search needs a query".to_string());
            }
            Ok(Some(Command::Search {
                query: flags.positionals.join(" "),
                limit: flags.limit.unwrap_or(20),
            }))
        }
        other => Err(format!("unknown memory subcommand: {other}")),
    }
}

/// Flag/positional split shared by every subcommand: `--user`, `--repo <path>`,
/// and free positionals.
struct Flags<'a> {
    user: bool,
    repo: Option<PathBuf>,
    limit: Option<usize>,
    positionals: Vec<&'a str>,
}

impl<'a> Flags<'a> {
    fn parse(args: &'a [String]) -> Result<Self, String> {
        let mut user = false;
        let mut repo = None;
        let mut limit = None;
        let mut positionals = Vec::new();
        let mut index = 0;
        while index < args.len() {
            match args[index].as_str() {
                "--user" => {
                    user = true;
                    index += 1;
                }
                "--repo" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| "missing value for --repo".to_string())?;
                    repo = Some(PathBuf::from(value));
                    index += 2;
                }
                "--limit" => {
                    let value = args
                        .get(index + 1)
                        .ok_or_else(|| "missing value for --limit".to_string())?;
                    limit = Some(
                        value
                            .parse::<usize>()
                            .map_err(|_| format!("invalid --limit: {value}"))?,
                    );
                    index += 2;
                }
                flag if flag.starts_with("--") => {
                    return Err(format!("unknown option: {flag}"));
                }
                positional => {
                    positionals.push(positional);
                    index += 1;
                }
            }
        }
        Ok(Flags {
            user,
            repo,
            limit,
            positionals,
        })
    }

    /// Resolve a mutation target: `--user` xor `--repo`, defaulting to the repo.
    fn target(&self) -> Result<Target, String> {
        if self.user && self.repo.is_some() {
            return Err("choose one of --user or --repo, not both".to_string());
        }
        if self.user {
            Ok(Target::User)
        } else {
            Ok(Target::Repo(self.repo.clone()))
        }
    }

    fn reject_user(&self, subcommand: &str) -> Result<(), String> {
        if self.user {
            return Err(format!("--user is not valid for `memory {subcommand}`"));
        }
        Ok(())
    }

    fn expect_positionals(&self, count: usize) -> Result<&[&'a str], String> {
        if self.positionals.len() != count {
            return Err(format!(
                "expected {count} positional argument(s), got {}",
                self.positionals.len()
            ));
        }
        Ok(&self.positionals)
    }
}

fn execute(command: Command) -> std::io::Result<i32> {
    match command {
        Command::Show { user, repo } => show(user, repo),
        Command::Add { target, text } => mutate(target, |doc, cap| doc.add(&text, cap)),
        Command::Replace { target, old, new } => {
            mutate(target, |doc, cap| doc.replace(&old, &new, cap))
        }
        Command::Remove { target, substring } => mutate(target, |doc, _cap| doc.remove(&substring)),
        Command::Status { repo } => status(repo),
        Command::Init { repo } => init(repo),
        Command::ReflectHook => Ok(memory::reflect::run_reflect_hook()),
        Command::IngestEvent => Ok(memory::history::run_ingest_event()),
        Command::Search { query, limit } => search(&query, limit),
    }
}

/// `shep memory search`: substring hits from the two memory files, then FTS5
/// hits from the session-history sidecar (most recent first).
fn search(query: &str, limit: usize) -> std::io::Result<i32> {
    let needle = query.to_lowercase();
    let mut memory_hits = Vec::new();
    let user_path = memory::user_memory_path();
    if let Ok(doc) = memory::load_or_create(&user_path, MemoryKind::User) {
        for entry in doc.entries() {
            if entry.to_lowercase().contains(&needle) {
                memory_hits.push(format!("user  {entry}"));
            }
        }
    }
    // Outside a repo there is simply no repo memory to scan.
    if let Ok(root) = memory::resolve_repo_root(None) {
        let repo_path = memory::repo_memory_path(&root);
        if let Ok(doc) = memory::load_or_create(&repo_path, MemoryKind::Repo) {
            for entry in doc.entries() {
                if entry.to_lowercase().contains(&needle) {
                    memory_hits.push(format!("repo  {entry}"));
                }
            }
        }
    }
    if !memory_hits.is_empty() {
        println!("memory entries:");
        for hit in &memory_hits {
            println!("  {hit}");
        }
    }

    let db_path = memory::history::history_db_path();
    let history_hits = if db_path.exists() {
        let conn = memory::history::open_db(&db_path)?;
        memory::history::search(&conn, query, limit)?
    } else {
        Vec::new()
    };
    if !history_hits.is_empty() {
        println!("session history:");
        for hit in &history_hits {
            println!(
                "  {}  [{} {}] {}",
                format_age(hit.ts),
                hit.kind,
                &hit.session_id[..hit.session_id.len().min(8)],
                hit.snippet.replace('\n', " ")
            );
        }
    }

    if memory_hits.is_empty() && history_hits.is_empty() {
        println!("no matches for \"{query}\"");
    }
    Ok(0)
}

/// Compact age like `3m`, `2h`, `5d` from a unix timestamp; `now` for the
/// future or the current minute.
fn format_age(ts: i64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let seconds = now.saturating_sub(ts);
    match seconds {
        s if s < 60 => "now".to_string(),
        s if s < 3_600 => format!("{}m", s / 60),
        s if s < 86_400 => format!("{}h", s / 3_600),
        s => format!("{}d", s / 86_400),
    }
}

/// Resolve a [`Target`] to its kind and on-disk path.
fn resolve_target(target: &Target) -> std::io::Result<(MemoryKind, PathBuf)> {
    match target {
        Target::User => Ok((MemoryKind::User, memory::user_memory_path())),
        Target::Repo(explicit) => {
            let root = memory::resolve_repo_root(explicit.as_deref())?;
            Ok((MemoryKind::Repo, memory::repo_memory_path(&root)))
        }
    }
}

/// Load the target file, apply `op`, and persist on success. Memory errors go to
/// stderr with a non-zero exit; the file is left untouched on error.
fn mutate(
    target: Target,
    op: impl FnOnce(&mut MemoryDoc, usize) -> Result<(), MemoryError>,
) -> std::io::Result<i32> {
    let (kind, path) = resolve_target(&target)?;
    let mut doc = memory::load_or_create(&path, kind)?;
    match op(&mut doc, kind.cap()) {
        Ok(()) => {
            memory::write_doc(&path, &doc)?;
            println!("ok — {} [{}]", path.display(), doc.usage(kind.cap()));
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn show(user: bool, repo: Option<PathBuf>) -> std::io::Result<i32> {
    // With no flag, show both files; otherwise show only what was asked for.
    let neither = !user && repo.is_none();
    let show_user = user || neither;
    let show_repo = repo.is_some() || neither;

    if show_user {
        print_file(MemoryKind::User, &memory::user_memory_path())?;
    }
    if show_repo {
        let root = memory::resolve_repo_root(repo.as_deref())?;
        print_file(MemoryKind::Repo, &memory::repo_memory_path(&root))?;
    }
    Ok(0)
}

fn print_file(kind: MemoryKind, path: &Path) -> std::io::Result<()> {
    let doc = memory::load_or_create(path, kind)?;
    println!("=== {} memory ({}) ===", kind.label(), path.display());
    print!("{}", doc.render());
    if doc.entries().is_empty() {
        println!("(empty — no entries yet)");
    }
    println!("[{}]", doc.usage(kind.cap()));
    Ok(())
}

fn status(repo: Option<PathBuf>) -> std::io::Result<i32> {
    let user_path = memory::user_memory_path();
    let user_doc = memory::load_or_create(&user_path, MemoryKind::User)?;
    println!(
        "user  {}  [{}]",
        user_path.display(),
        user_doc.usage(MemoryKind::User.cap())
    );

    let root = memory::resolve_repo_root(repo.as_deref())?;
    let repo_path = memory::repo_memory_path(&root);
    let repo_doc = memory::load_or_create(&repo_path, MemoryKind::Repo)?;
    println!(
        "repo  {}  [{}]",
        repo_path.display(),
        repo_doc.usage(MemoryKind::Repo.cap())
    );
    Ok(0)
}

fn init(repo: Option<PathBuf>) -> std::io::Result<i32> {
    let root = memory::resolve_repo_root(repo.as_deref())?;
    let paths = memory::bridges::resolve_bridge_paths(&root)?;
    match memory::bridges::install_bridges(&paths) {
        Ok(messages) => {
            println!("installed shep memory bridges for {}:", root.display());
            for message in messages {
                println!("  {message}");
            }
            Ok(0)
        }
        Err(err) => {
            eprintln!("{err}");
            Ok(1)
        }
    }
}

fn print_help() {
    eprintln!("shep memory commands (local file ops, no server needed):");
    eprintln!("  shep memory show [--user] [--repo <path>]        print memory (both if no flag)");
    eprintln!("  shep memory add \"<text>\" [--user|--repo <path>]   add an entry (default: repo)");
    eprintln!(
        "  shep memory replace \"<old>\" \"<new>\" [--user|--repo <path>]  replace by substring"
    );
    eprintln!(
        "  shep memory remove \"<substring>\" [--user|--repo <path>]     remove by substring"
    );
    eprintln!("  shep memory status [--repo <path>]               usage for both files");
    eprintln!("  shep memory search \"<query>\" [--limit <n>]        memory + session history");
    eprintln!(
        "  shep memory init [--repo <path>]                 install/refresh agent bridges + files"
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn no_args_requests_help() {
        assert_eq!(parse(&[]).unwrap(), None);
        assert_eq!(parse(&args(&["--help"])).unwrap(), None);
    }

    #[test]
    fn show_without_flags_targets_both() {
        assert_eq!(
            parse(&args(&["show"])).unwrap(),
            Some(Command::Show {
                user: false,
                repo: None
            })
        );
    }

    #[test]
    fn show_with_repo_path() {
        assert_eq!(
            parse(&args(&["show", "--repo", "/r"])).unwrap(),
            Some(Command::Show {
                user: false,
                repo: Some(PathBuf::from("/r"))
            })
        );
    }

    #[test]
    fn add_defaults_to_repo_target() {
        assert_eq!(
            parse(&args(&["add", "a fact"])).unwrap(),
            Some(Command::Add {
                target: Target::Repo(None),
                text: "a fact".to_string()
            })
        );
    }

    #[test]
    fn add_user_flag_selects_user() {
        assert_eq!(
            parse(&args(&["add", "a fact", "--user"])).unwrap(),
            Some(Command::Add {
                target: Target::User,
                text: "a fact".to_string()
            })
        );
    }

    #[test]
    fn add_with_explicit_repo_path() {
        assert_eq!(
            parse(&args(&["add", "fact", "--repo", "/work"])).unwrap(),
            Some(Command::Add {
                target: Target::Repo(Some(PathBuf::from("/work"))),
                text: "fact".to_string()
            })
        );
    }

    #[test]
    fn add_rejects_user_and_repo_together() {
        assert!(parse(&args(&["add", "fact", "--user", "--repo", "/r"])).is_err());
    }

    #[test]
    fn replace_takes_two_positionals() {
        assert_eq!(
            parse(&args(&["replace", "old", "new"])).unwrap(),
            Some(Command::Replace {
                target: Target::Repo(None),
                old: "old".to_string(),
                new: "new".to_string(),
            })
        );
    }

    #[test]
    fn replace_wrong_arity_errors() {
        assert!(parse(&args(&["replace", "only-one"])).is_err());
    }

    #[test]
    fn remove_takes_one_positional() {
        assert_eq!(
            parse(&args(&["remove", "needle"])).unwrap(),
            Some(Command::Remove {
                target: Target::Repo(None),
                substring: "needle".to_string(),
            })
        );
    }

    #[test]
    fn status_and_init_reject_user_flag() {
        assert!(parse(&args(&["status", "--user"])).is_err());
        assert!(parse(&args(&["init", "--user"])).is_err());
    }

    #[test]
    fn status_accepts_repo_path() {
        assert_eq!(
            parse(&args(&["status", "--repo", "/r"])).unwrap(),
            Some(Command::Status {
                repo: Some(PathBuf::from("/r"))
            })
        );
    }

    #[test]
    fn missing_repo_value_errors() {
        assert!(parse(&args(&["show", "--repo"])).is_err());
    }

    #[test]
    fn reflect_hook_parses_with_no_args() {
        assert_eq!(
            parse(&args(&["reflect-hook"])).unwrap(),
            Some(Command::ReflectHook)
        );
    }

    #[test]
    fn reflect_hook_rejects_flags_and_positionals() {
        assert!(parse(&args(&["reflect-hook", "--user"])).is_err());
        assert!(parse(&args(&["reflect-hook", "extra"])).is_err());
    }

    #[test]
    fn ingest_event_parses_bare_only() {
        assert_eq!(
            parse(&args(&["ingest-event"])).unwrap(),
            Some(Command::IngestEvent)
        );
        assert!(parse(&args(&["ingest-event", "extra"])).is_err());
    }

    #[test]
    fn search_joins_positionals_and_takes_limit() {
        assert_eq!(
            parse(&args(&["search", "login", "flow"])).unwrap(),
            Some(Command::Search {
                query: "login flow".to_string(),
                limit: 20,
            })
        );
        assert_eq!(
            parse(&args(&["search", "x", "--limit", "5"])).unwrap(),
            Some(Command::Search {
                query: "x".to_string(),
                limit: 5,
            })
        );
        assert!(parse(&args(&["search"])).is_err());
        assert!(parse(&args(&["search", "x", "--limit", "nope"])).is_err());
        assert!(parse(&args(&["search", "x", "--user"])).is_err());
    }

    #[test]
    fn unknown_subcommand_and_flag_error() {
        assert!(parse(&args(&["frobnicate"])).is_err());
        assert!(parse(&args(&["add", "--bogus", "x"])).is_err());
    }
}
