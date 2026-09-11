//! `shep doctor` — one deterministic pass over everything that can quietly
//! break a shep install: the server, the socket, launchd, the bridge and its
//! token, the agent hooks, push, leftover state, the error log, disk and the
//! docket. Every probe prints one `ok|warn|fail <check>: <detail>` line and a
//! `fix:` hint for anything that is not ok. No model, no lsof: each probe
//! uses the same call shep itself uses (a socket connect, a TCP connect, a
//! `launchctl print`).
//!
//! The classifiers are pure functions over facts the probes gather, so each
//! one is tested with fixtures rather than a live box. The command exits 1
//! only on a `fail`; warnings are advice, not a broken install.

use std::io;
use std::net::SocketAddr;
use std::path::Path;
use std::time::{Duration, SystemTime};

use serde::Serialize;

use super::status::{read_server_runtime_status, ServerRuntimeStatus};

pub(super) fn run_doctor_command(args: &[String]) -> io::Result<i32> {
    let json = match args {
        [] => false,
        [flag] if flag == "--json" => true,
        [flag] if matches!(flag.as_str(), "help" | "--help" | "-h") => {
            print_doctor_help();
            return Ok(0);
        }
        _ => {
            print_doctor_help();
            return Ok(2);
        }
    };

    let findings = run_probes();
    if json {
        println!("{}", serde_json::to_string(&findings)?);
    } else {
        print_findings(&findings);
    }
    Ok(exit_code(&findings))
}

