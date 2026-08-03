//! Live pane streaming for bridge clients.
//!
//! `pane.stream` gives a WebSocket client a live view of one pane plus a way to
//! type into it, without the client ever speaking the private bincode client
//! protocol. The bridge does that translation here, in Rust:
//!
//! - **Output** comes from `ClientMessage::ObserveTerminal` on the client socket
//!   with `RenderEncoding::SemanticFrame`, so the server hands us a cell grid
//!   (`FrameData`) rather than terminal escape bytes. Companion clients render
//!   the grid directly and need no terminal emulator.
//! - **Input** goes back out over the *JSON API* as `pane.send_input`, not over
//!   the observe connection. This is deliberate: the server drops input from
//!   observe clients, and the only bincode alternative — `ControlTerminal` —
//!   resizes the real pty to the client's dimensions and installs a resize lock
//!   the TUI honors. A phone attaching must never reflow the user's terminal.
//!
//! Output holds one connection for the life of the channel. Input cannot — the
//! API server takes exactly one request per connection — so keystrokes are
//! coalesced on a short tick and sent as one request per burst instead of one
//! per character.
//!
//! Everything here is bridge-local: no new JSON API method, no schema change,
//! and no `PROTOCOL_VERSION` bump. Only the wire format of the lines this module
//! emits is new, and that contract is with the companion app alone.

use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender, TrySendError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use interprocess::local_socket::traits::Stream as _;

use crate::protocol::{
    CellData, ClientKeybindings, ClientLaunchMode, ClientMessage, FrameData, FramingError,
    RenderEncoding, ServerMessage, MAX_FRAME_SIZE, PROTOCOL_VERSION,
};

/// Method name this module owns. Anything else proxies to the JSON API.
pub(super) const METHOD: &str = "pane.stream";

/// Resend the whole grid when this share of cells changed; below it, sending
/// individual cells is smaller than sending the frame.
const FULL_FRAME_CHANGE_RATIO: f32 = 0.6;
/// Periodic unconditional full frame, so a client that dropped a line heals.
const DEFAULT_FULL_EVERY: Duration = Duration::from_secs(30);
/// Emitted after this much frame silence so a client can tell "idle agent" from
/// "wedged stream".
const PING_AFTER: Duration = Duration::from_secs(15);
/// How long the observe read blocks before we re-check the cancel flag.
const OBSERVE_POLL: Duration = Duration::from_millis(250);
/// Input coalescing window. Well under human perception, and it collapses a
/// burst of keystrokes into one API request.
const INPUT_TICK: Duration = Duration::from_millis(16);
/// Cap on one coalesced input batch.
const INPUT_BATCH_MAX_BYTES: usize = 1024;
/// Pending input messages before we start dropping. Bounded so a stalled API
/// socket can never block the single WebSocket IO thread.
const INPUT_QUEUE_DEPTH: usize = 256;

/// One keystroke-ish unit from the client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum InputMsg {
    Text(String),
    Keys(Vec<String>),
}

/// A batch of input to send as a single `pane.send_input` request.
///
/// `pane.send_input` applies `text` before `keys`, so a batch may only ever hold
/// text that precedes its keys. Anything typed after a key starts a new batch,
/// which is what preserves ordering.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub(super) struct InputBatch {
    pub text: String,
    pub keys: Vec<String>,
}

impl InputBatch {
    fn is_empty(&self) -> bool {
        self.text.is_empty() && self.keys.is_empty()
    }
}

/// Coalesce queued input into as few ordered batches as possible.
pub(super) fn batch_input(msgs: &[InputMsg]) -> Vec<InputBatch> {
    let mut batches: Vec<InputBatch> = Vec::new();
    let mut current = InputBatch::default();
    for msg in msgs {
        match msg {
            InputMsg::Text(text) => {
                // Text after keys cannot join this batch without reordering.
                let starts_new_batch = !current.keys.is_empty()
                    || current.text.len() + text.len() > INPUT_BATCH_MAX_BYTES;
                if starts_new_batch && !current.is_empty() {
                    batches.push(std::mem::take(&mut current));
                }
                current.text.push_str(text);
            }
            InputMsg::Keys(keys) => current.keys.extend(keys.iter().cloned()),
        }
    }
    if !current.is_empty() {
        batches.push(current);
    }
    batches
}

