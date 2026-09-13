//! The bundled overseer plugin against a throwaway server: link it, let an
//! agent state change fire its hook, and check that the deterministic
//! situation and board appear in the plugin's state dir. A second pass drives
//! the brain path through a fake runtime (`sh`, never a real CLI) and checks
//! that its board is taken and nothing reaches the docket. The forbidden-verb
//! test pins the charter's hard rules to the script text itself.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use support::{
    cleanup_test_base, register_runtime_dir, register_spawned_shep_pid, unregister_spawned_shep_pid,
};

const PLUGIN_DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/plugins/overseer");

fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!("/tmp/hovs-{}-{nanos}", std::process::id()))
}

fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "shep-dev"
    } else {
        "shep"
    }
}

#[test]
fn overseer_script_never_uses_a_forbidden_verb() {
    // CHARTER hard rules 1–4: no pane input, no server surgery, no process
    // killing. Enforced on the text so a future edit cannot slip one in.
    let forbidden = regex::Regex::new(
        r"send-keys|send-text|send-input|server stop|live-handoff|pkill|agent send|kickstart|launchctl",
    )
    .unwrap();
    for file in [
        "overseer-tick",
        "overseer-board",
        "overseer-session",
        "shep-plugin.toml",
    ] {
        let text = fs::read_to_string(Path::new(PLUGIN_DIR).join(file)).unwrap();
        assert!(
            !forbidden.is_match(&text),
            "{file} contains a forbidden verb: {:?}",
            forbidden.find(&text).map(|m| m.as_str())
        );
    }
}

#[test]
fn overseer_manifest_declares_the_hooks_action_and_pane() {
    let manifest: toml::Value = toml::from_str(
        &fs::read_to_string(Path::new(PLUGIN_DIR).join("shep-plugin.toml")).unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["id"].as_str(), Some("overseer"));
    let events: Vec<&str> = manifest["events"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hook| hook["on"].as_str().unwrap())
        .collect();
    assert_eq!(events, vec!["pane.agent_status_changed", "pane.exited"]);
    for hook in manifest["events"].as_array().unwrap() {
        assert_eq!(hook["command"][0].as_str(), Some("./overseer-tick"));
        assert_eq!(hook["command"][1].as_str(), Some("--event"));
    }
    assert_eq!(manifest["actions"][0]["id"].as_str(), Some("tick"));
    assert_eq!(
        manifest["actions"][0]["command"].as_array().unwrap().len(),
        1
    );
    assert_eq!(manifest["panes"][0]["id"].as_str(), Some("board"));
    assert_eq!(manifest["panes"][1]["id"].as_str(), Some("session"));
    assert_eq!(manifest["panes"][1]["placement"].as_str(), Some("tab"));
    assert_eq!(
        manifest["panes"][1]["command"][0].as_str(),
        Some("./overseer-session")
    );
    for script in ["overseer-tick", "overseer-board", "overseer-session"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(Path::new(PLUGIN_DIR).join(script))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "{script} is not executable");
    }
}