fn print_doctor_help() {
    eprintln!(
        "shep doctor [--json]  check the server, socket, launchd, bridge, hooks, push and state"
    );
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum Level {
    Ok,
    Warn,
    Fail,
}

impl Level {
    fn label(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Warn => "warn",
            Level::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub(crate) struct Finding {
    pub level: Level,
    pub check: &'static str,
    pub detail: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fix: Option<String>,
}

impl Finding {
    fn ok(check: &'static str, detail: impl Into<String>) -> Self {
        Self {
            level: Level::Ok,
            check,
            detail: detail.into(),
            fix: None,
        }
    }

    fn warn(check: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Warn,
            check,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }

    fn fail(check: &'static str, detail: impl Into<String>, fix: impl Into<String>) -> Self {
        Self {
            level: Level::Fail,
            check,
            detail: detail.into(),
            fix: Some(fix.into()),
        }
    }
}

fn exit_code(findings: &[Finding]) -> i32 {
    if findings.iter().any(|f| f.level == Level::Fail) {
        1
    } else {
        0
    }
}

fn print_findings(findings: &[Finding]) {
    let width = findings
        .iter()
        .map(|f| f.check.len() + 1)
        .max()
        .unwrap_or(0);
    for finding in findings {
        let check = format!("{}:", finding.check);
        println!(
            "{:<4} {check:<width$}  {}",
            finding.level.label(),
            finding.detail
        );
        if let Some(fix) = &finding.fix {
            println!("{:<4} {:<width$}  fix: {fix}", "", "");
        }
    }
}

// ---------------------------------------------------------------------------
// Probes: gather facts from the live box, hand them to the classifiers.
// ---------------------------------------------------------------------------

const BRIDGE_CONNECT_TIMEOUT: Duration = Duration::from_millis(750);
const ERR_LOG_FRESH_WINDOW: Duration = Duration::from_secs(24 * 60 * 60);
const DISK_WARN_GIB: u64 = 15;
const LAUNCHD_JOBS: [&str; 2] = ["dev.shep.server", "dev.shep.bridge"];

fn run_probes() -> Vec<Finding> {
    let config_dir = crate::config::config_dir();
    let state_dir = crate::config::state_dir();
    let mut findings = Vec::new();

    // 1. server
    let server = read_server_runtime_status();
    findings.push(classify_server(
        &server,
        &crate::build_info::version(),
        crate::protocol::PROTOCOL_VERSION,
    ));

    // 2. socket holder
    let socket_path = crate::api::socket_path();
    findings.push(classify_socket(&socket_path, probe_socket(&socket_path)));

    // 3. launchd
    findings.extend(probe_launchd());

    // 4. bridge
    let addr = std::fs::read_to_string(config_dir.join("bridge-addr"))
        .ok()
        .map(|text| text.trim().to_string())
        .filter(|text| !text.is_empty());
    findings.push(classify_bridge(
        addr.as_deref(),
        tcp_listening,
        &launchd_domain(),
    ));

    // 5. bridge-token
    findings.push(classify_token(&config_dir.join("bridge-token")));

    // 6. integrations
    findings.push(classify_integration(
        &crate::integration::installed_integration_statuses(),
    ));

    // 7. memory hooks
    findings.push(probe_memory_hooks());

    // 8. push
    findings.push(classify_push(&config_dir));

    // 9. stale state
    findings.extend(classify_stale_state(&state_dir));

    // 10. err log
    findings.push(classify_err_log(
        &config_dir.join("launchd-server.err"),
        SystemTime::now(),
    ));

    // 11. disk
    findings.push(classify_disk(
        &config_dir,
        crate::platform::free_disk_bytes(&config_dir),
    ));

    // 12. docket
    findings.push(classify_docket(&crate::docket::docket_db_path()));

    findings
}

// 1. server -----------------------------------------------------------------

fn classify_server(
    server: &io::Result<ServerRuntimeStatus>,
    client_version: &str,
    client_protocol: u32,
) -> Finding {
    match server {
        Ok(ServerRuntimeStatus::Running {
            version, protocol, ..
        }) => {
            let version_label = version.as_deref().unwrap_or("unknown");
            let protocol_label = protocol
                .map(|p| p.to_string())
                .unwrap_or_else(|| "unknown".to_string());
            let detail = format!("running v{version_label}, protocol {protocol_label}");
            match protocol {
                Some(p) if *p != client_protocol => Finding::warn(
                    "server",
                    format!("{detail} — client protocol {client_protocol} ≠ server (restart_needed)"),
                    "restart the server: shep server live-handoff, or launchctl kickstart -k gui/$UID/dev.shep.server",
                ),
                _ if version.as_deref().is_some_and(|v| v != client_version) => Finding::warn(
                    "server",
                    format!("{detail} — client v{client_version} (restart_needed)"),
                    "restart the server: shep server live-handoff, or launchctl kickstart -k gui/$UID/dev.shep.server",
                ),
                _ => Finding::ok("server", detail),
            }
        }
        Ok(ServerRuntimeStatus::NotRunning) => Finding::fail(
            "server",
            "not running",
            "shep server, or launchctl kickstart -k gui/$UID/dev.shep.server",
        ),
        Err(err) => Finding::fail(
            "server",
            format!("ping failed: {err}"),
            "shep status server; if the socket is stale, restart the server",
        ),
    }
}

// 2. socket holder ----------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum SocketProbe {
    /// No socket file at the path.
    Missing,
    /// Connect succeeded: some process is accepting on it.
    Held,
    /// A file is there but nothing accepts (refused / timed out).
    Stale,
    /// Connect failed for another reason.
    Error(String),
}

fn probe_socket(path: &Path) -> SocketProbe {
    if !path.exists() {
        return SocketProbe::Missing;
    }
    match crate::ipc::connect_local_stream(path) {
        Ok(_) => SocketProbe::Held,
        Err(err)
            if matches!(
                err.kind(),
                io::ErrorKind::ConnectionRefused
                    | io::ErrorKind::NotFound
                    | io::ErrorKind::TimedOut
            ) =>
        {
            SocketProbe::Stale
        }
        Err(err) => SocketProbe::Error(err.to_string()),
    }
}

fn classify_socket(path: &Path, probe: SocketProbe) -> Finding {
    let shown = path.display();
    match probe {
        SocketProbe::Held => Finding::ok("socket", format!("held: {shown}")),
        SocketProbe::Missing => Finding::fail(
            "socket",
            format!("no socket file at {shown}"),
            "start the server: shep server, or launchctl kickstart -k gui/$UID/dev.shep.server",
        ),
        SocketProbe::Stale => Finding::fail(
            "socket",
            format!("stale socket file: nothing accepts on {shown}"),
            "restart the server (it removes the stale file on bind)",
        ),
        SocketProbe::Error(err) => Finding::fail(
            "socket",
            format!("connect to {shown} failed: {err}"),
            "check permissions on the socket and its directory",
        ),
    }
}

// 3. launchd ----------------------------------------------------------------

/// The facts `launchctl print gui/<uid>/<label>` gives us, parsed from its
/// tab-indented `key = value` body.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct LaunchdJob {
    pub state: Option<String>,
    pub pid: Option<u32>,
    pub runs: Option<u32>,
}

fn parse_launchctl_print(text: &str) -> LaunchdJob {
    let mut job = LaunchdJob::default();
    for line in text.lines() {
        // Only top-level keys are one tab deep; nested blocks (arguments,
        // environment) are deeper and must not be mistaken for them.
        let Some(rest) = line.strip_prefix('\t') else {
            continue;
        };
        if rest.starts_with('\t') {
            continue;
        }
        let Some((key, value)) = rest.split_once(" = ") else {
            continue;
        };
        match key.trim() {
            "state" => job.state = Some(value.trim().to_string()),
            "pid" => job.pid = value.trim().parse().ok(),
            "runs" => job.runs = value.trim().parse().ok(),
            _ => {}
        }
    }
    job
}

fn classify_launchd(label: &str, print: Option<&str>, domain: &str) -> Finding {
    let Some(text) = print else {
        return Finding::warn(
            "launchd",
            format!("{label} not loaded"),
            format!("launchctl bootstrap {domain} ~/Library/LaunchAgents/{label}.plist"),
        );
    };
    let job = parse_launchctl_print(text);
    let runs = job
        .runs
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_string());
    match job.pid {
        Some(pid) => Finding::ok("launchd", format!("{label} pid {pid} runs={runs}")),
        None => Finding::warn(
            "launchd",
            format!(
                "{label} loaded with no pid (state {}, runs={runs}) — KeepAlive spin? see docs",
                job.state.as_deref().unwrap_or("unknown")
            ),
            format!("tail <config>/launchd-*.err, then launchctl kickstart -k {domain}/{label}"),
        ),
    }
}