/// Turns successive full grids into either a full or an incremental JSON line.
///
/// `SemanticFrame` gives us the entire grid whenever anything changes, so the
/// diffing is ours to do; without it a single blinking cursor would push a whole
/// screen over the wire several times a second.
pub(super) struct FrameDiff {
    prev: Option<FrameData>,
    last_full: Option<Instant>,
    full_every: Duration,
}

impl FrameDiff {
    pub(super) fn new(full_every: Duration) -> Self {
        Self {
            prev: None,
            last_full: None,
            full_every,
        }
    }

    /// `None` means nothing changed and nothing needs to go out.
    pub(super) fn next(&mut self, frame: FrameData, now: Instant) -> Option<Value> {
        let dimensions_changed = self
            .prev
            .as_ref()
            .is_some_and(|prev| prev.width != frame.width || prev.height != frame.height);
        let due_full = self
            .last_full
            .is_none_or(|last| now.duration_since(last) >= self.full_every);

        let changed: Vec<usize> = match self.prev.as_ref() {
            Some(prev) if !dimensions_changed => {
                if prev.cells == frame.cells && prev.cursor == frame.cursor {
                    // Identical frame: the server already dedupes these, but a
                    // cursor-only comparison is cheap insurance.
                    return None;
                }
                frame
                    .cells
                    .iter()
                    .enumerate()
                    .zip(prev.cells.iter())
                    .filter_map(|((idx, cell), old)| (cell != old).then_some(idx))
                    .collect()
            }
            _ => Vec::new(),
        };

        let total = frame.cells.len().max(1);
        let full = self.prev.is_none()
            || dimensions_changed
            || due_full
            || (changed.len() as f32 / total as f32) > FULL_FRAME_CHANGE_RATIO;

        let cells: Vec<Value> = if full {
            frame
                .cells
                .iter()
                .enumerate()
                .map(|(idx, cell)| cell_json(idx, cell))
                .collect()
        } else {
            changed
                .iter()
                .map(|&idx| cell_json(idx, &frame.cells[idx]))
                .collect()
        };

        let mut line = json!({
            "type": "frame",
            "full": full,
            "w": frame.width,
            "h": frame.height,
            "cells": cells,
        });
        if let Some(cursor) = frame.cursor.as_ref() {
            line["cursor"] = json!({
                "x": cursor.x,
                "y": cursor.y,
                "visible": cursor.visible,
                "shape": cursor.shape,
            });
        }
        // Hyperlink URIs are indexed by the cells; resend the table whenever it
        // changes or the client would resolve stale indices.
        let links_changed = self
            .prev
            .as_ref()
            .is_none_or(|prev| prev.hyperlinks != frame.hyperlinks);
        if full || links_changed {
            line["links"] = json!(frame.hyperlinks);
        }

        if full {
            self.last_full = Some(now);
        }
        self.prev = Some(frame);
        Some(line)
    }
}

/// `[index, symbol, fg, bg, modifier, hyperlink]`.
///
/// `fg`/`bg` stay in the server's packed form (tag byte: 0x00 named, 0x01
/// indexed, 0x02 rgb) rather than being resolved to RGB here. Named and indexed
/// colors have to be resolved against the *client's* palette — that is what lets
/// the companion render a pane in shep's own colors instead of whatever the
/// server happened to pick.
///
/// `CellData::skip` is dropped: it is a ratatui-internal diffing hint and means
/// nothing to a renderer starting from scratch.
fn cell_json(idx: usize, cell: &CellData) -> Value {
    json!([
        idx,
        cell.symbol,
        cell.fg,
        cell.bg,
        cell.modifier,
        cell.hyperlink
    ])
}

/// Live channel state the bridge holds while a `pane.stream` is open.
pub(super) struct StreamHandle {
    cancel: Arc<AtomicBool>,
    input_tx: SyncSender<InputMsg>,
}

