//! The docket store: one sqlite table at `<state dir>/docket.db`, owned by the
//! server and edited only through the `docket.*` API. A recurring item is one
//! row for its whole life — completing it rolls `due` forward and stamps
//! `last_fired` rather than cloning the row.
//!
//! The first open imports the Phase 0 `docket.json` beside it (ids preserved)
//! and renames the file `docket.json.imported`, so nothing captured during the
//! prototype week is lost and nothing is imported twice.

use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OptionalExtension};

use super::dates::{next_due, Date};
use crate::api::schema::{DocketItem, DocketKind, DocketRepeat, DocketStatus};

/// Default on-disk location of the docket database.
pub(crate) fn docket_db_path() -> PathBuf {
    crate::config::state_dir().join("docket.db")
}

#[derive(Debug)]
pub(crate) enum DocketError {
    NotFound(i64),
    /// The verb does not apply to the item's current status.
    InvalidTransition(String),
    /// A field failed validation (bad date, missing repeat, empty title).
    Invalid(String),
    Store(io::Error),
}

impl DocketError {
    /// The API error code this maps to.
    pub(crate) fn code(&self) -> &'static str {
        match self {
            DocketError::NotFound(_) => "docket_not_found",
            DocketError::InvalidTransition(_) => "docket_invalid_transition",
            DocketError::Invalid(_) => "invalid_params",
            DocketError::Store(_) => "docket_store",
        }
    }
}

impl std::fmt::Display for DocketError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DocketError::NotFound(id) => write!(f, "docket item {id} not found"),
            DocketError::InvalidTransition(message) | DocketError::Invalid(message) => {
                f.write_str(message)
            }
            DocketError::Store(err) => write!(f, "docket store error: {err}"),
        }
    }
}

impl From<io::Error> for DocketError {
    fn from(err: io::Error) -> Self {
        DocketError::Store(err)
    }
}

impl From<rusqlite::Error> for DocketError {
    fn from(err: rusqlite::Error) -> Self {
        DocketError::Store(io_err(err))
    }
}

/// Fields for a brand-new item.
#[derive(Debug, Clone, Default)]
pub(crate) struct NewItem {
    pub title: String,
    pub kind: Option<DocketKind>,
    pub status: Option<DocketStatus>,
    pub due: Option<String>,
    pub repeat: Option<DocketRepeat>,
    pub source: Option<serde_json::Value>,
    pub notes: Option<String>,
}

/// Fields to change on an existing item; `None` leaves the column alone.
#[derive(Debug, Clone, Default)]
pub(crate) struct ItemPatch {
    pub title: Option<String>,
    pub notes: Option<String>,
    pub due: Option<String>,
    pub repeat: Option<DocketRepeat>,
    pub kind: Option<DocketKind>,
}

/// Open (creating if needed) the docket at `path`, ensure the schema, and
/// import a sibling `docket.json` if one is still waiting.
pub(crate) fn open_store(path: &Path) -> io::Result<Connection> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let conn = Connection::open(path).map_err(io_err)?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS items (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            title TEXT NOT NULL,
            kind TEXT NOT NULL,
            status TEXT NOT NULL,
            due TEXT,
            repeat_every TEXT,
            source_json TEXT,
            notes TEXT,
            created TEXT NOT NULL,
            updated TEXT NOT NULL,
            last_fired TEXT
        );",
    )
    .map_err(io_err)?;
    let json_path = path.with_file_name("docket.json");
    if json_path.is_file() {
        let imported = import_json(&conn, &json_path)?;
        let renamed = path.with_file_name("docket.json.imported");
        std::fs::rename(&json_path, &renamed)?;
        tracing::info!(
            imported,
            from = %json_path.display(),
            "imported the prototype docket into docket.db"
        );
    }
    Ok(conn)
}