#[cfg(target_os = "macos")]
fn probe_launchd() -> Vec<Finding> {
    let domain = launchd_domain();
    LAUNCHD_JOBS
        .iter()
        .map(|label| {
            let print = crate::platform::launchd_print(label);
            classify_launchd(label, print.as_deref(), &domain)
        })
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn probe_launchd() -> Vec<Finding> {
    vec![Finding::ok("launchd", "n/a")]
}

#[cfg(target_os = "macos")]
fn launchd_domain() -> String {
    crate::platform::launchd_domain()
}

#[cfg(not(target_os = "macos"))]
fn launchd_domain() -> String {
    "gui/$UID".to_string()
}

// 4. bridge -----------------------------------------------------------------

fn tcp_listening(addr: SocketAddr) -> bool {
    std::net::TcpStream::connect_timeout(&addr, BRIDGE_CONNECT_TIMEOUT).is_ok()
}

fn classify_bridge(
    addr: Option<&str>,
    listening: impl Fn(SocketAddr) -> bool,
    domain: &str,
) -> Finding {
    let Some(addr) = addr else {
        return Finding::warn(
            "bridge",
            "no <config>/bridge-addr — the bridge has not served on this binary",
            format!(
                "launchctl kickstart -k {domain}/dev.shep.bridge (or shep bridge --bind <ip:port>)"
            ),
        );
    };
    let Ok(parsed) = addr.parse::<SocketAddr>() else {
        return Finding::fail(
            "bridge",
            format!("bridge-addr holds {addr:?}, not an ip:port"),
            "remove <config>/bridge-addr and restart the bridge so it rewrites the file",
        );
    };
    if listening(parsed) {
        Finding::ok("bridge", format!("listening on {addr}"))
    } else {
        Finding::fail(
            "bridge",
            format!("bridge-addr says {addr} but nothing listens"),
            format!("launchctl kickstart -k {domain}/dev.shep.bridge"),
        )
    }
}

// 5. bridge-token -----------------------------------------------------------

const TOKEN_LEN: usize = 43;

fn classify_token(path: &Path) -> Finding {
    let content = match std::fs::read_to_string(path) {
        Ok(content) => content,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            return Finding::warn(
                "bridge-token",
                format!("missing: {}", path.display()),
                "the bridge mints one on its first run: shep bridge",
            );
        }
        Err(err) => {
            return Finding::fail(
                "bridge-token",
                format!("unreadable {}: {err}", path.display()),
                "fix ownership/permissions, or remove it and restart the bridge",
            );
        }
    };
    let token = content.trim();
    let well_formed = token.len() == TOKEN_LEN
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if !well_formed {
        return Finding::warn(
            "bridge-token",
            format!(
                "malformed: {} chars, want {TOKEN_LEN} of base64url",
                token.len()
            ),
            "remove <config>/bridge-token and restart the bridge; re-pair the phone",
        );
    }
    match token_mode(path) {
        Some(mode) if mode != 0o600 => Finding::warn(
            "bridge-token",
            format!("mode {mode:04o}, want 0600"),
            format!("chmod 600 {}", path.display()),
        ),
        _ => Finding::ok("bridge-token", "present, 0600, well-formed"),
    }
}

