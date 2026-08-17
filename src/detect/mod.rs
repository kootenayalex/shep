//! Agent state detection via terminal tail pattern matching.
//!
//! Each pane's live bottom-of-buffer text is read periodically and matched
//! against known agent output patterns to determine state.

pub mod manifest;
pub mod manifest_update;

/// The detected state of a terminal pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentState {
    /// Agent finished, prompt visible, nothing happening.
    Idle,
    /// Agent is actively working/processing.
    Working,
    /// Agent needs human input and is blocked on a response.
    Blocked,
    /// Plain shell or unrecognized program.
    Unknown,
}

/// Screen-derived agent state plus confidence metadata used for source arbitration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AgentDetection {
    pub state: AgentState,
    /// True when the current screen is an agent-owned viewer that shows
    /// transcript/history instead of the live prompt state.
    pub skip_state_update: bool,
    /// True when the current screen visibly shows live idle chrome.
    pub visible_idle: bool,
    /// True when the current screen visibly shows live UI chrome that needs
    /// human input. This is stronger than arbitrary prompt-like text in the
    /// scrollback and may override a non-blocked integration state.
    pub visible_blocker: bool,
    /// True when the current screen visibly shows live working chrome. PTY
    /// activity is the normal working authority; this remains diagnostic
    /// metadata and for non-PTY fallback paths.
    pub visible_working: bool,
}

/// Which agent we detected running in a pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Agent {
    Pi,
    Claude,
    Codex,
    Gemini,
    Cursor,
    Devin,
    Antigravity,
    Cline,
    Omp,
    Mastracode,
    OpenCode,
    GithubCopilot,
    Kimi,
    Kiro,
    Droid,
    Amp,
    Grok,
    Hermes,
    Kilo,
    Qodercli,
}

impl Agent {
    pub const SCREEN_MANIFEST_AGENTS: [Self; 18] = [
        Self::Pi,
        Self::Claude,
        Self::Codex,
        Self::Gemini,
        Self::Cursor,
        Self::Devin,
        Self::Antigravity,
        Self::Cline,
        Self::OpenCode,
        Self::GithubCopilot,
        Self::Kimi,
        Self::Kiro,
        Self::Droid,
        Self::Amp,
        Self::Grok,
        Self::Hermes,
        Self::Kilo,
        Self::Qodercli,
    ];
}

pub fn agent_label(agent: Agent) -> &'static str {
    match agent {
        Agent::Pi => "pi",
        Agent::Claude => "claude",
        Agent::Codex => "codex",
        Agent::Gemini => "gemini",
        Agent::Cursor => "cursor",
        Agent::Devin => "devin",
        Agent::Antigravity => "agy",
        Agent::Cline => "cline",
        Agent::Omp => "omp",
        Agent::Mastracode => "mastracode",
        Agent::OpenCode => "opencode",
        Agent::GithubCopilot => "copilot",
        Agent::Kimi => "kimi",
        Agent::Kiro => "kiro",
        Agent::Droid => "droid",
        Agent::Amp => "amp",
        Agent::Grok => "grok",
        Agent::Hermes => "hermes",
        Agent::Kilo => "kilo",
        Agent::Qodercli => "qodercli",
    }
}

