//! The runtime launch registry: what to run when a caller names a runtime
//! (`claude`, `opencode`, …) instead of spelling out argv.
//!
//! The facts live on the detection manifests (`[launch]` and `[headless]` in
//! `src/detect/manifests/*.toml`), so a runtime is described once — how to
//! recognise it on screen, how to start it, how to ask it one question. A
//! `[runtimes.<name>]` table in `config.toml` overrides either recipe. This
//! module is pure resolution; the `PATH` lookup is injected so it can be
//! tested without touching the machine, and nothing here spawns a process
//! except [`run_headless`] (the `shep runtime ask` CLI) and
//! [`run_headless_captured`] (the board's chat).

use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::config::RuntimeOverrideConfig;
pub use crate::detect::manifest::{HeadlessPrompt, HeadlessSpec, LaunchSpec};
use crate::detect::{agent_label, parse_agent_label, Agent};

/// Where a resolved recipe came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecipeSource {
    /// `[runtimes.<name>]` in `config.toml`.
    Config,
    /// The runtime's detection manifest.
    Manifest,
}

/// A runtime launch, ready to hand to a pty spawn.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLaunch {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Absolute path of `argv[0]` as found on `PATH`.
    pub bin_resolved: PathBuf,
    pub source: RecipeSource,
    /// The recipe's `session_new_args`, when it names conversations; see
    /// [`session_args`] for the splice.
    pub session_new_args: Option<Vec<String>>,
    /// The recipe's `session_resume_args`.
    pub session_resume_args: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeResolveError {
    /// Not a runtime shep knows and not configured under `[runtimes]`.
    Unknown { name: String },
    /// Known, but nothing to run: no `[launch]` (or `[headless]`) recipe, or
    /// none of the binaries it names is on `PATH`.
    NotLaunchable { name: String, tried: Vec<String> },
}

impl RuntimeResolveError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::Unknown { .. } => "runtime_unknown",
            Self::NotLaunchable { .. } => "runtime_not_launchable",
        }
    }
}

impl std::fmt::Display for RuntimeResolveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unknown { name } => write!(
                f,
                "unknown runtime {name}; see `shep runtime list` or add [runtimes.{name}] to config.toml"
            ),
            Self::NotLaunchable { name, tried } if tried.is_empty() => write!(
                f,
                "runtime {name} declares no launch recipe; add [runtimes.{name}] argv = [...] to config.toml"
            ),
            Self::NotLaunchable { name, tried } => write!(
                f,
                "runtime {name} is not launchable: none of {} is on PATH",
                tried.join(", ")
            ),
        }
    }
}

/// One row of `runtime.list`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimeSummary {
    pub name: String,
    pub launchable: bool,
    pub bin_resolved: Option<PathBuf>,
    pub headless: bool,
}

/// The canonical manifest name for `name`, when it is one shep detects.
fn agent_for(name: &str) -> Option<Agent> {
    parse_agent_label(name)
}

/// The `[runtimes.<name>]` override for `name`, looked up by the spelling
/// given and by the canonical label (`claude-code` finds `[runtimes.claude]`).
fn override_for<'a>(
    name: &str,
    overrides: &'a BTreeMap<String, RuntimeOverrideConfig>,
) -> Option<&'a RuntimeOverrideConfig> {
    overrides
        .get(name)
        .or_else(|| agent_for(name).and_then(|agent| overrides.get(agent_label(agent))))
}

