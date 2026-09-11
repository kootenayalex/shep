//! `shep docket` — the personal docket over the socket API. Every verb goes
//! through the running server (`docket.*`), which is the one owner of
//! `docket.db`; there is no local-file fallback because the docket being
//! served is the point — "the only way it fails is if the server is down".

use crate::api::schema::{
    DocketAddParams, DocketItem, DocketKind, DocketListParams, DocketPromoteParams, DocketRepeat,
    DocketStatus, DocketTarget, DocketUpdateParams, Method, Request,
};

pub(super) fn run_docket_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_docket_help();
        return Ok(2);
    };
    match subcommand {
        "add" => docket_add(&args[1..]),
        "list" | "ls" => docket_list(&args[1..]),
        "promote" => docket_promote(&args[1..]),
        "done" | "complete" => docket_target(&args[1..], "done", Method::DocketComplete),
        "discard" => docket_target(&args[1..], "discard", Method::DocketDiscard),
        "update" => docket_update(&args[1..]),
        "help" | "--help" | "-h" => {
            print_docket_help();
            Ok(0)
        }
        other => {
            eprintln!("unknown docket subcommand: {other}");
            print_docket_help();
            Ok(2)
        }
    }
}

/// Parsed `--flag value` pairs plus positionals, with the one `--json` switch.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    positionals: Vec<String>,
    kind: Option<DocketKind>,
    status: Option<DocketStatus>,
    due: Option<String>,
    repeat: Option<DocketRepeat>,
    notes: Option<String>,
    title: Option<String>,
    source: Option<serde_json::Value>,
    json: bool,
}

fn parse_args(args: &[String]) -> Result<Parsed, String> {
    let mut parsed = Parsed::default();
    let mut index = 0;
    while index < args.len() {
        let flag = args[index].as_str();
        let mut take_value = |name: &str| -> Result<String, String> {
            let value = args
                .get(index + 1)
                .ok_or_else(|| format!("missing value for --{name}"))?;
            index += 2;
            Ok(value.clone())
        };
        match flag {
            "--kind" => {
                let value = take_value("kind")?;
                parsed.kind = Some(DocketKind::parse(&value).ok_or_else(|| {
                    format!("invalid --kind {value} (captured|slated|recurring)")
                })?);
            }
            "--status" => {
                let value = take_value("status")?;
                parsed.status = Some(DocketStatus::parse(&value).ok_or_else(|| {
                    format!("invalid --status {value} (inbox|open|done|discarded)")
                })?);
            }
            "--due" => {
                let value = take_value("due")?;
                if crate::docket::dates::Date::parse(&value).is_none() {
                    return Err(format!("invalid --due {value} (YYYY-MM-DD)"));
                }
                parsed.due = Some(value);
            }
            "--repeat" => {
                let value = take_value("repeat")?;
                parsed.repeat = Some(
                    DocketRepeat::parse(&value)
                        .ok_or_else(|| format!("invalid --repeat {value} (1d|1w|2w|1m)"))?,
                );
            }
            "--notes" => parsed.notes = Some(take_value("notes")?),
            "--title" => parsed.title = Some(take_value("title")?),
            "--source" => {
                let value = take_value("source")?;
                parsed.source = Some(
                    serde_json::from_str(&value)
                        .map_err(|err| format!("invalid --source (must be JSON): {err}"))?,
                );
            }
            "--json" => {
                parsed.json = true;
                index += 1;
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {other}")),
            positional => {
                parsed.positionals.push(positional.to_string());
                index += 1;
            }
        }
    }
    Ok(parsed)
}

fn usage(line: &str) -> std::io::Result<i32> {
    eprintln!("usage: {line}");
    Ok(2)
}

fn parse_or_usage(args: &[String], line: &str) -> Result<Parsed, std::io::Result<i32>> {
    parse_args(args).map_err(|err| {
        eprintln!("{err}");
        usage(line)
    })
}

fn parse_id(raw: &str) -> Result<i64, String> {
    raw.trim_start_matches('#')
        .parse::<i64>()
        .map_err(|_| format!("invalid docket id: {raw}"))
}

const ADD_USAGE: &str = "shep docket add <title> [--kind captured|slated|recurring] [--due YYYY-MM-DD] [--repeat 1d|1w|2w|1m] [--notes TEXT] [--source JSON] [--json]";
const LIST_USAGE: &str = "shep docket list [--status inbox|open|done|discarded] [--json]";
const PROMOTE_USAGE: &str =
    "shep docket promote <id> <slated|recurring> [--due YYYY-MM-DD] [--repeat 1d|1w|2w|1m] [--json]";
const UPDATE_USAGE: &str = "shep docket update <id> [--title TEXT] [--due YYYY-MM-DD] [--repeat 1d|1w|2w|1m] [--notes TEXT] [--kind captured|slated|recurring] [--json]";

fn docket_add(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, ADD_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    let [title] = parsed.positionals.as_slice() else {
        return usage(ADD_USAGE);
    };
    if title.trim().is_empty() {
        return usage(ADD_USAGE);
    }
    item_request(
        "cli:docket:add",
        Method::DocketAdd(DocketAddParams {
            title: title.clone(),
            kind: parsed.kind,
            status: parsed.status,
            due: parsed.due,
            repeat: parsed.repeat,
            source: parsed.source,
            notes: parsed.notes,
        }),
        parsed.json,
    )
}

fn docket_list(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, LIST_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    if !parsed.positionals.is_empty() {
        return usage(LIST_USAGE);
    }
    let response = super::send_request(&Request {
        id: "cli:docket:list".into(),
        method: Method::DocketList(DocketListParams {
            status: parsed.status,
        }),
    })?;
    if parsed.json || response.get("error").is_some() {
        return super::print_response(&response);
    }
    let items: Vec<DocketItem> = serde_json::from_value(response["result"]["items"].clone())
        .map_err(|err| std::io::Error::other(format!("unexpected docket.list response: {err}")))?;
    if items.is_empty() {
        println!("docket is empty");
        return Ok(0);
    }
    for item in &items {
        println!("{}", format_item_line(item));
    }
    Ok(0)
}

fn docket_promote(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, PROMOTE_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    let [id, kind] = parsed.positionals.as_slice() else {
        return usage(PROMOTE_USAGE);
    };
    let id = match parse_id(id) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("{err}");
            return Ok(2);
        }
    };
    let Some(kind) = DocketKind::parse(kind) else {
        eprintln!("invalid kind {kind} (slated|recurring)");
        return Ok(2);
    };
    item_request(
        "cli:docket:promote",
        Method::DocketPromote(DocketPromoteParams {
            id,
            kind,
            due: parsed.due,
            repeat: parsed.repeat,
        }),
        parsed.json,
    )
}