impl StreamHandle {
    pub(super) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Queue one `{"ch":N,"data":{...}}` payload. Never blocks: the caller is
    /// the single WebSocket IO thread.
    pub(super) fn send_input(&self, data: &Value) -> Result<(), String> {
        let mut queued = Vec::new();
        if let Some(text) = data.get("text").and_then(|v| v.as_str()) {
            if !text.is_empty() {
                queued.push(InputMsg::Text(text.to_string()));
            }
        }
        if let Some(keys) = data.get("keys").and_then(|v| v.as_array()) {
            let keys: Vec<String> = keys
                .iter()
                .filter_map(|k| k.as_str().map(str::to_string))
                .collect();
            if !keys.is_empty() {
                queued.push(InputMsg::Keys(keys));
            }
        }
        if queued.is_empty() {
            return Err("data frame carried no text or keys".into());
        }
        for msg in queued {
            match self.input_tx.try_send(msg) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => return Err("input queue full".into()),
                Err(TrySendError::Disconnected(_)) => return Err("stream closed".into()),
            }
        }
        Ok(())
    }
}

/// Claim `pane.stream`, or hand the request back for normal API proxying.
pub(super) fn try_open(
    method: &str,
    params: Option<&Value>,
    ch: u64,
    out_tx: &mpsc::Sender<String>,
    api_socket: &Arc<PathBuf>,
) -> Option<Result<StreamHandle, String>> {
    if method != METHOD {
        return None;
    }
    Some(open(params, ch, out_tx, api_socket))
}

fn open(
    params: Option<&Value>,
    ch: u64,
    out_tx: &mpsc::Sender<String>,
    api_socket: &Arc<PathBuf>,
) -> Result<StreamHandle, String> {
    let params = params.ok_or("pane.stream requires params")?;
    let pane_id = params
        .get("pane_id")
        .and_then(|v| v.as_str())
        .ok_or("pane.stream requires pane_id")?
        .to_string();
    let full_every = params
        .get("full_every_ms")
        .and_then(|v| v.as_u64())
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_FULL_EVERY);
    let requested_size = match (params.get("cols"), params.get("rows")) {
        (Some(cols), Some(rows)) => cols
            .as_u64()
            .zip(rows.as_u64())
            .map(|(c, r)| (c as u16, r as u16)),
        _ => None,
    };

    let cancel = Arc::new(AtomicBool::new(false));
    let (input_tx, input_rx) = mpsc::sync_channel::<InputMsg>(INPUT_QUEUE_DEPTH);

    // Frame reader.
    {
        let cancel = cancel.clone();
        let out_tx = out_tx.clone();
        let api_socket = api_socket.clone();
        let pane_id = pane_id.clone();
        std::thread::spawn(move || {
            let result = observe_loop(
                &pane_id,
                requested_size,
                full_every,
                ch,
                &out_tx,
                &cancel,
                &api_socket,
            );
            if let Err(err) = result {
                let _ = out_tx.send(json!({"ch": ch, "error": err}).to_string());
            }
            let _ = out_tx.send(json!({"ch": ch, "eof": true}).to_string());
            cancel.store(true, Ordering::Relaxed);
        });
    }

    // Input writer.
    {
        let cancel = cancel.clone();
        let api_socket = api_socket.clone();
        std::thread::spawn(move || {
            input_loop(&pane_id, input_rx, &cancel, &api_socket);
        });
    }

    Ok(StreamHandle { cancel, input_tx })
}

/// Ask the JSON API how big the pane actually is, so the observer mirrors the
/// desktop instead of dictating a size to it.
fn resolve_pane_size(api_socket: &Path, pane_id: &str) -> Option<(u16, u16)> {
    let request = json!({
        "id": "bridge-stream-size",
        "method": "pane.get",
        "params": {"pane_id": pane_id},
    });
    let response = api_request(api_socket, &request).ok()?;
    let scroll = response.get("result")?.get("pane")?.get("scroll")?.clone();
    let cols = scroll.get("viewport_cols")?.as_u64()?;
    let rows = scroll.get("viewport_rows")?.as_u64()?;
    if cols == 0 || rows == 0 {
        return None;
    }
    Some((cols as u16, rows as u16))
}

