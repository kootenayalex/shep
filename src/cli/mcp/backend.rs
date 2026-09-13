//! What answers a tool call, behind a trait so the protocol loop can be
//! tested without a server.
//!
//! Three backings, three very different things: the JSON API over the session
//! socket (one request per connection, like every other client), the
//! bridge-local dispatcher (files on this machine — no server, no protocol
//! version), and one in-process probe (`shep doctor`, which is a pure function
//! over the box and has no business crossing a socket).

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use serde_json::{json, Value};

use crate::api::client::{ApiClient, ConnectionTarget};
use crate::api::schema::Request;

/// The socket API has no deadline of its own; a stdio server that blocks
/// forever looks to its client like a hung model.
const API_TIMEOUT: Duration = Duration::from_secs(30);

pub(crate) trait Backend {
    /// Ask the running server one API method. `Err` carries the message a
    /// caller should see.
    fn api(&self, method: &str, params: Value) -> Result<Value, String>;
    /// Answer a bridge-local method. `None` means it is not one — the caller
    /// turns that into an invalid-params refusal rather than an error result.
    fn local(&self, method: &str, params: Value) -> Option<Result<Value, String>>;
    /// The `shep doctor` findings, gathered in this process.
    fn doctor(&self) -> Value;
}

pub(crate) struct SocketBackend {
    socket: PathBuf,
}

impl SocketBackend {
    pub(crate) fn new(socket: PathBuf) -> Self {
        Self { socket }
    }

    pub(crate) fn socket(&self) -> &Path {
        &self.socket
    }
}

fn next_request_id() -> String {
    static NEXT: AtomicUsize = AtomicUsize::new(1);
    format!("mcp:{}", NEXT.fetch_add(1, Ordering::Relaxed))
}

impl Backend for SocketBackend {
    fn api(&self, method: &str, params: Value) -> Result<Value, String> {
        let request: Request = serde_json::from_value(
            json!({"id": next_request_id(), "method": method, "params": params}),
        )
        .map_err(|err| err.to_string())?;
        let client = ApiClient::for_target(ConnectionTarget::SocketPath(self.socket.clone()));
        let value = client
            .request_value_with_timeout(&request, API_TIMEOUT)
            .map_err(|err| err.to_string())?;
        if let Some(error) = value.get("error") {
            let message = error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("api error");
            let code = error
                .get("code")
                .and_then(Value::as_str)
                .unwrap_or_default();
            return Err(if code.is_empty() {
                message.to_string()
            } else {
                format!("{code}: {message}")
            });
        }
        value
            .get("result")
            .cloned()
            .ok_or_else(|| "api response had no result".to_string())
    }

    fn local(&self, method: &str, params: Value) -> Option<Result<Value, String>> {
        crate::cli::bridge::handle_local_method(method, &params, &self.socket)
    }

    fn doctor(&self) -> Value {
        json!({"findings": crate::cli::doctor::run_probes()})
    }
}