pub fn parse_agent_label(agent: &str) -> Option<Agent> {
    let name = normalized_agent_lookup_name(agent);
    match name.as_str() {
        "pi" => Some(Agent::Pi),
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        "gemini" => Some(Agent::Gemini),
        "cursor" | "cursor-agent" => Some(Agent::Cursor),
        "devin" | "devin-cli" | "devin cli" => Some(Agent::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(Agent::Antigravity),
        "cline" => Some(Agent::Cline),
        "omp" => Some(Agent::Omp),
        "mastracode" | "mastra-code" | "mastra code" => Some(Agent::Mastracode),
        "opencode" | "open-code" => Some(Agent::OpenCode),
        "copilot" | "github-copilot" | "ghcs" => Some(Agent::GithubCopilot),
        "kimi" | "kimi-code" | "kimi code" => Some(Agent::Kimi),
        "kiro" | "kiro-cli" => Some(Agent::Kiro),
        "droid" => Some(Agent::Droid),
        "amp" | "amp-local" => Some(Agent::Amp),
        "grok" | "grok-build" => Some(Agent::Grok),
        "hermes" | "hermes-agent" => Some(Agent::Hermes),
        "kilo" | "kilo-code" | "kilo code" => Some(Agent::Kilo),
        "qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(Agent::Qodercli),
        _ => None,
    }
}

/// Identify which agent is running from the process name.
/// Returns `None` for plain shells or unrecognized programs.
pub fn identify_agent(process_name: &str) -> Option<Agent> {
    let name = normalized_agent_lookup_name(process_name);
    // Match against known binary names
    match name.as_str() {
        "pi" => Some(Agent::Pi),
        "claude" | "claude-code" => Some(Agent::Claude),
        "codex" => Some(Agent::Codex),
        "gemini" => Some(Agent::Gemini),
        "cursor" | "cursor-agent" => Some(Agent::Cursor),
        "devin" | "devin-cli" | "devin cli" => Some(Agent::Devin),
        "agy" | "antigravity" | "antigravity-cli" => Some(Agent::Antigravity),
        "cline" => Some(Agent::Cline),
        "omp" => Some(Agent::Omp),
        "mastracode" | "mastra-code" | "mastra code" => Some(Agent::Mastracode),
        "opencode" | "open-code" => Some(Agent::OpenCode),
        "copilot" | "github-copilot" | "ghcs" => Some(Agent::GithubCopilot),
        "kimi" | "kimi-code" | "kimi code" => Some(Agent::Kimi),
        "kiro" | "kiro-cli" => Some(Agent::Kiro),
        "droid" => Some(Agent::Droid),
        "amp" | "amp-local" => Some(Agent::Amp),
        "grok" | "grok-build" => Some(Agent::Grok),
        "hermes" | "hermes-agent" => Some(Agent::Hermes),
        "kilo" | "kilo-code" | "kilo code" => Some(Agent::Kilo),
        "qodercli" | "qoderclicn" | "qoder" | "qodercn" => Some(Agent::Qodercli),
        _ => None,
    }
}

pub fn identify_agent_in_job(job: &crate::platform::ForegroundJob) -> Option<(Agent, String)> {
    if let Some(process) = job
        .processes
        .iter()
        .find(|process| process.pid == job.process_group_id)
    {
        let candidate = normalized_process_name(process);
        if let Some(agent) = identify_agent(&candidate) {
            return Some((agent, candidate));
        }
    }

    let mut best: Option<(u8, Agent, String)> = None;

    for process in &job.processes {
        let candidate = normalized_process_name(process);
        let Some(agent) = identify_agent(&candidate) else {
            continue;
        };
        let score = process_priority(process, &candidate);

        match &best {
            Some((best_score, _, _)) if *best_score >= score => {}
            _ => best = Some((score, agent, candidate)),
        }
    }

    best.map(|(_, agent, name)| (agent, name))
}

/// Detect the state of an agent from the live terminal tail snapshot.
/// If `agent` is `None`, returns `Unknown`.
#[cfg(test)]
pub fn detect_state(agent: Option<Agent>, screen_content: &str) -> AgentState {
    detect_agent(agent, screen_content).state
}

/// Detect state and whether a visible blocker is present on the current screen.
#[allow(dead_code)] // shim for existing callers; detect_agent_with_osc is the real path
pub fn detect_agent(agent: Option<Agent>, screen_content: &str) -> AgentDetection {
    detect_agent_with_osc(agent, screen_content, "", "")
}

/// Detect state using screen content plus OSC title/progress strings.
pub fn detect_agent_with_osc(
    agent: Option<Agent>,
    screen_content: &str,
    osc_title: &str,
    osc_progress: &str,
) -> AgentDetection {
    let Some(agent) = agent else {
        return AgentDetection {
            state: AgentState::Unknown,
            skip_state_update: false,
            visible_idle: false,
            visible_blocker: false,
            visible_working: false,
        };
    };
    manifest::detect_with_osc(
        agent,
        manifest::DetectionInput {
            screen: screen_content,
            osc_title,
            osc_progress,
        },
    )
}

pub fn should_skip_state_update(agent: Option<Agent>, screen_content: &str) -> bool {
    agent.is_some_and(|agent| manifest::should_skip_state_update(agent, screen_content))
}

/// Best-effort context-window percentage for the pane, using the agent's
/// manifest value-extractors over screen + OSC content. `None` when no agent or
/// no extractor matches.
// Called only from the unix-only screen-detection task; unused on Windows.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn extract_context_percent(
    agent: Option<Agent>,
    screen_content: &str,
    osc_title: &str,
    osc_progress: &str,
) -> Option<u8> {
    let agent = agent?;
    manifest::extract_percent(
        agent,
        manifest::DetectionInput {
            screen: screen_content,
            osc_title,
            osc_progress,
        },
    )
}

/// How many lines of screen content shep publishes as a pane's activity.
///
/// Three: enough for a phone row to show what an agent is doing rather than
/// only the last thing it printed, and few enough that a session's worth of
/// them is a screenful rather than a transcript.
pub const ACTIVITY_LINES: usize = 3;

/// How many rows at the bottom of a screen can be the agent's own chrome.
///
/// A rule inside this window is the input box's border or the separator above
/// a status bar. One further up is content — a table, or a divider the agent
/// drew in its own output — and cutting there would throw away the answer.
const CHROME_WINDOW: usize = 8;

/// The last lines of real content on the pane's screen — an answer to "what is
/// this agent actually saying right now", in reading order.
///
/// Agent TUIs frame their input box in box-drawing runes, pad with blanks, and
/// print a hint bar underneath it, so the bottom-most line with letters in it
/// is almost always the hint — every claude pane reported `bypass permissions
/// on (shift+tab to cycle)`, which says nothing about what the agent is doing.
/// So the input box and everything below it is cut first, and the lines are
/// taken from what is left. It is a display hint only — never detection
/// evidence, and never a substitute for an agent's own reported status.
// Called only from the unix-only screen-detection task; unused on Windows.
#[cfg_attr(not(unix), allow(dead_code))]
pub fn extract_activity_lines(screen_content: &str, max: usize) -> Vec<String> {
    let mut lines: Vec<&str> = screen_content.lines().map(str::trim).collect();
    while lines.last().is_some_and(|line| line.is_empty()) {
        lines.pop();
    }
    let chrome_from = lines.len().saturating_sub(CHROME_WINDOW);
    if let Some(cut) = (chrome_from..lines.len()).find(|&idx| is_rule(lines[idx])) {
        // Only if there is something left to say. A short screen whose every
        // content line sits below its first rule is better answered with the
        // chrome than with nothing.
        if lines[..cut].iter().any(|line| has_text(line)) {
            lines.truncate(cut);
        }
    }
    let mut collected: Vec<String> = lines
        .iter()
        .rev()
        .filter(|line| has_text(line))
        .take(max)
        .map(|line| {
            // Strip leading decoration — the box-drawing rune a bordered line
            // starts with, a prompt caret, a bullet, a spinner glyph.
            // Enumerating the runes is a losing game across agents, so drop
            // anything that is not alphanumeric until the text starts.
            line.trim_start_matches(|c: char| !c.is_alphanumeric())
                .trim()
                .chars()
                .take(200)
                .collect()
        })
        .collect();
    collected.reverse();
    collected
}