/// The session launcher execs what it is told, in the directory it is told,
/// with the overseer's files in the environment; it never calls shep.
#[test]
fn overseer_session_execs_the_configured_agent_in_the_configured_dir() {
    let dir = unique_test_dir();
    let state_dir = dir.join("state");
    let cwd = dir.join("cwd");
    fs::create_dir_all(&state_dir).unwrap();
    fs::create_dir_all(&cwd).unwrap();
    let run = |env: &[(&str, String)]| {
        let mut cmd = std::process::Command::new(Path::new(PLUGIN_DIR).join("overseer-session"));
        cmd.env_remove("SHEP_OVERSEER_SESSION_ARGV")
            .env_remove("SHEP_OVERSEER_SESSION_ID")
            .env_remove("SHEP_OVERSEER_SESSION_RESUME")
            .env_remove("SHEP_OVERSEER_SESSION_CWD")
            .env_remove("SHEP_OVERSEER_MCP_CONFIG")
            .env_remove("SHEP_OVERSEER_MCP_ARGS")
            .env("SHEP_PLUGIN_STATE_DIR", &state_dir);
        for (key, value) in env {
            cmd.env(key, value);
        }
        cmd.output().unwrap()
    };

    // Config: argv and cwd from `[plugins.overseer]`.
    let config = serde_json::json!({
        "session_argv": ["sh", "-c", "pwd; echo $SHEP_OVERSEER_SITUATION; echo $SHEP_OVERSEER_BOARD; echo $SHEP_OVERSEER_CHAT; echo $SHEP_OVERSEER_STATE_DIR"],
        "session_cwd": cwd.display().to_string(),
    });
    let out = run(&[("SHEP_PLUGIN_CONFIG_JSON", config.to_string())]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    assert_eq!(
        lines,
        vec![
            fs::canonicalize(&cwd).unwrap().display().to_string(),
            state_dir.join("situation.md").display().to_string(),
            state_dir.join("BOARD.md").display().to_string(),
            state_dir.join("chat.jsonl").display().to_string(),
            state_dir.display().to_string(),
        ]
    );

    // The env override wins over config, and a missing cwd falls back to
    // the state dir.
    let config = serde_json::json!({
        "session_argv": ["false"],
        "session_cwd": dir.join("missing").display().to_string(),
    });
    let out = run(&[
        ("SHEP_PLUGIN_CONFIG_JSON", config.to_string()),
        ("SHEP_OVERSEER_SESSION_ARGV", "sh -c pwd".to_string()),
    ]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout).trim(),
        fs::canonicalize(&state_dir).unwrap().display().to_string()
    );

    // An agent that cannot start is one line on stderr and exit 1.
    let out = run(&[(
        "SHEP_OVERSEER_SESSION_ARGV",
        "definitely-not-an-agent-xyz".to_string(),
    )]);
    assert_eq!(out.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(stderr.lines().count(), 1, "{stderr}");
    assert!(stderr.starts_with("overseer-session: cannot run"));
    let _ = fs::remove_dir_all(&dir);
}

/// The MCP mount: shep names the config and the words that attach it, the
/// launcher only places them. A configured argv decides for itself with
/// `{mcp_config}`; the default one gets `SHEP_OVERSEER_MCP_ARGS` appended.
#[test]
fn overseer_session_mounts_the_mcp_config() {
    let dir = unique_test_dir();
    let state_dir = dir.join("state");
    let bin = dir.join("bin");
    for d in [&state_dir, &bin] {
        fs::create_dir_all(d).unwrap();
    }
    let fake = bin.join("claude");
    fs::write(
        &fake,
        "#!/bin/sh
for arg in \"$@\"; do echo \"$arg\"; done
",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path_var = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let config = state_dir.join("mcp.json");
    let run = |env: &[(&str, String)]| {
        let mut cmd = std::process::Command::new(Path::new(PLUGIN_DIR).join("overseer-session"));
        cmd.env_remove("SHEP_OVERSEER_SESSION_ARGV")
            .env_remove("SHEP_OVERSEER_SESSION_ID")
            .env_remove("SHEP_OVERSEER_SESSION_RESUME")
            .env_remove("SHEP_OVERSEER_SESSION_CWD")
            .env_remove("SHEP_OVERSEER_MCP_CONFIG")
            .env_remove("SHEP_OVERSEER_MCP_ARGS")
            .env_remove("SHEP_PLUGIN_CONFIG_JSON")
            .env("PATH", &path_var)
            .env("SHEP_PLUGIN_STATE_DIR", &state_dir);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    let id = "0f5a1c2e-7b1d-4e3a-9c8b-0123456789ab".to_string();
    let args = format!(
        "[\"--mcp-config\", \"{}\", \"--strict-mcp-config\"]",
        config.display()
    );

    // The default argv takes the words shep substituted, after the session.
    let out = run(&[
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_MCP_CONFIG", config.display().to_string()),
        ("SHEP_OVERSEER_MCP_ARGS", args.clone()),
    ]);
    assert_eq!(
        out,
        vec![
            "--resume".to_string(),
            id.clone(),
            "--mcp-config".to_string(),
            config.display().to_string(),
            "--strict-mcp-config".to_string(),
        ]
    );

    // An empty list is how "this runtime cannot take tools" arrives.
    let out = run(&[
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_MCP_CONFIG", config.display().to_string()),
        ("SHEP_OVERSEER_MCP_ARGS", "[]".to_string()),
    ]);
    assert_eq!(out, vec!["--resume".to_string(), id.clone()]);

    // Junk is taken as no tools rather than guessed at.
    let out = run(&[
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_MCP_ARGS", "not json".to_string()),
    ]);
    assert_eq!(out, vec!["--resume".to_string(), id.clone()]);

    // A configured argv places the config itself and is not appended to.
    let plugin_config = serde_json::json!({
        "session_argv": ["claude", "--resume={session_id}", "--mcp-config", "{mcp_config}"],
    });
    let out = run(&[
        ("SHEP_PLUGIN_CONFIG_JSON", plugin_config.to_string()),
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_MCP_CONFIG", config.display().to_string()),
        ("SHEP_OVERSEER_MCP_ARGS", args),
    ]);
    assert_eq!(
        out,
        vec![
            format!("--resume={id}"),
            "--mcp-config".to_string(),
            config.display().to_string(),
        ]
    );
    let _ = fs::remove_dir_all(&dir);
}

/// The two halves of the mount, pinned to the scripts: the brain asks with a
/// profile, and the session launcher reads the env shep sets. Either spelling
/// drifting on its own is a mount that silently serves no tools.
#[test]
fn overseer_scripts_name_the_mcp_profile_and_env() {
    let tick = fs::read_to_string(Path::new(PLUGIN_DIR).join("overseer-tick")).unwrap();
    assert!(
        tick.contains("\"--mcp-profile\""),
        "the brain ask no longer passes --mcp-profile"
    );
    assert!(tick.contains("MCP_PROFILE = \"overseer\""));
    assert!(
        tick.contains("mcp__shep__*"),
        "the brain prompt no longer says which tools exist"
    );

    let session = fs::read_to_string(Path::new(PLUGIN_DIR).join("overseer-session")).unwrap();
    for name in ["SHEP_OVERSEER_MCP_CONFIG", "SHEP_OVERSEER_MCP_ARGS"] {
        assert!(session.contains(name), "overseer-session lost {name}");
    }
    assert!(session.contains("{mcp_config}"));
}

/// One session, two faces: with shep's session env the launcher runs the
/// board's conversation — `claude --session-id <id>` the first time,
/// `claude --resume <id>` after — substitutes `{session_id}` into a
/// configured argv, and runs where shep says.
#[test]
fn overseer_session_shares_the_boards_conversation() {
    let dir = unique_test_dir();
    let state_dir = dir.join("state");
    let shep_cwd = dir.join("shep-cwd");
    let config_cwd = dir.join("config-cwd");
    let bin = dir.join("bin");
    for d in [&state_dir, &shep_cwd, &config_cwd, &bin] {
        fs::create_dir_all(d).unwrap();
    }
    // A `claude` on PATH that prints its argv and cwd, so the default can
    // be seen without the real thing.
    let fake = bin.join("claude");
    fs::write(
        &fake,
        "#!/bin/sh\npwd\nfor arg in \"$@\"; do echo \"$arg\"; done\n",
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&fake, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let path_var = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let run = |env: &[(&str, String)]| {
        let mut cmd = std::process::Command::new(Path::new(PLUGIN_DIR).join("overseer-session"));
        cmd.env_remove("SHEP_OVERSEER_SESSION_ARGV")
            .env_remove("SHEP_OVERSEER_SESSION_ID")
            .env_remove("SHEP_OVERSEER_SESSION_RESUME")
            .env_remove("SHEP_OVERSEER_SESSION_CWD")
            .env_remove("SHEP_OVERSEER_MCP_CONFIG")
            .env_remove("SHEP_OVERSEER_MCP_ARGS")
            .env_remove("SHEP_PLUGIN_CONFIG_JSON")
            .env("PATH", &path_var)
            .env("SHEP_PLUGIN_STATE_DIR", &state_dir);
        for (key, value) in env {
            cmd.env(key, value);
        }
        let out = cmd.output().unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout)
            .lines()
            .map(str::to_string)
            .collect::<Vec<String>>()
    };
    let canon = |p: &Path| fs::canonicalize(p).unwrap().display().to_string();
    let id = "0f5a1c2e-7b1d-4e3a-9c8b-0123456789ab".to_string();

    // No id at all: plain `claude`, in the state dir.
    assert_eq!(run(&[]), vec![canon(&state_dir)]);

    // A new conversation, where shep says.
    let out = run(&[
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "0".to_string()),
        ("SHEP_OVERSEER_SESSION_CWD", shep_cwd.display().to_string()),
    ]);
    assert_eq!(
        out,
        vec![canon(&shep_cwd), "--session-id".to_string(), id.clone()]
    );

    // A begun one is resumed.
    let out = run(&[
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_SESSION_CWD", shep_cwd.display().to_string()),
    ]);
    assert_eq!(
        out,
        vec![canon(&shep_cwd), "--resume".to_string(), id.clone()]
    );

    // shep's cwd wins over the config's; without it the config's holds.
    let config = serde_json::json!({
        "session_argv": ["claude", "--model", "opus", "--resume={session_id}"],
        "session_cwd": config_cwd.display().to_string(),
    });
    let out = run(&[
        ("SHEP_PLUGIN_CONFIG_JSON", config.to_string()),
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_RESUME", "1".to_string()),
        ("SHEP_OVERSEER_SESSION_CWD", shep_cwd.display().to_string()),
    ]);
    assert_eq!(
        out,
        vec![
            canon(&shep_cwd),
            "--model".to_string(),
            "opus".to_string(),
            format!("--resume={id}"),
        ],
        "a configured argv substitutes the id and is not second-guessed"
    );
    let out = run(&[
        ("SHEP_PLUGIN_CONFIG_JSON", config.to_string()),
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
    ]);
    assert_eq!(out[0], canon(&config_cwd));

    // The env argv substitutes too, and wins over the config.
    let out = run(&[
        ("SHEP_PLUGIN_CONFIG_JSON", config.to_string()),
        (
            "SHEP_OVERSEER_SESSION_ARGV",
            "claude -r {session_id}".to_string(),
        ),
        ("SHEP_OVERSEER_SESSION_ID", id.clone()),
        ("SHEP_OVERSEER_SESSION_CWD", shep_cwd.display().to_string()),
    ]);
    assert_eq!(out, vec![canon(&shep_cwd), "-r".to_string(), id]);
    let _ = fs::remove_dir_all(&dir);
}

// --- server harness --------------------------------------------------------

struct SpawnedShep {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn Child + Send + Sync>,
}

impl Drop for SpawnedShep {
    fn drop(&mut self) {
        let pid = self.child.process_id();
        let _ = self.child.kill();
        if let Some(pid) = pid {
            let deadline = Instant::now() + Duration::from_secs(2);
            while Instant::now() < deadline {
                let mut status = 0;
                let result =
                    unsafe { libc::waitpid(pid as libc::pid_t, &mut status, libc::WNOHANG) };
                if result == pid as libc::pid_t || result == -1 {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
            unregister_spawned_shep_pid(Some(pid));
        }
    }
}

struct TestServer {
    base: PathBuf,
    config_home: PathBuf,
    socket_path: PathBuf,
    child: Option<SpawnedShep>,
}

impl TestServer {
    fn spawn(config: &str, extra_env: &[(&str, &str)]) -> Self {
        let base = unique_test_dir();
        let config_home = base.join("config");
        let runtime_dir = base.join("runtime");
        let socket_path = runtime_dir.join("shep.sock");
        let config_dir = config_home.join(app_dir_name());
        fs::create_dir_all(&config_dir).unwrap();
        fs::create_dir_all(&runtime_dir).unwrap();
        register_runtime_dir(&runtime_dir);
        fs::write(config_dir.join("config.toml"), config).unwrap();

        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 24,
                cols: 80,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut cmd = CommandBuilder::new(env!("CARGO_BIN_EXE_shep"));
        cmd.arg("server");
        cmd.env("XDG_CONFIG_HOME", &config_home);
        cmd.env("XDG_RUNTIME_DIR", &runtime_dir);
        cmd.env("XDG_STATE_HOME", Self::state_home(&config_home));
        cmd.env("SHEP_SOCKET_PATH", &socket_path);
        cmd.env_remove("SHEP_CLIENT_SOCKET_PATH");
        cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
        cmd.env_remove("SHEP_CONFIG_PATH");
        cmd.env_remove("HERDR_CONFIG_PATH");
        cmd.env_remove("SHEP_OVERSEER_RUNTIME");
        cmd.env("SHELL", "/bin/sh");
        cmd.env_remove("SHEP_ENV");
        cmd.env_remove("HERDR_ENV");
        for (key, value) in extra_env {
            cmd.env(key, value);
        }
        let child = pair.slave.spawn_command(cmd).unwrap();
        register_spawned_shep_pid(child.process_id());
        let server = Self {
            base,
            config_home,
            socket_path,
            child: Some(SpawnedShep {
                _master: pair.master,
                child,
            }),
        };
        server.wait_for_socket(Duration::from_secs(5));
        server
    }

    fn state_home(config_home: &Path) -> PathBuf {
        config_home.with_file_name("state")
    }

    /// Where the server puts the overseer's files: `<state dir>/plugins/overseer`.
    fn overseer_state_dir(&self) -> PathBuf {
        Self::state_home(&self.config_home)
            .join(app_dir_name())
            .join("plugins")
            .join("overseer")
    }

    fn wait_for_socket(&self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.socket_path.exists() && UnixStream::connect(&self.socket_path).is_ok() {
                return;
            }
            thread::sleep(Duration::from_millis(25));
        }
        panic!("socket did not appear at {}", self.socket_path.display());
    }

    fn request(&self, json: &str) -> serde_json::Value {
        let mut stream = UnixStream::connect(&self.socket_path).unwrap();
        stream.write_all(json.as_bytes()).unwrap();
        stream.write_all(b"\n").unwrap();
        stream.flush().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let mut buf = Vec::new();
        let mut byte = [0u8; 1];
        loop {
            match stream.read(&mut byte) {
                Ok(0) => break,
                Ok(_) if byte[0] == b'\n' => break,
                Ok(_) => buf.push(byte[0]),
                Err(err) => panic!("read failed: {err}"),
            }
        }
        serde_json::from_slice(&buf).unwrap()
    }

    fn cli(&self, args: &[&str]) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_shep"))
            .args(args)
            .env("XDG_CONFIG_HOME", &self.config_home)
            .env("XDG_STATE_HOME", Self::state_home(&self.config_home))
            .env("SHEP_SOCKET_PATH", &self.socket_path)
            .env_remove("SHEP_CLIENT_SOCKET_PATH")
            .env_remove("HERDR_CLIENT_SOCKET_PATH")
            .env_remove("SHEP_CONFIG_PATH")
            .env_remove("HERDR_CONFIG_PATH")
            .env_remove("SHEP_ENV")
            .env_remove("HERDR_ENV")
            .output()
            .unwrap()
    }

    fn link_overseer(&self) {
        let linked = self.request(&format!(
            r#"{{"id":"link","method":"plugin.link","params":{{"path":"{PLUGIN_DIR}"}}}}"#
        ));
        assert_eq!(linked["result"]["type"], "plugin_linked", "{linked}");
        assert_eq!(linked["result"]["plugin"]["plugin_id"], "overseer");
        let warnings = linked["result"]["plugin"]["warnings"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        assert!(warnings.is_empty(), "manifest warnings: {warnings:?}");
    }

    /// A workspace with one shell pane whose agent state we can set by hand.
    fn create_pane(&self) -> String {
        let created = self.request(&format!(
            r#"{{"id":"ws","method":"workspace.create","params":{{"cwd":"{}","focus":true}}}}"#,
            self.base.display()
        ));
        assert_eq!(created["result"]["type"], "workspace_created", "{created}");
        let workspace_id = created["result"]["workspace"]["workspace_id"]
            .as_str()
            .unwrap()
            .to_string();
        format!("{workspace_id}:p1")
    }

    fn report_agent(&self, pane_id: &str, state: &str) {
        let reported = self.request(&format!(
            r#"{{"id":"rep","method":"pane.report_agent","params":{{"pane_id":"{pane_id}","source":"shep:pi","agent":"pi","state":"{state}"}}}}"#
        ));
        assert_eq!(reported["result"]["type"], "ok", "{reported}");
    }

    fn wait_for_file(&self, path: &Path, timeout: Duration) -> String {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(text) = fs::read_to_string(path) {
                if !text.is_empty() {
                    return text;
                }
            }
            thread::sleep(Duration::from_millis(50));
        }
        let logs = self.request(
            r#"{"id":"logs","method":"plugin.log.list","params":{"plugin_id":"overseer","limit":5}}"#,
        );
        panic!("{} never appeared; plugin logs: {logs}", path.display());
    }

    fn finish(mut self) {
        drop(self.child.take());
        cleanup_test_base(&self.base);
    }
}

#[test]
fn agent_status_change_makes_the_overseer_write_a_deterministic_board() {
    let server = TestServer::spawn("onboarding = false\n", &[]);
    server.link_overseer();
    let pane_id = server.create_pane();
    server.report_agent(&pane_id, "blocked");

    let state_dir = server.overseer_state_dir();
    let situation = server.wait_for_file(&state_dir.join("situation.md"), Duration::from_secs(60));
    assert!(situation.starts_with("# situation "), "{situation}");
    assert!(
        situation.contains("trigger: pane.agent_status_changed"),
        "{situation}"
    );
    assert!(situation.contains("## agents"), "{situation}");
    assert!(situation.contains("blocked"), "{situation}");

    let board = server.wait_for_file(&state_dir.join("BOARD.md"), Duration::from_secs(60));
    assert!(board.starts_with("OVERSEER · "), "{board}");
    assert!(board.contains("deterministic"), "{board}");
    assert!(board.contains("blocked"), "{board}");
    assert!(board.lines().count() <= 12, "{board}");
    let facts: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(state_dir.join("situation.json")).unwrap())
            .unwrap();
    assert_eq!(facts["event"], "pane.agent_status_changed");
    assert!(facts["health"].is_array());
    assert!(facts["docket"]["items"].is_array());

    // No brain configured: nothing was captured and no brain stamp exists.
    assert!(!state_dir.join("last-brain").exists());
    let listed = server.cli(&["docket", "list", "--json"]);
    let docket: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert!(docket["result"]["items"].as_array().unwrap().is_empty());

    // The board pane opens from the manifest and shows the same file.
    let pane = server.request(
        r#"{"id":"pane","method":"plugin.pane.open","params":{"plugin_id":"overseer","entrypoint":"board","focus":false}}"#,
    );
    assert_eq!(pane["result"]["type"], "plugin_pane_opened", "{pane}");
    let board_pane_id = pane["result"]["plugin_pane"]["pane"]["pane_id"]
        .as_str()
        .unwrap()
        .to_string();
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let read = server.request(&format!(
            r#"{{"id":"read","method":"pane.read","params":{{"pane_id":"{board_pane_id}","source":"visible","format":"text"}}}}"#
        ));
        let text = read["result"]["read"]["text"].as_str().unwrap_or("");
        if text.contains("OVERSEER ·") {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "board pane never showed the board: {read}"
        );
        thread::sleep(Duration::from_millis(100));
    }

    // The action is the same tick by hand; the log shows it ran.
    let invoked = server.request(
        r#"{"id":"tick","method":"plugin.action.invoke","params":{"plugin_id":"overseer","action_id":"tick"}}"#,
    );
    assert_eq!(
        invoked["result"]["type"], "plugin_action_invoked",
        "{invoked}"
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let logs = server.request(
            r#"{"id":"logs","method":"plugin.log.list","params":{"plugin_id":"overseer","limit":10}}"#,
        );
        let done = logs["result"]["logs"]
            .as_array()
            .unwrap()
            .iter()
            .any(|log| log["action_id"] == "tick" && log["status"] == "succeeded");
        if done {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "tick action never succeeded: {logs}"
        );
        thread::sleep(Duration::from_millis(100));
    }

    server.finish();
}

