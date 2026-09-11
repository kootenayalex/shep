//! The runtime launch registry end to end: `shep runtime ask` against a fake
//! runtime manifest, and `agent.start { runtime }` / `runtime.list` over the
//! socket with a `[runtimes.<name>]` config override. No real coding-agent
//! CLI is ever invoked; every "runtime" here is `sh`.

mod support;

use std::fs;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use portable_pty::{native_pty_system, Child, CommandBuilder, MasterPty, PtySize};
use support::{
    cleanup_test_base, register_runtime_dir, register_spawned_shep_pid, unregister_spawned_shep_pid,
};

fn unique_test_dir() -> PathBuf {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    PathBuf::from(format!("/tmp/hrt-{}-{nanos}", std::process::id()))
}

fn app_dir_name() -> &'static str {
    if cfg!(debug_assertions) {
        "shep-dev"
    } else {
        "shep"
    }
}

/// A minimal but valid `pi` detection manifest carrying the sections under
/// test. Written to the local override path, so it shadows the bundled one
/// (which has neither section) for this test's config home only.
const PI_OVERRIDE_HEAD: &str = r#"
id = "pi"
version = "2099.01.01.1"
min_engine_version = 1
updated_at = "2099-01-01T00:00:00Z"
"#;
const PI_OVERRIDE_RULE: &str = r#"
[[rules]]
id = "working"
state = "working"
contains = ["Working"]
"#;

fn write_pi_override(config_home: &Path, sections: &str) {
    let dir = config_home.join(app_dir_name()).join("agent-detection");
    fs::create_dir_all(&dir).unwrap();
    fs::write(
        dir.join("pi.toml"),
        format!("{PI_OVERRIDE_HEAD}{sections}{PI_OVERRIDE_RULE}"),
    )
    .unwrap();
}

fn shep_cli(config_home: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_shep"));
    cmd.env("XDG_CONFIG_HOME", config_home)
        .env("XDG_STATE_HOME", config_home.with_file_name("state"))
        .env_remove("SHEP_SOCKET_PATH")
        .env_remove("HERDR_SOCKET_PATH")
        .env_remove("SHEP_CLIENT_SOCKET_PATH")
        .env_remove("HERDR_CLIENT_SOCKET_PATH")
        .env_remove("SHEP_CONFIG_PATH")
        .env_remove("HERDR_CONFIG_PATH")
        .env_remove("SHEP_ENV")
        .env_remove("HERDR_ENV");
    cmd
}

#[test]
fn runtime_ask_delivers_the_prompt_on_stdin_by_default() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    write_pi_override(
        &config_home,
        r#"
[headless]
argv = ["sh", "-c", "cat"]
"#,
    );

    let output = shep_cli(&config_home)
        .args(["runtime", "ask", "pi", "hello from the docket"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "hello from the docket"
    );

    // `-` reads the prompt from our stdin, which is how a plugin hands over a
    // situation too long for argv.
    let mut child = shep_cli(&config_home)
        .args(["runtime", "ask", "pi", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"line one\nline two\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "line one\nline two\n"
    );

    cleanup_test_base(&base);
}

#[test]
fn runtime_ask_passes_through_exit_codes_arg_prompts_and_timeouts() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    write_pi_override(
        &config_home,
        r#"
[headless]
argv = ["sh", "-c", "printf '%s' \"$1\"; exit 3", "sh"]
prompt = "arg"
"#,
    );

    let output = shep_cli(&config_home)
        .args(["runtime", "ask", "pi", "as an argument"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(3));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "as an argument");

    // A `[runtimes.<name>]` table in config.toml beats the manifest.
    let config_dir = config_home.join(app_dir_name());
    fs::write(
        config_dir.join("config.toml"),
        "onboarding = false\n[runtimes.pi]\nheadless_argv = [\"sh\", \"-c\", \"sleep 30\"]\n",
    )
    .unwrap();
    let started = Instant::now();
    let output = shep_cli(&config_home)
        .args(["runtime", "ask", "pi", "anything", "--timeout", "1"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(124));
    assert!(started.elapsed() < Duration::from_secs(10));
    assert!(String::from_utf8_lossy(&output.stderr).contains("did not answer within 1s"));

    // A runtime with no recipe is a clear error, not a crash.
    let output = shep_cli(&config_home)
        .args(["runtime", "ask", "kiro", "anything"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("runtime_not_launchable"));
    let output = shep_cli(&config_home)
        .args(["runtime", "ask", "not-a-runtime", "anything"])
        .stdin(Stdio::null())
        .output()
        .unwrap();
    assert_eq!(output.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&output.stderr).contains("runtime_unknown"));

    cleanup_test_base(&base);
}

// --- socket side: agent.start { runtime } and runtime.list -----------------

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

fn spawn_server(
    config_home: &Path,
    runtime_dir: &Path,
    socket_path: &Path,
    config: &str,
) -> SpawnedShep {
    let config_dir = config_home.join(app_dir_name());
    fs::create_dir_all(&config_dir).unwrap();
    fs::create_dir_all(runtime_dir).unwrap();
    register_runtime_dir(runtime_dir);
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

fn wait_for_socket(path: &Path, timeout: Duration) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if path.exists() && UnixStream::connect(path).is_ok() {
            return;
        }
        thread::sleep(Duration::from_millis(25));
    }
    panic!("socket did not appear at {}", path.display());
}

