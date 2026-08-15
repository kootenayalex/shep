//! `pane.transcript` — a pane's conversation, as a conversation.
//!
//! The companion's live view mirrors the pty: it shows exactly what the desktop
//! shows, which is the right answer when you are driving an agent and the wrong
//! one when you are catching up on what it did. This reads the agent's *own*
//! session log instead, so the phone can render turns the way a chat client
//! does rather than re-parsing a TUI that is redrawn on every keystroke.
//!
//! Bridge-local for the same reason `task.*` and `memory.*` are: it is a read
//! of files on this machine, so it needs no new server API method and no
//! protocol bump. It does ask the server one question — which session a pane is
//! running — because only the server knows that.
//!
//! Claude Code only, deliberately. Every agent logs its sessions differently and
//! guessing at a format we have not seen would produce plausible nonsense; an
//! explicit `unsupported agent` is a better answer.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

/// Turns returned when the caller does not say.
const DEFAULT_LIMIT: usize = 120;
const MAX_LIMIT: usize = 1000;
/// Per-message cap. Long tool output is the reason a transcript gets big, and
/// nobody reads 200 KB of it on a phone.
const MAX_TEXT: usize = 12_000;
const MAX_TOOL_INPUT: usize = 400;
const MAX_TOOL_RESULT: usize = 600;
/// How many sessions in a project directory to consider when the pane has not
/// told us which one it is.
const MAX_CANDIDATES: usize = 24;
/// Length of the fingerprint windows used to match a session to a pane.
const FINGERPRINT_WINDOW: usize = 32;
/// How much of a candidate session to read when fingerprinting it.
const MATCH_TAIL_BYTES: u64 = 512 * 1024;

pub(super) fn handle_local_method(
    method: &str,
    params: Option<&Value>,
    api_socket: &Path,
) -> Option<Result<Value, String>> {
    match method {
        "pane.transcript" => Some(transcript(params, api_socket)),
        _ => None,
    }
}

fn transcript(params: Option<&Value>, api_socket: &Path) -> Result<Value, String> {
    let params = params.ok_or("missing params")?;
    let target = params
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("missing target (pane id)")?;
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);

    let pane = find_pane(api_socket, target)?;
    let agent = pane
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if agent != "claude" {
        return Err(format!(
            "no transcript reader for agent {}",
            if agent.is_empty() { "(none)" } else { &agent }
        ));
    }
    let cwd = pane
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or("pane has no cwd")?;

    let (path, source) = resolve_session_file(&pane, cwd, api_socket, target)?;
    let raw = std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    let mut all = parse_turns(&raw);
    let truncated = all.len() > limit;
    if truncated {
        // Keep the tail: the end of a conversation is what you open it for.
        all.drain(..all.len() - limit);
    }
    let turns = all;

    Ok(json!({
        "transcript": {
            "agent": agent,
            "session_id": session_id_from_path(&path),
            "path": path.display().to_string(),
            "source": source,
            "truncated": truncated,
            "turns": turns,
        }
    }))
}

// ---------------------------------------------------------------------------
// Finding the session file
// ---------------------------------------------------------------------------

fn projects_dir() -> PathBuf {
    dirs_home().join(".claude").join("projects")
}

fn dirs_home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Claude's project-directory name for a working directory.
///
/// Every character that is not alphanumeric becomes `-`, so `/Users/a/.od/x`
/// and `/Users/a/-od/x` collide — that is Claude's rule, not ours, and matching
/// it exactly is the whole point.
pub(super) fn project_slug(cwd: &str) -> String {
    cwd.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect()
}

fn session_id_from_path(path: &Path) -> Option<String> {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .map(str::to_string)
}