#[cfg(unix)]
fn token_mode(path: &Path) -> Option<u32> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .ok()
        .map(|m| m.permissions().mode() & 0o777)
}

#[cfg(not(unix))]
fn token_mode(_path: &Path) -> Option<u32> {
    None
}

// 6. integrations -----------------------------------------------------------

fn classify_integration(statuses: &[crate::integration::IntegrationStatus]) -> Finding {
    use crate::api::schema::IntegrationTarget;
    use crate::integration::IntegrationStatusKind;
    let fix = "shep integration install claude";
    let Some(claude) = statuses
        .iter()
        .find(|s| s.target == IntegrationTarget::Claude)
    else {
        return Finding::warn(
            "integrations",
            "claude hook not available on this platform",
            fix,
        );
    };
    match claude.state {
        IntegrationStatusKind::Current => Finding::ok(
            "integrations",
            format!(
                "claude hook current (v{})",
                claude.installed_version.unwrap_or(claude.expected_version)
            ),
        ),
        IntegrationStatusKind::Outdated => Finding::warn(
            "integrations",
            format!(
                "claude hook outdated (v{} < v{})",
                claude
                    .installed_version
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "legacy".to_string()),
                claude.expected_version
            ),
            fix,
        ),
        IntegrationStatusKind::NotInstalled => {
            Finding::warn("integrations", "claude hook not installed", fix)
        }
    }
}

// 7. memory hooks -----------------------------------------------------------

fn probe_memory_hooks() -> Finding {
    let root = match crate::memory::resolve_repo_root(None) {
        Ok(root) => root,
        Err(_) => return Finding::ok("memory-hooks", "n/a (not inside a git repo)"),
    };
    let settings = root.join(".claude").join("settings.json");
    let content = std::fs::read_to_string(&settings).unwrap_or_default();
    let issues = crate::memory::bridges::audit_claude_hooks(&content, |path| path.exists());
    classify_memory_hooks(&settings, &issues)
}

fn classify_memory_hooks(settings: &Path, issues: &[crate::memory::bridges::HookIssue]) -> Finding {
    if issues.is_empty() {
        return Finding::ok("memory-hooks", format!("clean: {}", settings.display()));
    }
    let summary = issues
        .iter()
        .map(|issue| issue.to_string())
        .collect::<Vec<_>>()
        .join("; ");
    Finding::warn(
        "memory-hooks",
        format!(
            "{} issue(s) in {}: {summary}",
            issues.len(),
            settings.display()
        ),
        "shep memory init",
    )
}

// 8. push -------------------------------------------------------------------

fn classify_push(config_dir: &Path) -> Finding {
    if !config_dir.join("fcm-service-account.json").is_file() {
        return Finding::warn(
            "push",
            "no fcm-service-account.json — push to the phone is off",
            "drop the Firebase service account at <config>/fcm-service-account.json",
        );
    }
    let fcm = count_fcm_rows(&config_dir.join("push-endpoints.json"));
    if fcm == 0 {
        Finding::warn(
            "push",
            "service account present but no fcm rows in push-endpoints.json",
            "pair the phone (companion → pair) so it registers for push",
        )
    } else {
        Finding::ok("push", format!("{fcm} fcm device(s) registered"))
    }
}

fn count_fcm_rows(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
        return 0;
    };
    value
        .as_array()
        .map(|rows| {
            rows.iter()
                .filter(|row| row.get("transport").and_then(|t| t.as_str()) == Some("fcm"))
                .count()
        })
        .unwrap_or(0)
}

// 9. stale state ------------------------------------------------------------

