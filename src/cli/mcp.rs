//! `shep mcp` — the running session as capability-bounded tools over stdio.
//!
//! Three verbs. `serve` (the default) speaks MCP on stdin and stdout until the
//! client hangs up. `config` prints (or writes) the `mcpServers` block that
//! points a client at this executable. `tools` prints what a profile would
//! serve, which is the honest way to answer "what can this thing do".
//!
//! The profile is the whole security model: a tool outside it is absent from
//! `tools/list` and refused on call, and a tool inside it can be narrower than
//! its name suggests (capture-only `docket_add`, always-queued `agent_send`).
//! `[plugins.overseer] tools = [...]` replaces the `overseer` set from the one
//! config file a person already edits; `--allow` and `--deny` apply after it.

use std::path::PathBuf;

mod backend;
mod protocol;
mod tools;

use backend::SocketBackend;
use protocol::McpServer;
use tools::Profile;

const USAGE: &str = "\
usage: shep mcp [serve] [--socket PATH] [--profile NAME] [--allow a,b] [--deny a,b] [--tools a,b]
       shep mcp config [--profile NAME] [--output PATH]
       shep mcp tools [--profile NAME] [--allow a,b] [--deny a,b] [--tools a,b]

profiles: overseer (read + paging), read, all";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Serve,
    Config,
    Tools,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Options {
    verb: Verb,
    socket: Option<PathBuf>,
    profile: String,
    allow: Vec<String>,
    deny: Vec<String>,
    tools: Option<Vec<String>>,
    output: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            verb: Verb::Serve,
            socket: None,
            profile: "overseer".to_string(),
            allow: Vec::new(),
            deny: Vec::new(),
            tools: None,
            output: None,
        }
    }
}

pub(super) fn run_mcp_command(args: &[String]) -> std::io::Result<i32> {
    if args
        .iter()
        .any(|arg| matches!(arg.as_str(), "help" | "--help" | "-h"))
    {
        println!("{USAGE}");
        return Ok(0);
    }

    let options = match parse_args(args) {
        Ok(options) => options,
        Err(err) => {
            eprintln!("error: {err}");
            eprintln!("{USAGE}");
            return Ok(2);
        }
    };

    match options.verb {
        Verb::Config => run_config(&options),
        Verb::Tools | Verb::Serve => {
            let profile = match resolve_profile(&options) {
                Ok(profile) => profile,
                Err(err) => {
                    eprintln!("error: {err}");
                    return Ok(2);
                }
            };
            let socket = options
                .socket
                .clone()
                .unwrap_or_else(crate::api::socket_path);
            if options.verb == Verb::Tools {
                // Built the same way `serve` builds it, so this prints exactly
                // what a client would be offered.
                let backend = SocketBackend::new(socket);
                for name in McpServer::new(profile, &backend).tool_names() {
                    println!("{name}");
                }
                return Ok(0);
            }
            serve(profile, socket)
        }
    }
}

fn parse_args(args: &[String]) -> Result<Options, String> {
    let mut options = Options::default();
    let mut index = 0;
    if let Some(first) = args.first() {
        if !first.starts_with('-') {
            options.verb = match first.as_str() {
                "serve" => Verb::Serve,
                "config" => Verb::Config,
                "tools" => Verb::Tools,
                other => return Err(format!("unknown mcp subcommand: {other}")),
            };
            index = 1;
        }
    }

    while index < args.len() {
        let flag = args[index].as_str();
        let value = |name: &str| -> Result<String, String> {
            args.get(index + 1)
                .cloned()
                .ok_or_else(|| format!("missing value for --{name}"))
        };
        match flag {
            "--socket" => {
                options.socket = Some(PathBuf::from(value("socket")?));
                index += 2;
            }
            "--profile" => {
                options.profile = value("profile")?;
                index += 2;
            }
            "--allow" => {
                options.allow.extend(split_list(&value("allow")?));
                index += 2;
            }
            "--deny" => {
                options.deny.extend(split_list(&value("deny")?));
                index += 2;
            }
            "--tools" => {
                options
                    .tools
                    .get_or_insert_with(Vec::new)
                    .extend(split_list(&value("tools")?));
                index += 2;
            }
            "--output" => {
                options.output = Some(PathBuf::from(value("output")?));
                index += 2;
            }
            other => return Err(format!("unknown option: {other}")),
        }
    }

    Ok(options)
}

