//! The bundled overseer plugin against a throwaway server: link it, let an
//! agent state change fire its hook, and check that the deterministic
//! situation and board appear in the plugin's state dir. A second pass drives
//! the brain path through a fake runtime (`sh`, never a real CLI) and checks
//! that its proposals land in the docket exactly once. The forbidden-verb
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
    for file in ["overseer-tick", "overseer-board", "shep-plugin.toml"] {
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
    for script in ["overseer-tick", "overseer-board"] {
        use std::os::unix::fs::PermissionsExt;
        let mode = fs::metadata(Path::new(PLUGIN_DIR).join(script))
            .unwrap()
            .permissions()
            .mode();
        assert!(mode & 0o111 != 0, "{script} is not executable");
    }
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
    assert!(board.contains("BLOCKED — yours to answer"), "{board}");
    assert!(board.lines().count() <= 40, "{board}");
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
fn a_configured_brain_rewrites_the_board_and_captures_proposals_once() {
    // The "brain" is a shell script that ignores its prompt and answers with
    // a fixed board plus two proposals; `[plugins.overseer] runtime` names it
    // and `[runtimes.fake-brain]` says how to run it headlessly.
    let brain = unique_test_dir().join("brain.sh");
    fs::create_dir_all(brain.parent().unwrap()).unwrap();
    fs::write(
        &brain,
        r#"#!/bin/sh
cat >/dev/null
printf '%s\n' '```board' 'OVERSEER · brain' 'all quiet' '```' '```json' '[{"title":"rotate the xai key","source":{"kind":"situation","ref":"memory:12"},"notes":"owed since august"},{"title":"same source again","source":{"kind":"situation","ref":"memory:12"}}]' '```'
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
    assert_eq!(board, "OVERSEER · brain\nall quiet\n");
    assert!(state_dir.join("last-brain").exists());

    let listed = server.cli(&["docket", "list", "--json"]);
    let docket: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    let items = docket["result"]["items"].as_array().unwrap();
    assert_eq!(items.len(), 1, "dedupe by source failed: {items:?}");
    assert_eq!(items[0]["title"], "rotate the xai key");
    assert_eq!(items[0]["status"], "inbox");
    assert_eq!(items[0]["kind"], "captured");
    assert_eq!(items[0]["source"]["ref"], "memory:12");
    assert!(items[0]["due"].is_null());

    // A second event inside the ten-minute window refreshes the situation
    // but does not consult the brain again: the docket stays at one item and
    // the board stays the brain's.
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
    let listed = server.cli(&["docket", "list", "--json"]);
    let docket: serde_json::Value = serde_json::from_slice(&listed.stdout).unwrap();
    assert_eq!(docket["result"]["items"].as_array().unwrap().len(), 1);
    assert_eq!(
        fs::read_to_string(state_dir.join("BOARD.md")).unwrap(),
        "OVERSEER · brain\nall quiet\n"
    );
    let journal = fs::read_to_string(state_dir.join("journal.log")).unwrap();
    assert!(
        journal.contains("brain fake-brain · 1 captured"),
        "{journal}"
    );
    assert!(journal.contains("within its 10 min window"), "{journal}");

    server.finish();
}
