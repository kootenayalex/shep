//! `shep bridge` — WebSocket relay for companion clients (Android).
//!
//! A thin authenticated pass-through between WebSocket clients and the local
//! NDJSON API socket. No new methods: each relay "channel" is one API
//! connection — the client sends one request line, the bridge streams every
//! response line back, then signals EOF. The JSON API schema is the contract;
//! the bincode TUI protocol is never exposed.
//!
//! Wire format (WS text frames, one JSON object each):
//!   client → bridge: {"ch":1,"req":{...api request...}}
//!                    {"ch":1,"close":true}
//!   bridge → client: {"hello":{"server_version":"...","protocol":N}}
//!                    {"ch":1,"line":{...api response line...}}
//!                    {"ch":1,"eof":true} | {"ch":1,"error":"..."}
//!
//! Auth: `Authorization: Bearer <token>` header or `?token=<token>` query
//! parameter on the upgrade request; the token lives at
//! `<config dir>/bridge-token` (created on first run, 0600). Bind defaults to
//! loopback — pass the tailscale IP explicitly to reach it from the phone;
//! never bind a public interface.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tungstenite::handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse};
use tungstenite::{Message, WebSocket};

const DEFAULT_BIND: &str = "127.0.0.1:7431";
const USAGE: &str = "usage: shep bridge [--bind <ip:port>] [--socket <api-socket-path>] | shep bridge pair [--host <ip[:port]>] | shep bridge token | shep bridge notify-push";

pub(super) fn run_bridge_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(|arg| arg.as_str()) {
        Some("pair") => pair(&args[1..]),
        Some("notify-push") => push::notify_push(&args[1..]),
        Some("token") => {
            println!("{}", load_or_create_token()?);
            Ok(0)
        }
        Some("help" | "--help" | "-h") => {
            eprintln!("{USAGE}");
            Ok(0)
        }
        Some("--bind") | None => serve(args),
        _ => {
            eprintln!("{USAGE}");
            Ok(2)
        }
    }
}

/// Print the pairing info the companion app asks for (URL + token).
fn pair(args: &[String]) -> std::io::Result<i32> {
    let mut host = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--host" => host = iter.next().cloned(),
            _ => {
                eprintln!("{USAGE}");
                return Ok(2);
            }
        }
    }
    let host = host.unwrap_or_else(|| DEFAULT_BIND.to_string());
    let host = if host.contains(':') {
        host
    } else {
        format!("{host}:7431")
    };
    let token = load_or_create_token()?;
    let url = format!("ws://{host}/");
    let payload = format!(
        "shep://pair?url={}&token={}",
        percent_encode(&url),
        percent_encode(&token),
    );
    match render_qr(&payload) {
        Ok(image) => {
            println!("{image}");
        }
        Err(err) => eprintln!("(could not render QR: {err}; use the text values below)"),
    }
    println!("url: {url}");
    println!("token: {token}");
    println!("scan the QR in the companion app, or paste both into the pairing screen.");
    Ok(0)
}

/// Render `payload` as a terminal QR (light modules on the dark background, an
/// inverted QR that scanners read fine). Kept dependency-light: the `qrcode`
/// crate's unicode renderer, no image backend.
fn render_qr(payload: &str) -> Result<String, qrcode::types::QrError> {
    use qrcode::render::unicode;
    let code = qrcode::QrCode::new(payload.as_bytes())?;
    Ok(code
        .render::<unicode::Dense1x2>()
        .dark_color(unicode::Dense1x2::Light)
        .light_color(unicode::Dense1x2::Dark)
        .quiet_zone(true)
        .build())
}

