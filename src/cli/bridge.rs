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

mod stream;
mod transcript;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use tungstenite::handshake::server::{ErrorResponse, Request as WsRequest, Response as WsResponse};
use tungstenite::http::{header, HeaderValue, StatusCode};
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
            Err(unauthorized_response())
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
    let mut channels: HashMap<u64, ChannelHandle> = HashMap::new();

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
    for handle in channels.values() {
        handle.cancel();
    }
    Ok(())
}

/// What a channel id is currently bound to.
///
/// Relay channels are one-shot: a request in, response lines out, EOF. Stream
/// channels stay open and additionally accept `data` frames going the other way.
enum ChannelHandle {
    Relay(Arc<AtomicBool>),
    Stream(stream::StreamHandle),
}

impl ChannelHandle {
    fn cancel(&self) {
        match self {
            Self::Relay(cancel) => cancel.store(true, Ordering::Relaxed),
            Self::Stream(handle) => handle.cancel(),
        }
    }
}

/// Parse one inbound frame and open/close relay channels accordingly.
fn handle_client_frame(
    text: &str,
    out_tx: &mpsc::Sender<String>,
    channels: &mut HashMap<u64, ChannelHandle>,
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
        if let Some(handle) = channels.remove(&ch) {
            handle.cancel();
        }
        return;
    }
    // Input for an open stream channel. Kept ahead of the `req` guard so a
    // keystroke costs one queue send rather than a fresh channel and socket.
    if let Some(data) = frame.get("data") {
        match channels.get(&ch) {
            Some(ChannelHandle::Stream(handle)) => {
                if let Err(err) = handle.send_input(data) {
                    let _ = out_tx.send(channel_error(ch, &err));
                }
            }
            Some(ChannelHandle::Relay(_)) => {
                let _ = out_tx.send(channel_error(ch, "channel does not accept data"));
            }
            None => {
                let _ = out_tx.send(channel_error(ch, "no such channel"));
            }
        }
        return;
    }
    let Some(request) = frame.get("req") else {
        let _ = out_tx.send(channel_error(ch, "frame missing req"));
        return;
    };
    // Reusing a live channel id would orphan the running channel's cancel flag,
    // leaving a thread with no way to be stopped.
    if channels.contains_key(&ch) {
        let _ = out_tx.send(channel_error(ch, "channel already open"));
        return;
    }
    // Some methods are handled on the bridge itself rather than proxied to the
    // JSON API. Push registration is a companion-only fact (the phone's
    // UnifiedPush URL); task add/list/cancel/remove/clear/assign and memory
    // show/add/replace/remove are local file operations on `<state>/tasks.db` and the memory files —
    // exactly what the `shep task`/`shep memory` CLIs do, so exposing them here
    // (not as new API methods) keeps them out of the herdr API contract /
    // protocol version. `task.dispatch` is NOT local: it must spawn a pane, so
    // it proxies through to the server like everything else.
    if let Some(method) = request.get("method").and_then(|m| m.as_str()) {
        let params = request.get("params");
        // `pane.stream` is long-lived and duplex, so it is handled ahead of the
        // one-shot locals below rather than alongside them.
        if let Some(opened) = stream::try_open(method, params, ch, out_tx, api_socket) {
            match opened {
                Ok(handle) => {
                    channels.insert(ch, ChannelHandle::Stream(handle));
                }
                Err(message) => {
                    let _ = out_tx.send(
                        serde_json::json!({"ch": ch, "line": {"error": {"message": message}}})
                            .to_string(),
                    );
                    let _ = out_tx.send(serde_json::json!({"ch": ch, "eof": true}).to_string());
                }
            }
            return;
        }
        let local = push::handle_local_method(method, params)
            .or_else(|| task_local::handle_local_method(method, params))
            .or_else(|| memory_local::handle_local_method(method, params))
            .or_else(|| transcript::handle_local_method(method, params, api_socket));
        if let Some(outcome) = local {
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
    channels.insert(ch, ChannelHandle::Relay(cancel.clone()));
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

/// The 401 an unauthenticated upgrade attempt gets.
///
/// The status MUST be a failure status. tungstenite refuses to write an error
/// response whose status is 2xx (`ProtocolError::CustomResponseSuccessful`) and
/// instead drops the connection having sent nothing at all — which reaches the
/// client as an opaque "unexpected end of stream", indistinguishable from a
/// dead network. `ErrorResponse::new` defaults to 200, so set it explicitly.
///
/// `write_response` emits only the status line and headers (the body is
/// appended after it), so Content-Length has to be set by hand or the response
/// is only framed by the connection close.
fn unauthorized_response() -> ErrorResponse {
    const BODY: &str = "unauthorized\n";
    let mut response = ErrorResponse::new(Some(BODY.to_string()));
    *response.status_mut() = StatusCode::UNAUTHORIZED;
    let headers = response.headers_mut();
    headers.insert(
        header::WWW_AUTHENTICATE,
        HeaderValue::from_static("Bearer realm=\"shep bridge\""),
    );
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/plain; charset=utf-8"),
    );
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(BODY.len()));
    response
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

/// Companion push endpoints + the notification publish hook.
///
/// The phone registers itself over the bridge (`push.register`) and shep
/// persists it to `<config>/push-endpoints.json`. When something worth
/// interrupting a human happens, `[notifications] exec = "shep bridge
/// notify-push"` fires [`push::notify_push`], which reads `SHEP_NOTIFY_*` and
/// delivers it to each registered device.
///
/// Two transports, because they fail differently:
///
/// - **FCM** — the default. Google's push service is privileged by the OS, so a
///   high-priority message wakes the app out of Doze; nothing else on Android
///   reliably does. The payload is data-only, so the app builds the
///   notification itself and keeps its Approve/Deny actions.
/// - **UnifiedPush** — a plain http(s) POST to a broker (ntfy) the user runs.
///   Kept because it needs no Google dependency, and because rows registered by
///   older builds must keep working.
///
/// Each device also carries the [`crate::config::NotifyKind`]s it wants. Muting
/// happens here rather than on the phone so a muted kind costs no radio wake at
/// all — and so the phone's settings survive a reinstall.
mod push {
    use std::path::PathBuf;

    /// Where registered devices live. One JSON array; see [`load`] for the
    /// accepted shapes.
    fn endpoints_path() -> PathBuf {
        crate::config::config_dir().join("push-endpoints.json")
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) enum Transport {
        /// FCM registration token for one app install.
        Fcm { token: String },
        /// Broker URL that accepts an unauthenticated POST.
        UnifiedPush { url: String },
    }

    impl Transport {
        /// Stable per-device identity, used to dedup registrations and to
        /// address one device in `push.set_kinds`.
        fn key(&self) -> &str {
            match self {
                Transport::Fcm { token } => token,
                Transport::UnifiedPush { url } => url,
            }
        }

        fn name(&self) -> &'static str {
            match self {
                Transport::Fcm { .. } => "fcm",
                Transport::UnifiedPush { .. } => "unifiedpush",
            }
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub(super) struct Endpoint {
        pub(super) transport: Transport,
        pub(super) label: String,
        /// Kinds this device wants. `None` means "everything", which is what an
        /// older registration that predates per-kind routing implies.
        pub(super) kinds: Option<Vec<String>>,
    }

    impl Endpoint {
        /// Whether this device asked to hear about `kind`.
        ///
        /// An empty `SHEP_NOTIFY_KIND` (an exec fired by a build older than
        /// kinds, or by hand) is delivered to everyone rather than dropped —
        /// silence is the worse failure for a notification system.
        pub(super) fn wants(&self, kind: &str) -> bool {
            if kind.is_empty() {
                return true;
            }
            match &self.kinds {
                None => true,
                Some(kinds) => kinds.iter().any(|k| k == kind),
            }
        }
    }

    /// Parse the persisted device list.
    ///
    /// Three shapes are accepted, because the file predates both other columns:
    /// `{"endpoint":…}` (UnifiedPush, pre-transport), `{"transport":"fcm",
    /// "token":…}`, and `{"transport":"unifiedpush","endpoint":…}`.
    pub(super) fn load(path: &std::path::Path) -> Vec<Endpoint> {
        let Ok(text) = std::fs::read_to_string(path) else {
            return Vec::new();
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&text) else {
            return Vec::new();
        };
        value
            .as_array()
            .map(|items| items.iter().filter_map(parse_endpoint).collect())
            .unwrap_or_default()
    }

    fn parse_endpoint(item: &serde_json::Value) -> Option<Endpoint> {
        let declared = item.get("transport").and_then(|t| t.as_str());
        let token = item.get("token").and_then(|t| t.as_str());
        let url = item.get("endpoint").and_then(|e| e.as_str());
        let transport = match (declared, token, url) {
            (Some("fcm"), Some(token), _) if !token.is_empty() => Transport::Fcm {
                token: token.to_string(),
            },
            // No declared transport: infer from whichever field is present, so
            // rows written before this column keep loading.
            (None, Some(token), _) if !token.is_empty() => Transport::Fcm {
                token: token.to_string(),
            },
            (_, _, Some(url)) if !url.is_empty() => Transport::UnifiedPush {
                url: url.to_string(),
            },
            _ => return None,
        };
        let label = item
            .get("label")
            .and_then(|l| l.as_str())
            .unwrap_or_default()
            .to_string();
        let kinds = item.get("kinds").and_then(|k| k.as_array()).map(|arr| {
            arr.iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect()
        });
        Some(Endpoint {
            transport,
            label,
            kinds,
        })
    }

    pub(super) fn store(path: &std::path::Path, endpoints: &[Endpoint]) -> std::io::Result<()> {
        let array: Vec<serde_json::Value> = endpoints.iter().map(endpoint_json).collect();
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

    fn endpoint_json(endpoint: &Endpoint) -> serde_json::Value {
        let mut value = match &endpoint.transport {
            Transport::Fcm { token } => {
                serde_json::json!({"transport": "fcm", "token": token, "label": endpoint.label})
            }
            Transport::UnifiedPush { url } => {
                serde_json::json!({"transport": "unifiedpush", "endpoint": url, "label": endpoint.label})
            }
        };
        if let Some(kinds) = &endpoint.kinds {
            value["kinds"] = serde_json::json!(kinds);
        }
        value
    }

    fn is_valid_endpoint(url: &str) -> bool {
        (url.starts_with("http://") || url.starts_with("https://")) && url.len() <= 2048
    }

    /// FCM registration tokens are opaque; bound the length and reject
    /// whitespace rather than pretending to validate the format.
    fn is_valid_token(token: &str) -> bool {
        !token.is_empty() && token.len() <= 4096 && !token.chars().any(char::is_whitespace)
    }

    fn register_at(
        path: &std::path::Path,
        transport: Transport,
        label: &str,
        kinds: Option<Vec<String>>,
    ) -> Result<usize, String> {
        match &transport {
            Transport::UnifiedPush { url } if !is_valid_endpoint(url) => {
                return Err("endpoint must be an http(s) URL".to_string())
            }
            Transport::Fcm { token } if !is_valid_token(token) => {
                return Err("token must be a non-empty opaque string".to_string())
            }
            _ => {}
        }
        let mut endpoints = load(path);
        // Dedup by transport identity: re-registering the same device updates
        // it in place rather than piling up duplicates that each cost a send.
        if let Some(existing) = endpoints
            .iter_mut()
            .find(|e| e.transport.key() == transport.key())
        {
            existing.label = label.to_string();
            // Re-registration without an explicit kinds list keeps whatever the
            // device already chose; the app registers on every cold start and
            // must not silently reset the user's settings.
            if kinds.is_some() {
                existing.kinds = kinds;
            }
        } else {
            endpoints.push(Endpoint {
                transport,
                label: label.to_string(),
                kinds,
            });
        }
        let count = endpoints.len();
        store(path, &endpoints).map_err(|err| err.to_string())?;
        Ok(count)
    }

    /// Change which kinds one device wants.
    ///
    /// `key` addresses the device by token or endpoint URL. It may be omitted
    /// when exactly one device is registered — the overwhelmingly common case,
    /// and it saves the phone from having to remember its own token.
    fn set_kinds_at(
        path: &std::path::Path,
        key: Option<&str>,
        kinds: Vec<String>,
    ) -> Result<usize, String> {
        let mut endpoints = load(path);
        if endpoints.is_empty() {
            return Err("no registered devices".to_string());
        }
        let index = match key {
            Some(key) => endpoints
                .iter()
                .position(|e| e.transport.key() == key)
                .ok_or_else(|| "no registered device matches that token".to_string())?,
            None if endpoints.len() == 1 => 0,
            None => return Err("several devices registered; pass token to say which".to_string()),
        };
        endpoints[index].kinds = Some(kinds);
        let count = endpoints[index].kinds.as_ref().map_or(0, Vec::len);
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
                let token = params
                    .and_then(|p| p.get("token"))
                    .and_then(|t| t.as_str())
                    .unwrap_or_default()
                    .trim()
                    .to_string();
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
                let kinds = params
                    .and_then(|p| p.get("kinds"))
                    .and_then(kinds_from_json);
                // Explicit transport wins; otherwise the field that is present
                // says which one this is.
                let declared = params
                    .and_then(|p| p.get("transport"))
                    .and_then(|t| t.as_str());
                let transport = match (declared, token.is_empty(), url.is_empty()) {
                    (Some("fcm"), false, _) | (None, false, _) => Transport::Fcm { token },
                    (Some("unifiedpush"), _, false) | (None, true, false) => {
                        Transport::UnifiedPush { url }
                    }
                    (Some(other), _, _) if other != "fcm" && other != "unifiedpush" => {
                        return Some(Err(format!("unknown transport {other}")))
                    }
                    _ => return Some(Err("missing token or endpoint".to_string())),
                };
                Some(
                    register_at(&endpoints_path(), transport, &label, kinds)
                        .map(|count| serde_json::json!({"registered": true, "count": count})),
                )
            }
            "push.set_kinds" => {
                let Some(kinds) = params
                    .and_then(|p| p.get("kinds"))
                    .and_then(kinds_from_json)
                else {
                    return Some(Err("missing kinds".to_string()));
                };
                let key = params
                    .and_then(|p| p.get("token").or_else(|| p.get("endpoint")))
                    .and_then(|t| t.as_str())
                    .map(str::trim)
                    .filter(|t| !t.is_empty());
                Some(
                    set_kinds_at(&endpoints_path(), key, kinds)
                        .map(|count| serde_json::json!({"updated": true, "kinds": count})),
                )
            }
            "push.list" => {
                let list: Vec<serde_json::Value> = load(&endpoints_path())
                    .into_iter()
                    .map(|e| {
                        let mut value = endpoint_json(&e);
                        value["transport"] = serde_json::json!(e.transport.name());
                        value
                    })
                    .collect();
                Some(Ok(serde_json::json!({"endpoints": list})))
            }
            // The diagnostic this whole subsystem was missing. Push failing is
            // invisible by construction — nothing arrives, and nothing says so —
            // which is how a broken setup went unnoticed for weeks. This reports
            // per-device what actually happened, synchronously, to whoever asked.
            "push.test" => Some(Ok(test_send(&endpoints_path()))),
            _ => None,
        }
    }

    fn test_send(path: &std::path::Path) -> serde_json::Value {
        let endpoints = load(path);
        if endpoints.is_empty() {
            return serde_json::json!({"sent": 0, "results": [], "detail": "no registered devices"});
        }
        let payload = Payload {
            kind: "test".to_string(),
            state: String::new(),
            agent: "shep".to_string(),
            workspace: String::new(),
            pane_id: String::new(),
            title: "test notification".to_string(),
            task_id: String::new(),
            message: "if you can see this, push works".to_string(),
        };
        let mut sent = 0usize;
        let results: Vec<serde_json::Value> = endpoints
            .iter()
            .map(|endpoint| {
                let outcome = match &endpoint.transport {
                    Transport::UnifiedPush { url } => {
                        // Deliberately not rewritten by SHEP_NTFY_PUBLISH_BASE:
                        // a test should exercise the endpoint as registered.
                        post(url, &payload.fields().to_string())
                            .map(|_| "sent".to_string())
                            .unwrap_or_else(|err| format!("failed: {err}"))
                    }
                    Transport::Fcm { token } => match fcm::send(token, &payload) {
                        Ok(fcm::Delivery::Sent) => "sent".to_string(),
                        Ok(fcm::Delivery::Unregistered) => "unregistered".to_string(),
                        Err(err) => format!("failed: {err}"),
                    },
                };
                if outcome == "sent" {
                    sent += 1;
                }
                serde_json::json!({
                    "label": endpoint.label,
                    "transport": endpoint.transport.name(),
                    "outcome": outcome,
                })
            })
            .collect();
        serde_json::json!({"sent": sent, "results": results})
    }

    fn kinds_from_json(value: &serde_json::Value) -> Option<Vec<String>> {
        Some(
            value
                .as_array()?
                .iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect(),
        )
    }

    /// `shep bridge notify-push` — the `[notifications] exec` target. Reads the
    /// transition context from `SHEP_NOTIFY_*` and POSTs it to each registered
    /// endpoint. Best-effort and non-fatal: a dead endpoint must not wedge the
    /// exec-bridge, so failures are logged and the exit code stays 0.
    pub(super) fn notify_push(_args: &[String]) -> std::io::Result<i32> {
        let path = endpoints_path();
        let endpoints = load(&path);
        if endpoints.is_empty() {
            eprintln!("shep bridge notify-push: no registered devices; nothing to do");
            return Ok(0);
        }
        let kind = std::env::var("SHEP_NOTIFY_KIND").unwrap_or_default();
        let payload = Payload {
            kind: kind.clone(),
            state: std::env::var("SHEP_NOTIFY_STATE").unwrap_or_default(),
            agent: std::env::var("SHEP_NOTIFY_AGENT").unwrap_or_default(),
            workspace: std::env::var("SHEP_NOTIFY_WORKSPACE").unwrap_or_default(),
            pane_id: std::env::var("SHEP_NOTIFY_PANE_ID").unwrap_or_default(),
            title: std::env::var("SHEP_NOTIFY_TITLE").unwrap_or_default(),
            task_id: std::env::var("SHEP_NOTIFY_TASK_ID").unwrap_or_default(),
            message: truncate(
                &std::env::var("SHEP_NOTIFY_MESSAGE").unwrap_or_default(),
                400,
            ),
        };

        // Co-location shortcut for UnifiedPush only: when shep and the broker
        // run on the same host the resolver often can't resolve the broker's
        // MagicDNS name, and the tailnet round-trip is pointless anyway.
        // `SHEP_NTFY_PUBLISH_BASE` (e.g. http://127.0.0.1:2587) rewrites
        // scheme+host and keeps the topic path. It must never touch an FCM row:
        // rewriting Google's endpoint to loopback is how a working push setup
        // silently stops arriving.
        let base_override = std::env::var("SHEP_NTFY_PUBLISH_BASE").ok();
        let mut stale: Vec<String> = Vec::new();
        let mut delivered = 0usize;
        for endpoint in &endpoints {
            if !endpoint.wants(&kind) {
                continue;
            }
            match &endpoint.transport {
                Transport::UnifiedPush { url } => {
                    let target = resolve_publish_url(url, base_override.as_deref());
                    match post(&target, &payload.to_json()) {
                        Ok(_) => delivered += 1,
                        Err(err) => {
                            eprintln!("shep bridge notify-push: {target} failed: {err}")
                        }
                    }
                }
                Transport::Fcm { token } => match fcm::send(token, &payload) {
                    Ok(fcm::Delivery::Sent) => delivered += 1,
                    Ok(fcm::Delivery::Unregistered) => {
                        eprintln!(
                            "shep bridge notify-push: dropping unregistered device {}",
                            endpoint.label
                        );
                        stale.push(token.clone());
                    }
                    Err(err) => {
                        eprintln!("shep bridge notify-push: fcm send failed: {err}")
                    }
                },
            }
        }

        // A token that FCM says is gone will never work again — the app was
        // uninstalled or reinstalled — so drop it rather than paying a failed
        // request on every future notification.
        if !stale.is_empty() {
            let kept: Vec<Endpoint> = endpoints
                .into_iter()
                .filter(|e| !stale.iter().any(|token| token == e.transport.key()))
                .collect();
            if let Err(err) = store(&path, &kept) {
                eprintln!("shep bridge notify-push: could not prune stale devices: {err}");
            }
        }
        if delivered == 0 {
            eprintln!("shep bridge notify-push: nothing delivered for kind {kind:?}");
        }
        Ok(0)
    }

    /// What one notification says. Flat and all-strings because FCM's `data`
    /// block only carries strings, and the UnifiedPush body is the same shape so
    /// the app has one parser rather than two.
    pub(super) struct Payload {
        pub(super) kind: String,
        pub(super) state: String,
        pub(super) agent: String,
        pub(super) workspace: String,
        pub(super) pane_id: String,
        pub(super) title: String,
        pub(super) task_id: String,
        pub(super) message: String,
    }

    impl Payload {
        pub(super) fn fields(&self) -> serde_json::Value {
            serde_json::json!({
                "kind": self.kind,
                "state": self.state,
                "agent": self.agent,
                "workspace": self.workspace,
                "pane_id": self.pane_id,
                "title": self.title,
                "task_id": self.task_id,
                "message": self.message,
            })
        }

        fn to_json(&self) -> String {
            self.fields().to_string()
        }
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

    /// FCM HTTP v1 delivery.
    ///
    /// Sending needs an OAuth2 access token minted from a service-account key,
    /// which means signing a JWT with RS256. Rather than take an RSA/JWT crate
    /// for one signature an hour, this shells out to `openssl` — the same
    /// reasoning that already has this module shelling out to `curl` instead of
    /// depending on an HTTP client.
    pub(super) mod fcm {
        use super::Payload;
        use std::io::Write;
        use std::path::{Path, PathBuf};

        const SCOPE: &str = "https://www.googleapis.com/auth/firebase.messaging";
        /// Google mints one-hour tokens; refresh a minute early so a send never
        /// races the expiry.
        const EXPIRY_SKEW_SECS: u64 = 60;

        pub(super) enum Delivery {
            Sent,
            /// FCM says this token is dead: the app was uninstalled or its
            /// registration was replaced. Never recoverable.
            Unregistered,
        }

        fn service_account_path() -> PathBuf {
            crate::config::config_dir().join("fcm-service-account.json")
        }

        fn token_cache_path() -> PathBuf {
            crate::config::config_dir().join("fcm-token.json")
        }

        struct ServiceAccount {
            project_id: String,
            client_email: String,
            private_key: String,
            token_uri: String,
        }

        fn load_service_account(path: &Path) -> Result<ServiceAccount, String> {
            let text = std::fs::read_to_string(path).map_err(|err| {
                format!(
                    "no FCM service account at {}: {err} — download one from the \
                     Firebase console (Project settings → Service accounts)",
                    path.display()
                )
            })?;
            let value: serde_json::Value =
                serde_json::from_str(&text).map_err(|err| format!("malformed key file: {err}"))?;
            let field = |name: &str| -> Result<String, String> {
                value
                    .get(name)
                    .and_then(|v| v.as_str())
                    .filter(|v| !v.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| format!("key file is missing {name}"))
            };
            Ok(ServiceAccount {
                project_id: field("project_id")?,
                client_email: field("client_email")?,
                private_key: field("private_key")?,
                token_uri: field("token_uri")
                    .unwrap_or_else(|_| "https://oauth2.googleapis.com/token".to_string()),
            })
        }

        fn now_unix() -> u64 {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0)
        }

        fn base64url(bytes: &[u8]) -> String {
            use base64::Engine;
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
        }

        /// Sign `input` with the service account's RSA key, via `openssl`.
        ///
        /// openssl reads the key from a file, not stdin (stdin carries the data
        /// being signed), so the PEM is written to a 0600 file in the config
        /// directory for the duration of the call and removed after. It sits
        /// beside the service-account JSON, which already holds the same key at
        /// the same permissions, so this widens nothing.
        fn sign_rs256(private_key_pem: &str, input: &str) -> Result<Vec<u8>, String> {
            let key_path = crate::config::config_dir().join(format!(
                ".fcm-signing-key-{}-{}.pem",
                std::process::id(),
                now_unix()
            ));
            if let Some(parent) = key_path.parent() {
                std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
            }
            std::fs::write(&key_path, private_key_pem).map_err(|err| err.to_string())?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let _ = std::fs::set_permissions(&key_path, std::fs::Permissions::from_mode(0o600));
            }
            let result = sign_with_key_file(&key_path, input);
            let _ = std::fs::remove_file(&key_path);
            result
        }

        fn sign_with_key_file(key_path: &Path, input: &str) -> Result<Vec<u8>, String> {
            let mut child = std::process::Command::new("openssl")
                .arg("dgst")
                .arg("-sha256")
                .arg("-sign")
                .arg(key_path)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|err| format!("could not run openssl: {err}"))?;
            child
                .stdin
                .take()
                .ok_or("openssl stdin unavailable")?
                .write_all(input.as_bytes())
                .map_err(|err| err.to_string())?;
            let output = child.wait_with_output().map_err(|err| err.to_string())?;
            if !output.status.success() {
                return Err(format!(
                    "openssl signing failed: {}",
                    String::from_utf8_lossy(&output.stderr).trim()
                ));
            }
            Ok(output.stdout)
        }

        /// Build the signed JWT that is exchanged for an access token.
        fn build_assertion(account: &ServiceAccount, issued_at: u64) -> Result<String, String> {
            let header = base64url(br#"{"alg":"RS256","typ":"JWT"}"#);
            let claims = base64url(
                serde_json::json!({
                    "iss": account.client_email,
                    "scope": SCOPE,
                    "aud": account.token_uri,
                    "iat": issued_at,
                    "exp": issued_at + 3600,
                })
                .to_string()
                .as_bytes(),
            );
            let signing_input = format!("{header}.{claims}");
            let signature = sign_rs256(&account.private_key, &signing_input)?;
            Ok(format!("{signing_input}.{}", base64url(&signature)))
        }

        fn cached_token(path: &Path, now: u64) -> Option<String> {
            let text = std::fs::read_to_string(path).ok()?;
            let value: serde_json::Value = serde_json::from_str(&text).ok()?;
            let expires_at = value.get("expires_at")?.as_u64()?;
            if expires_at <= now + EXPIRY_SKEW_SECS {
                return None;
            }
            value
                .get("access_token")?
                .as_str()
                .filter(|t| !t.is_empty())
                .map(str::to_string)
        }

        fn cache_token(path: &Path, token: &str, expires_at: u64) {
            let body = serde_json::json!({"access_token": token, "expires_at": expires_at});
            if std::fs::write(path, body.to_string()).is_ok() {
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
                }
            }
        }

        fn access_token(account: &ServiceAccount) -> Result<String, String> {
            let cache = token_cache_path();
            let now = now_unix();
            if let Some(token) = cached_token(&cache, now) {
                return Ok(token);
            }
            let assertion = build_assertion(account, now)?;
            let form = format!(
                "grant_type=urn:ietf:params:oauth:grant-type:jwt-bearer&assertion={assertion}"
            );
            let response = super::post_form(&account.token_uri, &form)
                .map_err(|err| format!("token exchange failed: {err}"))?;
            let value: serde_json::Value = serde_json::from_str(&response.body)
                .map_err(|err| format!("token response was not JSON: {err}"))?;
            let token = value
                .get("access_token")
                .and_then(|t| t.as_str())
                .ok_or_else(|| format!("token response had no access_token: {}", response.body))?;
            let expires_in = value
                .get("expires_in")
                .and_then(|e| e.as_u64())
                .unwrap_or(3600);
            cache_token(&cache, token, now + expires_in);
            Ok(token.to_string())
        }

        /// Send one notification to one device.
        pub(super) fn send(token: &str, payload: &Payload) -> Result<Delivery, String> {
            let account = load_service_account(&service_account_path())?;
            let access = access_token(&account)?;
            let url = format!(
                "https://fcm.googleapis.com/v1/projects/{}/messages:send",
                account.project_id
            );
            // Data-only, deliberately: a `notification` block would have Android
            // render the notification itself, which drops the Approve/Deny
            // actions and the pane deep-link. High priority is what gets the app
            // woken out of Doze at all.
            let body = serde_json::json!({
                "message": {
                    "token": token,
                    "android": { "priority": "high" },
                    "data": payload.fields(),
                }
            })
            .to_string();
            let response = super::post_json_authorized(&url, &body, &access)?;
            if response.status == 200 {
                return Ok(Delivery::Sent);
            }
            // 404 UNREGISTERED / 403 SENDER_ID_MISMATCH mean this token is gone
            // for good; anything else may be transient and is worth reporting.
            if response.status == 404 || response.body.contains("UNREGISTERED") {
                return Ok(Delivery::Unregistered);
            }
            Err(format!(
                "fcm returned {}: {}",
                response.status,
                summarize_error(&response.body)
            ))
        }

        /// Reduce an FCM error body to its one useful sentence.
        ///
        /// The raw response is a ~500-character nested JSON document. This is
        /// surfaced in a phone's settings screen, where that is unreadable, and
        /// `error.message` already says the whole story.
        fn summarize_error(body: &str) -> String {
            serde_json::from_str::<serde_json::Value>(body)
                .ok()
                .and_then(|value| {
                    value
                        .get("error")?
                        .get("message")?
                        .as_str()
                        .map(str::to_string)
                })
                .unwrap_or_else(|| super::truncate(body.trim(), 200))
        }

        #[cfg(test)]
        mod tests {
            use super::*;

            #[test]
            fn cached_token_is_ignored_once_inside_the_refresh_skew() {
                let path = std::env::temp_dir()
                    .join(format!("shep-fcm-token-{}.json", std::process::id()));
                cache_token(&path, "abc", 1_000);
                // Comfortably valid.
                assert_eq!(cached_token(&path, 500).as_deref(), Some("abc"));
                // Inside the skew: treated as expired so a send never races it.
                assert!(cached_token(&path, 1_000 - EXPIRY_SKEW_SECS).is_none());
                assert!(cached_token(&path, 2_000).is_none());
                let _ = std::fs::remove_file(&path);
            }

            /// A phone settings screen shows this string, so it has to be one
            /// readable sentence rather than the raw nested error document.
            #[test]
            fn error_summary_keeps_the_sentence_and_drops_the_envelope() {
                let body = r#"{"error":{"code":400,"message":"The registration token is not a valid FCM registration token","status":"INVALID_ARGUMENT","details":[{"@type":"x"}]}}"#;
                assert_eq!(
                    summarize_error(body),
                    "The registration token is not a valid FCM registration token"
                );
                // A non-JSON body (a proxy error page, say) still says something.
                assert_eq!(summarize_error("  gateway timeout  "), "gateway timeout");
            }

            #[test]
            fn service_account_reports_what_is_missing() {
                let path =
                    std::env::temp_dir().join(format!("shep-fcm-sa-{}.json", std::process::id()));
                std::fs::write(&path, r#"{"project_id":"p"}"#).unwrap();
                let err = load_service_account(&path)
                    .err()
                    .expect("a key file missing client_email must not load");
                assert!(err.contains("client_email"), "{err}");
                let _ = std::fs::remove_file(&path);
            }

            #[test]
            fn missing_service_account_says_where_to_put_one() {
                let err = load_service_account(Path::new("/nonexistent/fcm.json"))
                    .err()
                    .expect("a missing key file must not load");
                assert!(err.contains("Firebase console"), "{err}");
            }

            /// The assertion must be three base64url segments over the exact
            /// signing input, or Google rejects it with an opaque error.
            #[test]
            fn assertion_is_three_segments_signed_over_the_first_two() {
                let key_path =
                    std::env::temp_dir().join(format!("shep-fcm-key-{}.pem", std::process::id()));
                let generated = std::process::Command::new("openssl")
                    .args([
                        "genpkey",
                        "-algorithm",
                        "RSA",
                        "-pkeyopt",
                        "rsa_keygen_bits:2048",
                    ])
                    .arg("-out")
                    .arg(&key_path)
                    .output();
                let Ok(output) = generated else { return };
                if !output.status.success() {
                    return;
                }
                let pem = std::fs::read_to_string(&key_path).unwrap();
                let account = ServiceAccount {
                    project_id: "p".into(),
                    client_email: "svc@example.com".into(),
                    private_key: pem,
                    token_uri: "https://oauth2.googleapis.com/token".into(),
                };
                let assertion = build_assertion(&account, 1_700_000_000).unwrap();
                let parts: Vec<&str> = assertion.split('.').collect();
                assert_eq!(parts.len(), 3, "{assertion}");
                assert!(!parts[2].is_empty());
                // base64url, not standard base64: '+' and '/' would be rejected.
                assert!(!assertion.contains('+') && !assertion.contains('/'));
                use base64::Engine;
                let claims = base64::engine::general_purpose::URL_SAFE_NO_PAD
                    .decode(parts[1])
                    .unwrap();
                let claims: serde_json::Value = serde_json::from_slice(&claims).unwrap();
                assert_eq!(claims["iss"], "svc@example.com");
                assert_eq!(claims["scope"], SCOPE);
                assert_eq!(claims["exp"].as_u64().unwrap(), 1_700_003_600);
                let _ = std::fs::remove_file(&key_path);
            }

            /// The temporary PEM must not survive the signature.
            #[test]
            fn signing_key_file_is_removed_even_when_openssl_rejects_the_key() {
                let before = signing_key_files();
                assert!(sign_rs256("not a key", "data").is_err());
                assert_eq!(signing_key_files(), before);
            }

            fn signing_key_files() -> usize {
                std::fs::read_dir(crate::config::config_dir())
                    .map(|entries| {
                        entries
                            .flatten()
                            .filter(|e| {
                                e.file_name()
                                    .to_string_lossy()
                                    .starts_with(".fcm-signing-key-")
                            })
                            .count()
                    })
                    .unwrap_or(0)
            }
        }
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

    /// An HTTP response we need to read, not just fire and forget.
    pub(super) struct Response {
        pub(super) status: u16,
        pub(super) body: String,
    }

    /// Run curl and split the trailing status code off the body.
    ///
    /// `-w '%{http_code}'` appends the code after the body, so the last three
    /// characters are the status and everything before them is the payload.
    fn curl_capture(args: &[&str]) -> Result<Response, String> {
        let output = std::process::Command::new("curl")
            .args(["-sS", "-m", "20", "-w", "%{http_code}"])
            .args(args)
            .output()
            .map_err(|err| format!("could not run curl: {err}"))?;
        if !output.status.success() {
            return Err(format!(
                "curl failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        let combined = String::from_utf8_lossy(&output.stdout).to_string();
        if combined.len() < 3 {
            return Err(format!("curl returned no status code: {combined:?}"));
        }
        let split = combined.len() - 3;
        let status = combined[split..].parse::<u16>().map_err(|_| {
            format!(
                "curl returned a malformed status code: {:?}",
                &combined[split..]
            )
        })?;
        Ok(Response {
            status,
            body: combined[..split].to_string(),
        })
    }

    /// POST a form-encoded body — the OAuth2 token exchange.
    pub(super) fn post_form(url: &str, form: &str) -> Result<Response, String> {
        curl_capture(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/x-www-form-urlencoded",
            "--data",
            form,
            url,
        ])
    }

    /// POST JSON with a bearer token — the FCM send.
    pub(super) fn post_json_authorized(
        url: &str,
        body: &str,
        access_token: &str,
    ) -> Result<Response, String> {
        curl_capture(&[
            "-X",
            "POST",
            "-H",
            "Content-Type: application/json",
            "-H",
            &format!("Authorization: Bearer {access_token}"),
            "--data",
            body,
            url,
        ])
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

        fn unified(url: &str) -> Transport {
            Transport::UnifiedPush {
                url: url.to_string(),
            }
        }

        fn fcm_token(token: &str) -> Transport {
            Transport::Fcm {
                token: token.to_string(),
            }
        }

        #[test]
        fn register_dedups_and_updates_label() {
            let path = tmp("dedup");
            std::fs::remove_file(&path).ok();
            assert_eq!(
                register_at(&path, unified("https://ntfy/UPa"), "phone", None).unwrap(),
                1
            );
            // Same URL, new label → still one entry, label updated.
            assert_eq!(
                register_at(&path, unified("https://ntfy/UPa"), "s22", None).unwrap(),
                1
            );
            assert_eq!(
                register_at(&path, unified("https://ntfy/UPb"), "avd", None).unwrap(),
                2
            );
            let loaded = load(&path);
            assert_eq!(loaded.len(), 2);
            assert_eq!(loaded[0].label, "s22");
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn register_rejects_non_http() {
            let path = tmp("reject");
            std::fs::remove_file(&path).ok();
            assert!(register_at(&path, unified("ftp://nope"), "x", None).is_err());
            assert!(register_at(&path, unified(""), "x", None).is_err());
            assert!(register_at(&path, fcm_token(""), "x", None).is_err());
            assert!(register_at(&path, fcm_token("has space"), "x", None).is_err());
            std::fs::remove_file(&path).ok();
        }

        /// The app re-registers on every cold start, and must not wipe the
        /// user's notification settings when it does.
        #[test]
        fn re_registering_without_kinds_keeps_the_chosen_ones() {
            let path = tmp("keep-kinds");
            std::fs::remove_file(&path).ok();
            register_at(
                &path,
                fcm_token("tok"),
                "phone",
                Some(vec!["blocked".into(), "done".into()]),
            )
            .unwrap();
            register_at(&path, fcm_token("tok"), "phone", None).unwrap();
            let loaded = load(&path);
            assert_eq!(loaded.len(), 1);
            assert_eq!(
                loaded[0].kinds.as_deref(),
                Some(&["blocked".to_string(), "done".to_string()][..])
            );
            std::fs::remove_file(&path).ok();
        }

        /// Rows written before the transport and kinds columns existed must
        /// still load, as UnifiedPush wanting everything.
        #[test]
        fn legacy_rows_load_as_unifiedpush_wanting_every_kind() {
            let path = tmp("legacy");
            std::fs::write(
                &path,
                r#"[{"endpoint":"https://ntfy.sh/abc?up=1","label":"CPH2611"}]"#,
            )
            .unwrap();
            let loaded = load(&path);
            assert_eq!(loaded.len(), 1);
            assert_eq!(loaded[0].transport, unified("https://ntfy.sh/abc?up=1"));
            assert!(loaded[0].kinds.is_none());
            for kind in ["blocked", "done", "task", "review"] {
                assert!(loaded[0].wants(kind));
            }
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn kinds_filter_delivery_per_device() {
            let endpoint = Endpoint {
                transport: fcm_token("tok"),
                label: "phone".into(),
                kinds: Some(vec!["blocked".into(), "review".into()]),
            };
            assert!(endpoint.wants("blocked"));
            assert!(endpoint.wants("review"));
            assert!(!endpoint.wants("done"));
            assert!(!endpoint.wants("task"));
            // An exec that reports no kind still gets through: dropping it
            // would turn an upgrade mismatch into total silence.
            assert!(endpoint.wants(""));
        }

        #[test]
        fn set_kinds_targets_the_only_device_without_being_told_which() {
            let path = tmp("setkinds-one");
            std::fs::remove_file(&path).ok();
            register_at(&path, fcm_token("tok"), "phone", None).unwrap();
            assert_eq!(set_kinds_at(&path, None, vec!["done".into()]).unwrap(), 1);
            assert_eq!(
                load(&path)[0].kinds.as_deref(),
                Some(&["done".to_string()][..])
            );
            std::fs::remove_file(&path).ok();
        }

        #[test]
        fn set_kinds_requires_a_target_when_several_devices_are_registered() {
            let path = tmp("setkinds-many");
            std::fs::remove_file(&path).ok();
            register_at(&path, fcm_token("a"), "phone", None).unwrap();
            register_at(&path, fcm_token("b"), "tablet", None).unwrap();
            assert!(set_kinds_at(&path, None, vec!["done".into()]).is_err());
            set_kinds_at(&path, Some("b"), vec!["done".into()]).unwrap();
            let loaded = load(&path);
            assert!(loaded[0].kinds.is_none());
            assert_eq!(loaded[1].kinds.as_deref(), Some(&["done".to_string()][..]));
            std::fs::remove_file(&path).ok();
        }

        /// The co-location rewrite is a UnifiedPush affordance and must never
        /// be reachable for an FCM row.
        ///
        /// This is the shape of the bug that made push stop arriving: a phone
        /// registered against one broker while `SHEP_NTFY_PUBLISH_BASE`
        /// redirected every publish to another, and nothing reported a failure
        /// because the POST to the wrong place succeeded.
        #[test]
        fn publish_url_rewrite_applies_only_to_unifiedpush() {
            let base = Some("http://127.0.0.1:2587");
            assert_eq!(
                resolve_publish_url("https://ntfy.example/UPa?up=1", base),
                "http://127.0.0.1:2587/UPa?up=1"
            );
            // An FCM device carries no URL to rewrite — the send target is
            // derived from the service account's project id instead — so the
            // transport itself is the guard.
            let endpoint = Endpoint {
                transport: fcm_token("tok"),
                label: "phone".into(),
                kinds: None,
            };
            assert!(matches!(endpoint.transport, Transport::Fcm { .. }));
        }

        /// A round-trip through the file must not quietly change a device.
        #[test]
        fn store_and_load_round_trip_both_transports() {
            let path = tmp("roundtrip");
            let original = vec![
                Endpoint {
                    transport: fcm_token("tok"),
                    label: "phone".into(),
                    kinds: Some(vec!["blocked".into()]),
                },
                Endpoint {
                    transport: unified("https://ntfy/UPa"),
                    label: "old".into(),
                    kinds: None,
                },
            ];
            store(&path, &original).unwrap();
            assert_eq!(load(&path), original);
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

/// Bridge-local task queue methods (`task.list`/`add`/`cancel`/`remove`/
/// `clear`/`assign`).
///
/// These mirror the `shep task` CLI: add/list/cancel are direct operations on
/// the local `<state>/tasks.db` (they work server-up-or-not), so the bridge —
/// which runs on the same box — performs them itself rather than inventing new
/// JSON API methods. `task.dispatch` is deliberately absent here: dispatching
/// must spawn a pane, which only the server can do, so it proxies through the
/// existing `task.dispatch` API method.
mod task_local {
    use crate::tasks::{self, TaskRecord, TaskRuntime};
    use serde_json::{json, Value};

    pub(super) fn handle_local_method(
        method: &str,
        params: Option<&Value>,
    ) -> Option<Result<Value, String>> {
        match method {
            "task.list" => Some(list()),
            "task.add" => Some(add(params)),
            "task.cancel" => Some(cancel(params)),
            "task.remove" => Some(remove(params)),
            "task.clear" => Some(clear()),
            "task.assign" => Some(assign(params)),
            _ => None,
        }
    }

    fn open() -> Result<rusqlite::Connection, String> {
        tasks::open_store(&tasks::tasks_db_path()).map_err(|err| err.to_string())
    }

    fn record_json(task: &TaskRecord) -> Value {
        json!({
            "id": task.id,
            "prompt": task.prompt,
            "repo": task.repo.display().to_string(),
            "runtime": task.runtime.as_str(),
            "use_worktree": task.use_worktree,
            "state": task.state.as_str(),
            "workspace_id": task.workspace_id,
            "created_at": task.created_at,
            "updated_at": task.updated_at,
        })
    }

    fn list() -> Result<Value, String> {
        let db = tasks::tasks_db_path();
        if !db.exists() {
            return Ok(json!({ "tasks": [] }));
        }
        let conn = open()?;
        let records = tasks::list_tasks(&conn).map_err(|err| err.to_string())?;
        let tasks: Vec<Value> = records.iter().map(record_json).collect();
        Ok(json!({ "tasks": tasks }))
    }

    fn add(params: Option<&Value>) -> Result<Value, String> {
        let params = params.ok_or("missing params")?;
        let prompt = params
            .get("prompt")
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .trim()
            .to_string();
        if prompt.is_empty() {
            return Err("missing prompt".to_string());
        }
        // The phone has no cwd to infer a repo from, so `repo` is required and
        // must resolve to a git repo root (same rule as `shep task add --repo`).
        let repo_path = params
            .get("repo")
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("missing repo (path to a git repository)")?;
        let repo = crate::memory::resolve_repo_root(Some(std::path::Path::new(repo_path)))
            .map_err(|err| err.to_string())?;
        let runtime = match params.get("runtime").and_then(|value| value.as_str()) {
            Some(raw) => TaskRuntime::parse(raw)
                .ok_or_else(|| format!("invalid runtime {raw} (claude|opencode)"))?,
            None => TaskRuntime::Claude,
        };
        let use_worktree = params
            .get("worktree")
            .and_then(|value| value.as_bool())
            .unwrap_or(false);
        let conn = open()?;
        let id = tasks::add_task(
            &conn,
            &prompt,
            &repo,
            runtime,
            use_worktree,
            tasks::unix_now(),
        )
        .map_err(|err| err.to_string())?;
        Ok(json!({ "id": id, "use_worktree": use_worktree }))
    }

    fn cancel(params: Option<&Value>) -> Result<Value, String> {
        let id = params
            .and_then(|params| params.get("id"))
            .and_then(|value| value.as_i64())
            .ok_or("missing id")?;
        let conn = open()?;
        let cancelled =
            tasks::cancel_task(&conn, id, tasks::unix_now()).map_err(|err| err.to_string())?;
        Ok(json!({ "cancelled": cancelled }))
    }

    fn task_id(params: Option<&Value>) -> Result<i64, String> {
        params
            .and_then(|params| params.get("id"))
            .and_then(|value| value.as_i64())
            .ok_or_else(|| "missing id".to_string())
    }

    fn remove(params: Option<&Value>) -> Result<Value, String> {
        let id = task_id(params)?;
        let conn = open()?;
        let removed = tasks::delete_task(&conn, id).map_err(|err| err.to_string())?;
        Ok(json!({ "removed": removed }))
    }

    /// Sweep every finished task. Absent store is an empty sweep, not an error —
    /// a phone clearing a queue that was never created has nothing to fix.
    fn clear() -> Result<Value, String> {
        if !tasks::tasks_db_path().exists() {
            return Ok(json!({ "removed": 0 }));
        }
        let conn = open()?;
        let removed = tasks::clear_finished(&conn).map_err(|err| err.to_string())?;
        Ok(json!({ "removed": removed }))
    }

    /// Hand an open task to a workspace whose agent is already running. The
    /// caller sends the prompt itself (`agent.send`); this records the linkage
    /// so the server's state tracker carries the task to blocked/done.
    fn assign(params: Option<&Value>) -> Result<Value, String> {
        let id = task_id(params)?;
        let workspace_id = params
            .and_then(|params| params.get("workspace_id"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .ok_or("missing workspace_id")?;
        let conn = open()?;
        let assigned = tasks::assign_task(&conn, id, workspace_id, tasks::unix_now())
            .map_err(|err| err.to_string())?;
        Ok(json!({ "assigned": assigned }))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_owns_task_add_list_cancel() {
            assert!(handle_local_method("task.dispatch", None).is_none());
            assert!(handle_local_method("session.snapshot", None).is_none());
            assert!(handle_local_method("task.list", None).is_some());
            assert!(handle_local_method("task.remove", None).is_some());
            assert!(handle_local_method("task.clear", None).is_some());
            assert!(handle_local_method("task.assign", None).is_some());
        }

        #[test]
        fn remove_and_assign_validate_their_params() {
            assert!(remove(Some(&json!({}))).is_err());
            assert!(assign(Some(&json!({ "id": 1 }))).is_err());
            assert!(assign(Some(&json!({ "workspace_id": "w1" }))).is_err());
            assert!(assign(Some(&json!({ "id": 1, "workspace_id": "  " }))).is_err());
        }

        #[test]
        fn add_requires_prompt_and_repo() {
            let missing_prompt = json!({ "repo": "/tmp" });
            assert!(add(Some(&missing_prompt)).is_err());
            let missing_repo = json!({ "prompt": "do a thing" });
            assert!(add(Some(&missing_repo)).is_err());
        }

        #[test]
        fn cancel_requires_id() {
            assert!(cancel(Some(&json!({}))).is_err());
        }
    }
}

/// Bridge-local shared-memory methods (`memory.show`/`add`/`replace`/`remove`).
///
/// Mirrors `shep memory`: operations on the plain-markdown memory files
/// (`~/.config/shep/memory/USER.md` and `<repo>/.shep/memory/MEMORY.md`). The
/// optional `repo` param selects the per-repo file; absent it targets the user
/// profile. Search (`shep memory search`) is over a separate FTS history db and
/// is intentionally not exposed here yet.
mod memory_local {
    use crate::memory::{self, MemoryDoc, MemoryKind};
    use serde_json::{json, Value};
    use std::path::{Path, PathBuf};

    pub(super) fn handle_local_method(
        method: &str,
        params: Option<&Value>,
    ) -> Option<Result<Value, String>> {
        match method {
            "memory.show" => Some(show(params)),
            "memory.add" => Some(add(params)),
            "memory.replace" => Some(replace(params)),
            "memory.remove" => Some(remove(params)),
            _ => None,
        }
    }

    fn target(params: Option<&Value>) -> Result<(PathBuf, MemoryKind), String> {
        let repo = params
            .and_then(|params| params.get("repo"))
            .and_then(|value| value.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty());
        match repo {
            Some(path) => {
                let root = memory::resolve_repo_root(Some(Path::new(path)))
                    .map_err(|err| err.to_string())?;
                Ok((memory::repo_memory_path(&root), MemoryKind::Repo))
            }
            None => Ok((memory::user_memory_path(), MemoryKind::User)),
        }
    }

    fn load(params: Option<&Value>) -> Result<(PathBuf, MemoryKind, MemoryDoc), String> {
        let (path, kind) = target(params)?;
        let doc = memory::load_or_create(&path, kind).map_err(|err| err.to_string())?;
        Ok((path, kind, doc))
    }

    fn show_json(kind: MemoryKind, doc: &MemoryDoc) -> Value {
        let usage = doc.usage(kind.cap());
        json!({
            "kind": kind.label(),
            "entries": doc.entries(),
            "used": usage.used,
            "cap": usage.cap,
            "percent": usage.percent(),
            "count": usage.entries,
        })
    }

    fn field(params: Option<&Value>, key: &str) -> String {
        params
            .and_then(|params| params.get(key))
            .and_then(|value| value.as_str())
            .unwrap_or_default()
            .to_string()
    }

    fn show(params: Option<&Value>) -> Result<Value, String> {
        let (_path, kind, doc) = load(params)?;
        Ok(show_json(kind, &doc))
    }

    fn add(params: Option<&Value>) -> Result<Value, String> {
        let text = field(params, "text");
        let (path, kind, mut doc) = load(params)?;
        doc.add(&text, kind.cap()).map_err(|err| err.to_string())?;
        memory::write_doc(&path, &doc).map_err(|err| err.to_string())?;
        Ok(show_json(kind, &doc))
    }

    fn replace(params: Option<&Value>) -> Result<Value, String> {
        let find = field(params, "find");
        let text = field(params, "text");
        let (path, kind, mut doc) = load(params)?;
        doc.replace(&find, &text, kind.cap())
            .map_err(|err| err.to_string())?;
        memory::write_doc(&path, &doc).map_err(|err| err.to_string())?;
        Ok(show_json(kind, &doc))
    }

    fn remove(params: Option<&Value>) -> Result<Value, String> {
        let find = field(params, "find");
        let (path, kind, mut doc) = load(params)?;
        doc.remove(&find).map_err(|err| err.to_string())?;
        memory::write_doc(&path, &doc).map_err(|err| err.to_string())?;
        Ok(show_json(kind, &doc))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn only_owns_memory_methods() {
            assert!(handle_local_method("memory.search", None).is_none());
            assert!(handle_local_method("session.snapshot", None).is_none());
            assert!(handle_local_method("memory.show", None).is_some());
        }

        #[test]
        fn absent_repo_targets_user_profile() {
            let (_path, kind) = target(None).unwrap();
            assert_eq!(kind, MemoryKind::User);
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

    /// Regression: the rejection used to be `ErrorResponse::new(..)`, which
    /// defaults to 200. tungstenite refuses to write a 2xx error response and
    /// closes the socket having sent nothing, so the companion app reported
    /// "unexpected end of stream" and a wrong token looked like a dead network.
    /// The non-success status is the load-bearing part of this response.
    #[test]
    fn unauthorized_response_is_a_real_401() {
        let response = unauthorized_response();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(
            !response.status().is_success(),
            "a success status makes tungstenite send nothing at all"
        );
        assert_eq!(
            response.headers().get(header::WWW_AUTHENTICATE).unwrap(),
            "Bearer realm=\"shep bridge\""
        );
        let body = response.body().as_ref().expect("body");
        assert_eq!(
            response.headers().get(header::CONTENT_LENGTH).unwrap(),
            body.len().to_string().as_str(),
            "write_response emits headers only; the body is appended after it"
        );
    }

    /// Assert the bytes that actually reach the client, via the same writer
    /// tungstenite uses on the rejection path.
    #[test]
    fn unauthorized_response_serializes_to_a_401_status_line() {
        let response = unauthorized_response();
        let mut out = Vec::new();
        tungstenite::handshake::server::write_response(&mut out, &response).unwrap();
        let wire = String::from_utf8(out).unwrap();
        assert!(
            wire.starts_with("HTTP/1.1 401 Unauthorized\r\n"),
            "unexpected status line: {wire:?}"
        );
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
        channels.insert(7u64, ChannelHandle::Relay(cancel.clone()));
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame(r#"{"ch":7,"close":true}"#, &out_tx, &mut channels, &sock);
        assert!(cancel.load(Ordering::Relaxed));
        assert!(channels.is_empty());
    }

    #[test]
    fn data_frame_without_an_open_channel_errors() {
        let (out_tx, out_rx) = mpsc::channel();
        let mut channels = HashMap::new();
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame(
            r#"{"ch":3,"data":{"text":"hi"}}"#,
            &out_tx,
            &mut channels,
            &sock,
        );
        assert!(out_rx.recv().unwrap().contains("no such channel"));
    }

    #[test]
    fn data_frame_on_a_relay_channel_errors() {
        let (out_tx, out_rx) = mpsc::channel();
        let mut channels = HashMap::new();
        channels.insert(3u64, ChannelHandle::Relay(Arc::new(AtomicBool::new(false))));
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame(
            r#"{"ch":3,"data":{"text":"hi"}}"#,
            &out_tx,
            &mut channels,
            &sock,
        );
        assert!(out_rx
            .recv()
            .unwrap()
            .contains("channel does not accept data"));
    }

    #[test]
    fn reusing_a_live_channel_id_errors_instead_of_orphaning_it() {
        let (out_tx, out_rx) = mpsc::channel();
        let mut channels = HashMap::new();
        let cancel = Arc::new(AtomicBool::new(false));
        channels.insert(4u64, ChannelHandle::Relay(cancel.clone()));
        let sock = Arc::new(PathBuf::from("/nonexistent"));
        handle_client_frame(
            r#"{"ch":4,"req":{"method":"session.snapshot"}}"#,
            &out_tx,
            &mut channels,
            &sock,
        );
        assert!(out_rx.recv().unwrap().contains("channel already open"));
        // The original channel is untouched and still cancellable.
        assert!(!cancel.load(Ordering::Relaxed));
        assert_eq!(channels.len(), 1);
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