fn classify_stale_state(state_dir: &Path) -> Vec<Finding> {
    let mut findings = Vec::new();
    let tasks = state_dir.join("tasks.db");
    if tasks.exists() {
        findings.push(Finding::warn(
            "stale",
            format!(
                "{} survives the queue retirement (2026-09-10)",
                tasks.display()
            ),
            format!(
                "safe to archive: mv {} {}.retired",
                tasks.display(),
                tasks.display()
            ),
        ));
    }
    let docket_json = state_dir.join("docket.json");
    if docket_json.exists() {
        findings.push(Finding::warn(
            "stale",
            format!(
                "{} has not been imported into docket.db",
                docket_json.display()
            ),
            "run any `shep docket` command (e.g. shep docket list) to import it",
        ));
    }
    if findings.is_empty() {
        findings.push(Finding::ok("stale", "none"));
    }
    findings
}

// 10. err log ---------------------------------------------------------------

const ERR_LOG_LINE_MAX: usize = 120;

fn classify_err_log(path: &Path, now: SystemTime) -> Finding {
    let Ok(meta) = std::fs::metadata(path) else {
        return Finding::ok("err-log", "none");
    };
    let age = meta
        .modified()
        .ok()
        .and_then(|mtime| now.duration_since(mtime).ok());
    match age {
        Some(age) if age < ERR_LOG_FRESH_WINDOW => {
            let last = std::fs::read_to_string(path)
                .ok()
                .and_then(|text| {
                    text.lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .map(truncate_line)
                })
                .unwrap_or_else(|| "(empty)".to_string());
            Finding::warn(
                "err-log",
                format!("{} written {} ago: {last}", path.display(), format_age(age)),
                format!("read its tail: tail -50 {}", path.display()),
            )
        }
        _ => Finding::ok("err-log", "quiet"),
    }
}

fn truncate_line(line: &str) -> String {
    let line = line.trim();
    if line.chars().count() <= ERR_LOG_LINE_MAX {
        return line.to_string();
    }
    let mut out: String = line.chars().take(ERR_LOG_LINE_MAX - 1).collect();
    out.push('…');
    out
}

fn format_age(age: Duration) -> String {
    let secs = age.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else {
        format!("{}h", secs / 3600)
    }
}

// 11. disk ------------------------------------------------------------------

fn classify_disk(path: &Path, free_bytes: Option<u64>) -> Finding {
    let Some(free) = free_bytes else {
        return Finding::ok("disk", "free space unknown on this platform");
    };
    let gib = free / (1024 * 1024 * 1024);
    let detail = format!(
        "{gib} GiB free on the filesystem holding {}",
        path.display()
    );
    if gib < DISK_WARN_GIB {
        Finding::warn(
            "disk",
            detail,
            format!("below {DISK_WARN_GIB} GiB — reclaim space (df -h, then prune caches/targets)"),
        )
    } else {
        Finding::ok("disk", detail)
    }
}

// 12. docket ----------------------------------------------------------------