/// Percent-encode a query-parameter value (RFC 3986 unreserved set passes
/// through). The companion app decodes via `Uri.getQueryParameter`.
fn percent_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char);
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn serve(args: &[String]) -> std::io::Result<i32> {
    let mut bind = DEFAULT_BIND.to_string();
    let mut socket_path: Option<PathBuf> = None;
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        match arg.as_str() {
            "--bind" => match iter.next() {
                Some(addr) => bind = addr.clone(),
                None => {
                    eprintln!("{USAGE}");
                    return Ok(2);
                }
            },
            "--socket" => match iter.next() {
                Some(path) => socket_path = Some(PathBuf::from(path)),
                None => {
                    eprintln!("{USAGE}");
                    return Ok(2);
                }
            },
            _ => {
                eprintln!("{USAGE}");
                return Ok(2);
            }
        }
    }

    let api_socket = Arc::new(socket_path.unwrap_or_else(crate::api::socket_path));
    let token = load_or_create_token()?;
    let listener = TcpListener::bind(&bind)?;
    eprintln!("shep bridge listening on ws://{bind}/ (api socket: {api_socket:?})");
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        let token = token.clone();
        let api_socket = api_socket.clone();
        std::thread::spawn(move || {
            if let Err(err) = handle_ws_client(stream, &token, &api_socket) {
                tracing::debug!(err = %err, "bridge client ended");
            }
        });
    }
    Ok(0)
}

/// Accept one WebSocket client, then relay frames <-> API channels until it
/// disconnects.
fn handle_ws_client(
    stream: TcpStream,
    token: &str,
    api_socket: &Arc<PathBuf>,
) -> Result<(), Box<dyn std::error::Error>> {
    stream.set_nodelay(true).ok();
    let expected = token.to_string();
    // The Err type is tungstenite's ErrorResponse (a full http::Response);
    // accept_hdr fixes the callback signature, so the size is not ours to
    // shrink.
    #[allow(clippy::result_large_err)]
    let auth_check = move |request: &WsRequest, response: WsResponse| {
        if request_authorized(request, &expected) {
            Ok(response)
        } else {
            Err(ErrorResponse::new(Some("unauthorized".into())))
        }
    };
    let mut socket = tungstenite::accept_hdr(stream, auth_check)?;
    // Poll reads so queued outbound frames from channel readers keep flowing
    // through this single IO thread (tungstenite sockets don't split).
    socket
        .get_ref()
        .set_read_timeout(Some(Duration::from_millis(50)))?;

    socket.send(Message::Text(
        serde_json::json!({
            "hello": {
                "server_version": env!("CARGO_PKG_VERSION"),
                "protocol": crate::protocol::PROTOCOL_VERSION,
            }
        })
        .to_string(),
    ))?;

    let (out_tx, out_rx) = mpsc::channel::<String>();
    let mut channels: HashMap<u64, Arc<AtomicBool>> = HashMap::new();

    loop {
        while let Ok(frame) = out_rx.try_recv() {
            socket.send(Message::Text(frame))?;
        }
        match socket.read() {
            Ok(Message::Text(text)) => {
                handle_client_frame(&text, &out_tx, &mut channels, api_socket);
            }
            Ok(Message::Close(_)) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(err))
                if err.kind() == std::io::ErrorKind::WouldBlock
                    || err.kind() == std::io::ErrorKind::TimedOut => {}
            Err(err) => return Err(err.into()),
        }
    }
    for cancel in channels.values() {
        cancel.store(true, Ordering::Relaxed);
    }
    Ok(())
}

/// Parse one inbound frame and open/close relay channels accordingly.
fn handle_client_frame(
    text: &str,
    out_tx: &mpsc::Sender<String>,
    channels: &mut HashMap<u64, Arc<AtomicBool>>,
    api_socket: &Arc<PathBuf>,
) {
    let Ok(frame) = serde_json::from_str::<serde_json::Value>(text) else {
        let _ = out_tx.send(serde_json::json!({"error": "invalid frame"}).to_string());
        return;
    };
    let Some(ch) = frame.get("ch").and_then(|value| value.as_u64()) else {
        let _ = out_tx.send(serde_json::json!({"error": "frame missing ch"}).to_string());
        return;
    };
    if frame.get("close").and_then(|value| value.as_bool()) == Some(true) {
        if let Some(cancel) = channels.remove(&ch) {
            cancel.store(true, Ordering::Relaxed);
        }
        return;
    }
    let Some(request) = frame.get("req") else {
        let _ = out_tx.send(channel_error(ch, "frame missing req"));
        return;
    };
    // Push registration lives on the bridge, not the JSON API: the endpoint is a
    // companion-only fact (the phone's UnifiedPush URL) and handling it here keeps
    // it out of the herdr API contract / protocol version. Everything else proxies.
    if let Some(method) = request.get("method").and_then(|m| m.as_str()) {
        if let Some(outcome) = push::handle_local_method(method, request.get("params")) {
            let line = match outcome {
                Ok(result) => serde_json::json!({"result": result}),
                Err(message) => serde_json::json!({"error": {"message": message}}),
            };
            let _ = out_tx.send(serde_json::json!({"ch": ch, "line": line}).to_string());
            let _ = out_tx.send(serde_json::json!({"ch": ch, "eof": true}).to_string());
            return;
        }
    }
    let cancel = Arc::new(AtomicBool::new(false));
    channels.insert(ch, cancel.clone());
    let request_line = request.to_string();
    let out_tx = out_tx.clone();
    let api_socket = api_socket.clone();
    std::thread::spawn(move || relay_channel(ch, &request_line, &out_tx, &cancel, &api_socket));
}