/// One request, first response line, connection closed.
fn api_request(api_socket: &Path, request: &Value) -> std::io::Result<Value> {
    use std::io::{BufRead, BufReader};

    let mut stream = crate::ipc::connect_local_stream(api_socket)?;
    stream.write_all(request.to_string().as_bytes())?;
    stream.write_all(b"\n")?;
    stream.flush()?;
    let reader = BufReader::new(stream);
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(value) = serde_json::from_str::<Value>(&line) {
            return Ok(value);
        }
    }
    Err(std::io::Error::other("no response"))
}

#[allow(clippy::too_many_arguments)]
fn observe_loop(
    pane_id: &str,
    requested_size: Option<(u16, u16)>,
    full_every: Duration,
    ch: u64,
    out_tx: &mpsc::Sender<String>,
    cancel: &AtomicBool,
    api_socket: &Arc<PathBuf>,
) -> Result<(), String> {
    let (cols, rows) = requested_size
        .or_else(|| resolve_pane_size(api_socket, pane_id))
        .unwrap_or((80, 24));

    let client_socket =
        crate::server::socket_paths::derive_client_socket_from_api_socket(api_socket.as_path());
    let mut stream = crate::ipc::connect_local_stream(&client_socket)
        .map_err(|err| format!("client socket {}: {err}", client_socket.display()))?;
    stream
        .set_nonblocking(false)
        .map_err(|err| err.to_string())?;

    // `TerminalAttach` here means "not a full app client" — it does not attach
    // to or resize anything. Only ObserveTerminal follows, never
    // ControlTerminal/AttachTerminal, which would resize the real pty.
    let hello = ClientMessage::Hello {
        version: PROTOCOL_VERSION,
        cols,
        rows,
        cell_width_px: 0,
        cell_height_px: 0,
        requested_encoding: RenderEncoding::SemanticFrame,
        keybindings: ClientKeybindings::Server,
        launch_mode: ClientLaunchMode::TerminalAttach,
    };
    crate::protocol::write_message(&mut stream, &hello).map_err(|err| err.to_string())?;

    let welcome: ServerMessage =
        crate::protocol::read_message(&mut stream, MAX_FRAME_SIZE).map_err(|e| e.to_string())?;
    match welcome {
        ServerMessage::Welcome {
            error: Some(error), ..
        } => return Err(error),
        ServerMessage::Welcome {
            encoding: RenderEncoding::SemanticFrame,
            ..
        } => {}
        ServerMessage::Welcome { encoding, .. } => {
            return Err(format!(
                "server negotiated unsupported encoding {encoding:?}"
            ))
        }
        other => return Err(format!("unexpected handshake reply {other:?}")),
    }

    crate::protocol::write_message(
        &mut stream,
        &ClientMessage::ObserveTerminal {
            target: pane_id.to_string(),
        },
    )
    .map_err(|err| err.to_string())?;

    let _ =
        out_tx.send(json!({"ch": ch, "line": {"type": "size", "w": cols, "h": rows}}).to_string());

    stream
        .set_recv_timeout(Some(OBSERVE_POLL))
        .map_err(|err| err.to_string())?;

    let mut diff = FrameDiff::new(full_every);
    let mut last_emit = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Ok(());
        }
        match crate::protocol::read_message::<_, ServerMessage>(&mut stream, MAX_FRAME_SIZE) {
            Ok(ServerMessage::Frame(frame)) => {
                if let Some(line) = diff.next(frame, Instant::now()) {
                    if out_tx
                        .send(json!({"ch": ch, "line": line}).to_string())
                        .is_err()
                    {
                        return Ok(());
                    }
                    last_emit = Instant::now();
                }
            }
            Ok(ServerMessage::ServerShutdown { reason }) => {
                let _ = out_tx
                    .send(json!({"ch": ch, "line": {"type": "end", "reason": reason}}).to_string());
                return Ok(());
            }
            Ok(_) => {}
            Err(err) if is_timeout(&err) => {
                if last_emit.elapsed() >= PING_AFTER {
                    let _ = out_tx.send(json!({"ch": ch, "line": {"type": "ping"}}).to_string());
                    last_emit = Instant::now();
                }
            }
            Err(err) => return Err(err.to_string()),
        }
    }
}