fn send_request(socket_path: &Path, json: &str) -> serde_json::Value {
    let mut stream = UnixStream::connect(socket_path).unwrap();
    stream.write_all(json.as_bytes()).unwrap();
    stream.write_all(b"\n").unwrap();
    stream.flush().unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
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

#[test]
fn agent_start_by_runtime_name_uses_the_config_override_and_lists_it() {
    let base = unique_test_dir();
    let config_home = base.join("config");
    let runtime_dir = base.join("runtime");
    let socket_path = runtime_dir.join("shep.sock");
    let child = spawn_server(
        &config_home,
        &runtime_dir,
        &socket_path,
        "onboarding = false\n\n[runtimes.pi]\nargv = [\"/bin/sh\", \"-c\", \"printf runtime-ok; sleep 2\"]\nenv = { PI_FROM_RECIPE = \"1\" }\n",
    );
    wait_for_socket(&socket_path, Duration::from_secs(5));

    let listed = send_request(
        &socket_path,
        r#"{"id":"rl","method":"runtime.list","params":{}}"#,
    );
    assert_eq!(listed["result"]["type"], "runtime_list");
    let runtimes = listed["result"]["runtimes"].as_array().unwrap();
    let pi = runtimes.iter().find(|r| r["name"] == "pi").unwrap();
    assert_eq!(pi["launchable"], true);
    assert_eq!(pi["bin_resolved"], "/bin/sh");
    assert_eq!(pi["headless"], false);
    // Every bundled manifest is listed, launchable or not.
    assert!(runtimes
        .iter()
        .any(|r| r["name"] == "kiro" && r["launchable"] == false));

    let started = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"as","method":"agent.start","params":{{"name":"by-name","cwd":"{}","runtime":"pi"}}}}"#,
            base.display()
        ),
    );
    assert_eq!(started["result"]["type"], "agent_started", "{started}");
    assert_eq!(started["result"]["argv"][0], "/bin/sh");
    assert_eq!(started["result"]["argv"][2], "printf runtime-ok; sleep 2");

    let both = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"both","method":"agent.start","params":{{"name":"both","cwd":"{}","runtime":"pi","argv":["/bin/sh"]}}}}"#,
            base.display()
        ),
    );
    assert_eq!(both["error"]["code"], "invalid_agent_argv");

    let unknown = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"unk","method":"agent.start","params":{{"name":"unk","cwd":"{}","runtime":"not-a-runtime"}}}}"#,
            base.display()
        ),
    );
    assert_eq!(unknown["error"]["code"], "runtime_unknown");

    let unlaunchable = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"kiro","method":"agent.start","params":{{"name":"kiro","cwd":"{}","runtime":"kiro"}}}}"#,
            base.display()
        ),
    );
    assert_eq!(unlaunchable["error"]["code"], "runtime_not_launchable");

    let neither = send_request(
        &socket_path,
        &format!(
            r#"{{"id":"neither","method":"agent.start","params":{{"name":"neither","cwd":"{}"}}}}"#,
            base.display()
        ),
    );
    assert_eq!(neither["error"]["code"], "invalid_agent_argv");

    drop(child);
    cleanup_test_base(&base);
}