/// One API connection: write the request line, stream every response line
/// back tagged with the channel id, then EOF.
fn relay_channel(
    ch: u64,
    request_line: &str,
    out_tx: &mpsc::Sender<String>,
    cancel: &AtomicBool,
    api_socket: &std::path::Path,
) {
    match relay_channel_io(request_line, out_tx, cancel, ch, api_socket) {
        Ok(()) => {
            let _ = out_tx.send(serde_json::json!({"ch": ch, "eof": true}).to_string());
        }
        Err(err) => {
            let _ = out_tx.send(channel_error(ch, &err.to_string()));
        }
    }
}

fn relay_channel_io(
    request_line: &str,
    out_tx: &mpsc::Sender<String>,
    cancel: &AtomicBool,
    ch: u64,
    api_socket: &std::path::Path,
) -> std::io::Result<()> {
    let mut stream = crate::ipc::connect_local_stream(api_socket)?;
    stream.write_all(request_line.as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        // Pass the raw line through untouched when it parses; wrap otherwise.
        let payload: serde_json::Value = match serde_json::from_str(&line) {
            Ok(value) => value,
            Err(_) => serde_json::Value::String(line),
        };
        if out_tx
            .send(serde_json::json!({"ch": ch, "line": payload}).to_string())
            .is_err()
        {
            break;
        }
    }
    Ok(())
}

fn channel_error(ch: u64, message: &str) -> String {
    serde_json::json!({"ch": ch, "error": message}).to_string()
}

/// Constant-time-ish token comparison is unnecessary here (tailnet-only,
/// random 256-bit token), but never log or echo the expected value.
fn request_authorized(request: &WsRequest, expected: &str) -> bool {
    if expected.is_empty() {
        return false;
    }
    let header = request
        .headers()
        .get("authorization")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if header == Some(expected) {
        return true;
    }
    request
        .uri()
        .query()
        .map(|query| {
            query
                .split('&')
                .any(|pair| pair.strip_prefix("token=") == Some(expected))
        })
        .unwrap_or(false)
}

fn token_path() -> PathBuf {
    crate::config::config_dir().join("bridge-token")
}

fn load_or_create_token() -> std::io::Result<String> {
    load_or_create_token_at(&token_path())
}