fn docket_target(
    args: &[String],
    verb: &str,
    method: fn(DocketTarget) -> Method,
) -> std::io::Result<i32> {
    let line = format!("shep docket {verb} <id> [--json]");
    let parsed = match parse_or_usage(args, &line) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    let [id] = parsed.positionals.as_slice() else {
        return usage(&line);
    };
    let id = match parse_id(id) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("{err}");
            return Ok(2);
        }
    };
    item_request(
        "cli:docket:target",
        method(DocketTarget { id }),
        parsed.json,
    )
}

fn docket_update(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, UPDATE_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    let [id] = parsed.positionals.as_slice() else {
        return usage(UPDATE_USAGE);
    };
    let id = match parse_id(id) {
        Ok(id) => id,
        Err(err) => {
            eprintln!("{err}");
            return Ok(2);
        }
    };
    if parsed.title.is_none()
        && parsed.due.is_none()
        && parsed.repeat.is_none()
        && parsed.notes.is_none()
        && parsed.kind.is_none()
    {
        eprintln!("nothing to update");
        return usage(UPDATE_USAGE);
    }
    item_request(
        "cli:docket:update",
        Method::DocketUpdate(DocketUpdateParams {
            id,
            title: parsed.title,
            notes: parsed.notes,
            due: parsed.due,
            repeat: parsed.repeat,
            kind: parsed.kind,
        }),
        parsed.json,
    )
}

/// Send a verb that answers with one item; print it as a line (or the raw
/// response with `--json`).
fn item_request(id: &'static str, method: Method, json: bool) -> std::io::Result<i32> {
    let response = super::send_request(&Request {
        id: id.into(),
        method,
    })?;
    if json || response.get("error").is_some() {
        return super::print_response(&response);
    }
    let item: DocketItem = serde_json::from_value(response["result"]["item"].clone())
        .map_err(|err| std::io::Error::other(format!("unexpected docket response: {err}")))?;
    println!("{}", format_item_line(&item));
    Ok(0)
}