fn split_list(raw: &str) -> Vec<String> {
    raw.split(',')
        .map(str::trim)
        .filter(|entry| !entry.is_empty())
        .map(str::to_string)
        .collect()
}

/// `--tools` replaces everything; otherwise `[plugins.overseer] tools`
/// replaces the built-in `overseer` set; then `--allow` and `--deny`.
fn resolve_profile(options: &Options) -> Result<Profile, String> {
    let mut profile = match &options.tools {
        Some(names) => {
            let mut profile = Profile::empty();
            profile.allow(names)?;
            profile
        }
        None => match configured_profile(&options.profile)? {
            Some(profile) => profile,
            None => Profile::named(&options.profile).ok_or_else(|| {
                format!("unknown profile `{}` (overseer|read|all)", options.profile)
            })?,
        },
    };
    profile.allow(&options.allow)?;
    profile.deny(&options.deny)?;
    Ok(profile)
}

fn configured_profile(profile: &str) -> Result<Option<Profile>, String> {
    if profile != "overseer" {
        return Ok(None);
    }
    let config = crate::config::Config::load().config;
    let Some(table) = config.plugins.get("overseer") else {
        return Ok(None);
    };
    Profile::from_config(table).transpose()
}

fn run_config(options: &Options) -> std::io::Result<i32> {
    match &options.output {
        Some(path) => {
            crate::mcp_config::write_client_config(path, &options.profile)?;
            eprintln!("wrote {}", path.display());
        }
        None => {
            let value = crate::mcp_config::client_config(&options.profile);
            println!("{}", serde_json::to_string_pretty(&value)?);
        }
    }
    Ok(0)
}

fn serve(profile: Profile, socket: PathBuf) -> std::io::Result<i32> {
    // File-only, and before anything else: from here on stdout belongs to the
    // protocol.
    crate::logging::init_file_logging("shep-mcp.log");
    let backend = SocketBackend::new(socket);
    tracing::info!(
        event = "mcp.serve",
        subsystem = "mcp",
        outcome = "started",
        socket = %backend.socket().display(),
        tools = profile.permitted().len(),
        "shep mcp serving"
    );
    let mut server = McpServer::new(profile, &backend);
    let stdin = std::io::stdin();
    let stdout = std::io::stdout();
    protocol::serve(stdin.lock(), stdout.lock(), &mut server)?;
    Ok(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn mcp_args_default_to_serving_the_overseer_profile() {
        let options = parse_args(&[]).unwrap();
        assert_eq!(options.verb, Verb::Serve);
        assert_eq!(options.profile, "overseer");
        assert!(options.socket.is_none());
    }

    #[test]
    fn mcp_args_parse_every_verb_and_flag() {
        let options = parse_args(&args(&[
            "tools",
            "--profile",
            "all",
            "--socket",
            "/tmp/s.sock",
            "--allow",
            "push, seen",
            "--deny",
            "agent_send",
            "--tools",
            "read",
        ]))
        .unwrap();
        assert_eq!(options.verb, Verb::Tools);
        assert_eq!(options.profile, "all");
        assert_eq!(options.socket, Some(PathBuf::from("/tmp/s.sock")));
        assert_eq!(options.allow, vec!["push", "seen"]);
        assert_eq!(options.deny, vec!["agent_send"]);
        assert_eq!(options.tools, Some(vec!["read".to_string()]));

        assert!(parse_args(&args(&["nope"])).is_err());
        assert!(parse_args(&args(&["--wat"])).is_err());
        assert!(parse_args(&args(&["--profile"])).is_err());
    }

    #[test]
    fn mcp_explicit_tools_replace_the_profile_and_flags_apply_after() {
        let options = Options {
            tools: Some(args(&["docket"])),
            allow: args(&["doctor"]),
            deny: args(&["docket_discard"]),
            ..Options::default()
        };
        let profile = resolve_profile(&options).unwrap();
        let names: Vec<&str> = profile.permitted().iter().map(|tool| tool.name).collect();
        assert!(names.contains(&"docket_promote"));
        assert!(names.contains(&"doctor"));
        assert!(!names.contains(&"docket_discard"));
        assert!(!names.contains(&"session_overview"));
    }

    #[test]
    fn mcp_unknown_profile_is_refused() {
        let options = Options {
            profile: "wide-open".to_string(),
            ..Options::default()
        };
        let err = resolve_profile(&options).unwrap_err();
        assert!(err.contains("unknown profile"), "{err}");
    }
}