/// Resolve the interactive launch for `name`: the config override when it
/// sets `argv`, else the manifest's `[launch]` with `bin` then each
/// `fallback_bins` entry tried on `PATH`.
pub fn resolve_launch(
    name: &str,
    overrides: &BTreeMap<String, RuntimeOverrideConfig>,
    find_on_path: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<ResolvedLaunch, RuntimeResolveError> {
    let name = name.trim();
    let agent = agent_for(name);
    if let Some(argv) = override_for(name, overrides)
        .map(|over| &over.argv)
        .filter(|argv| !argv.is_empty())
    {
        let program = &argv[0];
        let Some(bin_resolved) = find_on_path(program) else {
            return Err(RuntimeResolveError::NotLaunchable {
                name: name.to_string(),
                tried: vec![program.clone()],
            });
        };
        let over = override_for(name, overrides);
        let env = over
            .map(|over| over.env.clone().into_iter().collect())
            .unwrap_or_default();
        return Ok(ResolvedLaunch {
            argv: argv.clone(),
            env,
            bin_resolved,
            source: RecipeSource::Config,
            session_new_args: over.and_then(|over| over.session_new_args.clone()),
            session_resume_args: over.and_then(|over| over.session_resume_args.clone()),
        });
    }
    let Some(agent) = agent else {
        return Err(RuntimeResolveError::Unknown {
            name: name.to_string(),
        });
    };
    let Some(mut spec) = crate::detect::manifest::launch_spec(agent) else {
        return Err(RuntimeResolveError::NotLaunchable {
            name: name.to_string(),
            tried: Vec::new(),
        });
    };
    // Per-field: the session recipes override on their own, the argv does
    // not have to come along.
    if let Some(over) = override_for(name, overrides) {
        if over.session_new_args.is_some() {
            spec.session_new_args = over.session_new_args.clone();
        }
        if over.session_resume_args.is_some() {
            spec.session_resume_args = over.session_resume_args.clone();
        }
    }
    resolve_launch_spec(name, &spec, find_on_path)
}

/// The manifest half of [`resolve_launch`], separated so it can be tested
/// against a spec that never came from a file.
pub fn resolve_launch_spec(
    name: &str,
    spec: &LaunchSpec,
    find_on_path: &dyn Fn(&str) -> Option<PathBuf>,
) -> Result<ResolvedLaunch, RuntimeResolveError> {
    let candidates =
        std::iter::once(spec.bin.as_str()).chain(spec.fallback_bins.iter().map(String::as_str));
    let mut tried = Vec::new();
    for candidate in candidates {
        if let Some(bin_resolved) = find_on_path(candidate) {
            let mut argv = if spec.argv.is_empty() {
                vec![spec.bin.clone()]
            } else {
                spec.argv.clone()
            };
            if argv[0] == spec.bin {
                argv[0] = candidate.to_string();
            }
            return Ok(ResolvedLaunch {
                argv,
                env: spec.env.clone().into_iter().collect(),
                bin_resolved,
                source: RecipeSource::Manifest,
                session_new_args: spec.session_new_args.clone(),
                session_resume_args: spec.session_resume_args.clone(),
            });
        }
        tried.push(candidate.to_string());
    }
    Err(RuntimeResolveError::NotLaunchable {
        name: name.to_string(),
        tried,
    })
}

/// Resolve the one-shot question recipe for `name`: the config override when
/// it sets `headless_argv`, else the manifest's `[headless]`. The override's
/// `headless_session_new_args` / `headless_session_resume_args` are
/// per-field: on their own they land on the manifest's recipe, and with
/// `headless_argv` they are the only way the override can share a session.
pub fn resolve_headless(
    name: &str,
    overrides: &BTreeMap<String, RuntimeOverrideConfig>,
) -> Result<(HeadlessSpec, RecipeSource), RuntimeResolveError> {
    let name = name.trim();
    let over = override_for(name, overrides);
    if let Some(over) = over.filter(|over| !over.headless_argv.is_empty()) {
        return Ok((
            HeadlessSpec {
                argv: over.headless_argv.clone(),
                prompt: over.headless_prompt.unwrap_or_default(),
                output: Default::default(),
                session_new_args: over.headless_session_new_args.clone(),
                session_resume_args: over.headless_session_resume_args.clone(),
            },
            RecipeSource::Config,
        ));
    }
    let Some(agent) = agent_for(name) else {
        return Err(RuntimeResolveError::Unknown {
            name: name.to_string(),
        });
    };
    let Some(mut spec) = crate::detect::manifest::headless_spec(agent) else {
        return Err(RuntimeResolveError::NotLaunchable {
            name: name.to_string(),
            tried: Vec::new(),
        });
    };
    if let Some(over) = over {
        if over.headless_session_new_args.is_some() {
            spec.session_new_args = over.headless_session_new_args.clone();
        }
        if over.headless_session_resume_args.is_some() {
            spec.session_resume_args = over.headless_session_resume_args.clone();
        }
    }
    Ok((spec, RecipeSource::Manifest))
}

/// The conversation a headless question belongs to: `id` is substituted
/// for `{session_id}` in the recipe's `session_new_args` (first question)
/// or `session_resume_args` (every later one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadlessSession {
    pub id: String,
    pub resume: bool,
}

