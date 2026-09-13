//! `shep overseer` — the overseer's read of the room from a terminal. Every
//! verb goes through the running server (`overseer.*`), which is the one
//! process that holds the plugin's state dir; there is no local-file
//! fallback, for the same reason `shep docket` has none.
//!
//! `sample` prints the board's left-hand facts as one compact block, `chat`
//! puts a question to the overseer's headless runtime (with `--wait` it
//! subscribes to `overseer.chat_turn` *before* sending, so the answer cannot
//! be missed), and `tick` asks for a fresh situation when the last has aged
//! out. The docket is deliberately absent: that is `shep docket list`.

use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::api::client::{ApiClient, ApiClientError};
use crate::api::schema::{
    EventsSubscribeParams, Method, OverseerChatParams, OverseerChatRole, OverseerChatTurn,
    OverseerHealthLevel, OverseerSample, OverseerSampleParams, OverseerTickParams, Request,
    Subscription,
};

/// Matches the server's `CHAT_TIMEOUT` — waiting longer than the runtime is
/// given to answer only hides the failure.
const DEFAULT_CHAT_WAIT_SECS: u64 = 120;
/// What `overseer.sample` returns when `--chat` is omitted.
const DEFAULT_CHAT_TURNS: u32 = 20;
/// The server keeps this many turns; asking for more is a typo, not a wish.
const MAX_CHAT_TURNS: u32 = 200;

const SAMPLE_USAGE: &str = "shep overseer sample [--chat N] [--json]";
const CHAT_USAGE: &str = "shep overseer chat <text|-> [--wait [--timeout SECS]] [--json]";
const TICK_USAGE: &str = "shep overseer tick [--max-age SECS] [--json]";

pub(super) fn run_overseer_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_overseer_help();
        return Ok(2);
    };
    match subcommand {
        "sample" | "board" => overseer_sample(&args[1..]),
        "chat" | "ask" => overseer_chat(&args[1..]),
        "tick" => overseer_tick(&args[1..]),
        "help" | "--help" | "-h" => {
            print_overseer_help();
            Ok(0)
        }
        other => {
            eprintln!("unknown overseer subcommand: {other}");
            print_overseer_help();
            Ok(2)
        }
    }
}