/// Resolve which `.jsonl` a pane is writing, and say how confident we are.
///
/// `reported` — the agent told shep its session id through the integration hook
/// (`shep integration install claude`), so this is exact. `matched` — we
/// fingerprinted the pane's visible output against the candidates. `only` — one
/// session has ever run in this directory. The distinction matters to the
/// caller: a `matched` transcript is a good guess, not a fact, and the app says
/// so.
fn resolve_session_file(
    pane: &Map<String, Value>,
    cwd: &str,
    api_socket: &Path,
    target: &str,
) -> Result<(PathBuf, &'static str), String> {
    if let Some(session) = pane.get("agent_session").and_then(Value::as_object) {
        let kind = session.get("kind").and_then(Value::as_str).unwrap_or("");
        let value = session.get("value").and_then(Value::as_str).unwrap_or("");
        if kind == "path" && !value.is_empty() {
            let path = PathBuf::from(value);
            if path.is_file() {
                return Ok((path, "reported"));
            }
        }
        if kind == "id" && !value.is_empty() {
            let direct = projects_dir()
                .join(project_slug(cwd))
                .join(format!("{value}.jsonl"));
            if direct.is_file() {
                return Ok((direct, "reported"));
            }
            // A pane's cwd can move (an agent that cd'd, a worktree); the id is
            // still authoritative, so look for it anywhere.
            if let Some(found) = find_session_anywhere(value) {
                return Ok((found, "reported"));
            }
        }
    }

    let dir = projects_dir().join(project_slug(cwd));
    let mut candidates = sessions_in(&dir)?;
    if candidates.is_empty() {
        return Err(format!("no claude sessions recorded for {cwd}"));
    }
    if candidates.len() == 1 {
        return Ok((candidates.remove(0), "only"));
    }
    // Several agents share this directory — which is Alex's normal case, six
    // claude panes all in $HOME — so the mtime alone tells us nothing.
    let screen = pane_text(api_socket, target).unwrap_or_default();
    if squeeze(&screen).len() < FINGERPRINT_WINDOW {
        // A freshly-cleared pane. Worth saying plainly: there is no bug to hunt
        // here, and after `/clear` claude has started a new session anyway.
        return Err(
            "this pane's screen is empty, so there is nothing to match a session against — \
             it will have a transcript again once the agent has said something"
                .to_string(),
        );
    }
    match best_match(&candidates, &screen) {
        Some(path) => Ok((path, "matched")),
        None => Err(format!(
            "none of the {} claude sessions in {} matches this pane's output — \
             run `shep integration install claude` so the agent reports its \
             session id and this stops being a guess",
            candidates.len(),
            dir.display()
        )),
    }
}