fn has_text(line: &str) -> bool {
    line.chars().any(|c| c.is_alphanumeric())
}

/// A frame line: a run of box-drawing runes long enough to be a border rather
/// than punctuation.
///
/// It cannot require the line to be textless. claude bakes a label into the top
/// border of its input box — `───────── clear-chat-history ──` — and reading
/// that as content is how `clear-chat-history ──` ended up quoted as an agent's
/// most recent line.
fn is_rule(line: &str) -> bool {
    const FRAME_RUN: usize = 4;
    let mut run = 0usize;
    for c in line.chars() {
        if matches!(c, '\u{2500}'..='\u{257f}') {
            run += 1;
            if run >= FRAME_RUN {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

pub(crate) fn full_lifecycle_hook_authority(source: &str, agent_label: &str) -> bool {
    matches!(
        (source, agent_label),
        ("shep:pi", "pi")
            | ("shep:omp", "omp")
            | ("shep:mastracode", "mastracode")
            | ("shep:hermes", "hermes")
            | ("shep:opencode", "opencode")
            | ("shep:kilo", "kilo")
            | ("shep:kimi", "kimi")
    )
}

// ---------------------------------------------------------------------------
// Process identification (platform-specific)
// ---------------------------------------------------------------------------

/// Get the foreground job for a given child PID.
/// Delegates to platform-specific implementation.
pub fn foreground_job(child_pid: u32) -> Option<crate::platform::ForegroundJob> {
    crate::platform::foreground_job(child_pid)
}

/// Get the foreground process group leader as a one-process job.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_group_leader_job(
    process_group_id: u32,
) -> Option<crate::platform::ForegroundJob> {
    crate::platform::foreground_group_leader_job(process_group_id)
}

/// Get the foreground process group for a pane shell PID.
/// This is cheaper than collecting every process in the foreground job.
pub fn foreground_process_group_id(child_pid: u32) -> Option<u32> {
    crate::platform::foreground_process_group_id(child_pid)
}

fn normalized_process_name(process: &crate::platform::ForegroundProcess) -> String {
    let effective = process.argv0.as_deref().unwrap_or(&process.name);
    let lower_effective = effective.to_lowercase();

    if is_generic_runtime_or_shell(&lower_effective) {
        if let Some(wrapped_agent) =
            wrapped_agent_name_from_runtime_argv(&lower_effective, process.argv.as_deref())
        {
            return wrapped_agent;
        }
    }

    if identify_agent(effective).is_some() {
        return effective.to_string();
    }

    if let Some(wrapped_agent) = argv0_agent_name(process.argv.as_deref())
        .or_else(|| cmdline_argv0_agent_name(process.cmdline.as_deref().unwrap_or_default()))
    {
        return wrapped_agent;
    }

    effective.to_string()
}

fn wrapped_agent_name_from_runtime_argv(runtime: &str, argv: Option<&[String]>) -> Option<String> {
    let argv = argv?;
    let runtime = normalized_agent_lookup_name(path_basename(runtime));

    match runtime.as_str() {
        "node" | "bun" => script_arg_agent_name(argv, &["-e", "--eval", "-p", "--print"], &[]),
        "python" | "python3" => script_arg_agent_name(argv, &["-c"], &["-m"]),
        "sh" | "bash" | "zsh" | "fish" => script_arg_agent_name(argv, &["-c"], &[]),
        "cmd" => windows_cmd_arg_agent_name(argv),
        "powershell" | "pwsh" => powershell_arg_agent_name(argv),
        "tmux" => None,
        _ => None,
    }
}

fn windows_cmd_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "/c" | "/k" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command))
            }
            "/d" | "/s" | "/q" | "/a" | "/u" | "/e:on" | "/e:off" | "/f:on" | "/f:off"
            | "/v:on" | "/v:off" => continue,
            _ => {}
        }
    }
    None
}

fn powershell_arg_agent_name(argv: &[String]) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        let flag = arg.trim_matches('"').to_lowercase();
        match flag.as_str() {
            "-file" | "-f" | "/file" => {
                return args
                    .next()
                    .and_then(|path| agent_name_from_path_token(path));
            }
            "-command" | "-c" | "/command" | "/c" => {
                return args
                    .next()
                    .and_then(|command| command_text_agent_name(command));
            }
            "-encodedcommand" | "-enc" | "/encodedcommand" | "/enc" => return None,
            "-configurationname" | "-executionpolicy" | "-outputformat" | "-psconsolefile"
            | "-version" | "-windowstyle" | "-workingdirectory" => {
                let _ = args.next();
            }
            _ if flag.starts_with('-') || flag.starts_with('/') => {}
            _ => return agent_name_from_path_token(arg),
        }
    }
    None
}