/// Import the Phase 0 `{"version":1,"items":[…]}` file, preserving ids.
/// Rows whose id already exists are skipped, never overwritten. Returns the
/// number of rows inserted.
fn import_json(conn: &Connection, json_path: &Path) -> io::Result<usize> {
    #[derive(serde::Deserialize)]
    struct File {
        #[serde(default)]
        items: Vec<Item>,
    }
    #[derive(serde::Deserialize)]
    struct Item {
        id: i64,
        title: String,
        kind: DocketKind,
        status: DocketStatus,
        #[serde(default)]
        due: Option<String>,
        #[serde(default)]
        repeat: Option<Repeat>,
        #[serde(default)]
        source: Option<serde_json::Value>,
        #[serde(default)]
        notes: Option<String>,
        #[serde(default)]
        created: Option<String>,
        #[serde(default)]
        updated: Option<String>,
        #[serde(default)]
        last_fired: Option<String>,
    }
    #[derive(serde::Deserialize)]
    struct Repeat {
        every: DocketRepeat,
    }

    let text = std::fs::read_to_string(json_path)?;
    let file: File = serde_json::from_str(&text).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{}: {err}", json_path.display()),
        )
    })?;
    let now = utc_now(conn)?;
    let mut inserted = 0;
    for item in file.items {
        let created = item.created.unwrap_or_else(|| now.clone());
        let updated = item.updated.unwrap_or_else(|| created.clone());
        let source_json = item
            .source
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        inserted += conn
            .execute(
                "INSERT OR IGNORE INTO items
                    (id, title, kind, status, due, repeat_every, source_json, notes,
                     created, updated, last_fired)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                rusqlite::params![
                    item.id,
                    item.title,
                    item.kind.as_str(),
                    item.status.as_str(),
                    item.due,
                    item.repeat.map(|repeat| repeat.every.as_str()),
                    source_json,
                    item.notes,
                    created,
                    updated,
                    item.last_fired,
                ],
            )
            .map_err(io_err)?;
    }
    Ok(inserted)
}

/// Today's date in the server's local calendar.
pub(crate) fn today(conn: &Connection) -> io::Result<Date> {
    let raw: String = conn
        .query_row("SELECT date('now', 'localtime')", [], |row| row.get(0))
        .map_err(io_err)?;
    Date::parse(&raw).ok_or_else(|| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("sqlite returned an unparseable date {raw:?}"),
        )
    })
}

fn utc_now(conn: &Connection) -> io::Result<String> {
    conn.query_row("SELECT strftime('%Y-%m-%dT%H:%M:%SZ', 'now')", [], |row| {
        row.get(0)
    })
    .map_err(io_err)
}

const SELECT_COLUMNS: &str = "id, title, kind, status, due, repeat_every, source_json, notes, \
                              created, updated, last_fired";

fn row_to_item(row: &rusqlite::Row<'_>, today: Date) -> rusqlite::Result<DocketItem> {
    let kind: String = row.get(2)?;
    let status: String = row.get(3)?;
    let repeat: Option<String> = row.get(5)?;
    let source_json: Option<String> = row.get(6)?;
    let status = DocketStatus::parse(&status).unwrap_or(DocketStatus::Inbox);
    let due: Option<String> = row.get(4)?;
    let overdue = status == DocketStatus::Open
        && due
            .as_deref()
            .and_then(Date::parse)
            .is_some_and(|date| date < today);
    Ok(DocketItem {
        id: row.get(0)?,
        title: row.get(1)?,
        kind: DocketKind::parse(&kind).unwrap_or(DocketKind::Captured),
        status,
        due,
        repeat: repeat.as_deref().and_then(DocketRepeat::parse),
        source: source_json.and_then(|json| serde_json::from_str(&json).ok()),
        notes: row.get(7)?,
        created: row.get(8)?,
        updated: row.get(9)?,
        last_fired: row.get(10)?,
        overdue,
    })
}

/// Every item (or those in `status`), in docket order: dated open items by
/// due ascending (overdue first by construction), then the inbox, then undated
/// open items, then done/discarded — the last three groups newest-updated
/// first.
pub(crate) fn list(
    conn: &Connection,
    status: Option<DocketStatus>,
) -> io::Result<(Date, Vec<DocketItem>)> {
    let today = today(conn)?;
    let mut statement = conn
        .prepare(&format!(
            "SELECT {SELECT_COLUMNS} FROM items WHERE (?1 IS NULL OR status = ?1)"
        ))
        .map_err(io_err)?;
    let rows = statement
        .query_map(rusqlite::params![status.map(DocketStatus::as_str)], |row| {
            row_to_item(row, today)
        })
        .map_err(io_err)?;
    let mut items = rows.collect::<Result<Vec<_>, _>>().map_err(io_err)?;
    sort_docket(&mut items);
    Ok((today, items))
}