fn classify_docket(path: &Path) -> Finding {
    if !path.exists() {
        return Finding::warn(
            "docket",
            format!("no store yet at {}", path.display()),
            "the server creates it on first use: shep docket list",
        );
    }
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    );
    let items = conn
        .map_err(|err| err.to_string())
        .and_then(|conn| crate::docket::list(&conn, None).map_err(|err| err.to_string()));
    match items {
        Ok((_, items)) => {
            let overdue = items.iter().filter(|item| item.overdue).count();
            let inbox = items
                .iter()
                .filter(|item| item.status == crate::api::schema::DocketStatus::Inbox)
                .count();
            Finding::ok("docket", format!("{overdue} overdue · {inbox} inbox"))
        }
        Err(err) => Finding::fail(
            "docket",
            format!("cannot open {}: {err}", path.display()),
            "check the file is a sqlite db the server can open; restore it from backup if not",
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::bridges::HookIssue;
    use std::path::PathBuf;
    use std::time::UNIX_EPOCH;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("shep-doctor-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn running(version: &str, protocol: u32) -> io::Result<ServerRuntimeStatus> {
        Ok(ServerRuntimeStatus::Running {
            version: Some(version.to_string()),
            protocol: Some(protocol),
            capabilities: None,
        })
    }

    #[test]
    fn server_ok_when_versions_and_protocol_match() {
        let f = classify_server(&running("1.2.3", 18), "1.2.3", 18);
        assert_eq!(f.level, Level::Ok);
        assert_eq!(f.detail, "running v1.2.3, protocol 18");
        assert!(f.fix.is_none());
    }

    #[test]
    fn server_warns_on_protocol_and_version_mismatch() {
        let f = classify_server(&running("1.2.3", 17), "1.2.3", 18);
        assert_eq!(f.level, Level::Warn);
        assert!(f.detail.contains("client protocol 18"), "{}", f.detail);
        let f = classify_server(&running("1.2.2", 18), "1.2.3", 18);
        assert_eq!(f.level, Level::Warn);
        assert!(f.detail.contains("restart_needed"), "{}", f.detail);
    }

    #[test]
    fn server_fails_when_not_running() {
        let f = classify_server(&Ok(ServerRuntimeStatus::NotRunning), "1", 18);
        assert_eq!(f.level, Level::Fail);
        let f = classify_server(&Err(io::Error::other("boom")), "1", 18);
        assert_eq!(f.level, Level::Fail);
        assert!(f.detail.contains("boom"));
    }

    #[test]
    fn socket_classifier_maps_probe_outcomes() {
        let path = Path::new("/tmp/x/shep.sock");
        assert_eq!(classify_socket(path, SocketProbe::Held).level, Level::Ok);
        let stale = classify_socket(path, SocketProbe::Stale);
        assert_eq!(stale.level, Level::Fail);
        assert!(stale.detail.starts_with("stale socket file"));
        assert_eq!(
            classify_socket(path, SocketProbe::Missing).level,
            Level::Fail
        );
        assert_eq!(
            classify_socket(path, SocketProbe::Error("eacces".into())).level,
            Level::Fail
        );
    }

    #[test]
    fn socket_probe_sees_missing_stale_and_held() {
        let dir = temp_dir("socket");
        let path = dir.join("shep.sock");
        assert_eq!(probe_socket(&path), SocketProbe::Missing);
        let listener = std::os::unix::net::UnixListener::bind(&path).unwrap();
        assert_eq!(probe_socket(&path), SocketProbe::Held);
        drop(listener);
        // The file survives the listener; nothing accepts on it now.
        assert!(path.exists());
        assert_eq!(probe_socket(&path), SocketProbe::Stale);
        let _ = std::fs::remove_dir_all(&dir);
    }

    const LAUNCHCTL_RUNNING: &str = "gui/501/dev.shep.bridge = {\n\tactive count = 1\n\tstate = running\n\n\tprogram = /Users/alex/.local/bin/shep\n\targuments = {\n\t\t/Users/alex/.local/bin/shep\n\t\tbridge\n\t}\n\tenvironment = {\n\t\tpid = 999\n\t}\n\truns = 14\n\tpid = 75116\n}\n";
    const LAUNCHCTL_SPINNING: &str = "gui/501/dev.shep.server = {\n\tactive count = 0\n\tstate = waiting\n\truns = 240\n\tlast exit code = 1\n}\n";

    #[test]
    fn launchctl_print_parser_reads_top_level_keys_only() {
        let job = parse_launchctl_print(LAUNCHCTL_RUNNING);
        assert_eq!(
            job,
            LaunchdJob {
                state: Some("running".into()),
                pid: Some(75116),
                runs: Some(14)
            }
        );
        let job = parse_launchctl_print(LAUNCHCTL_SPINNING);
        assert_eq!(job.pid, None);
        assert_eq!(job.runs, Some(240));
    }

    #[test]
    fn launchd_classifier_covers_running_spinning_and_unloaded() {
        let ok = classify_launchd("dev.shep.bridge", Some(LAUNCHCTL_RUNNING), "gui/501");
        assert_eq!(ok.level, Level::Ok);
        assert_eq!(ok.detail, "dev.shep.bridge pid 75116 runs=14");
        let spin = classify_launchd("dev.shep.server", Some(LAUNCHCTL_SPINNING), "gui/501");
        assert_eq!(spin.level, Level::Warn);
        assert!(spin.detail.contains("KeepAlive spin"), "{}", spin.detail);
        assert!(spin.detail.contains("runs=240"), "{}", spin.detail);
        let gone = classify_launchd("dev.shep.server", None, "gui/501");
        assert_eq!(gone.level, Level::Warn);
        assert!(gone
            .fix
            .as_deref()
            .unwrap()
            .contains("launchctl bootstrap gui/501"));
    }

    #[test]
    fn bridge_classifier_uses_the_listen_probe() {
        let ok = classify_bridge(Some("100.83.179.75:7431"), |_| true, "gui/501");
        assert_eq!(ok.level, Level::Ok);
        let dead = classify_bridge(Some("100.83.179.75:7431"), |_| false, "gui/501");
        assert_eq!(dead.level, Level::Fail);
        assert_eq!(
            dead.fix.as_deref(),
            Some("launchctl kickstart -k gui/501/dev.shep.bridge")
        );
        let none = classify_bridge(None, |_| true, "gui/501");
        assert_eq!(none.level, Level::Warn);
        assert!(none.detail.contains("has not served"));
        let junk = classify_bridge(Some("nope"), |_| true, "gui/501");
        assert_eq!(junk.level, Level::Fail);
    }

    #[test]
    fn bridge_probe_detects_a_real_listener() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        assert!(tcp_listening(addr));
        drop(listener);
        assert!(!tcp_listening(addr));
    }

    #[cfg(unix)]
    #[test]
    fn token_classifier_checks_presence_shape_and_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = temp_dir("token");
        let path = dir.join("bridge-token");
        assert_eq!(classify_token(&path).level, Level::Warn);

        let good = "A".repeat(20) + "-_" + &"b".repeat(21);
        assert_eq!(good.len(), TOKEN_LEN);
        std::fs::write(&path, format!("{good}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        let loose = classify_token(&path);
        assert_eq!(loose.level, Level::Warn);
        assert!(loose.detail.contains("0644"), "{}", loose.detail);

        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(classify_token(&path).level, Level::Ok);

        std::fs::write(&path, "short\n").unwrap();
        let short = classify_token(&path);
        assert_eq!(short.level, Level::Warn);
        assert!(short.detail.contains("malformed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn integration_classifier_reads_the_claude_row() {
        use crate::api::schema::IntegrationTarget;
        use crate::integration::{IntegrationStatus, IntegrationStatusKind};
        let row = |state, installed| IntegrationStatus {
            target: IntegrationTarget::Claude,
            path: PathBuf::from("/tmp/hook"),
            state,
            installed_version: installed,
            expected_version: 3,
        };
        let current = classify_integration(&[row(IntegrationStatusKind::Current, Some(3))]);
        assert_eq!(current.level, Level::Ok);
        let old = classify_integration(&[row(IntegrationStatusKind::Outdated, Some(2))]);
        assert_eq!(old.level, Level::Warn);
        assert!(old.detail.contains("v2 < v3"));
        assert_eq!(old.fix.as_deref(), Some("shep integration install claude"));
        let none = classify_integration(&[row(IntegrationStatusKind::NotInstalled, None)]);
        assert_eq!(none.level, Level::Warn);
        assert_eq!(classify_integration(&[]).level, Level::Warn);
    }

    #[test]
    fn memory_hooks_classifier_summarises_issues() {
        let settings = Path::new("/repo/.claude/settings.json");
        assert_eq!(classify_memory_hooks(settings, &[]).level, Level::Ok);
        let issues = vec![
            HookIssue::Missing {
                event: "Stop".into(),
                kind: "reflect-hook".into(),
            },
            HookIssue::Unreachable {
                event: "Stop".into(),
                command: "/gone/shep memory reflect-hook".into(),
            },
        ];
        let f = classify_memory_hooks(settings, &issues);
        assert_eq!(f.level, Level::Warn);
        assert!(f.detail.starts_with("2 issue(s)"), "{}", f.detail);
        assert_eq!(f.fix.as_deref(), Some("shep memory init"));
    }

    #[test]
    fn push_classifier_counts_fcm_rows() {
        let dir = temp_dir("push");
        assert_eq!(classify_push(&dir).level, Level::Warn);
        std::fs::write(dir.join("fcm-service-account.json"), "{}").unwrap();
        let zero = classify_push(&dir);
        assert_eq!(zero.level, Level::Warn);
        assert!(zero.fix.as_deref().unwrap().contains("pair the phone"));
        std::fs::write(
            dir.join("push-endpoints.json"),
            r#"[{"transport":"fcm","token":"a"},{"transport":"unifiedpush","endpoint":"u"},{"endpoint":"legacy"},{"transport":"fcm","token":"b"}]"#,
        )
        .unwrap();
        let two = classify_push(&dir);
        assert_eq!(two.level, Level::Ok);
        assert_eq!(two.detail, "2 fcm device(s) registered");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stale_state_flags_tasks_db_and_unimported_docket_json() {
        let dir = temp_dir("stale");
        let clean = classify_stale_state(&dir);
        assert_eq!(clean.len(), 1);
        assert_eq!(clean[0].level, Level::Ok);
        std::fs::write(dir.join("tasks.db"), "").unwrap();
        std::fs::write(dir.join("docket.json"), "{}").unwrap();
        let findings = classify_stale_state(&dir);
        assert_eq!(findings.len(), 2);
        assert!(findings.iter().all(|f| f.level == Level::Warn));
        assert!(findings[0].detail.contains("tasks.db"));
        assert!(findings[1].fix.as_deref().unwrap().contains("shep docket"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn err_log_warns_only_when_fresh_and_quotes_the_last_line() {
        let dir = temp_dir("errlog");
        let path = dir.join("launchd-server.err");
        assert_eq!(classify_err_log(&path, SystemTime::now()).level, Level::Ok);
        let long = "x".repeat(200);
        std::fs::write(&path, format!("first\nsecond\n{long}\n\n\n")).unwrap();
        let fresh = classify_err_log(&path, SystemTime::now());
        assert_eq!(fresh.level, Level::Warn);
        assert!(fresh.detail.ends_with('…'), "{}", fresh.detail);
        assert!(fresh.detail.contains(&"x".repeat(119)));
        assert!(!fresh.detail.contains(&"x".repeat(120)));
        // Pretend a day and a half has passed.
        let later = SystemTime::now() + Duration::from_secs(36 * 3600);
        assert_eq!(classify_err_log(&path, later).level, Level::Ok);
        // An mtime in the future (clock skew) is not "fresh".
        assert_eq!(classify_err_log(&path, UNIX_EPOCH).level, Level::Ok);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn disk_classifier_thresholds_at_fifteen_gib() {
        let gib = 1024u64 * 1024 * 1024;
        let path = Path::new("/cfg");
        assert_eq!(classify_disk(path, Some(14 * gib)).level, Level::Warn);
        assert_eq!(classify_disk(path, Some(15 * gib)).level, Level::Ok);
        assert_eq!(classify_disk(path, None).level, Level::Ok);
    }

    #[test]
    fn docket_classifier_counts_overdue_and_inbox_read_only() {
        let dir = temp_dir("docket");
        let path = dir.join("docket.db");
        assert_eq!(classify_docket(&path).level, Level::Warn);

        let conn = crate::docket::open_store(&path).unwrap();
        let item = |title: &str, status, due: Option<&str>| crate::docket::NewItem {
            title: title.into(),
            kind: Some(if due.is_some() {
                crate::api::schema::DocketKind::Slated
            } else {
                crate::api::schema::DocketKind::Captured
            }),
            status,
            due: due.map(String::from),
            ..Default::default()
        };
        crate::docket::add(
            &conn,
            item(
                "late",
                Some(crate::api::schema::DocketStatus::Open),
                Some("2000-01-01"),
            ),
        )
        .unwrap();
        crate::docket::add(&conn, item("captured", None, None)).unwrap();
        crate::docket::add(&conn, item("captured too", None, None)).unwrap();
        drop(conn);

        let f = classify_docket(&path);
        assert_eq!(f.level, Level::Ok, "{}", f.detail);
        assert_eq!(f.detail, "1 overdue · 2 inbox");

        std::fs::write(&path, "not a database").unwrap();
        assert_eq!(classify_docket(&path).level, Level::Fail);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exit_code_fails_only_on_fail() {
        let ok = Finding::ok("a", "x");
        let warn = Finding::warn("b", "y", "z");
        assert_eq!(exit_code(&[ok.clone(), warn.clone()]), 0);
        assert_eq!(exit_code(&[ok, warn, Finding::fail("c", "y", "z")]), 1);
    }

    #[test]
    fn json_shape_omits_fix_when_ok() {
        let json = serde_json::to_value([
            Finding::ok("server", "running"),
            Finding::warn("push", "none", "pair"),
        ])
        .unwrap();
        assert_eq!(
            json[0],
            serde_json::json!({"level":"ok","check":"server","detail":"running"})
        );
        assert_eq!(json[1]["level"], "warn");
        assert_eq!(json[1]["fix"], "pair");
    }
}
