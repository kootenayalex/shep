//! FTS5 session-history sidecar: a searchable log of what every agent session
//! said and was asked, fed by claude-code lifecycle hooks (`shep memory
//! ingest-event`) and queried by `shep memory search`.
//!
//! The database lives at `<state dir>/history.db` (see
//! [`crate::config::state_dir`]). Ingestion is FAIL-OPEN end to end: a hook
//! caller must never see a non-zero exit or stdout noise from us — parse or
//! storage problems are logged to stderr and swallowed, because breaking a
//! user's claude session over a history-log bug is never worth it.

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

/// One ingested lifecycle event, ready for storage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HistoryEvent {
    pub session_id: String,
    /// The hook event name, e.g. `UserPromptSubmit`, `Stop`.
    pub kind: String,
    /// The searchable text for this event (may be empty for pure lifecycle
    /// markers like `SessionStart`).
    pub text: String,
}

/// A search result row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SearchHit {
    pub ts: i64,
    pub session_id: String,
    pub kind: String,
    /// FTS5 snippet with `[`/`]` around matched terms.
    pub snippet: String,
}

/// Default on-disk location of the history database.
pub(crate) fn history_db_path() -> PathBuf {
    crate::config::state_dir().join("history.db")
}

/// Open (creating if needed) the history database at `path` and ensure the
/// schema exists. The FTS5 index is an external-content table over `events`;
/// rows are inserted into both in [`insert_event`].
pub(crate) fn open_db(path: &Path) -> io::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).map_err(io_err)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS events (
            id INTEGER PRIMARY KEY,
            session_id TEXT NOT NULL,
            kind TEXT NOT NULL,
            ts INTEGER NOT NULL,
            text TEXT NOT NULL
        );
        CREATE VIRTUAL TABLE IF NOT EXISTS events_fts
            USING fts5(text, content='events', content_rowid='id');",
    )
    .map_err(io_err)?;
    Ok(conn)
}

/// Store one event (and index its text) at unix time `ts`.
pub(crate) fn insert_event(conn: &Connection, event: &HistoryEvent, ts: i64) -> io::Result<()> {
    conn.execute(
        "INSERT INTO events (session_id, kind, ts, text) VALUES (?1, ?2, ?3, ?4)",
        rusqlite::params![event.session_id, event.kind, ts, event.text],
    )
    .map_err(io_err)?;
    let rowid = conn.last_insert_rowid();
    conn.execute(
        "INSERT INTO events_fts (rowid, text) VALUES (?1, ?2)",
        rusqlite::params![rowid, event.text],
    )
    .map_err(io_err)?;
    Ok(())
}

/// Full-text search, most recent first.
pub(crate) fn search(conn: &Connection, query: &str, limit: usize) -> io::Result<Vec<SearchHit>> {
    let mut statement = conn
        .prepare(
            "SELECT e.ts, e.session_id, e.kind,
                    snippet(events_fts, 0, '[', ']', '…', 16)
             FROM events_fts
             JOIN events e ON e.id = events_fts.rowid
             WHERE events_fts MATCH ?1
             ORDER BY e.ts DESC
             LIMIT ?2",
        )
        .map_err(io_err)?;
    let rows = statement
        .query_map(rusqlite::params![fts_query(query), limit as i64], |row| {
            Ok(SearchHit {
                ts: row.get(0)?,
                session_id: row.get(1)?,
                kind: row.get(2)?,
                snippet: row.get(3)?,
            })
        })
        .map_err(io_err)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(io_err)
}

/// Turn free-form user input into a safe FTS5 MATCH expression: each
/// whitespace token becomes a quoted term (quotes doubled), joined implicitly
/// as AND. This keeps FTS5 query operators (`"`, `*`, `NEAR`, column filters)
/// from ever reaching the parser, so arbitrary input cannot raise a syntax
/// error.
pub(crate) fn fts_query(user: &str) -> String {
    user.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Extract a storable event from a raw hook payload. `None` when the payload
/// is not JSON or carries no `hook_event_name` — the caller drops it silently
/// (fail-open).
pub(crate) fn parse_event(raw: &str) -> Option<HistoryEvent> {
    let value: serde_json::Value = serde_json::from_str(raw).ok()?;
    let kind = value.get("hook_event_name")?.as_str()?.to_string();
    let session_id = value
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    // Which field carries the human-meaningful text differs per event.
    let text_field = match kind.as_str() {
        "UserPromptSubmit" => "prompt",
        // Stop payloads: last_assistant_message avoids transcript_path lag.
        "Stop" | "SubagentStop" => "last_assistant_message",
        "SessionStart" => "source",
        "SessionEnd" => "reason",
        _ => "",
    };
    let text = value
        .get(text_field)
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    Some(HistoryEvent {
        session_id,
        kind,
        text,
    })
}

/// `shep memory ingest-event`: read one hook payload from stdin and record it.
/// Always exits 0 and never writes to stdout — hook callers may interpret
/// stdout, and a history bug must not disturb the session.
pub(crate) fn run_ingest_event() -> i32 {
    use std::io::Read;
    let mut raw = String::new();
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return 0;
    }
    let Some(event) = parse_event(&raw) else {
        return 0;
    };
    let ts = unix_now();
    let result = open_db(&history_db_path()).and_then(|conn| insert_event(&conn, &event, ts));
    if let Err(err) = result {
        eprintln!("shep memory ingest-event: {err}");
    }
    0
}