/// `#<id> <status> <kind> <due or -> <title>`, with `!` in the gutter when
/// the item is overdue.
fn format_item_line(item: &DocketItem) -> String {
    let marker = if item.overdue { '!' } else { ' ' };
    let repeat = item
        .repeat
        .map(|repeat| format!("/{}", repeat.as_str()))
        .unwrap_or_default();
    format!(
        "{marker} #{:<3} {:<9} {:<9} {:<10}{} {}",
        item.id,
        item.status.as_str(),
        item.kind.as_str(),
        item.due.as_deref().unwrap_or("-"),
        repeat,
        item.title
    )
}

fn print_docket_help() {
    eprintln!("shep docket commands (need a running server):");
    eprintln!("  {ADD_USAGE}");
    eprintln!("  {LIST_USAGE}");
    eprintln!("  {PROMOTE_USAGE}");
    eprintln!("  shep docket done <id> [--json]        complete (a repeating item rolls forward)");
    eprintln!("  shep docket discard <id> [--json]");
    eprintln!("  {UPDATE_USAGE}");
    eprintln!();
    eprintln!("list marks overdue items with `!`; ids may be written as 3 or #3.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn parse_args_collects_flags_and_positionals() {
        let parsed = parse_args(&args(&[
            "rotate the key",
            "--kind",
            "recurring",
            "--due",
            "2026-09-20",
            "--repeat",
            "1m",
            "--notes",
            "billing wall-clock",
            "--source",
            r#"{"file":"MEMORY.md","line":9}"#,
            "--json",
        ]))
        .unwrap();
        assert_eq!(parsed.positionals, vec!["rotate the key".to_string()]);
        assert_eq!(parsed.kind, Some(DocketKind::Recurring));
        assert_eq!(parsed.due.as_deref(), Some("2026-09-20"));
        assert_eq!(parsed.repeat, Some(DocketRepeat::Monthly));
        assert_eq!(parsed.notes.as_deref(), Some("billing wall-clock"));
        assert_eq!(
            parsed.source,
            Some(serde_json::json!({"file":"MEMORY.md","line":9}))
        );
        assert!(parsed.json);
    }

    #[test]
    fn parse_args_rejects_bad_values_before_the_socket() {
        assert!(parse_args(&args(&["--due", "next tuesday"]))
            .unwrap_err()
            .contains("YYYY-MM-DD"));
        assert!(parse_args(&args(&["--repeat", "3w"]))
            .unwrap_err()
            .contains("1d|1w|2w|1m"));
        assert!(parse_args(&args(&["--kind", "urgent"]))
            .unwrap_err()
            .contains("captured|slated|recurring"));
        assert!(parse_args(&args(&["--source", "not json"]))
            .unwrap_err()
            .contains("JSON"));
        assert!(parse_args(&args(&["--notes"]))
            .unwrap_err()
            .contains("missing value"));
        assert!(parse_args(&args(&["--bogus"]))
            .unwrap_err()
            .contains("unknown option"));
    }

    #[test]
    fn ids_accept_a_leading_hash() {
        assert_eq!(parse_id("3"), Ok(3));
        assert_eq!(parse_id("#12"), Ok(12));
        assert!(parse_id("three").is_err());
    }

    #[test]
    fn item_line_marks_overdue_and_repeat() {
        let mut item = DocketItem {
            id: 3,
            title: "check the backups".into(),
            kind: DocketKind::Recurring,
            status: DocketStatus::Open,
            due: Some("2026-09-01".into()),
            repeat: Some(DocketRepeat::Weekly),
            source: None,
            notes: None,
            created: String::new(),
            updated: String::new(),
            last_fired: None,
            overdue: true,
        };
        assert_eq!(
            format_item_line(&item),
            "! #3   open      recurring 2026-09-01/1w check the backups"
        );
        item.overdue = false;
        item.due = None;
        item.repeat = None;
        item.status = DocketStatus::Inbox;
        item.kind = DocketKind::Captured;
        assert_eq!(
            format_item_line(&item),
            "  #3   inbox     captured  -          check the backups"
        );
    }
}