fn command_text_agent_name(command: &str) -> Option<String> {
    let mut rest = command;
    while let Some((token, next)) = command_text_token(rest) {
        let token = token.trim();
        if token.eq_ignore_ascii_case("&")
            || token.eq_ignore_ascii_case(".")
            || token.eq_ignore_ascii_case("call")
        {
            rest = next;
            continue;
        }
        return agent_name_from_path_token(token);
    }
    None
}

fn command_text_token(input: &str) -> Option<(&str, &str)> {
    let input = input.trim_start();
    let first = input.chars().next()?;
    if first == '"' || first == '\'' {
        let start = first.len_utf8();
        if let Some(end) = input[start..].find(first) {
            let end = start + end;
            return Some((&input[start..end], &input[end + first.len_utf8()..]));
        }
        return Some((&input[start..], ""));
    }

    let end = input.find(char::is_whitespace).unwrap_or(input.len());
    Some((&input[..end], &input[end..]))
}

fn script_arg_agent_name(
    argv: &[String],
    eval_flags: &[&str],
    module_flags: &[&str],
) -> Option<String> {
    let mut args = argv.iter().skip(1);
    while let Some(arg) = args.next() {
        if arg == "--" {
            return args
                .next()
                .and_then(|token| agent_name_from_path_token(token));
        }

        if flag_matches(arg, eval_flags) || flag_matches(arg, module_flags) {
            return None;
        }

        if arg.starts_with('-') {
            if option_takes_value(arg) {
                let _ = args.next();
            }
            continue;
        }

        return agent_name_from_path_token(arg);
    }

    None
}

fn flag_matches(arg: &str, flags: &[&str]) -> bool {
    flags
        .iter()
        .any(|flag| arg == *flag || short_flag_payload(arg, flag) || long_flag_value(arg, flag))
}

fn short_flag_payload(arg: &str, flag: &str) -> bool {
    flag.starts_with('-')
        && !flag.starts_with("--")
        && arg.starts_with(flag)
        && arg.len() > flag.len()
}

fn long_flag_value(arg: &str, flag: &str) -> bool {
    flag.starts_with("--")
        && arg
            .strip_prefix(flag)
            .is_some_and(|rest| rest.starts_with('='))
}

fn option_takes_value(arg: &str) -> bool {
    matches!(
        arg,
        "-r" | "--require"
            | "--loader"
            | "--import"
            | "--experimental-loader"
            | "--inspect-port"
            | "-W"
            | "-X"
            | "-S"
            | "-L"
            | "-o"
    )
}

fn argv0_agent_name(argv: Option<&[String]>) -> Option<String> {
    agent_name_from_path_token(argv?.first()?)
}

fn cmdline_argv0_agent_name(cmdline: &str) -> Option<String> {
    agent_name_from_path_token(cmdline.split_whitespace().next()?)
}

fn agent_name_from_path_token(token: &str) -> Option<String> {
    let trimmed = token.trim_matches(|c| matches!(c, '"' | '\''));
    if trimmed.is_empty() || trimmed.starts_with('-') {
        return None;
    }

    agent_name_from_basename(path_basename(trimmed))
        .or_else(|| agent_name_from_known_package_path(trimmed))
        .or_else(|| resolved_agent_name_from_path_token(trimmed))
}

fn agent_name_from_known_package_path(path: &str) -> Option<String> {
    let components: Vec<String> = path
        .split(['/', '\\'])
        .filter(|component| !component.is_empty())
        .map(normalized_agent_lookup_name)
        .collect();

    for window in components.windows(5) {
        if window
            == [
                "node_modules",
                "@earendil-works",
                "pi-coding-agent",
                "dist",
                "cli",
            ]
        {
            return Some(agent_label(Agent::Pi).to_string());
        }
    }
    None
}

fn resolved_agent_name_from_path_token(token: &str) -> Option<String> {
    let path = std::path::Path::new(token);
    if path.components().count() < 2 {
        return None;
    }

    let resolved = std::fs::canonicalize(path).ok()?;
    let basename = resolved.file_name()?.to_str()?;
    agent_name_from_basename(basename)
}

fn agent_name_from_basename(basename: &str) -> Option<String> {
    let agent = parse_agent_label(basename)?;
    Some(agent_label(agent).to_string())
}

fn normalized_agent_lookup_name(name: &str) -> String {
    let mut name = name.trim().to_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".ps1", ".js"] {
        if name.ends_with(suffix) {
            name.truncate(name.len() - suffix.len());
            break;
        }
    }
    name
}

fn path_basename(path: &str) -> &str {
    path.rsplit(['/', '\\'])
        .find(|component| !component.is_empty())
        .unwrap_or(path)
}

fn process_priority(process: &crate::platform::ForegroundProcess, normalized_name: &str) -> u8 {
    let lower_name = normalized_name.to_lowercase();
    if lower_name != process.name.to_lowercase() {
        return 3;
    }
    if !is_generic_runtime_or_shell(&lower_name) {
        return 2;
    }
    1
}