fn unix_now() -> i64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn io_err(err: rusqlite::Error) -> io::Error {
    io::Error::other(format!("history db: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::memory::tests::temp_dir;

    fn temp_db(tag: &str) -> (PathBuf, Connection) {
        let dir = temp_dir(tag);
        let path = dir.join("history.db");
        let conn = open_db(&path).expect("fts5 must be available in bundled sqlite");
        (dir, conn)
    }

    #[test]
    fn open_creates_schema_including_fts5() {
        let (dir, conn) = temp_db("schema");
        // Proves the bundled sqlite actually ships FTS5.
        conn.execute_batch("INSERT INTO events_fts(rowid, text) VALUES (1, 'probe')")
            .unwrap();
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn insert_then_search_finds_snippet_most_recent_first() {
        let (dir, conn) = temp_db("roundtrip");
        let event = |text: &str| HistoryEvent {
            session_id: "s1".to_string(),
            kind: "UserPromptSubmit".to_string(),
            text: text.to_string(),
        };
        insert_event(&conn, &event("fix the login flow"), 100).unwrap();
        insert_event(&conn, &event("login page styling pass"), 200).unwrap();
        insert_event(&conn, &event("unrelated database work"), 300).unwrap();

        let hits = search(&conn, "login", 10).unwrap();
        assert_eq!(hits.len(), 2);
        // Most recent first.
        assert_eq!(hits[0].ts, 200);
        assert_eq!(hits[1].ts, 100);
        assert!(hits[0].snippet.contains("[login]"));
        assert_eq!(hits[0].kind, "UserPromptSubmit");
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn search_respects_limit() {
        let (dir, conn) = temp_db("limit");
        for ts in 0..5 {
            let event = HistoryEvent {
                session_id: "s".to_string(),
                kind: "Stop".to_string(),
                text: "repeated needle".to_string(),
            };
            insert_event(&conn, &event, ts).unwrap();
        }
        assert_eq!(search(&conn, "needle", 3).unwrap().len(), 3);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn fts_query_neutralizes_operators() {
        assert_eq!(fts_query("login"), "\"login\"");
        assert_eq!(fts_query("a b"), "\"a\" \"b\"");
        // Quotes are doubled; stars/NEAR end up inside quoted terms.
        assert_eq!(fts_query("\"broken"), "\"\"\"broken\"");
        let (dir, conn) = temp_db("operators");
        // Hostile inputs must not raise FTS5 syntax errors.
        for hostile in ["\"unbalanced", "a* NEAR b", "col:val", "(paren"] {
            search(&conn, hostile, 5).unwrap();
        }
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn parse_event_extracts_per_kind_text_fields() {
        let prompt = parse_event(
            r#"{"hook_event_name":"UserPromptSubmit","session_id":"s","prompt":"do the thing"}"#,
        )
        .unwrap();
        assert_eq!(prompt.kind, "UserPromptSubmit");
        assert_eq!(prompt.text, "do the thing");

        let stop = parse_event(
            r#"{"hook_event_name":"Stop","session_id":"s","last_assistant_message":"done"}"#,
        )
        .unwrap();
        assert_eq!(stop.text, "done");

        let start =
            parse_event(r#"{"hook_event_name":"SessionStart","session_id":"s","source":"resume"}"#)
                .unwrap();
        assert_eq!(start.text, "resume");

        let end =
            parse_event(r#"{"hook_event_name":"SessionEnd","session_id":"s","reason":"exit"}"#)
                .unwrap();
        assert_eq!(end.text, "exit");
    }

    #[test]
    fn parse_event_fails_open() {
        assert_eq!(parse_event("not json"), None);
        assert_eq!(parse_event(r#"{"session_id":"s"}"#), None);
        // Unknown kinds still record (empty text) — session markers are useful.
        let other = parse_event(r#"{"hook_event_name":"PreToolUse","session_id":"s"}"#).unwrap();
        assert_eq!(other.text, "");
        // Missing session_id defaults rather than dropping the event.
        let anon = parse_event(r#"{"hook_event_name":"Stop"}"#).unwrap();
        assert_eq!(anon.session_id, "unknown");
    }
}