fn find_session_anywhere(session_id: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(projects_dir()).ok()?;
    for entry in entries.flatten() {
        let candidate = entry.path().join(format!("{session_id}.jsonl"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Session files in a project directory, newest first.
fn sessions_in(dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for entry in std::fs::read_dir(dir)
        .map_err(|err| err.to_string())?
        .flatten()
    {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }
        let modified = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .unwrap_or(std::time::UNIX_EPOCH);
        files.push((modified, path));
    }
    files.sort_by_key(|(modified, _)| std::cmp::Reverse(*modified));
    Ok(files
        .into_iter()
        .take(MAX_CANDIDATES)
        .map(|(_, path)| path)
        .collect())
}

/// Pick the session whose recent messages appear on the pane's screen.
///
/// Whitespace is stripped from both sides before comparing: the terminal wraps
/// a long message mid-word across rows, so any match that respects line breaks
/// fails on exactly the messages long enough to be distinctive.
fn best_match(candidates: &[PathBuf], screen: &str) -> Option<PathBuf> {
    let haystack = squeeze(screen);
    if haystack.len() < FINGERPRINT_WINDOW {
        return None;
    }
    let mut best: Option<(usize, &PathBuf)> = None;
    for path in candidates {
        let text = read_tail(path, MATCH_TAIL_BYTES);
        let score = fingerprints(&text)
            .iter()
            .filter(|needle| haystack.contains(needle.as_str()))
            .count();
        if score == 0 {
            continue;
        }
        // Candidates arrive newest-first, so `>` keeps the newer of a tie.
        let better = match best {
            Some((top, _)) => score > top,
            None => true,
        };
        if better {
            best = Some((score, path));
        }
    }
    best.map(|(_, path)| path.clone())
}

/// Distinctive snippets from a session's last few messages.
fn fingerprints(raw: &str) -> Vec<String> {
    let mut out = Vec::new();
    for turn in parse_turns(raw).iter().rev().take(6) {
        let text = turn.get("text").and_then(Value::as_str).unwrap_or_default();
        let squeezed = squeeze(text);
        if squeezed.len() < FINGERPRINT_WINDOW {
            continue;
        }
        // The tail of a message survives redraws better than its head, which
        // scrolls off first.
        let chars: Vec<char> = squeezed.chars().collect();
        let start = chars.len().saturating_sub(FINGERPRINT_WINDOW * 3);
        for window in chars[start..].chunks(FINGERPRINT_WINDOW) {
            if window.len() == FINGERPRINT_WINDOW {
                out.push(window.iter().collect());
            }
        }
    }
    out
}

fn squeeze(text: &str) -> String {
    text.chars().filter(|c| !c.is_whitespace()).collect()
}

/// The last `max` bytes of a file, from the first whole line onwards.
///
/// Matching only ever looks at a session's most recent turns, and a busy
/// session log runs to megabytes — reading a dozen of those in full to compare
/// their last paragraph would make opening a transcript feel broken.
fn read_tail(path: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let len = file.metadata().map(|meta| meta.len()).unwrap_or(0);
    let mut buffer = Vec::new();
    if len > max && file.seek(SeekFrom::End(-(max as i64))).is_err() {
        return String::new();
    }
    if file.read_to_end(&mut buffer).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&buffer).into_owned();
    if len > max {
        // The seek landed mid-line; that line is not parseable JSON.
        match text.find('\n') {
            Some(index) => text[index + 1..].to_string(),
            None => String::new(),
        }
    } else {
        text
    }
}

// ---------------------------------------------------------------------------
// Talking to the server
// ---------------------------------------------------------------------------

fn api_call(api_socket: &Path, method: &str, params: Value) -> Result<Value, String> {
    let request = json!({"id": "bridge:transcript", "method": method, "params": params});
    let mut stream = crate::ipc::connect_local_stream(api_socket).map_err(|err| err.to_string())?;
    stream
        .write_all(format!("{request}\n").as_bytes())
        .map_err(|err| err.to_string())?;
    stream.flush().map_err(|err| err.to_string())?;
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).map_err(|err| err.to_string())?;
    let value: Value = serde_json::from_str(line.trim()).map_err(|err| err.to_string())?;
    if let Some(error) = value.get("error") {
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("api error");
        return Err(message.to_string());
    }
    value
        .get("result")
        .cloned()
        .ok_or_else(|| "api response had no result".to_string())
}

/// The pane's row in `session.snapshot`.
///
/// Walked recursively rather than indexed by a known path: the snapshot's shape
/// has moved more than once, and every version has agreed that a pane is the
/// object carrying `pane_id`.
fn find_pane(api_socket: &Path, target: &str) -> Result<Map<String, Value>, String> {
    let snapshot = api_call(api_socket, "session.snapshot", json!({}))?;
    let mut found = None;
    walk(&snapshot, &mut |object| {
        if found.is_some() {
            return;
        }
        if object.get("pane_id").and_then(Value::as_str) == Some(target)
            && object.contains_key("cwd")
        {
            found = Some(object.clone());
        }
    });
    found.ok_or_else(|| format!("no pane {target}"))
}

fn walk(value: &Value, visit: &mut impl FnMut(&Map<String, Value>)) {
    match value {
        Value::Object(object) => {
            visit(object);
            for nested in object.values() {
                walk(nested, visit);
            }
        }
        Value::Array(items) => {
            for item in items {
                walk(item, visit);
            }
        }
        _ => {}
    }
}

fn pane_text(api_socket: &Path, target: &str) -> Result<String, String> {
    let result = api_call(
        api_socket,
        "agent.read",
        json!({"target": target, "source": "recent", "lines": 400}),
    )?;
    let text = result
        .get("read")
        .and_then(|read| read.get("text"))
        .or_else(|| result.get("text"))
        .and_then(Value::as_str)
        .unwrap_or_default();
    Ok(text.to_string())
}

// ---------------------------------------------------------------------------
// Parsing
// ---------------------------------------------------------------------------

/// Normalise a Claude session log into chat turns.
///
/// Filtered out on purpose: `isMeta` user entries (hook feedback and system
/// reminders the user never typed), sidechain entries (a subagent's own
/// conversation, which belongs to a different thread), and user entries that
/// carry nothing but tool results — those are attached to the tool call that
/// produced them instead of standing as turns.
pub(super) fn parse_turns(raw: &str) -> Vec<Value> {
    let entries: Vec<Value> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect();

    let results = collect_tool_results(&entries);
    let mut turns: Vec<Value> = Vec::new();

    for entry in &entries {
        if entry.get("isSidechain").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let kind = entry.get("type").and_then(Value::as_str).unwrap_or("");
        let timestamp = entry
            .get("timestamp")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let content = entry.get("message").and_then(|m| m.get("content"));
        match kind {
            "user" => {
                if entry.get("isMeta").and_then(Value::as_bool) == Some(true) {
                    continue;
                }
                let text = user_text(content);
                if text.trim().is_empty() {
                    continue;
                }
                let (role, text) = classify_user(&text);
                turns.push(json!({
                    "role": role,
                    "ts": timestamp,
                    "text": clip(&text, MAX_TEXT),
                }));
            }
            "assistant" => {
                let (text, thinking, blocks) = assistant_parts(content, &results);
                if text.trim().is_empty() && thinking.trim().is_empty() && blocks.is_empty() {
                    continue;
                }
                // Claude writes thinking, prose and each tool call as separate
                // entries; a reader wants them as one reply.
                if let Some(last) = turns.last_mut() {
                    if last.get("role").and_then(Value::as_str) == Some("assistant") {
                        merge_assistant(last, &text, &thinking, blocks);
                        continue;
                    }
                }
                turns.push(json!({
                    "role": "assistant",
                    "ts": timestamp,
                    "text": clip(&text, MAX_TEXT),
                    "thinking": clip(&thinking, MAX_TEXT),
                    "blocks": blocks,
                }));
            }
            _ => {}
        }
    }
    turns
}

fn merge_assistant(turn: &mut Value, text: &str, thinking: &str, blocks: Vec<Value>) {
    if let Some(object) = turn.as_object_mut() {
        if !text.trim().is_empty() {
            let existing = object
                .get("text")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let joined = if existing.trim().is_empty() {
                text.to_string()
            } else {
                format!("{existing}\n\n{text}")
            };
            object.insert("text".into(), json!(clip(&joined, MAX_TEXT)));
        }
        if !thinking.trim().is_empty() {
            let existing = object
                .get("thinking")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            let joined = if existing.trim().is_empty() {
                thinking.to_string()
            } else {
                format!("{existing}\n\n{thinking}")
            };
            object.insert("thinking".into(), json!(clip(&joined, MAX_TEXT)));
        }
        if !blocks.is_empty() {
            let mut existing = object
                .get("blocks")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            existing.extend(blocks);
            object.insert("blocks".into(), Value::Array(existing));
        }
    }
}

/// `tool_use_id` → what came back, so a call can be shown with its outcome.
fn collect_tool_results(entries: &[Value]) -> std::collections::HashMap<String, Value> {
    let mut map = std::collections::HashMap::new();
    for entry in entries {
        let Some(blocks) = entry
            .get("message")
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(id) = block.get("tool_use_id").and_then(Value::as_str) else {
                continue;
            };
            let failed = block
                .get("is_error")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            map.insert(
                id.to_string(),
                json!({
                    "ok": !failed,
                    "preview": clip(&block_text(block.get("content")), MAX_TOOL_RESULT),
                }),
            );
        }
    }
    map
}