/// The placeholder a session recipe carries where the id goes.
pub const SESSION_ID_PLACEHOLDER: &str = "{session_id}";

/// The session arguments for `session` under `spec`: the resume or new
/// recipe with `{session_id}` replaced verbatim in every word. `None` when
/// the recipe does not name conversations, so the argv goes out unchanged.
pub fn session_args(spec: &HeadlessSpec, session: &HeadlessSession) -> Option<Vec<String>> {
    if !spec.shares_session() {
        return None;
    }
    let recipe = if session.resume {
        spec.session_resume_args.as_deref()
    } else {
        spec.session_new_args.as_deref()
    }?;
    Some(substitute_session_id(recipe, &session.id))
}

/// `{session_id}` -> `id` in every word.
pub fn substitute_session_id(args: &[String], id: &str) -> Vec<String> {
    args.iter()
        .map(|word| word.replace(SESSION_ID_PLACEHOLDER, id))
        .collect()
}

/// Every runtime shep can name — the detection manifests plus any
/// `[runtimes.<name>]` table — with whether it can be launched and asked.
pub fn list_runtimes(
    overrides: &BTreeMap<String, RuntimeOverrideConfig>,
    find_on_path: &dyn Fn(&str) -> Option<PathBuf>,
) -> Vec<RuntimeSummary> {
    let mut names: Vec<String> = Agent::SCREEN_MANIFEST_AGENTS
        .iter()
        .map(|agent| agent_label(*agent).to_string())
        .collect();
    for name in overrides.keys() {
        let canonical = agent_for(name).map(agent_label).unwrap_or(name.as_str());
        if !names.iter().any(|known| known == canonical) {
            names.push(name.clone());
        }
    }
    names.sort();
    names.dedup();
    names
        .into_iter()
        .map(|name| {
            let launch = resolve_launch(&name, overrides, find_on_path).ok();
            RuntimeSummary {
                launchable: launch.is_some(),
                bin_resolved: launch.map(|launch| launch.bin_resolved),
                headless: resolve_headless(&name, overrides).is_ok(),
                name,
            }
        })
        .collect()
}

/// `PATH` lookup the way a shell would do it: a name with a `/` is taken as
/// a path and only checked to exist; anything else is searched in each `PATH`
/// entry for an executable regular file.
pub fn find_on_path(program: &str) -> Option<PathBuf> {
    find_on_path_in(program, std::env::var_os("PATH").as_deref())
}

pub fn find_on_path_in(program: &str, path_var: Option<&std::ffi::OsStr>) -> Option<PathBuf> {
    if program.is_empty() {
        return None;
    }
    if program.contains('/') {
        let path = PathBuf::from(program);
        return is_executable_file(&path).then_some(path);
    }
    std::env::split_paths(path_var?)
        .map(|dir| dir.join(program))
        .find(|candidate| is_executable_file(candidate))
}

#[cfg(unix)]
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(not(unix))]
fn is_executable_file(path: &Path) -> bool {
    path.is_file()
}

/// What a headless run came back with.
#[derive(Debug)]
pub struct HeadlessOutcome {
    /// `None` when the child was killed for exceeding the timeout.
    pub exit_code: Option<i32>,
    pub timed_out: bool,
}

