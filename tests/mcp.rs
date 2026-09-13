//! `shep mcp` against a throwaway server: a real stdio client session over
//! piped stdin/stdout, driven through the handshake, the tool list, and one
//! call of each backing kind (API, bridge-local, in-process).
//!
//! The point of the profile is proved twice here — `docket_promote` is absent
//! and refused, and `docket_add`'s due date is dropped while its `source`
//! survives when the `docket-inbox` group is granted on top of the overseer
//! profile — because that forcing is the whole reason a brain can be handed
//! tools at all. The `overseer` profile itself no longer carries
//! `docket-inbox` (2026-09-13: the docket is the person's own list).

mod support;

use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child as PtyChild, CommandBuilder, MasterPty, PtySize};
use serde_json::{json, Value};
use support::{
    cleanup_test_base, register_runtime_dir, register_spawned_shep_pid,
    unregister_spawned_shep_pid, wait_for_socket,
};

fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!("/tmp/hmcp-{}-{nanos}", std::process::id()))
}

fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "shep-dev"
    } else {
        "shep"
    }
}

struct SpawnedShep {
    _master: Box<dyn MasterPty + Send>,
    child: Box<dyn PtyChild + Send + Sync>,
}

impl Drop for SpawnedShep {
    fn drop(&mut self) {
        // Reap without blocking, exactly as `tests/api_ping.rs` does: a killed
        // PTY child can sit unreaped long enough to hang a blocking `wait()`
        // for the rest of the run.
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
        }
        unregister_spawned_shep_pid(pid);
    }
}

/// Copied from `tests/api_ping.rs::spawn_shep_with_options`: every XDG root and
/// the socket path are isolated, so a test never reads or writes the
/// developer's own state, docket or config.
fn spawn_server(config_home: &Path, runtime_dir: &Path, socket_path: &Path) -> SpawnedShep {
    let config_dir = config_home.join(app_dir_name());
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
    fs::write(config_dir.join("config.toml"), "onboarding = false\n").unwrap();

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
    cmd.env("XDG_CONFIG_HOME", config_home);
    cmd.env("XDG_RUNTIME_DIR", runtime_dir);
    cmd.env("XDG_STATE_HOME", config_home.with_file_name("state"));
    cmd.env("SHEP_SOCKET_PATH", socket_path);
    cmd.env_remove("SHEP_CLIENT_SOCKET_PATH");
    cmd.env_remove("HERDR_CLIENT_SOCKET_PATH");
    cmd.env_remove("SHEP_CONFIG_PATH");
    cmd.env_remove("HERDR_CONFIG_PATH");
    cmd.env("SHELL", "/bin/sh");
    cmd.env_remove("SHEP_ENV");
    cmd.env_remove("HERDR_ENV");
    let child = pair.slave.spawn_command(cmd).unwrap();
    register_spawned_shep_pid(child.process_id());
    SpawnedShep {
        _master: pair.master,
        child,
    }
}