/// The list ordering, separated so it can be tested without a clock.
pub(crate) fn sort_docket(items: &mut [DocketItem]) {
    fn rank(item: &DocketItem) -> u8 {
        match item.status {
            DocketStatus::Open if item.due.is_some() => 0,
            DocketStatus::Inbox => 1,
            DocketStatus::Open => 2,
            DocketStatus::Done | DocketStatus::Discarded => 3,
        }
    }
    items.sort_by(|a, b| {
        rank(a).cmp(&rank(b)).then_with(|| {
            if rank(a) == 0 {
                a.due.cmp(&b.due).then_with(|| a.id.cmp(&b.id))
            } else {
                b.updated.cmp(&a.updated).then_with(|| b.id.cmp(&a.id))
            }
        })
    });
}

pub(crate) fn get(conn: &Connection, id: i64) -> Result<DocketItem, DocketError> {
    let today = today(conn)?;
    conn.query_row(
        &format!("SELECT {SELECT_COLUMNS} FROM items WHERE id = ?1"),
        rusqlite::params![id],
        |row| row_to_item(row, today),
    )
    .optional()?
    .ok_or(DocketError::NotFound(id))
}

fn validate_due(due: Option<&str>) -> Result<(), DocketError> {
    match due {
        Some(raw) if Date::parse(raw).is_none() => Err(DocketError::Invalid(format!(
            "due must be YYYY-MM-DD, got {raw:?}"
        ))),
        _ => Ok(()),
    }
}

fn validate_title(title: &str) -> Result<(), DocketError> {
    if title.trim().is_empty() {
        return Err(DocketError::Invalid("title must not be empty".into()));
    }
    Ok(())
}

pub(crate) fn add(conn: &Connection, item: NewItem) -> Result<DocketItem, DocketError> {
    validate_title(&item.title)?;
    validate_due(item.due.as_deref())?;
    let kind = item.kind.unwrap_or(DocketKind::Captured);
    let status = item.status.unwrap_or(match kind {
        DocketKind::Captured => DocketStatus::Inbox,
        DocketKind::Slated | DocketKind::Recurring => DocketStatus::Open,
    });
    let source_json = item
        .source
        .as_ref()
        .map(serde_json::to_string)
        .transpose()
        .map_err(|err| DocketError::Invalid(format!("source is not JSON: {err}")))?;
    let now = utc_now(conn)?;
    conn.execute(
        "INSERT INTO items
            (title, kind, status, due, repeat_every, source_json, notes, created, updated)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8)",
        rusqlite::params![
            item.title.trim(),
            kind.as_str(),
            status.as_str(),
            item.due,
            item.repeat.map(DocketRepeat::as_str),
            source_json,
            item.notes,
            now,
        ],
    )?;
    get(conn, conn.last_insert_rowid())
}

pub(crate) fn update(
    conn: &Connection,
    id: i64,
    patch: ItemPatch,
) -> Result<DocketItem, DocketError> {
    get(conn, id)?;
    if let Some(title) = patch.title.as_deref() {
        validate_title(title)?;
    }
    validate_due(patch.due.as_deref())?;
    let now = utc_now(conn)?;
    conn.execute(
        "UPDATE items SET
            title = COALESCE(?2, title),
            notes = COALESCE(?3, notes),
            due = COALESCE(?4, due),
            repeat_every = COALESCE(?5, repeat_every),
            kind = COALESCE(?6, kind),
            updated = ?7
         WHERE id = ?1",
        rusqlite::params![
            id,
            patch.title.as_deref().map(str::trim),
            patch.notes,
            patch.due,
            patch.repeat.map(DocketRepeat::as_str),
            patch.kind.map(DocketKind::as_str),
            now,
        ],
    )?;
    get(conn, id)
}