/// A headless run with its output kept: what [`run_headless_captured`]
/// returns. Each stream is capped at [`HEADLESS_CAPTURE_MAX_BYTES`].
#[derive(Debug)]
pub struct HeadlessCapture {
    pub outcome: HeadlessOutcome,
    pub stdout: String,
    pub stderr: String,
}

/// The most of each captured stream that is kept; the rest is dropped with
/// a note at the end of the text.
pub const HEADLESS_CAPTURE_MAX_BYTES: usize = 64 * 1024;

/// Run `spec` once with `prompt`, streaming the child's stdout and stderr to
/// ours. The prompt goes on stdin (closed after the write) or as the last
/// argument, per `spec.prompt`. On timeout the child is killed.
pub fn run_headless(
    spec: &HeadlessSpec,
    prompt: &str,
    timeout: Duration,
    cwd: Option<&Path>,
) -> std::io::Result<HeadlessOutcome> {
    run_headless_with(spec, prompt, timeout, cwd, None, false).map(|capture| capture.outcome)
}

/// [`run_headless`] with the child's stdout and stderr captured instead of
/// inherited, each capped at [`HEADLESS_CAPTURE_MAX_BYTES`], and, with a
/// `session`, the recipe's session arguments spliced after the argv (see
/// [`session_args`]). The board's chat uses this: the answer is the stdout.
pub fn run_headless_captured(
    spec: &HeadlessSpec,
    prompt: &str,
    timeout: Duration,
    cwd: Option<&Path>,
    session: Option<&HeadlessSession>,
) -> std::io::Result<HeadlessCapture> {
    run_headless_with(spec, prompt, timeout, cwd, session, true)
}

/// The argv a run uses: the recipe's, then the session arguments when
/// there is a session and a recipe for it, then the prompt when it travels
/// as an argument.
pub fn headless_argv(
    spec: &HeadlessSpec,
    prompt: &str,
    session: Option<&HeadlessSession>,
) -> Vec<String> {
    let mut argv = spec.argv.clone();
    if let Some(args) = session.and_then(|session| session_args(spec, session)) {
        argv.extend(args);
    }
    if spec.prompt == HeadlessPrompt::Arg {
        argv.push(prompt.to_string());
    }
    argv
}