fn user_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => strip_reminders(text),
        Some(Value::Array(blocks)) => {
            let mut parts = Vec::new();
            for block in blocks {
                if block.get("type").and_then(Value::as_str) == Some("text") {
                    if let Some(text) = block.get("text").and_then(Value::as_str) {
                        parts.push(strip_reminders(text));
                    }
                }
            }
            parts.join("\n")
        }
        _ => String::new(),
    }
}

/// Separate what the human typed from what the harness injected in their name.
///
/// Both arrive as `type: "user"`, and rendering a background-task notification
/// as a chat bubble from Alex would be an outright misattribution. These become
/// `system` turns instead, which the app renders as a note rather than a
/// message, with the XML reduced to the part worth reading.
fn classify_user(text: &str) -> (&'static str, String) {
    let trimmed = text.trim();
    if trimmed.starts_with("<task-notification>") {
        let summary = inner(trimmed, "summary").unwrap_or_else(|| "background task".to_string());
        return ("system", summary);
    }
    if trimmed.starts_with("<command-name>") {
        let name = inner(trimmed, "command-name").unwrap_or_else(|| "command".to_string());
        let args = inner(trimmed, "command-args").unwrap_or_default();
        let line = format!("{name} {args}");
        return ("system", line.trim().to_string());
    }
    if trimmed.starts_with("<local-command-stdout>") {
        return ("system", "command output".to_string());
    }
    ("user", trimmed.to_string())
}