/// `inbox → open` as `slated` or `recurring` (a `captured` kind is refused:
/// promotion is exactly the act of deciding what a captured item is).
pub(crate) fn promote(
    conn: &Connection,
    id: i64,
    kind: DocketKind,
    due: Option<String>,
    repeat: Option<DocketRepeat>,
) -> Result<DocketItem, DocketError> {
    let current = get(conn, id)?;
    if current.status != DocketStatus::Inbox {
        return Err(DocketError::InvalidTransition(format!(
            "docket item {id} is {}, only inbox items can be promoted",
            current.status.as_str()
        )));
    }
    if kind == DocketKind::Captured {
        return Err(DocketError::Invalid(
            "promote needs a kind of slated or recurring".into(),
        ));
    }
    validate_due(due.as_deref())?;
    let repeat = repeat.or(current.repeat);
    if kind == DocketKind::Recurring && repeat.is_none() {
        return Err(DocketError::Invalid(
            "a recurring item needs a repeat (1d, 1w, 2w, 1m)".into(),
        ));
    }
    let now = utc_now(conn)?;
    conn.execute(
        "UPDATE items SET
            status = 'open',
            kind = ?2,
            due = COALESCE(?3, due),
            repeat_every = ?4,
            updated = ?5
         WHERE id = ?1",
        rusqlite::params![
            id,
            kind.as_str(),
            due,
            repeat.map(DocketRepeat::as_str),
            now
        ],
    )?;
    get(conn, id)
}

/// `open → done`; a repeating item instead stays open with `due` rolled past
/// today and `last_fired` stamped. Returns the row as it is afterwards.
pub(crate) fn complete(conn: &Connection, id: i64) -> Result<DocketItem, DocketError> {
    let current = get(conn, id)?;
    if current.status != DocketStatus::Open {
        return Err(DocketError::InvalidTransition(format!(
            "docket item {id} is {}, only open items can be completed",
            current.status.as_str()
        )));
    }
    let now = utc_now(conn)?;
    match current.repeat {
        Some(repeat) => {
            let today = today(conn)?;
            let next = next_due(current.due.as_deref().and_then(Date::parse), repeat, today);
            conn.execute(
                "UPDATE items SET due = ?2, last_fired = ?3, updated = ?3 WHERE id = ?1",
                rusqlite::params![id, next.format(), now],
            )?;
        }
        None => {
            conn.execute(
                "UPDATE items SET status = 'done', last_fired = ?2, updated = ?2 WHERE id = ?1",
                rusqlite::params![id, now],
            )?;
        }
    }
    get(conn, id)
}

/// Any live (`inbox`/`open`) item → `discarded`.
pub(crate) fn discard(conn: &Connection, id: i64) -> Result<DocketItem, DocketError> {
    let current = get(conn, id)?;
    if matches!(current.status, DocketStatus::Done | DocketStatus::Discarded) {
        return Err(DocketError::InvalidTransition(format!(
            "docket item {id} is already {}",
            current.status.as_str()
        )));
    }
    let now = utc_now(conn)?;
    conn.execute(
        "UPDATE items SET status = 'discarded', updated = ?2 WHERE id = ?1",
        rusqlite::params![id, now],
    )?;
    get(conn, id)
}

/// Remove a row outright. `Ok(false)` when there was no such row.
// No API verb reaches this yet: `discard` is the user-facing removal, and a
// hard delete is kept as the store's own escape hatch (tests, future cleanup).
#[allow(dead_code)]
pub(crate) fn delete(conn: &Connection, id: i64) -> io::Result<bool> {
    let removed = conn
        .execute("DELETE FROM items WHERE id = ?1", rusqlite::params![id])
        .map_err(io_err)?;
    Ok(removed > 0)
}