fn is_generic_runtime_or_shell(name: &str) -> bool {
    let name = normalized_agent_lookup_name(path_basename(name));
    matches!(
        name.as_str(),
        "sh" | "bash"
            | "zsh"
            | "fish"
            | "tmux"
            | "node"
            | "bun"
            | "python"
            | "python3"
            | "cmd"
            | "powershell"
            | "pwsh"
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// The single-line case, which several of these assert on directly.
    fn extract_activity_line(screen_content: &str) -> Option<String> {
        extract_activity_lines(screen_content, 1).pop()
    }

    #[test]
    fn activity_line_skips_the_input_box_frame() {
        // The realistic failure: a naive "last non-empty line" returns the
        // bottom border of the agent's input box on every single pane.
        let screen = "\u{2726} Metamorphosing… (3s · thinking)\n\
                      \u{256d}\u{2500}\u{2500}\u{2500}\u{2500}\u{256e}\n\
                      \u{2502} > \u{2502}\n\
                      \u{2570}\u{2500}\u{2500}\u{2500}\u{2500}\u{256f}\n   \n";
        assert_eq!(
            extract_activity_line(screen).as_deref(),
            Some("Metamorphosing… (3s · thinking)")
        );
    }

    #[test]
    fn activity_line_strips_leading_decoration() {
        for framed in [
            "\u{2502} running tests",
            "> running tests",
            "\u{2726} running tests",
        ] {
            assert_eq!(
                extract_activity_line(framed).as_deref(),
                Some("running tests"),
                "input: {framed:?}"
            );
        }
    }

    #[test]
    fn activity_line_is_none_without_real_content() {
        assert_eq!(extract_activity_line(""), None);
        assert_eq!(
            extract_activity_line("   \n\u{2500}\u{2500}\u{2500}\n"),
            None
        );
    }

    #[test]
    fn activity_line_is_bounded() {
        let long = "x".repeat(500);
        let line = extract_activity_line(&long).expect("content");
        assert_eq!(line.chars().count(), 200);
    }

    /// A real claude screen, trimmed. Every pane on the phone said the same
    /// thing — `bypass permissions on (shift+tab to cycle)` — because that hint
    /// bar is the bottom-most line with letters in it on every claude pane in
    /// the session. It is chrome, and it is the same chrome on all of them.
    #[test]
    fn activity_lines_skip_the_hint_bar_under_the_input_box() {
        let rule = "\u{2500}".repeat(40);
        let screen = format!(
            "Your 52 files remain the complete series minus this one program.\n\
             \u{273b} Crunched for 6m 41s\n\
             \u{203b} recap: retrying the Internet Archive once it is reachable\n\
             {rule}\n\
             \u{276f}\n\
             {rule}\n  \
             \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle) \u{b7} \u{2190} for agents\n"
        );
        assert_eq!(
            extract_activity_lines(&screen, 3),
            vec![
                "Your 52 files remain the complete series minus this one program.".to_string(),
                "Crunched for 6m 41s".to_string(),
                "recap: retrying the Internet Archive once it is reachable".to_string(),
            ]
        );
    }

    /// claude bakes the branch name into the *top* border of its input box.
    /// Requiring a rule to be textless stopped the cut at the bottom border
    /// instead, so the phone quoted `clear-chat-history ──` as what the agent
    /// was saying. A configured status line above the box is not chrome we can
    /// name — it is arbitrary user text — so it stays.
    #[test]
    fn a_labelled_border_is_still_the_input_box() {
        let rule = "\u{2500}".repeat(40);
        let screen = format!(
            "\u{2726} Wiring oracles into CI (2h 51m \u{b7} \u{2193} 508.8k tokens)\n  \
             \u{2ffb} \u{25fc} Oracles in required CI, demo seed, org export\n     \
             \u{2026} +4 completed\n                 \
             11% until auto-compact \u{b7} \u{25ce} /goal active (2h)\n\
             {rule} clear-chat-history \u{2500}\u{2500}\n\
             \u{276f}\n\
             {rule}\n  \
             \u{23f5}\u{23f5} bypass permissions on (shift+tab to cycle)\n"
        );
        assert_eq!(
            extract_activity_lines(&screen, 3),
            vec![
                "Oracles in required CI, demo seed, org export".to_string(),
                // The leading `… +` goes with the rest of the decoration.
                "4 completed".to_string(),
                "11% until auto-compact \u{b7} \u{25ce} /goal active (2h)".to_string(),
            ],
            "nothing from the box down survives"
        );
    }

    /// Reading order, so a surface with three rows renders them the way they
    /// appeared on the screen rather than upside down.
    #[test]
    fn activity_lines_come_back_oldest_first() {
        let screen = "first\nsecond\nthird\nfourth\n";
        assert_eq!(
            extract_activity_lines(screen, 3),
            vec![
                "second".to_string(),
                "third".to_string(),
                "fourth".to_string()
            ]
        );
        assert_eq!(extract_activity_line(screen).as_deref(), Some("fourth"));
    }

    /// A divider in the agent's own output is content, not chrome — cutting at
    /// the first rule anywhere on the screen would throw the answer away.
    #[test]
    fn a_rule_far_above_the_bottom_is_not_chrome() {
        let rule = "\u{2500}".repeat(20);
        let screen = format!(
            "{rule}\nline a\nline b\nline c\nline d\nline e\nline f\nline g\nline h\nline i\n"
        );
        assert_eq!(
            extract_activity_lines(&screen, 2),
            vec!["line h".to_string(), "line i".to_string()]
        );
    }

    #[test]
    fn activity_lines_are_empty_without_real_content() {
        assert!(extract_activity_lines("", 3).is_empty());
        assert!(extract_activity_lines("   \n\u{2500}\u{2500}\u{2500}\n", 3).is_empty());
    }

    fn foreground_process(
        pid: u32,
        name: &str,
        argv: &[&str],
    ) -> crate::platform::ForegroundProcess {
        crate::platform::ForegroundProcess {
            pid,
            name: name.to_string(),
            argv0: None,
            argv: Some(argv.iter().map(|arg| (*arg).to_string()).collect()),
            cmdline: Some(argv.join(" ")),
        }
    }

    #[cfg(unix)]
    fn temp_detection_path(name: &str) -> std::path::PathBuf {
        let unique = format!(
            "shep-detect-tests-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time should be after unix epoch")
                .as_nanos()
        );
        std::env::temp_dir().join(unique)
    }

    #[test]
    fn moved_agent_detection_routes_through_production_dispatch() {
        let detection = detect_agent(Some(Agent::Pi), "Working...");

        assert_eq!(detection.state, AgentState::Working);
        assert!(detection.visible_working);
    }

    // ---- Agent identification ----

    #[test]
    fn identify_known_agents() {
        assert_eq!(identify_agent("pi"), Some(Agent::Pi));
        assert_eq!(identify_agent("claude"), Some(Agent::Claude));
        assert_eq!(identify_agent("claude-code"), Some(Agent::Claude));
        assert_eq!(identify_agent("codex"), Some(Agent::Codex));
        assert_eq!(identify_agent("gemini"), Some(Agent::Gemini));
        assert_eq!(identify_agent("cursor"), Some(Agent::Cursor));
        assert_eq!(identify_agent("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(identify_agent("devin"), Some(Agent::Devin));
        assert_eq!(identify_agent("devin-cli"), Some(Agent::Devin));
        assert_eq!(identify_agent("agy"), Some(Agent::Antigravity));
        assert_eq!(identify_agent("antigravity-cli"), Some(Agent::Antigravity));
        assert_eq!(identify_agent("cline"), Some(Agent::Cline));
        assert_eq!(identify_agent("omp"), Some(Agent::Omp));
        assert_eq!(identify_agent("mastracode"), Some(Agent::Mastracode));
        assert_eq!(identify_agent("mastra-code"), Some(Agent::Mastracode));
        assert_eq!(identify_agent("opencode"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("opencode.exe"), Some(Agent::OpenCode));
        assert_eq!(identify_agent("kimi"), Some(Agent::Kimi));
        assert_eq!(identify_agent("Kimi Code"), Some(Agent::Kimi));
        assert_eq!(identify_agent("kiro"), Some(Agent::Kiro));
        assert_eq!(identify_agent("kiro-cli"), Some(Agent::Kiro));
        assert_eq!(identify_agent("copilot"), Some(Agent::GithubCopilot));
        assert_eq!(identify_agent("ghcs"), Some(Agent::GithubCopilot));
        assert_eq!(identify_agent("grok"), Some(Agent::Grok));
        assert_eq!(identify_agent("grok-build"), Some(Agent::Grok));
        assert_eq!(identify_agent("hermes"), Some(Agent::Hermes));
        assert_eq!(identify_agent("hermes-agent"), Some(Agent::Hermes));
        assert_eq!(identify_agent("kilo"), Some(Agent::Kilo));
        assert_eq!(identify_agent("kilo-code"), Some(Agent::Kilo));
    }

    #[test]
    fn parse_known_agent_labels() {
        assert_eq!(parse_agent_label("pi"), Some(Agent::Pi));
        assert_eq!(parse_agent_label("claude"), Some(Agent::Claude));
        assert_eq!(parse_agent_label("cursor-agent"), Some(Agent::Cursor));
        assert_eq!(parse_agent_label("devin-cli"), Some(Agent::Devin));
        assert_eq!(parse_agent_label("agy"), Some(Agent::Antigravity));
        assert_eq!(parse_agent_label("antigravity"), Some(Agent::Antigravity));
        assert_eq!(parse_agent_label("omp"), Some(Agent::Omp));
        assert_eq!(parse_agent_label("mastracode"), Some(Agent::Mastracode));
        assert_eq!(parse_agent_label("mastra code"), Some(Agent::Mastracode));
        assert_eq!(parse_agent_label("opencode.exe"), Some(Agent::OpenCode));
        assert_eq!(parse_agent_label("copilot"), Some(Agent::GithubCopilot));
        assert_eq!(parse_agent_label("kimi-code"), Some(Agent::Kimi));
        assert_eq!(
            parse_agent_label("github-copilot"),
            Some(Agent::GithubCopilot)
        );
        assert_eq!(parse_agent_label("amp-local"), Some(Agent::Amp));
        assert_eq!(parse_agent_label("kiro-cli"), Some(Agent::Kiro));
        assert_eq!(parse_agent_label("grok-build"), Some(Agent::Grok));
        assert_eq!(parse_agent_label("hermes-agent"), Some(Agent::Hermes));
        assert_eq!(parse_agent_label("kilo-code"), Some(Agent::Kilo));
    }

    #[test]
    fn agent_labels_use_display_names() {
        assert_eq!(agent_label(Agent::Pi), "pi");
        assert_eq!(agent_label(Agent::GithubCopilot), "copilot");
        assert_eq!(agent_label(Agent::OpenCode), "opencode");
        assert_eq!(agent_label(Agent::Devin), "devin");
        assert_eq!(agent_label(Agent::Antigravity), "agy");
        assert_eq!(agent_label(Agent::Omp), "omp");
        assert_eq!(agent_label(Agent::Mastracode), "mastracode");
        assert_eq!(agent_label(Agent::Kiro), "kiro");
        assert_eq!(agent_label(Agent::Grok), "grok");
        assert_eq!(agent_label(Agent::Hermes), "hermes");
        assert_eq!(agent_label(Agent::Kilo), "kilo");
    }

    #[test]
    fn mastracode_is_hook_authority_without_screen_manifest() {
        assert!(full_lifecycle_hook_authority(
            "shep:mastracode",
            "mastracode"
        ));
        assert!(!Agent::SCREEN_MANIFEST_AGENTS.contains(&Agent::Mastracode));
    }

    #[test]
    fn identify_unknown_processes() {
        assert_eq!(identify_agent("bash"), None);
        assert_eq!(identify_agent("zsh"), None);
        assert_eq!(identify_agent("vim"), None);
        assert_eq!(identify_agent("node"), None);
    }

    #[test]
    fn identify_case_insensitive() {
        assert_eq!(identify_agent("Pi"), Some(Agent::Pi));
        assert_eq!(identify_agent("CLAUDE"), Some(Agent::Claude));
        assert_eq!(identify_agent("Codex"), Some(Agent::Codex));
        assert_eq!(identify_agent("Devin"), Some(Agent::Devin));
    }

    #[test]
    fn identify_agent_in_job_prefers_wrapped_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![
                foreground_process(1, "node", &["node", "/path/to/bin/codex"]),
                foreground_process(2, "bash", &["bash"]),
            ],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Codex, "codex".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_prefers_recognized_process_group_leader() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 42,
            processes: vec![
                foreground_process(42, "claude", &["claude"]),
                foreground_process(43, "node", &["node", "/tmp/mcp/bin/codex"]),
            ],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Claude, "claude".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_falls_back_when_process_group_leader_is_unrecognized() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 42,
            processes: vec![
                foreground_process(42, "bash", &["bash"]),
                foreground_process(43, "node", &["node", "/tmp/mcp/bin/codex"]),
            ],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Codex, "codex".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_nix_wrapped_codex_from_cmdline_argv0() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                ".codex-wrapped",
                &["/etc/profiles/per-user/user/bin/codex", "--model", "gpt-5"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Codex, "codex".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_canonicalizes_nix_wrapped_aliases_from_cmdline_argv0() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                ".claude-code-wrapped",
                &["/nix/store/example/bin/claude-code"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Claude, "claude".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_shell_wrapped_pi() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "sh",
                &["/bin/sh", "/tmp/test-bin/pi"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Pi, "pi".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_bun_wrapped_omp() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "bun",
                &["bun", "/home/can/.bun/bin/omp"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Omp, "omp".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_node_wrapped_pi_package_cli() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "node.exe",
                &[
                    "node.exe",
                    "C:\\Users\\shep\\AppData\\Roaming\\npm\\node_modules\\@earendil-works\\pi-coding-agent\\dist\\cli.js",
                ],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Pi, "pi".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_ignores_non_cli_pi_package_script() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "node.exe",
                &[
                    "node.exe",
                    "C:\\Users\\shep\\AppData\\Roaming\\npm\\node_modules\\@earendil-works\\pi-coding-agent\\scripts\\build.js",
                ],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None);
    }

    #[test]
    fn identify_agent_in_job_detects_windows_cmd_wrapped_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "cmd.exe",
                &[
                    "cmd.exe",
                    "/D",
                    "/S",
                    "/C",
                    "C:\\Users\\shep\\AppData\\Roaming\\npm\\codex.cmd --model gpt-5",
                ],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Codex, "codex".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_powershell_file_wrapped_claude() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "powershell.exe",
                &[
                    "powershell.exe",
                    "-NoProfile",
                    "-File",
                    "C:\\Users\\shep\\Documents\\PowerShell\\Scripts\\claude.ps1",
                ],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Claude, "claude".to_string()))
        );
    }

    // A plain shell pane launched with shep's injected prompt integration
    // must still classify as a shell, not an agent, even though its argv now
    // carries a -Command payload.
    #[test]
    fn identify_agent_in_job_ignores_shep_powershell_shell_integration_argv() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "powershell.exe",
                &[
                    "powershell.exe",
                    "-NoExit",
                    "-Command",
                    crate::pane::WINDOWS_POWERSHELL_SHELL_INTEGRATION_COMMAND,
                ],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None);
    }

    #[test]
    fn identify_agent_in_job_detects_opencode_exe_from_pnpm_package() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "opencode.exe",
                &["/home/user/.local/share/pnpm/global/node_modules/opencode-ai/bin/opencode.exe"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::OpenCode, "opencode.exe".to_string()))
        );
    }

    #[test]
    fn identify_agent_in_job_detects_opencode_exe_from_argv0_path() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                123,
                "MainThread",
                &["/home/user/.local/share/pnpm/global/node_modules/opencode-ai/bin/opencode.exe"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::OpenCode, "opencode".to_string()))
        );
    }

    #[test]
    fn wrapped_agent_name_from_runtime_argv_ignores_plain_shell_flags() {
        assert_eq!(
            wrapped_agent_name_from_runtime_argv("bash", Some(&["bash".into(), "-lc".into()])),
            None
        );
    }

    #[test]
    fn identify_agent_in_job_ignores_python_c_argument_named_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "python3",
                &["python3", "-c", "import time; time.sleep(60)", "/tmp/codex"],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None);
    }

    #[test]
    fn identify_agent_in_job_ignores_node_eval_argument_named_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "node",
                &["node", "-e", "setTimeout(() => {}, 60000)", "/tmp/codex"],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None);
    }

    #[test]
    fn identify_agent_in_job_ignores_shell_c_argument_named_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "bash",
                &["bash", "-c", "sleep 60", "/tmp/codex"],
            )],
        };

        assert_eq!(identify_agent_in_job(&job), None);
    }

    #[test]
    fn identify_agent_in_job_detects_python_script_named_codex() {
        let job = crate::platform::ForegroundJob {
            process_group_id: 123,
            processes: vec![foreground_process(
                1,
                "python3",
                &["python3", "/tmp/codex", "--model", "gpt-5"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Codex, "codex".to_string()))
        );
    }

    #[test]
    fn cmdline_argv0_agent_name_canonicalizes_known_aliases() {
        assert_eq!(
            cmdline_argv0_agent_name("/nix/store/example/bin/ghcs"),
            Some("copilot".to_string())
        );
    }

    #[test]
    fn cmdline_argv0_agent_name_requires_exact_agent_basename() {
        assert_eq!(cmdline_argv0_agent_name("/tmp/my-codex-helper"), None);
    }

    #[cfg(unix)]
    #[test]
    fn identify_agent_in_job_resolves_cursor_agent_symlink_argv0() {
        let dir = temp_detection_path("cursor-agent-symlink");
        std::fs::create_dir_all(&dir).expect("test directory should be created");
        let target = dir.join("cursor-agent");
        let link = dir.join("agent");
        std::fs::write(&target, b"#!/bin/sh\n").expect("target should be written");
        std::os::unix::fs::symlink(&target, &link).expect("symlink should be created");

        let argv0 = link.to_string_lossy().into_owned();
        let job = crate::platform::ForegroundJob {
            process_group_id: 42,
            processes: vec![foreground_process(
                42,
                "MainThread",
                &[&argv0, "--use-system-ca", "/tmp/index.js"],
            )],
        };

        assert_eq!(
            identify_agent_in_job(&job),
            Some((Agent::Cursor, "cursor".to_string()))
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    // ---- Screen detection routing ----

    #[test]
    fn no_agent_returns_unknown() {
        assert_eq!(detect_state(None, "anything"), AgentState::Unknown);
    }

    // ---- Process identification (real PTY) ----

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_job_detects_sleep() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        // Spawn "sleep 999" — a known, deterministic process
        let mut cmd = CommandBuilder::new("sleep");
        cmd.arg("999");
        let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
        let pid = child.process_id().expect("no pid");

        // Give the process a moment to become the foreground group
        std::thread::sleep(std::time::Duration::from_millis(50));

        let job = foreground_job(pid).expect("expected foreground job");
        assert!(
            job.processes.iter().any(|p| p.name == "sleep"),
            "expected sleep in {job:?}"
        );
        assert_eq!(
            identify_agent_in_job(&job),
            None,
            "sleep should not map to an agent"
        );

        // Clean up
        child.kill().ok();
        child.wait().ok();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn foreground_job_detects_shell_running_command() {
        use portable_pty::{native_pty_system, CommandBuilder, PtySize};
        use std::io::Write;

        let pty_system = native_pty_system();
        let pair = pty_system
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("failed to open pty");

        // Spawn a shell, then run a command inside it
        let cmd = CommandBuilder::new("sh");
        let mut child = pair.slave.spawn_command(cmd).expect("failed to spawn");
        let pid = child.process_id().expect("no pid");

        // Write a command to the shell
        let mut writer = pair.master.take_writer().expect("no writer");
        // Use exec so sleep replaces sh as the foreground process
        writer.write_all(b"exec sleep 999\n").ok();
        drop(writer);

        std::thread::sleep(std::time::Duration::from_millis(100));

        let job = foreground_job(pid).expect("expected foreground job");
        assert!(
            job.processes.iter().any(|p| p.name == "sleep"),
            "expected sleep in {job:?}"
        );
        assert_eq!(
            identify_agent_in_job(&job),
            None,
            "sleep should not map to an agent"
        );

        child.kill().ok();
        child.wait().ok();
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn proc_stat_parsing_handles_spaces_in_comm() {
        // Verify our /proc/pid/stat parser correctly extracts fields
        // even when (comm) could contain spaces.
        let pid = std::process::id();
        let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap();

        // Our parsing: find last ')' then split the rest
        let close_paren = stat.rfind(')').expect("should have closing paren");
        let rest = &stat[close_paren + 2..];
        let fields: Vec<&str> = rest.split_whitespace().collect();

        // We should have enough fields (at least 6 for tpgid)
        assert!(
            fields.len() >= 6,
            "not enough fields in stat: {}",
            fields.len()
        );

        // Field 0 should be a valid state char (S, R, D, etc.)
        let state = fields[0];
        assert!(
            ["S", "R", "D", "Z", "T", "t", "W", "X", "I"].contains(&state),
            "unexpected state: {state}"
        );

        // Field 5 (tpgid) should parse as i32 (can be -1 if no controlling terminal)
        let tpgid: i32 = fields[5].parse().expect("tpgid should be a number");
        // In CI/test environments without a terminal, tpgid is typically -1
        let _ = tpgid;
    }
}