/// The one launch-and-wait loop behind both entry points. With `capture`
/// the output streams are piped and drained on threads (a child that fills
/// a pipe nobody reads would otherwise block forever); without it they are
/// the caller's own and the returned strings are empty.
fn run_headless_with(
    spec: &HeadlessSpec,
    prompt: &str,
    timeout: Duration,
    cwd: Option<&Path>,
    session: Option<&HeadlessSession>,
    capture: bool,
) -> std::io::Result<HeadlessCapture> {
    let argv = headless_argv(spec, prompt, session);
    let Some(program) = argv.first() else {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "headless recipe has an empty argv",
        ));
    };
    let mut command = Command::new(program);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let (out, err) = if capture {
        (Stdio::piped(), Stdio::piped())
    } else {
        (Stdio::inherit(), Stdio::inherit())
    };
    command.stdout(out).stderr(err);
    command.stdin(if spec.prompt == HeadlessPrompt::Stdin {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    let mut child = command.spawn()?;
    let writer = child.stdin.take().map(|mut stdin| {
        let prompt = prompt.to_string();
        std::thread::spawn(move || {
            let _ = stdin.write_all(prompt.as_bytes());
            // Dropping closes stdin so the runtime sees EOF and starts.
        })
    });
    let stdout_reader = child.stdout.take().map(|stdout| {
        std::thread::spawn(move || read_capped_output(stdout, HEADLESS_CAPTURE_MAX_BYTES, "output"))
    });
    let stderr_reader = child.stderr.take().map(|stderr| {
        std::thread::spawn(move || read_capped_output(stderr, HEADLESS_CAPTURE_MAX_BYTES, "output"))
    });
    let deadline = Instant::now() + timeout;
    let outcome = loop {
        if let Some(status) = child.try_wait()? {
            break HeadlessOutcome {
                exit_code: status.code(),
                timed_out: false,
            };
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            break HeadlessOutcome {
                exit_code: None,
                timed_out: true,
            };
        }
        std::thread::sleep(Duration::from_millis(25));
    };
    if let Some(writer) = writer {
        let _ = writer.join();
    }
    let drain = |reader: Option<std::thread::JoinHandle<String>>| {
        reader
            .and_then(|reader| reader.join().ok())
            .unwrap_or_default()
    };
    Ok(HeadlessCapture {
        outcome,
        stdout: drain(stdout_reader),
        stderr: drain(stderr_reader),
    })
}

/// Read a child's stream to EOF keeping the first `cap` bytes; anything past
/// that is consumed and dropped so the child never blocks on a full pipe,
/// and the text ends with a note naming `what` was cut.
pub(crate) fn read_capped_output(mut reader: impl Read, cap: usize, what: &str) -> String {
    let mut kept = Vec::with_capacity(cap.min(8192));
    let mut buf = [0u8; 8192];
    let mut truncated = false;
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                let remaining = cap.saturating_sub(kept.len());
                if remaining > 0 {
                    kept.extend_from_slice(&buf[..n.min(remaining)]);
                }
                if n > remaining {
                    truncated = true;
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
    let mut output = String::from_utf8_lossy(&kept).into_owned();
    if truncated {
        output.push_str(&format!("\n[shep truncated {what} after {cap} bytes]"));
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(bin: &str, fallbacks: &[&str], argv: &[&str]) -> LaunchSpec {
        LaunchSpec {
            bin: bin.into(),
            fallback_bins: fallbacks.iter().map(|s| s.to_string()).collect(),
            version_args: vec!["--version".into()],
            argv: argv.iter().map(|s| s.to_string()).collect(),
            env: BTreeMap::new(),
            session_new_args: None,
            session_resume_args: None,
        }
    }

    fn headless(argv: &[&str], prompt: HeadlessPrompt) -> HeadlessSpec {
        HeadlessSpec {
            argv: argv.iter().map(|s| s.to_string()).collect(),
            prompt,
            output: Default::default(),
            session_new_args: None,
            session_resume_args: None,
        }
    }

    fn path_with(found: &[&str]) -> impl Fn(&str) -> Option<PathBuf> {
        let found: Vec<String> = found.iter().map(|s| s.to_string()).collect();
        move |bin: &str| {
            found
                .iter()
                .any(|f| f == bin)
                .then(|| PathBuf::from(format!("/usr/local/bin/{bin}")))
        }
    }

    #[test]
    fn launch_uses_bin_when_on_path() {
        let resolved = resolve_launch_spec(
            "claude",
            &spec("claude", &["openclaude"], &["claude"]),
            &path_with(&["claude"]),
        )
        .unwrap();
        assert_eq!(resolved.argv, vec!["claude"]);
        assert_eq!(
            resolved.bin_resolved,
            PathBuf::from("/usr/local/bin/claude")
        );
        assert_eq!(resolved.source, RecipeSource::Manifest);
    }

    #[test]
    fn launch_swaps_argv0_for_the_fallback_that_was_found() {
        let resolved = resolve_launch_spec(
            "claude",
            &spec("claude", &["openclaude"], &["claude", "--resume"]),
            &path_with(&["openclaude"]),
        )
        .unwrap();
        assert_eq!(resolved.argv, vec!["openclaude", "--resume"]);
        assert_eq!(
            resolved.bin_resolved,
            PathBuf::from("/usr/local/bin/openclaude")
        );
    }

    #[test]
    fn launch_defaults_argv_to_bin_and_names_every_bin_tried() {
        let resolved =
            resolve_launch_spec("codex", &spec("codex", &[], &[]), &path_with(&["codex"])).unwrap();
        assert_eq!(resolved.argv, vec!["codex"]);

        let err = resolve_launch_spec(
            "claude",
            &spec("claude", &["openclaude"], &["claude"]),
            &path_with(&[]),
        )
        .unwrap_err();
        assert_eq!(
            err,
            RuntimeResolveError::NotLaunchable {
                name: "claude".into(),
                tried: vec!["claude".into(), "openclaude".into()],
            }
        );
        assert_eq!(err.code(), "runtime_not_launchable");
        assert!(err.to_string().contains("claude, openclaude"));
    }

    #[test]
    fn config_override_wins_over_the_manifest_and_needs_its_program_on_path() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude".to_string(),
            RuntimeOverrideConfig {
                argv: vec!["my-claude".into(), "--model".into(), "local".into()],
                env: [("FOO".to_string(), "bar".to_string())]
                    .into_iter()
                    .collect(),
                ..Default::default()
            },
        );
        let resolved =
            resolve_launch("claude-code", &overrides, &path_with(&["my-claude"])).unwrap();
        assert_eq!(resolved.source, RecipeSource::Config);
        assert_eq!(resolved.argv[0], "my-claude");
        assert_eq!(resolved.env, vec![("FOO".to_string(), "bar".to_string())]);
        assert_eq!(
            resolved.session_new_args, None,
            "an override without them shares nothing"
        );

        let err = resolve_launch("claude", &overrides, &path_with(&["claude"])).unwrap_err();
        assert_eq!(
            err,
            RuntimeResolveError::NotLaunchable {
                name: "claude".into(),
                tried: vec!["my-claude".into()],
            }
        );
    }

    #[test]
    fn unknown_runtime_is_its_own_error_and_a_configured_one_is_listed() {
        let err = resolve_launch("nope", &BTreeMap::new(), &path_with(&[])).unwrap_err();
        assert_eq!(err.code(), "runtime_unknown");

        let mut overrides = BTreeMap::new();
        overrides.insert(
            "local-llm".to_string(),
            RuntimeOverrideConfig {
                argv: vec!["llm".into()],
                headless_argv: vec!["llm".into(), "ask".into()],
                ..Default::default()
            },
        );
        let listed = list_runtimes(&overrides, &path_with(&["llm"]));
        let local = listed.iter().find(|r| r.name == "local-llm").unwrap();
        assert!(local.launchable);
        assert!(local.headless);
        assert_eq!(
            local.bin_resolved,
            Some(PathBuf::from("/usr/local/bin/llm"))
        );
        // Manifest runtimes without [launch] are listed but not launchable.
        let pi = listed.iter().find(|r| r.name == "pi").unwrap();
        assert!(!pi.launchable);
        assert!(!pi.headless);
        assert!(listed.windows(2).all(|pair| pair[0].name < pair[1].name));
    }

    #[test]
    fn headless_override_and_manifest_recipes_resolve() {
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "opencode".to_string(),
            RuntimeOverrideConfig {
                headless_argv: vec!["sh".into(), "-c".into(), "cat".into()],
                headless_prompt: Some(HeadlessPrompt::Arg),
                ..Default::default()
            },
        );
        let (spec, source) = resolve_headless("open-code", &overrides).unwrap();
        assert_eq!(source, RecipeSource::Config);
        assert_eq!(spec.prompt, HeadlessPrompt::Arg);
        assert_eq!(spec.argv, vec!["sh", "-c", "cat"]);

        let err = resolve_headless("nope", &BTreeMap::new()).unwrap_err();
        assert_eq!(err.code(), "runtime_unknown");
    }

    #[test]
    fn session_args_are_per_field_overrides_and_absent_means_no_sharing() {
        // A `headless_argv` override on its own cannot share a session.
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "local-llm".to_string(),
            RuntimeOverrideConfig {
                headless_argv: vec!["llm".into(), "ask".into()],
                ..Default::default()
            },
        );
        let (spec, _) = resolve_headless("local-llm", &overrides).unwrap();
        assert!(!spec.shares_session());
        assert_eq!(
            headless_argv(
                &spec,
                "q",
                Some(&HeadlessSession {
                    id: "X".into(),
                    resume: true
                })
            ),
            vec!["llm", "ask"]
        );

        // With the session fields it can.
        overrides.insert(
            "local-llm".to_string(),
            RuntimeOverrideConfig {
                headless_argv: vec!["llm".into(), "ask".into()],
                headless_session_new_args: Some(vec!["--new".into(), "{session_id}".into()]),
                headless_session_resume_args: Some(vec!["--continue={session_id}".into()]),
                ..Default::default()
            },
        );
        let (spec, _) = resolve_headless("local-llm", &overrides).unwrap();
        assert!(spec.shares_session());
        assert_eq!(
            headless_argv(
                &spec,
                "q",
                Some(&HeadlessSession {
                    id: "X".into(),
                    resume: false
                })
            ),
            vec!["llm", "ask", "--new", "X"]
        );
        assert_eq!(
            headless_argv(
                &spec,
                "q",
                Some(&HeadlessSession {
                    id: "X".into(),
                    resume: true
                })
            ),
            vec!["llm", "ask", "--continue=X"]
        );

        // The bundled claude manifest shares; the session fields alone
        // override it without replacing its argv.
        let (claude, source) = resolve_headless("claude", &BTreeMap::new()).unwrap();
        assert_eq!(source, RecipeSource::Manifest);
        assert!(claude.shares_session());
        assert_eq!(
            headless_argv(
                &claude,
                "q",
                Some(&HeadlessSession {
                    id: "u-1".into(),
                    resume: false
                })
            ),
            vec![
                "claude",
                "-p",
                "--output-format",
                "text",
                "--session-id",
                "u-1"
            ]
        );
        assert_eq!(
            headless_argv(
                &claude,
                "q",
                Some(&HeadlessSession {
                    id: "u-1".into(),
                    resume: true
                })
            ),
            vec!["claude", "-p", "--output-format", "text", "--resume", "u-1"]
        );
        assert_eq!(
            headless_argv(&claude, "q", None),
            vec!["claude", "-p", "--output-format", "text"],
            "no session, no splice"
        );
        let mut overrides = BTreeMap::new();
        overrides.insert(
            "claude".to_string(),
            RuntimeOverrideConfig {
                headless_session_resume_args: Some(vec!["-r".into(), "{session_id}".into()]),
                ..Default::default()
            },
        );
        let (claude, source) = resolve_headless("claude", &overrides).unwrap();
        assert_eq!(source, RecipeSource::Manifest);
        assert_eq!(claude.argv, vec!["claude", "-p", "--output-format", "text"]);
        assert_eq!(
            session_args(
                &claude,
                &HeadlessSession {
                    id: "u-1".into(),
                    resume: true
                }
            ),
            Some(vec!["-r".into(), "u-1".into()])
        );

        // The launch recipe carries the same two.
        let launch = resolve_launch("claude", &BTreeMap::new(), &path_with(&["claude"])).unwrap();
        assert_eq!(
            launch.session_resume_args,
            Some(vec!["--resume".into(), "{session_id}".into()])
        );
        assert_eq!(
            substitute_session_id(launch.session_new_args.as_deref().unwrap_or(&[]), "u-2"),
            vec!["--session-id", "u-2"]
        );
        // A prompt that travels as an argument stays last.
        let mut arg = headless(&["ask"], HeadlessPrompt::Arg);
        arg.session_new_args = Some(vec!["--sid".into(), "{session_id}".into()]);
        arg.session_resume_args = Some(vec!["--rid".into(), "{session_id}".into()]);
        assert_eq!(
            headless_argv(
                &arg,
                "hello",
                Some(&HeadlessSession {
                    id: "S".into(),
                    resume: false
                })
            ),
            vec!["ask", "--sid", "S", "hello"]
        );
    }

    #[test]
    fn run_headless_captured_splices_the_session_args() {
        let mut spec = headless(
            &["sh", "-c", "printf '%s\\n' \"$@\"", "argv0"],
            HeadlessPrompt::Arg,
        );
        spec.session_new_args = Some(vec!["--session-id".into(), "{session_id}".into()]);
        spec.session_resume_args = Some(vec!["--resume".into(), "{session_id}".into()]);
        let session = HeadlessSession {
            id: "abc".into(),
            resume: false,
        };
        let capture =
            run_headless_captured(&spec, "q", Duration::from_secs(10), None, Some(&session))
                .unwrap();
        assert_eq!(capture.stdout, "--session-id\nabc\nq\n");
        let session = HeadlessSession {
            id: "abc".into(),
            resume: true,
        };
        let capture =
            run_headless_captured(&spec, "q", Duration::from_secs(10), None, Some(&session))
                .unwrap();
        assert_eq!(capture.stdout, "--resume\nabc\nq\n");
        let capture =
            run_headless_captured(&spec, "q", Duration::from_secs(10), None, None).unwrap();
        assert_eq!(capture.stdout, "q\n");
    }

    #[test]
    fn find_on_path_only_accepts_executable_regular_files() {
        let dir = std::env::temp_dir().join(format!("shep-runtimes-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain");
        std::fs::write(&plain, "").unwrap();
        assert_eq!(find_on_path_in("plain", Some(dir.as_os_str())), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
            assert_eq!(
                find_on_path_in("plain", Some(dir.as_os_str())),
                Some(plain.clone())
            );
            assert_eq!(
                find_on_path_in(plain.to_str().unwrap(), None),
                Some(plain.clone())
            );
        }
        assert_eq!(find_on_path_in("", Some(dir.as_os_str())), None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn run_headless_delivers_the_prompt_on_stdin_and_times_out() {
        let spec = headless(&["sh", "-c", "cat >/dev/null"], HeadlessPrompt::Stdin);
        let outcome = run_headless(&spec, "hello", Duration::from_secs(10), None).unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);

        let slow = headless(&["sh", "-c", "sleep 30"], HeadlessPrompt::Arg);
        let outcome = run_headless(&slow, "x", Duration::from_millis(200), None).unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, None);
    }

    #[test]
    fn run_headless_captured_returns_stdout_and_times_out() {
        let echo = headless(
            &[
                "sh",
                "-c",
                "read -r line; printf 'answer: %s\\n' \"$line\"; echo warn >&2",
            ],
            HeadlessPrompt::Stdin,
        );
        let capture =
            run_headless_captured(&echo, "hello", Duration::from_secs(10), None, None).unwrap();
        assert_eq!(capture.outcome.exit_code, Some(0));
        assert!(!capture.outcome.timed_out);
        assert_eq!(capture.stdout, "answer: hello\n");
        assert_eq!(capture.stderr, "warn\n");

        let slow = headless(&["sh", "-c", "echo early; sleep 30"], HeadlessPrompt::Arg);
        let capture =
            run_headless_captured(&slow, "x", Duration::from_millis(200), None, None).unwrap();
        assert!(capture.outcome.timed_out);
        assert_eq!(capture.outcome.exit_code, None);
        assert_eq!(
            capture.stdout, "early\n",
            "what arrived before the kill is kept"
        );

        let failing = headless(&["sh", "-c", "echo nope >&2; exit 3"], HeadlessPrompt::Arg);
        let capture =
            run_headless_captured(&failing, "x", Duration::from_secs(10), None, None).unwrap();
        assert_eq!(capture.outcome.exit_code, Some(3));
        assert_eq!(capture.stderr, "nope\n");
    }

    #[test]
    fn read_capped_output_keeps_the_head_and_says_so() {
        assert_eq!(
            read_capped_output("abcdef".as_bytes(), 3, "output"),
            "abc\n[shep truncated output after 3 bytes]"
        );
        assert_eq!(read_capped_output("abc".as_bytes(), 3, "output"), "abc");
        assert_eq!(read_capped_output("".as_bytes(), 3, "output"), "");
    }
}