fn inner(text: &str, tag: &str) -> Option<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = text.find(&open)? + open.len();
    let end = text[start..].find(&close)? + start;
    let value = text[start..end].trim();
    (!value.is_empty()).then(|| value.to_string())
}

/// Drop `<system-reminder>` blocks: the harness writes them, the user did not,
/// and showing them as things Alex said would be a lie.
fn strip_reminders(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find("<system-reminder>") {
        out.push_str(&rest[..start]);
        match rest[start..].find("</system-reminder>") {
            Some(end) => rest = &rest[start + end + "</system-reminder>".len()..],
            None => {
                rest = "";
                break;
            }
        }
    }
    out.push_str(rest);
    out.trim().to_string()
}

/// Split one assistant entry into prose, thinking, and ordered blocks.
///
/// The blocks keep source order, which is the whole point: a reply is "said
/// this, ran that, said this" and collapsing it into a paragraph followed by a
/// list of tools loses which sentence each call belongs to.
fn assistant_parts(
    content: Option<&Value>,
    results: &std::collections::HashMap<String, Value>,
) -> (String, String, Vec<Value>) {
    let mut text = Vec::new();
    let mut thinking = Vec::new();
    let mut ordered = Vec::new();
    let Some(content_blocks) = content.and_then(Value::as_array) else {
        if let Some(Value::String(plain)) = content {
            return (
                plain.clone(),
                String::new(),
                vec![json!({"kind": "text", "text": clip(plain, MAX_TEXT)})],
            );
        }
        return (String::new(), String::new(), ordered);
    };
    for block in content_blocks {
        match block.get("type").and_then(Value::as_str) {
            Some("text") => {
                if let Some(value) = block.get("text").and_then(Value::as_str) {
                    if value.trim().is_empty() {
                        continue;
                    }
                    text.push(value.to_string());
                    ordered.push(json!({"kind": "text", "text": clip(value, MAX_TEXT)}));
                }
            }
            Some("thinking") => {
                if let Some(value) = block.get("thinking").and_then(Value::as_str) {
                    if !value.trim().is_empty() {
                        thinking.push(value.to_string());
                    }
                }
            }
            Some("tool_use") => {
                let id = block.get("id").and_then(Value::as_str).unwrap_or_default();
                let name = block.get("name").and_then(Value::as_str).unwrap_or("tool");
                ordered.push(json!({
                    "kind": "tool",
                    "name": name,
                    "summary": clip(&tool_summary(name, block.get("input")), MAX_TOOL_INPUT),
                    "result": results.get(id).cloned().unwrap_or(Value::Null),
                }));
            }
            _ => {}
        }
    }
    (text.join("\n"), thinking.join("\n"), ordered)
}