/// Parsed `--flag value` pairs plus positionals, across all three verbs.
#[derive(Debug, Default, PartialEq, Eq)]
struct Parsed {
    positionals: Vec<String>,
    chat_turns: Option<u32>,
    max_age: Option<u64>,
    timeout: Option<u64>,
    wait: bool,
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
            "--chat" => {
                let value = take_value("chat")?;
                let turns = value.parse::<u32>().ok().filter(|n| *n <= MAX_CHAT_TURNS);
                parsed.chat_turns = Some(turns.ok_or_else(|| {
                    format!("invalid --chat {value} (whole turns, 0..={MAX_CHAT_TURNS})")
                })?);
            }
            "--max-age" => {
                let value = take_value("max-age")?;
                parsed.max_age = Some(value.parse::<u64>().map_err(|_| {
                    format!("invalid --max-age {value} (whole seconds, 0 forces a tick)")
                })?);
            }
            "--timeout" => {
                let value = take_value("timeout")?;
                let secs = value.parse::<u64>().ok().filter(|secs| *secs > 0);
                parsed.timeout =
                    Some(secs.ok_or_else(|| {
                        format!("invalid --timeout {value} (whole seconds, > 0)")
                    })?);
            }
            "--wait" => {
                parsed.wait = true;
                index += 1;
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

/// `--json` prints the `result` object alone: the envelope's `id` is ours,
/// not news. An error is still printed whole, so the code travels with it.
fn print_result(response: &serde_json::Value, json: bool) -> Option<std::io::Result<i32>> {
    if response.get("error").is_some() {
        eprintln!("{}", serde_json::to_string(response).unwrap());
        return Some(Ok(1));
    }
    if json {
        println!("{}", serde_json::to_string(&response["result"]).unwrap());
        return Some(Ok(0));
    }
    None
}

fn overseer_sample(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, SAMPLE_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    if !parsed.positionals.is_empty() || parsed.wait {
        return usage(SAMPLE_USAGE);
    }
    let response = super::send_request(&Request {
        id: "cli:overseer:sample".into(),
        method: Method::OverseerSample(OverseerSampleParams {
            chat_turns: parsed.chat_turns,
        }),
    })?;
    if let Some(exit) = print_result(&response, parsed.json) {
        return exit;
    }
    let sample: OverseerSample = serde_json::from_value(response["result"]["sample"].clone())
        .map_err(|err| {
            std::io::Error::other(format!("unexpected overseer.sample response: {err}"))
        })?;
    print!("{}", render_sample(&sample, unix_now()));
    Ok(0)
}

fn overseer_chat(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, CHAT_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    if parsed.chat_turns.is_some() || parsed.max_age.is_some() {
        return usage(CHAT_USAGE);
    }
    if parsed.timeout.is_some() && !parsed.wait {
        eprintln!("--timeout only means something with --wait");
        return usage(CHAT_USAGE);
    }
    let [text] = parsed.positionals.as_slice() else {
        return usage(CHAT_USAGE);
    };
    let text = if text == "-" {
        let mut question = String::new();
        std::io::Read::read_to_string(&mut std::io::stdin(), &mut question)?;
        question
    } else {
        text.clone()
    };
    if text.trim().is_empty() {
        eprintln!("empty question");
        return usage(CHAT_USAGE);
    }
    let request = Request {
        id: "cli:overseer:chat".into(),
        method: Method::OverseerChat(OverseerChatParams { text }),
    };
    if parsed.wait {
        let timeout = Duration::from_secs(parsed.timeout.unwrap_or(DEFAULT_CHAT_WAIT_SECS));
        return chat_and_wait(request, timeout, parsed.json);
    }
    let response = super::send_request(&request)?;
    if let Some(exit) = print_result(&response, parsed.json) {
        return exit;
    }
    let turn: OverseerChatTurn = serde_json::from_value(response["result"]["turn"].clone())
        .map_err(|err| {
            std::io::Error::other(format!("unexpected overseer.chat response: {err}"))
        })?;
    println!("{}", format_turn(&turn, unix_now()));
    Ok(0)
}

/// Subscribe first, then ask: an answer that lands between the two would
/// otherwise be an event nobody was listening for.
fn chat_and_wait(request: Request, timeout: Duration, json: bool) -> std::io::Result<i32> {
    let subscribe = Request {
        id: "cli:overseer:chat:subscribe".into(),
        method: Method::EventsSubscribe(EventsSubscribeParams {
            subscriptions: vec![Subscription::OverseerChatTurn {}],
        }),
    };
    let client = ApiClient::local();
    let (ack, mut stream) = client
        .subscribe_value(&subscribe, Some(timeout))
        .map_err(api_error_to_io)?;
    if let Err(err) = crate::api::client::parse_response_value(ack) {
        if let ApiClientError::ErrorResponse(response) = err {
            eprintln!("{}", serde_json::to_string(&response).unwrap());
            return Ok(1);
        }
        return Err(api_error_to_io(err));
    }

    let response = super::send_request(&request)?;
    if response.get("error").is_some() {
        eprintln!("{}", serde_json::to_string(&response).unwrap());
        return Ok(1);
    }

    let deadline = Instant::now() + timeout;
    loop {
        if Instant::now() >= deadline {
            eprintln!("the overseer did not answer within {}s", timeout.as_secs());
            return Ok(1);
        }
        match stream.next_value() {
            Ok(None) => {
                eprintln!("subscription closed before the answer arrived");
                return Ok(1);
            }
            Ok(Some(event)) => {
                let Some(turn) = overseer_turn(&event) else {
                    continue;
                };
                if json {
                    println!("{}", serde_json::to_string(&event).unwrap());
                } else {
                    println!("{}", turn.text);
                }
                return Ok(0);
            }
            Err(ApiClientError::Io(err)) if timed_out(&err) => {
                eprintln!("the overseer did not answer within {}s", timeout.as_secs());
                return Ok(1);
            }
            Err(err) => return Err(api_error_to_io(err)),
        }
    }
}

/// The `overseer`-role turn inside one subscription event, when that is what
/// this event is. Our own `you` turn comes back first and is not the answer.
fn overseer_turn(event: &serde_json::Value) -> Option<OverseerChatTurn> {
    if event["event"] != "overseer_chat_turn" {
        return None;
    }
    let turn: OverseerChatTurn = serde_json::from_value(event["data"]["turn"].clone()).ok()?;
    (turn.role == OverseerChatRole::Overseer).then_some(turn)
}

fn overseer_tick(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_or_usage(args, TICK_USAGE) {
        Ok(parsed) => parsed,
        Err(exit) => return exit,
    };
    if !parsed.positionals.is_empty() || parsed.wait || parsed.chat_turns.is_some() {
        return usage(TICK_USAGE);
    }
    let response = super::send_request(&Request {
        id: "cli:overseer:tick".into(),
        method: Method::OverseerTick(OverseerTickParams {
            max_age_seconds: parsed.max_age,
        }),
    })?;
    if let Some(exit) = print_result(&response, parsed.json) {
        return exit;
    }
    let result = &response["result"];
    println!(
        "{}",
        format_tick(
            result["invoked"].as_bool().unwrap_or(false),
            result["in_flight"].as_bool().unwrap_or(false),
            result["situation_age_seconds"].as_u64(),
            result["log_id"].as_str(),
        )
    );
    Ok(0)
}

fn format_tick(
    invoked: bool,
    in_flight: bool,
    situation_age_seconds: Option<u64>,
    log_id: Option<&str>,
) -> String {
    match (invoked, in_flight) {
        (true, _) => match log_id {
            Some(log_id) => format!("tick invoked (log {log_id})"),
            None => "tick invoked".to_string(),
        },
        (false, true) => "tick already in flight".to_string(),
        (false, false) => match situation_age_seconds {
            Some(age) => format!("tick skipped: situation is {age}s old"),
            None => "tick skipped".to_string(),
        },
    }
}

/// The board's left-hand column as a block of lines: the read of the room,
/// where the tick and the brain stand, health, the chat tail, the session.
/// The docket is not here — that is `shep docket list`.
fn render_sample(sample: &OverseerSample, now: u64) -> String {
    let mut out = String::new();
    if !sample.plugin_linked {
        out.push_str("plugin not linked\n");
    }
    if sample.narrative.is_empty() {
        out.push_str("✦ nothing read yet\n");
    } else {
        out.push_str("✦ read of the room\n");
        for line in &sample.narrative {
            out.push_str(&format!("  {line}\n"));
        }
    }
    out.push_str(&format!("{}\n", format_status(sample)));
    for finding in &sample.health {
        let glyph = match finding.level {
            OverseerHealthLevel::Ok => '✓',
            OverseerHealthLevel::Warn => '⚠',
            OverseerHealthLevel::Fail => '◉',
        };
        let detail = finding.detail.trim();
        if detail.is_empty() {
            out.push_str(&format!("  {glyph} {}\n", finding.check));
        } else {
            out.push_str(&format!("  {glyph} {} {detail}\n", finding.check));
        }
    }
    for turn in &sample.chat {
        out.push_str(&format!("  {}\n", format_turn(turn, now)));
    }
    if sample.chat_pending {
        out.push_str("  ✦   waiting for an answer\n");
    }
    out.push_str(&format!("{}\n", format_session(sample)));
    out
}

/// `tick hh:mm · brain 4m ago · claude headless`, with whatever is missing
/// said plainly rather than dropped.
fn format_status(sample: &OverseerSample) -> String {
    let mut parts = vec![
        match sample.tick_at.as_deref() {
            Some(at) => format!("tick {at}"),
            None => "tick never".to_string(),
        },
        match sample.brain_age_seconds {
            Some(age) => format!("brain {} ago", format_age(age)),
            None => "brain never".to_string(),
        },
        match sample.runtime.as_deref() {
            Some(runtime) => format!("{runtime} headless"),
            None => "no runtime".to_string(),
        },
    ];
    if sample.tick_in_flight {
        parts.push("tick in flight".to_string());
    }
    parts.join(" · ")
}

fn format_session(sample: &OverseerSample) -> String {
    match sample.session.id.as_deref() {
        Some(id) if sample.session.started => format!("session {id} (started)"),
        Some(id) => format!("session {id} (not started)"),
        None => "session not started".to_string(),
    }
}

/// `you  2m ago  what needs me?` — the overseer speaks as `✦`, as on the
/// board.
fn format_turn(turn: &OverseerChatTurn, now: u64) -> String {
    let who = match turn.role {
        OverseerChatRole::You => "you",
        OverseerChatRole::Overseer => "✦",
    };
    let age = format_age(now.saturating_sub(turn.at));
    format!("{who:<3} {age:>4} ago  {}", turn.text)
}

/// Ages are read at a glance, so one unit is enough.
fn format_age(seconds: u64) -> String {
    match seconds {
        0..=89 => format!("{seconds}s"),
        90..=5399 => format!("{}m", (seconds + 30) / 60),
        5400..=86399 => format!("{}h", (seconds + 1800) / 3600),
        _ => format!("{}d", (seconds + 43200) / 86400),
    }
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

fn timed_out(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock
    )
}

fn api_error_to_io(err: ApiClientError) -> std::io::Error {
    match err {
        ApiClientError::Io(err) => err,
        err => std::io::Error::other(err),
    }
}

fn print_overseer_help() {
    eprintln!("shep overseer commands (need a running server):");
    eprintln!("  {SAMPLE_USAGE}");
    eprintln!("                                        what the overseer last sensed and said (--chat N turns, default {DEFAULT_CHAT_TURNS})");
    eprintln!("  {CHAT_USAGE}");
    eprintln!("                                        one question to the overseer's headless runtime; `-` reads it from stdin,");
    eprintln!("                                        --wait blocks for the answer (default {DEFAULT_CHAT_WAIT_SECS}s)");
    eprintln!("  {TICK_USAGE}");
    eprintln!("                                        ask the plugin for a fresh situation (default --max-age 60, 0 forces)");
    eprintln!();
    eprintln!("the docket is not part of the sample: use `shep docket list`.");
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{OverseerHealthFinding, OverseerNarrativeSource, OverseerSessionInfo};

    fn args(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|part| part.to_string()).collect()
    }

    #[test]
    fn parse_args_collects_flags_and_positionals() {
        let parsed = parse_args(&args(&[
            "what needs me?",
            "--wait",
            "--timeout",
            "30",
            "--json",
        ]))
        .unwrap();
        assert_eq!(parsed.positionals, vec!["what needs me?".to_string()]);
        assert!(parsed.wait);
        assert_eq!(parsed.timeout, Some(30));
        assert!(parsed.json);
        let sample = parse_args(&args(&["--chat", "0"])).unwrap();
        assert_eq!(sample.chat_turns, Some(0));
        let tick = parse_args(&args(&["--max-age", "0"])).unwrap();
        assert_eq!(tick.max_age, Some(0));
    }

    #[test]
    fn parse_args_rejects_bad_values_before_the_socket() {
        assert!(parse_args(&args(&["--chat", "201"]))
            .unwrap_err()
            .contains("0..=200"));
        assert!(parse_args(&args(&["--chat", "lots"]))
            .unwrap_err()
            .contains("invalid --chat"));
        assert!(parse_args(&args(&["--timeout", "0"]))
            .unwrap_err()
            .contains("> 0"));
        assert!(parse_args(&args(&["--timeout", "a while"]))
            .unwrap_err()
            .contains("invalid --timeout"));
        assert!(parse_args(&args(&["--max-age", "-1"]))
            .unwrap_err()
            .contains("invalid --max-age"));
        assert!(parse_args(&args(&["--timeout"]))
            .unwrap_err()
            .contains("missing value"));
        assert!(parse_args(&args(&["--bogus"]))
            .unwrap_err()
            .contains("unknown option"));
    }

    #[test]
    fn a_lone_dash_is_a_positional_not_an_option() {
        let parsed = parse_args(&args(&["-", "--wait"])).unwrap();
        assert_eq!(parsed.positionals, vec!["-".to_string()]);
        assert!(parsed.wait);
    }

    #[test]
    fn ages_use_one_unit() {
        assert_eq!(format_age(0), "0s");
        assert_eq!(format_age(89), "89s");
        assert_eq!(format_age(90), "2m");
        assert_eq!(format_age(240), "4m");
        assert_eq!(format_age(5400), "2h");
        assert_eq!(format_age(86_400 * 3), "3d");
    }

    #[test]
    fn tick_says_which_of_the_three_things_happened() {
        assert_eq!(
            format_tick(true, true, None, Some("log-7")),
            "tick invoked (log log-7)"
        );
        assert_eq!(
            format_tick(false, true, Some(4), Some("log-7")),
            "tick already in flight"
        );
        assert_eq!(
            format_tick(false, false, Some(12), None),
            "tick skipped: situation is 12s old"
        );
    }

    fn sample() -> OverseerSample {
        OverseerSample {
            plugin_linked: true,
            sampled: true,
            narrative: vec!["two agents are blocked on you".into()],
            source: OverseerNarrativeSource::Brain,
            tick_at: Some("14:32".into()),
            situation_age_seconds: Some(12),
            brain_age_seconds: Some(240),
            runtime: Some("claude".into()),
            tick_in_flight: false,
            health: vec![
                OverseerHealthFinding {
                    level: OverseerHealthLevel::Ok,
                    check: "socket".into(),
                    detail: "listening".into(),
                    fix: None,
                },
                OverseerHealthFinding {
                    level: OverseerHealthLevel::Warn,
                    check: "disk".into(),
                    detail: "10 GiB free".into(),
                    fix: None,
                },
            ],
            chat: vec![OverseerChatTurn {
                at: 940,
                role: OverseerChatRole::You,
                text: "what needs me?".into(),
            }],
            chat_total: 1,
            chat_pending: true,
            session: OverseerSessionInfo {
                id: Some("2f1c".into()),
                started: true,
            },
        }
    }

    #[test]
    fn the_board_reads_top_to_bottom() {
        assert_eq!(
            render_sample(&sample(), 1000),
            "\
✦ read of the room
  two agents are blocked on you
tick 14:32 · brain 4m ago · claude headless
  ✓ socket listening
  ⚠ disk 10 GiB free
  you  60s ago  what needs me?
  ✦   waiting for an answer
session 2f1c (started)
"
        );
    }

    #[test]
    fn an_empty_overseer_says_so_plainly() {
        let empty = OverseerSample {
            sampled: true,
            ..Default::default()
        };
        assert_eq!(
            render_sample(&empty, 1000),
            "\
plugin not linked
✦ nothing read yet
tick never · brain never · no runtime
session not started
"
        );
    }

    #[test]
    fn only_the_overseer_role_ends_the_wait() {
        let you = serde_json::json!({
            "event": "overseer_chat_turn",
            "data": {"turn": {"at": 1, "role": "you", "text": "hi"}, "pending": true},
        });
        assert!(overseer_turn(&you).is_none());
        let other = serde_json::json!({"event": "overseer_updated", "data": {}});
        assert!(overseer_turn(&other).is_none());
        let answer = serde_json::json!({
            "event": "overseer_chat_turn",
            "data": {"turn": {"at": 2, "role": "overseer", "text": "nothing"}, "pending": false},
        });
        assert_eq!(overseer_turn(&answer).unwrap().text, "nothing");
    }
}
