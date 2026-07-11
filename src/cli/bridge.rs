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
const USAGE: &str = "usage: shep bridge [--bind <ip:port>] [--socket <api-socket-path>] | shep bridge pair [--host <ip[:port]>] | shep bridge token";

pub(super) fn run_bridge_command(args: &[String]) -> std::io::Result<i32> {
    match args.first().map(|arg| arg.as_str()) {
        Some("pair") => pair(&args[1..]),
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
    println!("url: ws://{host}/");
    println!("token: {token}");
    println!("paste both into the companion app's pairing screen.");
    Ok(0)
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