#[test]
fn a_configured_brain_rewrites_the_board_and_never_writes_the_docket() {
    // The "brain" is a shell script that ignores its prompt and answers with
    // a fixed, sectioned board — and, the way an older prompt's habit would,
    // a second fenced block of proposals. `[plugins.overseer] runtime` names
    // it and `[runtimes.fake-brain]` says how to run it headlessly. The
    // board is taken; the proposals reach nothing.
    let brain = unique_test_dir().join("brain.sh");
    fs::create_dir_all(brain.parent().unwrap()).unwrap();
    fs::write(
        &brain,
        r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '```board' 'OVERSEER · brain' '## claude' 'working on the thing.' '## room' 'all quiet' '```' '```json' '[{"title":"rotate the xai key","source":{"kind":"situation","ref":"memory:12"},"notes":"owed since august"}]' '```'
"#,
    )
    .unwrap();
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&brain, fs::Permissions::from_mode(0o755)).unwrap();
    }
    let config = format!(
        "onboarding = false\n\n[plugins.overseer]\nruntime = \"fake-brain\"\n\n[runtimes.fake-brain]\nheadless_argv = [\"{}\"]\n",
        brain.display()
    );
    let server = TestServer::spawn(&config, &[]);
    server.link_overseer();
    let pane_id = server.create_pane();
    server.report_agent(&pane_id, "working");

    let state_dir = server.overseer_state_dir();
    let board = server.wait_for_file(&state_dir.join("BOARD.md"), Duration::from_secs(60));
    assert_eq!(
        board,
        "OVERSEER · brain\n## claude\nworking on the thing.\n## room\nall quiet\n"
    );
    assert!(state_dir.join("last-brain").exists());

    let listed = server.cli(&["docket", "list", "--json"]);
    let docket: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let items = docket["result"]["items"].as_array().unwrap();
    assert!(
        items.is_empty(),
        "the overseer proposes nothing into the docket: {items:?}"
    );

    // A second event inside the ten-minute window refreshes the situation
    // but does not consult the brain again: the board stays the brain's.
    let before = fs::read_to_string(state_dir.join("situation.md")).unwrap();
    server.report_agent(&pane_id, "idle");
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let now = fs::read_to_string(state_dir.join("situation.md")).unwrap_or_default();
        if now != before && now.contains("idle") {
            break;
        }
        assert!(Instant::now() < deadline, "situation.md was not refreshed");
        thread::sleep(Duration::from_millis(50));
    }
    assert_eq!(
        fs::read_to_string(state_dir.join("BOARD.md")).unwrap(),
        "OVERSEER · brain\n## claude\nworking on the thing.\n## room\nall quiet\n"
    );
    let journal = fs::read_to_string(state_dir.join("journal.log")).unwrap();
    assert!(journal.contains("brain fake-brain"), "{journal}");
    assert!(!journal.contains("captured"), "{journal}");
    assert!(journal.contains("within its 10 min window"), "{journal}");

    server.finish();
}
