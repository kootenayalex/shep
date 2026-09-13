//! `shep runtime` — the runtime launch registry from the command line.
//! `list` asks the running server (`runtime.list`) because launchability is
//! judged against the server's `PATH`, the one that will actually spawn the
//! agent. `ask` runs locally: it resolves the runtime's `[headless]` recipe,
//! hands it the prompt on stdin (or as an argument) and streams the answer
//! back, so a plugin or a script can consult a CLI without a pane.
//!
//! `--mcp-profile NAME` writes the MCP client config for that profile under
//! the state dir and hands the runtime the recipe's `mcp_config_args`, so the
//! question can be answered with shep's own tools in reach. `ask` still never
//! passes session arguments: one question, its own conversation.

use std::time::Duration;

use crate::api::schema::{EmptyParams, Method, Request, RuntimeInfo};

const LIST_USAGE: &str = "usage: shep runtime list [--json]";
const ASK_USAGE: &str = "usage: shep runtime ask <name> <prompt|-> [--timeout SECS] [--cwd PATH] [--mcp-profile NAME]  (- reads the prompt from stdin)";
const DEFAULT_ASK_TIMEOUT_SECS: u64 = 120;
/// Exit status when the runtime is killed for exceeding `--timeout`; the
/// same code `timeout(1)` uses.
const TIMED_OUT_EXIT_CODE: i32 = 124;

pub(super) fn run_runtime_command(args: &[String]) -> std::io::Result<i32> {
    let Some(subcommand) = args.first().map(String::as_str) else {
        print_runtime_help();
        return Ok(2);
    };
    match subcommand {
        "list" | "ls" => runtime_list(&args[1..]),
        "ask" => runtime_ask(&args[1..]),
        "help" | "--help" | "-h" => {
            print_runtime_help();
            Ok(0)
        }
        other => {
            eprintln!("unknown runtime subcommand: {other}");
            print_runtime_help();
            Ok(2)
        }
    }
}

fn runtime_list(args: &[String]) -> std::io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        _ => {
            eprintln!("{LIST_USAGE}");
            return Ok(2);
        }
    };
    let response = super::send_request(&Request {
        id: "cli:runtime:list".into(),
        method: Method::RuntimeList(EmptyParams::default()),
    })?;
    if json || response.get("error").is_some() {
        return super::print_response(&response);
    }
    let runtimes: Vec<RuntimeInfo> = serde_json::from_value(response["result"]["runtimes"].clone())
        .map_err(|err| std::io::Error::other(format!("unexpected runtime.list response: {err}")))?;
    for line in format_runtime_lines(&runtimes) {
        println!("{line}");
    }
    Ok(0)
}

/// One line per runtime: `name  launchable|-  headless|-  <bin>`, columns
/// padded to the longest name so the eye can scan the flags.
fn format_runtime_lines(runtimes: &[RuntimeInfo]) -> Vec<String> {
    let width = runtimes
        .iter()
        .map(|runtime| runtime.name.len())
        .max()
        .unwrap_or(0);
    runtimes
        .iter()
        .map(|runtime| {
            let mut line = format!(
                "{:width$}  {:<10}  {:<8}",
                runtime.name,
                if runtime.launchable {
                    "launchable"
                } else {
                    "-"
                },
                if runtime.headless { "headless" } else { "-" },
                width = width
            );
            if let Some(bin) = &runtime.bin_resolved {
                line.push_str("  ");
                line.push_str(bin);
            }
            line.trim_end().to_string()
        })
        .collect()
}

#[derive(Debug, PartialEq, Eq)]
struct AskArgs {
    name: String,
    prompt: PromptSource,
    timeout: Duration,
    cwd: Option<String>,
    /// The `shep mcp` profile to hand the runtime, if any.
    mcp_profile: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
enum PromptSource {
    Given(String),
    Stdin,
}

fn parse_ask_args(args: &[String]) -> Result<AskArgs, String> {
    let mut positionals = Vec::new();
    let mut timeout = Duration::from_secs(DEFAULT_ASK_TIMEOUT_SECS);
    let mut cwd = None;
    let mut mcp_profile = None;
    let mut index = 0;
    while index < args.len() {
        match args[index].as_str() {
            "--timeout" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --timeout".to_string())?;
                let secs = value
                    .parse::<u64>()
                    .ok()
                    .filter(|secs| *secs > 0)
                    .ok_or_else(|| format!("invalid --timeout {value} (whole seconds, > 0)"))?;
                timeout = Duration::from_secs(secs);
                index += 2;
            }
            "--cwd" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --cwd".to_string())?;
                cwd = Some(value.clone());
                index += 2;
            }
            "--mcp-profile" => {
                let value = args
                    .get(index + 1)
                    .ok_or_else(|| "missing value for --mcp-profile".to_string())?;
                if value.trim().is_empty() {
                    return Err("--mcp-profile needs a profile name".to_string());
                }
                mcp_profile = Some(value.trim().to_string());
                index += 2;
            }
            other if other.starts_with("--") => return Err(format!("unknown option: {other}")),
            other => {
                positionals.push(other.to_string());
                index += 1;
            }
        }
    }
    let [name, prompt] = positionals.as_slice() else {
        return Err(ASK_USAGE.to_string());
    };
    if name.trim().is_empty() {
        return Err("runtime name must not be empty".to_string());
    }
    let prompt = if prompt == "-" {
        PromptSource::Stdin
    } else {
        PromptSource::Given(prompt.clone())
    };
    Ok(AskArgs {
        name: name.clone(),
        prompt,
        timeout,
        cwd,
        mcp_profile,
    })
}

