//! The runtime launch registry: what to run when a caller names a runtime
//! (`claude`, `opencode`, …) instead of spelling out argv.
//!
//! The facts live on the detection manifests (`[launch]` and `[headless]` in
//! `src/detect/manifests/*.toml`), so a runtime is described once — how to
//! recognise it on screen, how to start it, how to ask it one question. A
//! `[runtimes.<name>]` table in `config.toml` overrides either recipe. This
//! module is pure resolution; the `PATH` lookup is injected so it can be
//! tested without touching the machine, and nothing here spawns a process
//! except [`run_headless`], which the `shep runtime ask` CLI calls.

use std::collections::BTreeMap;
use std::io::Write;
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
        let env = override_for(name, overrides)
            .map(|over| over.env.clone().into_iter().collect())
            .unwrap_or_default();
        return Ok(ResolvedLaunch {
            argv: argv.clone(),
            env,
            bin_resolved,
            source: RecipeSource::Config,
        });
    }
    let Some(agent) = agent else {
        return Err(RuntimeResolveError::Unknown {
            name: name.to_string(),
        });
    };
    let Some(spec) = crate::detect::manifest::launch_spec(agent) else {
        return Err(RuntimeResolveError::NotLaunchable {
            name: name.to_string(),
            tried: Vec::new(),
        });
    };
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
/// it sets `headless_argv`, else the manifest's `[headless]`.
pub fn resolve_headless(
    name: &str,
    overrides: &BTreeMap<String, RuntimeOverrideConfig>,
) -> Result<(HeadlessSpec, RecipeSource), RuntimeResolveError> {
    let name = name.trim();
    if let Some(over) = override_for(name, overrides).filter(|over| !over.headless_argv.is_empty())
    {
        return Ok((
            HeadlessSpec {
                argv: over.headless_argv.clone(),
                prompt: over.headless_prompt.unwrap_or_default(),
                output: Default::default(),
            },
            RecipeSource::Config,
        ));
    }
    let Some(agent) = agent_for(name) else {
        return Err(RuntimeResolveError::Unknown {
            name: name.to_string(),
        });
    };
    crate::detect::manifest::headless_spec(agent)
        .map(|spec| (spec, RecipeSource::Manifest))
        .ok_or_else(|| RuntimeResolveError::NotLaunchable {
            name: name.to_string(),
            tried: Vec::new(),
        })
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

/// Run `spec` once with `prompt`, streaming the child's stdout and stderr to
/// ours. The prompt goes on stdin (closed after the write) or as the last
/// argument, per `spec.prompt`. On timeout the child is killed.
pub fn run_headless(
    spec: &HeadlessSpec,
    prompt: &str,
    timeout: Duration,
    cwd: Option<&Path>,
) -> std::io::Result<HeadlessOutcome> {
    let mut argv = spec.argv.clone();
    if spec.prompt == HeadlessPrompt::Arg {
        argv.push(prompt.to_string());
    }
    let mut command = Command::new(&argv[0]);
    command.args(&argv[1..]);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    command.stdout(Stdio::inherit()).stderr(Stdio::inherit());
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
    Ok(outcome)
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
        let spec = HeadlessSpec {
            argv: vec!["sh".into(), "-c".into(), "cat >/dev/null".into()],
            prompt: HeadlessPrompt::Stdin,
            output: Default::default(),
        };
        let outcome = run_headless(&spec, "hello", Duration::from_secs(10), None).unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);

        let slow = HeadlessSpec {
            argv: vec!["sh".into(), "-c".into(), "sleep 30".into()],
            prompt: HeadlessPrompt::Arg,
            output: Default::default(),
        };
        let outcome = run_headless(&slow, "x", Duration::from_millis(200), None).unwrap();
        assert!(outcome.timed_out);
        assert_eq!(outcome.exit_code, None);
    }
}