fn io_err(err: rusqlite::Error) -> io::Error {
    io::Error::other(err)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_db(name: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        std::env::temp_dir()
            .join(format!("shep-docket-{name}-{}-{nanos}", std::process::id()))
            .join("docket.db")
    }

    fn cleanup(path: &Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn slated(conn: &Connection, title: &str, due: &str) -> DocketItem {
        add(
            conn,
            NewItem {
                title: title.into(),
                kind: Some(DocketKind::Slated),
                due: Some(due.into()),
                ..NewItem::default()
            },
        )
        .unwrap()
    }

    #[test]
    fn add_defaults_captured_items_into_the_inbox() {
        let path = temp_db("add");
        let conn = open_store(&path).unwrap();
        let item = add(
            &conn,
            NewItem {
                title: "  rotate the xai key ".into(),
                source: Some(serde_json::json!({"file": "MEMORY.md", "line": 12})),
                ..NewItem::default()
            },
        )
        .unwrap();
        assert_eq!(item.title, "rotate the xai key");
        assert_eq!(item.kind, DocketKind::Captured);
        assert_eq!(item.status, DocketStatus::Inbox);
        assert_eq!(
            item.source,
            Some(serde_json::json!({"file": "MEMORY.md", "line": 12}))
        );
        assert!(!item.overdue);
        assert!(item.created.ends_with('Z'));

        let slated = slated(&conn, "renew", "2030-01-01");
        assert_eq!(slated.status, DocketStatus::Open);

        let err = add(
            &conn,
            NewItem {
                title: "bad".into(),
                due: Some("2026-02-30".into()),
                ..NewItem::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        let err = add(
            &conn,
            NewItem {
                title: "   ".into(),
                ..NewItem::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn overdue_is_open_with_a_past_due_date() {
        let path = temp_db("overdue");
        let conn = open_store(&path).unwrap();
        let past = slated(&conn, "late", "2000-01-01");
        assert!(past.overdue);
        let future = slated(&conn, "later", "2999-12-31");
        assert!(!future.overdue);
        // Done items are never overdue, whatever their date.
        let finished = complete(&conn, past.id).unwrap();
        assert_eq!(finished.status, DocketStatus::Done);
        assert!(!finished.overdue);
        assert!(finished.last_fired.is_some());
        // Neither is an inbox item that happens to carry a suggested date.
        let inbox = add(
            &conn,
            NewItem {
                title: "suggested".into(),
                due: Some("2000-01-01".into()),
                ..NewItem::default()
            },
        )
        .unwrap();
        assert!(!inbox.overdue);
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn list_orders_dated_open_then_inbox_then_open_then_closed() {
        let path = temp_db("order");
        let conn = open_store(&path).unwrap();
        let closed = slated(&conn, "closed", "2000-01-02");
        complete(&conn, closed.id).unwrap();
        let undated = add(
            &conn,
            NewItem {
                title: "undated open".into(),
                kind: Some(DocketKind::Slated),
                ..NewItem::default()
            },
        )
        .unwrap();
        let inbox = add(
            &conn,
            NewItem {
                title: "captured".into(),
                ..NewItem::default()
            },
        )
        .unwrap();
        let later = slated(&conn, "later", "2999-01-01");
        let overdue = slated(&conn, "overdue", "2000-01-01");
        let discarded = add(
            &conn,
            NewItem {
                title: "nope".into(),
                ..NewItem::default()
            },
        )
        .unwrap();
        discard(&conn, discarded.id).unwrap();

        let (_, items) = list(&conn, None).unwrap();
        let ids: Vec<i64> = items.iter().map(|item| item.id).collect();
        // Same-second `updated` stamps fall back to id descending inside a
        // group, so only the group order and the due order are asserted.
        assert_eq!(&ids[..2], &[overdue.id, later.id]);
        assert_eq!(ids[2], inbox.id);
        assert_eq!(ids[3], undated.id);
        let tail: std::collections::HashSet<i64> = ids[4..].iter().copied().collect();
        assert_eq!(tail, [closed.id, discarded.id].into_iter().collect());

        let (_, only_inbox) = list(&conn, Some(DocketStatus::Inbox)).unwrap();
        assert_eq!(
            only_inbox.iter().map(|item| item.id).collect::<Vec<_>>(),
            vec![inbox.id]
        );
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn sort_docket_is_stable_on_updated_within_a_group() {
        fn item(id: i64, status: DocketStatus, due: Option<&str>, updated: &str) -> DocketItem {
            DocketItem {
                id,
                title: String::new(),
                kind: DocketKind::Slated,
                status,
                due: due.map(String::from),
                repeat: None,
                source: None,
                notes: None,
                created: updated.into(),
                updated: updated.into(),
                last_fired: None,
                overdue: false,
            }
        }
        let mut items = vec![
            item(1, DocketStatus::Done, None, "2026-09-01T00:00:00Z"),
            item(2, DocketStatus::Done, None, "2026-09-05T00:00:00Z"),
            item(3, DocketStatus::Inbox, None, "2026-09-02T00:00:00Z"),
            item(4, DocketStatus::Inbox, None, "2026-09-03T00:00:00Z"),
            item(
                5,
                DocketStatus::Open,
                Some("2026-10-01"),
                "2026-09-01T00:00:00Z",
            ),
            item(
                6,
                DocketStatus::Open,
                Some("2026-09-01"),
                "2026-09-01T00:00:00Z",
            ),
            item(7, DocketStatus::Open, None, "2026-09-01T00:00:00Z"),
        ];
        sort_docket(&mut items);
        let ids: Vec<i64> = items.iter().map(|item| item.id).collect();
        assert_eq!(ids, vec![6, 5, 4, 3, 7, 2, 1]);
    }

    #[test]
    fn promote_moves_inbox_items_into_the_docket() {
        let path = temp_db("promote");
        let conn = open_store(&path).unwrap();
        let captured = add(
            &conn,
            NewItem {
                title: "file the register".into(),
                ..NewItem::default()
            },
        )
        .unwrap();
        let err = promote(&conn, captured.id, DocketKind::Recurring, None, None).unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        let err = promote(&conn, captured.id, DocketKind::Captured, None, None).unwrap_err();
        assert_eq!(err.code(), "invalid_params");

        let promoted = promote(
            &conn,
            captured.id,
            DocketKind::Slated,
            Some("2026-10-07".into()),
            None,
        )
        .unwrap();
        assert_eq!(promoted.status, DocketStatus::Open);
        assert_eq!(promoted.kind, DocketKind::Slated);
        assert_eq!(promoted.due.as_deref(), Some("2026-10-07"));

        let err = promote(&conn, captured.id, DocketKind::Slated, None, None).unwrap_err();
        assert_eq!(err.code(), "docket_invalid_transition");
        let err = promote(&conn, 999, DocketKind::Slated, None, None).unwrap_err();
        assert_eq!(err.code(), "docket_not_found");
        assert_eq!(err.to_string(), "docket item 999 not found");
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn completing_a_recurring_item_rolls_the_same_row_forward() {
        let path = temp_db("recur");
        let conn = open_store(&path).unwrap();
        let weekly = add(
            &conn,
            NewItem {
                title: "check the backups".into(),
                kind: Some(DocketKind::Recurring),
                due: Some("2000-01-01".into()),
                repeat: Some(DocketRepeat::Weekly),
                ..NewItem::default()
            },
        )
        .unwrap();
        assert!(weekly.overdue);

        let rolled = complete(&conn, weekly.id).unwrap();
        assert_eq!(rolled.id, weekly.id);
        assert_eq!(rolled.status, DocketStatus::Open);
        assert!(!rolled.overdue, "next due must land after today");
        let today = today(&conn).unwrap();
        let next = Date::parse(rolled.due.as_deref().unwrap()).unwrap();
        assert!(next > today);
        assert!(next.to_days() - today.to_days() <= 7);
        assert!(rolled.last_fired.is_some());

        let (_, items) = list(&conn, None).unwrap();
        assert_eq!(items.len(), 1, "one row per recurring item");

        // A recurring row that lost its repeat completes like a one-off.
        let once = add(
            &conn,
            NewItem {
                title: "one-off".into(),
                kind: Some(DocketKind::Slated),
                due: Some("2026-01-01".into()),
                ..NewItem::default()
            },
        )
        .unwrap();
        assert_eq!(complete(&conn, once.id).unwrap().status, DocketStatus::Done);
        let err = complete(&conn, once.id).unwrap_err();
        assert_eq!(err.code(), "docket_invalid_transition");
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn update_changes_only_the_given_fields() {
        let path = temp_db("update");
        let conn = open_store(&path).unwrap();
        let item = slated(&conn, "old title", "2026-10-01");
        let updated = update(
            &conn,
            item.id,
            ItemPatch {
                notes: Some("some notes".into()),
                repeat: Some(DocketRepeat::Monthly),
                ..ItemPatch::default()
            },
        )
        .unwrap();
        assert_eq!(updated.title, "old title");
        assert_eq!(updated.due.as_deref(), Some("2026-10-01"));
        assert_eq!(updated.notes.as_deref(), Some("some notes"));
        assert_eq!(updated.repeat, Some(DocketRepeat::Monthly));
        let err = update(
            &conn,
            item.id,
            ItemPatch {
                due: Some("next week".into()),
                ..ItemPatch::default()
            },
        )
        .unwrap_err();
        assert_eq!(err.code(), "invalid_params");
        assert!(matches!(
            update(&conn, 42, ItemPatch::default()).unwrap_err(),
            DocketError::NotFound(42)
        ));
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn discard_and_delete() {
        let path = temp_db("discard");
        let conn = open_store(&path).unwrap();
        let item = slated(&conn, "meh", "2026-10-01");
        assert_eq!(
            discard(&conn, item.id).unwrap().status,
            DocketStatus::Discarded
        );
        assert_eq!(
            discard(&conn, item.id).unwrap_err().code(),
            "docket_invalid_transition"
        );
        assert!(delete(&conn, item.id).unwrap());
        assert!(!delete(&conn, item.id).unwrap());
        assert!(matches!(
            get(&conn, item.id).unwrap_err(),
            DocketError::NotFound(_)
        ));
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn first_open_imports_the_prototype_json_and_retires_it() {
        let path = temp_db("import");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json_path = path.with_file_name("docket.json");
        std::fs::write(
            &json_path,
            r#"{"version":1,"items":[
              {"id":7,"title":"Keep It Lit: file the register","kind":"captured","status":"inbox",
               "source":{"file":"~/.claude/projects/-Users-alex/memory/MEMORY.md","line":23},
               "notes":"Suggested due 2026-10-07.","created":"2026-09-11T17:25:08Z","updated":"2026-09-11T17:25:08Z"},
              {"id":9,"title":"check backups","kind":"recurring","status":"open","due":"2026-09-14",
               "repeat":{"every":"1w"},"created":"2026-09-11T17:25:08Z","updated":"2026-09-11T17:25:08Z",
               "last_fired":"2026-09-07T09:00:00Z"},
              {"id":3,"title":"done already","kind":"slated","status":"done","due":"2026-09-01"}
            ]}"#,
        )
        .unwrap();

        let conn = open_store(&path).unwrap();
        assert!(!json_path.exists(), "the json is renamed after import");
        assert!(path.with_file_name("docket.json.imported").is_file());

        let (_, items) = list(&conn, None).unwrap();
        let mut ids: Vec<i64> = items.iter().map(|item| item.id).collect();
        ids.sort_unstable();
        assert_eq!(ids, vec![3, 7, 9], "ids are preserved");
        let captured = get(&conn, 7).unwrap();
        assert_eq!(captured.status, DocketStatus::Inbox);
        assert_eq!(captured.source.as_ref().unwrap()["line"], 23);
        assert_eq!(captured.notes.as_deref(), Some("Suggested due 2026-10-07."));
        let recurring = get(&conn, 9).unwrap();
        assert_eq!(recurring.repeat, Some(DocketRepeat::Weekly));
        assert_eq!(
            recurring.last_fired.as_deref(),
            Some("2026-09-07T09:00:00Z")
        );
        let bare = get(&conn, 3).unwrap();
        assert!(bare.created.ends_with('Z'), "missing stamps are filled in");

        // New rows continue after the imported ids.
        let fresh = add(
            &conn,
            NewItem {
                title: "new".into(),
                ..NewItem::default()
            },
        )
        .unwrap();
        assert!(fresh.id > 9);

        // Reopening does not import again, and a second json is not clobbered
        // over existing ids.
        drop(conn);
        std::fs::write(
            &json_path,
            r#"{"version":1,"items":[{"id":7,"title":"overwritten?","kind":"captured","status":"inbox"}]}"#,
        )
        .unwrap();
        let conn = open_store(&path).unwrap();
        assert_eq!(
            get(&conn, 7).unwrap().title,
            "Keep It Lit: file the register"
        );
        drop(conn);
        cleanup(&path);
    }

    #[test]
    fn a_corrupt_prototype_json_fails_the_open_and_is_left_in_place() {
        let path = temp_db("corrupt");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let json_path = path.with_file_name("docket.json");
        std::fs::write(&json_path, "{not json").unwrap();
        let err = open_store(&path).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(json_path.is_file());
        cleanup(&path);
    }
}