fn load_or_create_token_at(path: &PathBuf) -> std::io::Result<String> {
    if let Ok(existing) = std::fs::read_to_string(path) {
        let existing = existing.trim().to_string();
        if !existing.is_empty() {
            return Ok(existing);
        }
    }
    let mut bytes = [0u8; 32];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let token = base64_url(&bytes);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, format!("{token}\n"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

// Type alias so handle_ws_client's signature stays readable if it grows.
#[allow(dead_code)]
type WsSocket = WebSocket<TcpStream>;

/// Companion push endpoints + the blocked-transition publish hook.
///
/// The phone registers its UnifiedPush endpoint over the bridge (`push.register`)
/// and shep persists it to `<config>/push-endpoints.json`. When an agent blocks,
/// `[notifications] exec = "shep bridge notify-push"` fires `notify_push`, which
/// reads `SHEP_NOTIFY_*` and POSTs a small JSON body to each endpoint. Under
/// UnifiedPush the body is opaque passthrough — the companion's MessagingReceiver
/// parses it and renders the actionable notification.
mod push {
    use std::path::PathBuf;

    /// Where registered endpoints live. One JSON array of `{endpoint,label,added_unix}`.
    fn endpoints_path() -> PathBuf {
        crate::config::config_dir().join("push-endpoints.json")
    }

    #[derive(Clone)]
    struct Endpoint {
        url: String,
        label: String,
    }

    fn load(path: &std::path::Path) -> Vec<Endpoint> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        value
            .as_array()
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| {
                        let url = item.get("endpoint")?.as_str()?.to_string();
                        let label = item
                            .get("label")
                            .and_then(|l| l.as_str())
                            .unwrap_or_default()
                            .to_string();
                        Some(Endpoint { url, label })
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    fn store(path: &std::path::Path, endpoints: &[Endpoint]) -> std::io::Result<()> {
        let array: Vec<serde_json::Value> = endpoints
            .iter()
            .map(|e| serde_json::json!({"endpoint": e.url, "label": e.label}))
            .collect();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, serde_json::Value::Array(array).to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
        Ok(())
    }

    fn is_valid_endpoint(url: &str) -> bool {
        (url.starts_with("http://") || url.starts_with("https://")) && url.len() <= 2048
    }

    fn register_at(path: &std::path::Path, url: &str, label: &str) -> Result<usize, String> {
        if !is_valid_endpoint(url) {
            return Err("endpoint must be an http(s) URL".to_string());
        }
        let mut endpoints = load(path);
        // Dedup by URL: re-registration (endpoint rotation keeps the same URL until
        // the distributor reassigns it) updates the label rather than piling up.
        if let Some(existing) = endpoints.iter_mut().find(|e| e.url == url) {
            existing.label = label.to_string();
        } else {
            endpoints.push(Endpoint {
                url: url.to_string(),
                label: label.to_string(),
            });
        }
        let count = endpoints.len();
        store(path, &endpoints).map_err(|err| err.to_string())?;
        Ok(count)
    }

    /// Intercept bridge-local methods. Returns `None` for anything that should
    /// proxy to the JSON API; `Some(Ok/Err)` when this module owns the method.
    pub(super) fn handle_local_method(
        method: &str,
        params: Option<&serde_json::Value>,
    ) -> Option<Result<serde_json::Value, String>> {
        match method {
            "push.register" => {
                let url = params
                    .and_then(|p| p.get("endpoint"))
                    .and_then(|e| e.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
                let label = params
                    .and_then(|p| p.get("label"))
                    .and_then(|l| l.as_str())
                    .unwrap_or_default()
                    .to_string();
                if url.is_empty() {
                    return Some(Err("missing endpoint".to_string()));
                }
                Some(
                    register_at(&endpoints_path(), &url, &label)
                        .map(|count| serde_json::json!({"registered": true, "count": count})),
                )
            }
            "push.list" => {
                let list: Vec<serde_json::Value> = load(&endpoints_path())
                    .into_iter()
                    .map(|e| serde_json::json!({"endpoint": e.url, "label": e.label}))
                    .collect();
                Some(Ok(serde_json::json!({"endpoints": list})))
            }
            _ => None,
        }
    }

    /// `shep bridge notify-push` — the `[notifications] exec` target. Reads the
    /// transition context from `SHEP_NOTIFY_*` and POSTs it to each registered
    /// endpoint. Best-effort and non-fatal: a dead endpoint must not wedge the
    /// exec-bridge, so failures are logged and the exit code stays 0.
    pub(super) fn notify_push(_args: &[String]) -> std::io::Result<i32> {
        let endpoints = load(&endpoints_path());
        if endpoints.is_empty() {
            eprintln!("shep bridge notify-push: no registered endpoints; nothing to do");
            return Ok(0);
        }
        let state = std::env::var("SHEP_NOTIFY_STATE").unwrap_or_default();
        let agent = std::env::var("SHEP_NOTIFY_AGENT").unwrap_or_default();
        let workspace = std::env::var("SHEP_NOTIFY_WORKSPACE").unwrap_or_default();
        let pane_id = std::env::var("SHEP_NOTIFY_PANE_ID").unwrap_or_default();
        let message = truncate(
            &std::env::var("SHEP_NOTIFY_MESSAGE").unwrap_or_default(),
            400,
        );
        let body = serde_json::json!({
            "state": state,
            "agent": agent,
            "workspace": workspace,
            "pane_id": pane_id,
            "message": message,
        })
        .to_string();

        // Co-location shortcut: when shep and ntfy run on the same host (the
        // usual setup) the mini's resolver can't resolve the broker's MagicDNS
        // name, and a tailnet round-trip is pointless anyway. `SHEP_NTFY_PUBLISH_BASE`
        // (e.g. http://127.0.0.1:2587) rewrites scheme+host and keeps the topic
        // path, so the publish stays on loopback. Unset = POST the endpoint as-is.
        let base_override = std::env::var("SHEP_NTFY_PUBLISH_BASE").ok();
        for endpoint in &endpoints {
            let target = resolve_publish_url(&endpoint.url, base_override.as_deref());
            if let Err(err) = post(&target, &body) {
                eprintln!("shep bridge notify-push: {target} failed: {err}");
            }
        }
        Ok(0)
    }

    /// Rewrite `endpoint`'s scheme+authority to `base` while preserving the topic
    /// path and query. `resolve_publish_url("https://h/UPa?up=1", Some("http://127.0.0.1:2587"))`
    /// → `"http://127.0.0.1:2587/UPa?up=1"`. Returns the endpoint unchanged when
    /// `base` is None or either value is malformed.
    fn resolve_publish_url(endpoint: &str, base: Option<&str>) -> String {
        let Some(base) = base else {
            return endpoint.to_string();
        };
        let base = base.trim_end_matches('/');
        // Path starts at the first '/' after the "scheme://" authority.
        let after_scheme = match endpoint.find("://") {
            Some(idx) => &endpoint[idx + 3..],
            None => return endpoint.to_string(),
        };
        match after_scheme.find('/') {
            Some(idx) => format!("{base}{}", &after_scheme[idx..]),
            None => base.to_string(),
        }
    }

    fn truncate(text: &str, max: usize) -> String {
        if text.chars().count() <= max {
            return text.to_string();
        }
        text.chars().take(max).collect::<String>() + "…"
    }

    /// POST `body` to `url` via `curl`. curl is ubiquitous on the unix targets
    /// this fork supports, handles http(s)+redirects, and keeps shep free of an
    /// HTTP-client dependency for a fire-and-forget publish.
    fn post(url: &str, body: &str) -> std::io::Result<()> {
        let status = std::process::Command::new("curl")
            .arg("-sS")
            .arg("-m")
            .arg("10")
            .arg("-X")
            .arg("POST")
            .arg("-H")
            .arg("Content-Type: application/json")
            .arg("--data")
            .arg(body)
            .arg(url)
            .stdout(std::process::Stdio::null())
            .status()?;
        if status.success() {
            Ok(())
        } else {
            Err(std::io::Error::other(format!("curl exited with {status}")))
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn tmp(name: &str) -> PathBuf {
            std::env::temp_dir().join(format!(
                "shep-push-test-{}-{}.json",
                std::process::id(),
                name
            ))
        }

        #[test]
        fn register_dedups_and_updates_label() {
            let path = tmp("dedup");
            std::fs::remove_file(&path).ok();
            assert_eq!(register_at(&path, "https://ntfy/UPa", "phone").unwrap(), 1);
            // Same URL, new label → still one entry, label updated.
            assert_eq!(register_at(&path, "https://ntfy/UPa", "s22").unwrap(), 1);
            assert_eq!(register_at(&path, "https://ntfy/UPb", "avd").unwrap(), 2);
            let loaded = load(&path);
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded[0].label, "s22");
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn register_rejects_non_http() {
            let path = tmp("reject");
            std::fs::remove_file(&path).ok();
            assert!(register_at(&path, "ftp://nope", "x").is_err());
            assert!(register_at(&path, "", "x").is_err());
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn handle_local_method_only_owns_push_names() {
            assert!(handle_local_method("session.snapshot", None).is_none());
            assert!(handle_local_method("agent.read", None).is_none());
            // push.register with no params surfaces a friendly error, not a proxy.
            let outcome = handle_local_method("push.register", None);
            assert!(matches!(outcome, Some(Err(_))));
        }

        #[test]
        fn missing_endpoint_field_errors() {
            let params = serde_json::json!({"label": "x"});
            let outcome = handle_local_method("push.register", Some(&params));
            assert!(matches!(outcome, Some(Err(_))));
        }

        #[test]
        fn truncate_caps_length() {
            assert_eq!(truncate("abc", 5), "abc");
            assert_eq!(truncate(&"x".repeat(10), 3), "xxx…");
        }

        #[test]
        fn publish_url_rewrites_authority_keeps_topic() {
            assert_eq!(
                resolve_publish_url(
                    "https://ntfy.example.ts.net/UPabc?up=1",
                    Some("http://127.0.0.1:2587")
                ),
                "http://127.0.0.1:2587/UPabc?up=1"
            );
            // Trailing slash on base is normalized.
            assert_eq!(
                resolve_publish_url("https://h/UPx", Some("http://127.0.0.1:2587/")),
                "http://127.0.0.1:2587/UPx"
            );
            // No override → unchanged.
            assert_eq!(resolve_publish_url("https://h/UPx", None), "https://h/UPx");
            // Malformed endpoint → unchanged.
            assert_eq!(
                resolve_publish_url("not-a-url", Some("http://127.0.0.1:2587")),
                "not-a-url"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ws_request(auth: Option<&str>, uri: &str) -> WsRequest {
        let mut builder = tungstenite::handshake::client::Request::builder().uri(uri);
        if let Some(auth) = auth {
            builder = builder.header("authorization", auth);
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn bearer_header_authorizes() {
        let request = ws_request(Some("Bearer sekrit"), "ws://x/");
        assert!(request_authorized(&request, "sekrit"));
        assert!(!request_authorized(&request, "other"));
    }

    #[test]
    fn query_token_authorizes() {
        let request = ws_request(None, "ws://x/?a=1&token=sekrit");
        assert!(request_authorized(&request, "sekrit"));
        let request = ws_request(None, "ws://x/?token=wrong");
        assert!(!request_authorized(&request, "sekrit"));
    }

    #[test]
    fn missing_credentials_or_empty_expected_denies() {
        let request = ws_request(None, "ws://x/");
        assert!(!request_authorized(&request, "sekrit"));
        let request = ws_request(Some("Bearer sekrit"), "ws://x/");
        assert!(!request_authorized(&request, ""));
    }

    #[test]
    fn token_is_created_once_and_reused() {
        let dir = std::env::temp_dir().join(format!("shep-bridge-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bridge-token");
        std::fs::remove_file(&path).ok();
        let first = load_or_create_token_at(&path).unwrap();
        let second = load_or_create_token_at(&path).unwrap();
        assert_eq!(first, second);
        assert!(first.len() >= 40, "256-bit token, base64url");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }

    #[test]
    fn close_frame_cancels_channel() {
        let (out_tx, _out_rx) = mpsc::channel();
        let mut channels = HashMap::new();
        let cancel = Arc::new(AtomicBool::new(false));
        channels.insert(7, cancel.clone());
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame(r#"{"ch":7,"close":true}"#, &out_tx, &mut channels, &sock);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(channels.is_empty());
    }

    #[test]
    fn malformed_frames_report_errors() {
        let (out_tx, out_rx) = mpsc::channel();
        let mut channels = HashMap::new();
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame("not json", &out_tx, &mut channels, &sock);
        assert!(out_rx.recv().unwrap().contains("invalid frame"));
        handle_client_frame(r#"{"req":{}}"#, &out_tx, &mut channels, &sock);
        assert!(out_rx.recv().unwrap().contains("missing ch"));
        handle_client_frame(r#"{"ch":1}"#, &out_tx, &mut channels, &sock);
        assert!(out_rx.recv().unwrap().contains("missing req"));
    }
}