/// A one-line "what did it do", using each tool's most telling argument.
fn tool_summary(name: &str, input: Option<&Value>) -> String {
    let Some(object) = input.and_then(Value::as_object) else {
        return String::new();
    };
    let preferred: &[&str] = match name {
        "Bash" => &["command"],
        "Read" | "Write" | "NotebookEdit" => &["file_path"],
        "Edit" => &["file_path"],
        "Glob" | "Grep" => &["pattern", "path"],
        "WebFetch" => &["url"],
        "WebSearch" => &["query"],
        "Agent" | "Task" => &["description", "prompt"],
        "Skill" => &["skill"],
        _ => &[],
    };
    for key in preferred {
        if let Some(value) = object.get(*key).and_then(Value::as_str) {
            return one_line(value);
        }
    }
    // Unknown tool: the first short string argument beats printing raw JSON.
    for (_, value) in object.iter() {
        if let Some(text) = value.as_str() {
            return one_line(text);
        }
    }
    String::new()
}

fn one_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Tool results are a string, or a list of content blocks, depending on the
/// tool.
fn block_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .filter_map(|block| block.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let head: String = text.chars().take(max).collect();
    format!("{head}\n… truncated")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_slug_matches_claudes_rule() {
        assert_eq!(project_slug("/Users/alex"), "-Users-alex");
        assert_eq!(
            project_slug("/Users/alex/vault/dev/shep-android"),
            "-Users-alex-vault-dev-shep-android"
        );
        // A dot becomes a dash like every other separator, which is why
        // `/x/.od/y` and `/x/-od/y` land in the same directory.
        assert_eq!(project_slug("/x/.od/y"), "-x--od-y");
    }

    fn line(value: Value) -> String {
        value.to_string()
    }

    /// The repo has no tempfile dev-dependency; pid + name is the house pattern.
    fn scratch(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("shep-transcript-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn parses_a_conversation_into_turns() {
        let raw = [
            line(json!({
                "type": "user",
                "timestamp": "t1",
                "message": {"role": "user", "content": "run the tests"}
            })),
            line(json!({
                "type": "assistant",
                "timestamp": "t2",
                "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "On it."},
                    {"type": "tool_use", "id": "tu1", "name": "Bash",
                     "input": {"command": "just check", "description": "run tests"}}
                ]}
            })),
            line(json!({
                "type": "user",
                "timestamp": "t3",
                "message": {"role": "user", "content": [
                    {"type": "tool_result", "tool_use_id": "tu1", "content": "2763 passed"}
                ]}
            })),
            line(json!({
                "type": "assistant",
                "timestamp": "t4",
                "message": {"role": "assistant", "content": [
                    {"type": "text", "text": "All green."}
                ]}
            })),
        ]
        .join("\n");

        let turns = parse_turns(&raw);
        assert_eq!(turns.len(), 2, "tool results are not turns: {turns:?}");
        assert_eq!(turns[0]["role"], "user");
        assert_eq!(turns[0]["text"], "run the tests");
        assert_eq!(turns[1]["role"], "assistant");
        // The two assistant entries either side of the tool result are one reply.
        assert_eq!(turns[1]["text"], "On it.\n\nAll green.");
        // …and its blocks stay in the order they happened, so the reader can see
        // that the tool ran between the two sentences.
        let blocks = turns[1]["blocks"].as_array().unwrap();
        assert_eq!(blocks.len(), 3);
        assert_eq!(blocks[0]["kind"], "text");
        assert_eq!(blocks[0]["text"], "On it.");
        assert_eq!(blocks[1]["kind"], "tool");
        assert_eq!(blocks[1]["name"], "Bash");
        assert_eq!(blocks[1]["summary"], "just check");
        assert_eq!(blocks[1]["result"]["ok"], true);
        assert_eq!(blocks[1]["result"]["preview"], "2763 passed");
        assert_eq!(blocks[2]["text"], "All green.");
    }

    #[test]
    fn drops_meta_reminders_and_sidechains() {
        let raw = [
            line(json!({
                "type": "user", "isMeta": true, "timestamp": "t1",
                "message": {"role": "user", "content": "Stop hook feedback: reflect"}
            })),
            line(json!({
                "type": "assistant", "isSidechain": true, "timestamp": "t2",
                "message": {"role": "assistant", "content": [{"type": "text", "text": "subagent"}]}
            })),
            line(json!({
                "type": "user", "timestamp": "t3",
                "message": {"role": "user", "content":
                    "<system-reminder>ignore me</system-reminder>real question"}
            })),
        ]
        .join("\n");

        let turns = parse_turns(&raw);
        assert_eq!(turns.len(), 1);
        assert_eq!(turns[0]["text"], "real question");
        assert_eq!(turns[0]["role"], "user");
    }

    #[test]
    fn harness_injections_are_not_attributed_to_the_user() {
        let raw = [
            line(json!({
                "type": "user", "timestamp": "t1",
                "message": {"role": "user", "content":
                    "<task-notification>\n<task-id>beets</task-id>\n\
                     <summary>Background command \"run the sweep\" completed</summary>\n\
                     </task-notification>"}
            })),
            line(json!({
                "type": "user", "timestamp": "t2",
                "message": {"role": "user", "content":
                    "<command-name>/effort</command-name><command-args>high</command-args>"}
            })),
            line(json!({
                "type": "user", "timestamp": "t3",
                "message": {"role": "user", "content": "and now the real ask"}
            })),
        ]
        .join("\n");

        let turns = parse_turns(&raw);
        assert_eq!(turns.len(), 3);
        assert_eq!(turns[0]["role"], "system");
        assert_eq!(
            turns[0]["text"],
            "Background command \"run the sweep\" completed"
        );
        assert_eq!(turns[1]["role"], "system");
        assert_eq!(turns[1]["text"], "/effort high");
        assert_eq!(turns[2]["role"], "user");
    }

    #[test]
    fn read_tail_starts_at_a_whole_line() {
        let dir = scratch("tail");
        let path = dir.join("s.jsonl");
        std::fs::write(&path, "first line\nsecond line\nthird line\n").unwrap();
        // Small enough to land mid-"second".
        let tail = read_tail(&path, 20);
        assert!(tail.starts_with("third line"), "got {tail:?}");
        // Whole file when it fits.
        assert!(read_tail(&path, 4096).starts_with("first line"));
    }

    #[test]
    fn matches_a_session_to_a_wrapped_screen() {
        let dir = scratch("match");
        let mine = dir.join("mine.jsonl");
        let other = dir.join("other.jsonl");
        std::fs::write(
            &mine,
            line(json!({
                "type": "assistant", "timestamp": "t",
                "message": {"role": "assistant", "content": [{"type": "text",
                    "text": "the reported-fire overlay is invisible under the smoke layer"}]}
            })),
        )
        .unwrap();
        std::fs::write(
            &other,
            line(json!({
                "type": "assistant", "timestamp": "t",
                "message": {"role": "assistant", "content": [{"type": "text",
                    "text": "nothing whatsoever to do with the pane in question here"}]}
            })),
        )
        .unwrap();

        // The pane wraps mid-word, which is exactly what a line-respecting
        // matcher fails on.
        let screen = "the reported-fire overlay is invis\nible under the smoke layer";
        let picked = best_match(&[mine.clone(), other], screen).expect("a match");
        assert_eq!(picked, mine);
    }

    #[test]
    fn refuses_to_guess_when_nothing_matches() {
        let dir = scratch("noguess");
        let a = dir.join("a.jsonl");
        let b = dir.join("b.jsonl");
        std::fs::write(&a, "").unwrap();
        std::fs::write(&b, "").unwrap();
        assert!(best_match(&[a, b], "an unrelated screen full of other words entirely").is_none());
    }

    #[test]
    fn only_answers_for_its_own_method() {
        let socket = std::path::Path::new("/nonexistent.sock");
        assert!(handle_local_method("task.list", None, socket).is_none());
        assert!(handle_local_method("pane.transcript", None, socket).is_some());
    }
}