/// Where `--mcp-profile NAME` writes its client config: one file per profile
/// under the state dir, so repeated asks reuse it and two profiles never
/// overwrite each other.
fn mcp_config_path(profile: &str) -> std::path::PathBuf {
    crate::config::state_dir()
        .join("mcp")
        .join(format!("{profile}.json"))
}

fn runtime_ask(args: &[String]) -> std::io::Result<i32> {
    let parsed = match parse_ask_args(args) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!("{err}");
            return Ok(2);
        }
    };
    let prompt = match parsed.prompt {
        PromptSource::Given(prompt) => prompt,
        PromptSource::Stdin => {
            let mut prompt = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut prompt)?;
            prompt
        }
    };
    let overrides = crate::config::Config::load().config.runtimes;
    let (spec, _source) = match crate::runtimes::resolve_headless(&parsed.name, &overrides) {
        Ok(resolved) => resolved,
        Err(err) => {
            eprintln!("{}: {err}", err.code());
            return Ok(2);
        }
    };
    let cwd = parsed.cwd.map(std::path::PathBuf::from);
    // The config is written whenever a profile is asked for, even when this
    // runtime's recipe cannot use it: the file is the honest record of what
    // was requested, and `headless_argv` splices nothing without a recipe.
    let mcp = match parsed.mcp_profile.as_deref() {
        Some(profile) => {
            let path = mcp_config_path(profile);
            if let Err(err) = crate::mcp_config::write_client_config(&path, profile) {
                eprintln!("could not write {}: {err}", path.display());
                return Ok(1);
            }
            if spec.mcp_config_args.is_none() {
                eprintln!(
                    "runtime {} declares no mcp_config_args; --mcp-profile {profile} is ignored",
                    parsed.name
                );
            }
            Some(path)
        }
        None => None,
    };
    let outcome = match crate::runtimes::run_headless(
        &spec,
        &prompt,
        parsed.timeout,
        cwd.as_deref(),
        mcp.as_deref(),
    ) {
        Ok(outcome) => outcome,
        Err(err) => {
            eprintln!("could not run {}: {err}", spec.argv.join(" "));
            return Ok(1);
        }
    };
    if outcome.timed_out {
        eprintln!(
            "runtime {} did not answer within {}s; killed",
            parsed.name,
            parsed.timeout.as_secs()
        );
        return Ok(TIMED_OUT_EXIT_CODE);
    }
    Ok(outcome.exit_code.unwrap_or(1))
}

fn print_runtime_help() {
    eprintln!("usage: shep runtime <command>");
    eprintln!();
    eprintln!("commands:");
    eprintln!("  list [--json]                       runtimes shep can name, and whether this server can launch or ask them");
    eprintln!("  ask <name> <prompt|-> [--timeout SECS] [--cwd PATH] [--mcp-profile NAME]");
    eprintln!("                                      one-shot question through the runtime's [headless] recipe (default timeout {DEFAULT_ASK_TIMEOUT_SECS}s)");
    eprintln!("                                      --mcp-profile hands the runtime `shep mcp` tools bounded to that profile");
    eprintln!();
    eprintln!("a [runtimes.<name>] table in config.toml (argv, env, headless_argv, headless_prompt, and the per-field session_new_args, session_resume_args, mcp_config_args, headless_session_new_args, headless_session_resume_args, headless_mcp_config_args) overrides the manifest.");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ask_args_take_name_prompt_and_options() {
        let parsed =
            parse_ask_args(&args(&["claude", "hi", "--timeout", "5", "--cwd", "/tmp"])).unwrap();
        assert_eq!(
            parsed,
            AskArgs {
                name: "claude".into(),
                prompt: PromptSource::Given("hi".into()),
                timeout: Duration::from_secs(5),
                cwd: Some("/tmp".into()),
                mcp_profile: None,
            }
        );
        let parsed =
            parse_ask_args(&args(&["claude", "hi", "--mcp-profile", " overseer "])).unwrap();
        assert_eq!(parsed.mcp_profile.as_deref(), Some("overseer"));
        assert!(mcp_config_path("overseer").ends_with("mcp/overseer.json"));
        let parsed = parse_ask_args(&args(&["claude", "-"])).unwrap();
        assert_eq!(parsed.prompt, PromptSource::Stdin);
        assert_eq!(
            parsed.timeout,
            Duration::from_secs(DEFAULT_ASK_TIMEOUT_SECS)
        );
    }

    #[test]
    fn ask_args_reject_bad_shapes() {
        assert!(parse_ask_args(&args(&["claude"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--timeout"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--timeout", "0"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--timeout", "soon"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--bogus"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--mcp-profile"])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "--mcp-profile", " "])).is_err());
        assert!(parse_ask_args(&args(&["claude", "hi", "extra"])).is_err());
    }

    #[test]
    fn list_lines_align_and_flag_each_runtime() {
        let lines = format_runtime_lines(&[
            RuntimeInfo {
                name: "claude".into(),
                launchable: true,
                bin_resolved: Some("/opt/bin/claude".into()),
                headless: true,
            },
            RuntimeInfo {
                name: "pi".into(),
                launchable: false,
                bin_resolved: None,
                headless: false,
            },
        ]);
        assert_eq!(
            lines,
            vec![
                "claude  launchable  headless  /opt/bin/claude",
                "pi      -           -",
            ]
        );
    }
}