fn shep_cli(config_home: &Path, socket_path: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_shep"));
    cmd.env("XDG_CONFIG_HOME", config_home)
        .env("XDG_STATE_HOME", config_home.with_file_name("state"))
        .env("SHEP_SOCKET_PATH", socket_path)
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("SHEP_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("SHEP_CONFIG_PATH")
        .env_remove("HERDR_CONFIG_PATH")
        .env_remove("SHEP_ENV")
        .env_remove("HERDR_ENV");
    cmd
}

/// One `shep mcp` process, spoken to the way a client does.
struct McpClient {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl McpClient {
    fn spawn(config_home: &Path, socket_path: &Path, profile: &str, extra: &[&str]) -> Self {
        let mut child = shep_cli(config_home, socket_path)
            .args(["mcp", "--socket"])
            .arg(socket_path)
            .args(["--profile", profile])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .unwrap();
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
        }
    }

    fn send(&mut self, message: Value) {
        writeln!(self.stdin, "{message}").unwrap();
        self.stdin.flush().unwrap();
    }

    fn read(&mut self) -> Value {
        let mut line = String::new();
        let read = self.stdout.read_line(&mut line).unwrap();
        assert!(read > 0, "shep mcp closed stdout early");
        serde_json::from_str(&line).unwrap_or_else(|err| panic!("not json-rpc: {line:?} ({err})"))
    }

    fn request(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
        let response = self.read();
        assert_eq!(response["id"], json!(id));
        response
    }

    fn call(&mut self, id: i64, name: &str, arguments: Value) -> Value {
        self.request(
            id,
            "tools/call",
            json!({"name": name, "arguments": arguments}),
        )
    }

    fn finish(mut self) {
        drop(self.stdin);
        let mut rest = String::new();
        let _ = self.stdout.read_to_string(&mut rest);
        let status = self.child.wait().unwrap();
        assert!(status.success(), "shep mcp exited with {status}");
    }
}

#[test]
fn mcp_serves_the_overseer_profile_over_stdio() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("shep.sock");
    let server = spawn_server(&config_home, &runtime_dir, &socket_path);
    wait_for_socket(&socket_path, Duration::from_secs(10));

    // The overseer profile plus the capture group, so the inbox forcing
    // (`docket_add` without `docket`) is still exercised end to end.
    let mut client = McpClient::spawn(
        &config_home,
        &socket_path,
        "overseer",
        &["--allow", "docket-inbox"],
    );

    let initialize = client.request(
        1,
        "initialize",
        json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
    );
    assert_eq!(initialize["result"]["protocolVersion"], json!("2025-06-18"));
    assert_eq!(initialize["result"]["serverInfo"]["name"], json!("shep"));

    // A notification gets no answer at all; the next read must be tools/list.
    client.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));

    let listed = client.request(2, "tools/list", json!({}));
    let names: Vec<String> = listed["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|tool| tool["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"session_overview".to_string()), "{names:?}");
    assert!(names.contains(&"docket_add".to_string()), "{names:?}");
    assert!(!names.contains(&"docket_promote".to_string()), "{names:?}");
    assert!(!names.contains(&"agent_send".to_string()), "{names:?}");

    // API backing.
    let overview = client.call(3, "session_overview", json!({}));
    assert_eq!(overview["result"]["isError"], json!(false));
    assert!(
        overview["result"]["structuredContent"]["agents"].is_array(),
        "{overview}"
    );

    // Capture forcing: the due date is dropped, the provenance is kept.
    let added = client.call(
        4,
        "docket_add",
        json!({
            "title": "rotate the xai key",
            "due": "2030-01-01",
            "source": {"file": "x", "line": 1},
        }),
    );
    assert_eq!(added["result"]["isError"], json!(false), "{added}");

    let output = shep_cli(&config_home, &socket_path)
        .args(["docket", "list", "--json"])
        .output()
        .unwrap();
    assert!(output.status.success(), "shep docket list failed");
    let listed: Value = serde_json::from_slice(&output.stdout).unwrap();
    let items = listed["result"]["items"].as_array().unwrap();
    let item = items
        .iter()
        .find(|item| item["title"] == json!("rotate the xai key"))
        .unwrap_or_else(|| panic!("the capture did not land: {listed}"));
    assert_eq!(item["status"], json!("inbox"));
    assert_eq!(item["kind"], json!("captured"));
    assert_eq!(item["source"], json!({"file": "x", "line": 1}));
    assert!(item.get("due").is_none(), "the due date survived: {item}");

    // A tool outside the profile is refused, and reads as unknown.
    let refused = client.call(5, "docket_promote", json!({"id": 1, "kind": "slated"}));
    assert_eq!(refused["error"]["code"], json!(-32602), "{refused}");
    assert!(refused["result"].is_null());

    // In-process backing.
    let doctor = client.call(6, "doctor", json!({}));
    assert!(
        doctor["result"]["structuredContent"]["findings"].is_array(),
        "{doctor}"
    );

    client.finish();
    drop(server);
    cleanup_test_base(&base);
}

#[test]
fn mcp_config_points_a_client_at_this_executable_and_socket() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let socket_path = base.join("runtime").join("shep.sock");
    fs::create_dir_all(config_home.join(app_dir_name())).unwrap();

    let output = shep_cli(&config_home, &socket_path)
        .args(["mcp", "config", "--profile", "overseer"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let value: Value = serde_json::from_slice(&output.stdout).unwrap();
    let server = &value["mcpServers"]["shep"];
    assert_eq!(server["command"], json!(env!("CARGO_BIN_EXE_shep")));
    assert_eq!(
        server["args"],
        json!([
            "mcp",
            "--profile",
            "overseer",
            "--socket",
            socket_path.display().to_string()
        ])
    );

    cleanup_test_base(&base);
}