fn is_timeout(err: &FramingError) -> bool {
    matches!(
        err,
        FramingError::Io(io)
            if io.kind() == std::io::ErrorKind::WouldBlock
                || io.kind() == std::io::ErrorKind::TimedOut
    )
}

/// Drain queued input, coalesce it, and write it to the JSON API over one
/// long-lived connection.
fn input_loop(
    pane_id: &str,
    input_rx: mpsc::Receiver<InputMsg>,
    cancel: &AtomicBool,
    api_socket: &Arc<PathBuf>,
) {
    let mut pending: Vec<InputMsg> = Vec::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        match input_rx.recv_timeout(INPUT_TICK) {
            Ok(msg) => {
                pending.push(msg);
                // Absorb whatever else already arrived in this window.
                while let Ok(msg) = input_rx.try_recv() {
                    pending.push(msg);
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => return,
        }
        if pending.is_empty() {
            continue;
        }
        for batch in batch_input(&pending) {
            let mut params = json!({"pane_id": pane_id});
            if !batch.text.is_empty() {
                params["text"] = json!(batch.text);
            }
            if !batch.keys.is_empty() {
                params["keys"] = json!(batch.keys);
            }
            let request = json!({
                "id": "bridge-stream-input",
                "method": "pane.send_input",
                "params": params,
            });
            if let Err(err) = send_input_batch(api_socket.as_path(), &request) {
                tracing::debug!(err = %err, "pane.stream input send failed");
            }
        }
        pending.clear();
    }
}

/// Send one input batch to the JSON API.
///
/// A fresh connection per batch is not an oversight: the API server reads
/// exactly one request line per connection (`read_initial_request_line` in
/// src/api/server.rs) and then moves on, so anything written to a reused socket
/// is silently discarded. The 16 ms coalescing above is what keeps this cheap —
/// a burst of typing becomes one connection, not one per character.
///
/// The response is read and dropped: leaving it unread would back the server up
/// on its write.
fn send_input_batch(api_socket: &Path, request: &Value) -> std::io::Result<()> {
    let response = api_request(api_socket, request)?;
    if let Some(error) = response.get("error") {
        tracing::debug!(error = %error, "pane.stream input rejected");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::CursorState;

    fn cell(symbol: &str) -> CellData {
        CellData {
            symbol: symbol.to_string(),
            fg: 0,
            bg: 0,
            modifier: 0,
            skip: false,
            hyperlink: None,
        }
    }

    fn frame(width: u16, height: u16, symbols: &[&str]) -> FrameData {
        FrameData {
            cells: symbols.iter().map(|s| cell(s)).collect(),
            width,
            height,
            cursor: None,
            hyperlinks: Vec::new(),
            graphics: Vec::new(),
        }
    }

    #[test]
    fn try_open_only_owns_pane_stream() {
        let (out_tx, _rx) = mpsc::channel();
        let socket = Arc::new(PathBuf::from("/tmp/does-not-matter.sock"));
        assert!(try_open("pane.read", None, 1, &out_tx, &socket).is_none());
        assert!(try_open("session.snapshot", None, 1, &out_tx, &socket).is_none());
        // Claimed, but rejected for missing params — it never reaches a socket.
        let claimed = try_open(METHOD, None, 1, &out_tx, &socket);
        assert!(matches!(claimed, Some(Err(_))));
    }

    #[test]
    fn first_frame_is_full() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let line = diff
            .next(frame(2, 1, &["a", "b"]), Instant::now())
            .expect("first frame emits");
        assert_eq!(line["full"], json!(true));
        assert_eq!(line["cells"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn unchanged_frame_emits_nothing() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        diff.next(frame(2, 1, &["a", "b"]), now).unwrap();
        assert!(diff.next(frame(2, 1, &["a", "b"]), now).is_none());
    }

    #[test]
    fn single_cell_change_emits_one_cell() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        diff.next(frame(4, 1, &["a", "b", "c", "d"]), now).unwrap();
        let line = diff
            .next(frame(4, 1, &["a", "b", "X", "d"]), now)
            .expect("changed frame emits");
        assert_eq!(line["full"], json!(false));
        let cells = line["cells"].as_array().unwrap();
        assert_eq!(cells.len(), 1);
        assert_eq!(cells[0][0], json!(2));
        assert_eq!(cells[0][1], json!("X"));
    }

    #[test]
    fn dimension_change_forces_full() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        diff.next(frame(2, 1, &["a", "b"]), now).unwrap();
        let line = diff.next(frame(3, 1, &["a", "b", "c"]), now).unwrap();
        assert_eq!(line["full"], json!(true));
        assert_eq!(line["w"], json!(3));
    }

    #[test]
    fn majority_change_forces_full() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        diff.next(frame(4, 1, &["a", "b", "c", "d"]), now).unwrap();
        let line = diff.next(frame(4, 1, &["W", "X", "Y", "Z"]), now).unwrap();
        assert_eq!(line["full"], json!(true));
        assert_eq!(line["cells"].as_array().unwrap().len(), 4);
    }

    #[test]
    fn periodic_full_frame_heals_a_dropped_line() {
        let mut diff = FrameDiff::new(Duration::from_millis(0));
        let now = Instant::now();
        diff.next(frame(4, 1, &["a", "b", "c", "d"]), now).unwrap();
        // full_every of zero means every frame re-syncs.
        let line = diff.next(frame(4, 1, &["a", "b", "X", "d"]), now).unwrap();
        assert_eq!(line["full"], json!(true));
    }

    #[test]
    fn diff_preserves_packed_color_and_modifier_bits() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let mut styled = frame(1, 1, &["x"]);
        // 0x02 tag = rgb; high modifier bits carry the underline style.
        styled.cells[0].fg = 0x02E09A55;
        styled.cells[0].bg = 0x01000004;
        styled.cells[0].modifier = 0x1001;
        let line = diff.next(styled, Instant::now()).unwrap();
        let cell = &line["cells"][0];
        assert_eq!(cell[2], json!(0x02E09A55u32));
        assert_eq!(cell[3], json!(0x01000004u32));
        assert_eq!(cell[4], json!(0x1001u16));
    }

    #[test]
    fn cursor_omitted_when_none_and_present_when_set() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        let line = diff.next(frame(1, 1, &["a"]), now).unwrap();
        assert!(line.get("cursor").is_none());

        let mut with_cursor = frame(1, 1, &["a"]);
        with_cursor.cursor = Some(CursorState {
            x: 3,
            y: 4,
            visible: true,
            shape: 2,
        });
        let line = diff.next(with_cursor, now).unwrap();
        assert_eq!(line["cursor"]["x"], json!(3));
        assert_eq!(line["cursor"]["shape"], json!(2));
    }

    #[test]
    fn cursor_move_alone_still_emits_a_frame() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let now = Instant::now();
        let mut first = frame(2, 1, &["a", "b"]);
        first.cursor = Some(CursorState {
            x: 0,
            y: 0,
            visible: true,
            shape: 0,
        });
        diff.next(first, now).unwrap();

        let mut moved = frame(2, 1, &["a", "b"]);
        moved.cursor = Some(CursorState {
            x: 1,
            y: 0,
            visible: true,
            shape: 0,
        });
        let line = diff.next(moved, now).expect("cursor move emits");
        assert_eq!(line["cursor"]["x"], json!(1));
        assert!(line["cells"].as_array().unwrap().is_empty());
    }

    #[test]
    fn hyperlink_index_survives_diff() {
        let mut diff = FrameDiff::new(DEFAULT_FULL_EVERY);
        let mut linked = frame(1, 1, &["x"]);
        linked.cells[0].hyperlink = Some(0);
        linked.hyperlinks = vec!["https://example.invalid".into()];
        let line = diff.next(linked, Instant::now()).unwrap();
        assert_eq!(line["cells"][0][5], json!(0));
        assert_eq!(line["links"][0], json!("https://example.invalid"));
    }

    #[test]
    fn input_batcher_merges_consecutive_text() {
        let batches = batch_input(&[
            InputMsg::Text("h".into()),
            InputMsg::Text("i".into()),
            InputMsg::Text("!".into()),
        ]);
        assert_eq!(batches.len(), 1);
        assert_eq!(batches[0].text, "hi!");
        assert!(batches[0].keys.is_empty());
    }

    #[test]
    fn input_batcher_splits_at_keys_boundary() {
        // pane.send_input applies text before keys, so "ls" + enter + "x" must
        // not collapse into one batch or the x would arrive before the enter.
        let batches = batch_input(&[
            InputMsg::Text("ls".into()),
            InputMsg::Keys(vec!["enter".into()]),
            InputMsg::Text("x".into()),
        ]);
        assert_eq!(batches.len(), 2);
        assert_eq!(batches[0].text, "ls");
        assert_eq!(batches[0].keys, vec!["enter".to_string()]);
        assert_eq!(batches[1].text, "x");
        assert!(batches[1].keys.is_empty());
    }

    #[test]
    fn input_batcher_caps_batch_bytes() {
        let msgs: Vec<InputMsg> = (0..(INPUT_BATCH_MAX_BYTES + 10))
            .map(|_| InputMsg::Text("a".into()))
            .collect();
        let batches = batch_input(&msgs);
        assert!(batches.len() >= 2);
        assert!(batches
            .iter()
            .all(|batch| batch.text.len() <= INPUT_BATCH_MAX_BYTES));
    }

    #[test]
    fn input_batcher_ignores_empty_input() {
        assert!(batch_input(&[]).is_empty());
    }

    #[test]
    fn send_input_rejects_a_payload_with_nothing_in_it() {
        let (tx, _rx) = mpsc::sync_channel(4);
        let handle = StreamHandle {
            cancel: Arc::new(AtomicBool::new(false)),
            input_tx: tx,
        };
        assert!(handle.send_input(&json!({})).is_err());
        assert!(handle.send_input(&json!({"text": ""})).is_err());
        assert!(handle.send_input(&json!({"text": "a"})).is_ok());
        assert!(handle.send_input(&json!({"keys": ["enter"]})).is_ok());
    }

    #[test]
    fn send_input_reports_overflow_rather_than_blocking() {
        let (tx, _rx) = mpsc::sync_channel(1);
        let handle = StreamHandle {
            cancel: Arc::new(AtomicBool::new(false)),
            input_tx: tx,
        };
        assert!(handle.send_input(&json!({"text": "a"})).is_ok());
        let err = handle
            .send_input(&json!({"text": "b"}))
            .expect_err("queue is full");
        assert!(err.contains("full"), "unexpected error: {err}");
    }

    /// This module's real code, excluding the test block below (which names the
    /// very things these guards forbid).
    fn production_source() -> &'static str {
        include_str!("stream.rs")
            .split("#[cfg(test)]")
            .next()
            .expect("source has a non-test half")
    }

    /// Input must not reuse an API connection.
    ///
    /// The API server reads exactly one request per connection and then stops
    /// listening on it, so a cached socket swallows every request after the
    /// first — silently, since the write still succeeds into the buffer. That
    /// bug drops characters mid-word while looking like a flaky network.
    #[test]
    fn input_does_not_hold_a_reusable_api_connection() {
        let production = production_source();
        assert!(
            !production.contains("struct ApiConn"),
            "input must open a connection per batch; the API server takes one request per connection"
        );
        assert!(
            production.contains("fn send_input_batch"),
            "input should go through the per-batch send path"
        );
    }

    /// The one invariant whose violation silently resizes the user's real
    /// terminal: an observer must never escalate to a controlling attach.
    #[test]
    fn stream_never_constructs_control_or_attach() {
        let code = production_source()
            .lines()
            .filter(|line| !line.trim_start().starts_with("//"))
            .collect::<String>();
        assert!(
            !code.contains("ControlTerminal"),
            "pane.stream must never send ControlTerminal: it resizes the real pty"
        );
        assert!(
            !code.contains("AttachTerminal"),
            "pane.stream must never send AttachTerminal: it resizes the real pty"
        );
    }
}
