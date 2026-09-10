//! `pane.todos` — the checklist an agent is working through right now, read
//! back out of its own session.
//!
//! Bridge-local for the same reason `pane.transcript` is: this reads files that
//! already exist on this machine, so it needs no API method and no protocol
//! bump. It does ask the server which session a pane is running, which is why
//! it takes the api socket.
//!
//! Two sources, in order of preference:
//!
//! 1. `~/.claude/tasks/<session>/<n>.json` — the harness's own store, already
//!    exactly the shape we want. It is not reliable: most session directories
//!    on a real machine have been emptied, keeping only their lock files.
//! 2. Folding `TaskCreate` / `TaskUpdate` back out of the session transcript,
//!    last writer wins. Slower and derived, but it survives the cleanup that
//!    empties (1), so it is the fallback rather than the other way round.
//!
//! Read-only by design. A checklist is the agent's working state, not
//! something a phone should reach in and edit.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::transcript::{
    block_text, dirs_home, find_pane, resolve_session_file, session_id_from_path,
};

/// `TaskCreate` announces the id it assigned in its result text, not in its
/// input, so the fold has to read it back out of this sentence.
const CREATED_MARKER: &str = "Task #";

pub(super) fn handle_local_method(
    method: &str,
    params: Option<&Value>,
    api_socket: &Path,
) -> Option<Result<Value, String>> {
    match method {
        "pane.todos" => Some(todos(params, api_socket)),
        _ => None,
    }
}

fn todos(params: Option<&Value>, api_socket: &Path) -> Result<Value, String> {
    let params = params.ok_or("missing params")?;
    let target = params
        .get("target")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or("missing target (pane id)")?;

    let pane = find_pane(api_socket, target)?;
    let agent = pane
        .get("agent")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if agent != "claude" {
        return Err(format!(
            "no todo reader for agent {}",
            if agent.is_empty() { "(none)" } else { &agent }
        ));
    }
    let cwd = pane
        .get("cwd")
        .and_then(Value::as_str)
        .ok_or("pane has no cwd")?;

    let (path, _) = resolve_session_file(&pane, cwd, api_socket, target)?;
    let session_id = session_id_from_path(&path);

    if let Some(id) = session_id.as_deref() {
        let stored = from_store(&store_dir(id));
        if !stored.is_empty() {
            return Ok(render(stored, "store", session_id));
        }
    }

    let raw = std::fs::read_to_string(&path).map_err(|err| format!("{}: {err}", path.display()))?;
    Ok(render(from_transcript(&raw), "transcript", session_id))
}

fn render(items: Vec<Todo>, source: &str, session_id: Option<String>) -> Value {
    json!({
        "todos": {
            "session_id": session_id,
            "source": source,
            "items": items.into_iter().map(Todo::into_json).collect::<Vec<_>>(),
        }
    })
}

fn store_dir(session_id: &str) -> PathBuf {
    dirs_home().join(".claude").join("tasks").join(session_id)
}

// ---------------------------------------------------------------------------
// The todo itself
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct Todo {
    pub id: String,
    pub subject: String,
    pub active_form: String,
    pub description: String,
    pub status: String,
    pub blocks: Vec<String>,
    pub blocked_by: Vec<String>,
}

impl Todo {
    fn into_json(self) -> Value {
        json!({
            "id": self.id,
            "subject": self.subject,
            "activeForm": self.active_form,
            "description": self.description,
            "status": self.status,
            "blocks": self.blocks,
            "blockedBy": self.blocked_by,
        })
    }
}

/// Ids are `"1"`, `"2"`, … so a plain string sort puts 10 before 2. Sort
/// numerically, and keep anything unparseable in a stable tail.
fn sort_by_id(items: &mut [Todo]) {
    items.sort_by_key(|item| item.id.parse::<u64>().unwrap_or(u64::MAX));
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn text(map: &Map<String, Value>, key: &str) -> String {
    map.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

// ---------------------------------------------------------------------------
// Source 1: the harness's own store
// ---------------------------------------------------------------------------

fn from_store(dir: &Path) -> Vec<Todo> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut items = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(Value::Object(record)) = serde_json::from_str::<Value>(&raw) else {
            continue;
        };
        let id = text(&record, "id");
        if id.is_empty() {
            continue;
        }
        items.push(Todo {
            id,
            subject: text(&record, "subject"),
            active_form: text(&record, "activeForm"),
            description: text(&record, "description"),
            status: text(&record, "status"),
            blocks: strings(record.get("blocks")),
            blocked_by: strings(record.get("blockedBy")),
        });
    }
    sort_by_id(&mut items);
    items
}

// ---------------------------------------------------------------------------
// Source 2: folding the transcript
// ---------------------------------------------------------------------------

pub(super) fn from_transcript(raw: &str) -> Vec<Todo> {
    let entries: Vec<Value> = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        // A subagent runs its own checklist; this pane is not working through it.
        .filter(|entry| entry.get("isSidechain").and_then(Value::as_bool) != Some(true))
        .collect();

    let results = result_index(&entries);
    let mut items: HashMap<String, Todo> = HashMap::new();
    let mut order: Vec<String> = Vec::new();

    for entry in &entries {
        let Some(blocks) = entry
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(Value::as_array)
        else {
            continue;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_use") {
                continue;
            }
            let name = block
                .get("name")
                .and_then(Value::as_str)
                .unwrap_or_default();
            let Some(input) = block.get("input").and_then(Value::as_object) else {
                continue;
            };
            let call_id = block.get("id").and_then(Value::as_str).unwrap_or_default();
            let result = results.get(call_id);
            // A rejected call changed nothing, so folding it would invent state
            // the agent never had. (A real one: a TaskUpdate sent a `tasks`
            // array, was refused for a missing `taskId`, and was then reissued
            // one id at a time.)
            if matches!(result, Some((false, _))) {
                continue;
            }
            let result_text = result.map(|(_, text)| text.as_str()).unwrap_or_default();

            match name {
                "TaskCreate" => {
                    let id =
                        created_id(result_text).unwrap_or_else(|| (order.len() + 1).to_string());
                    let todo = Todo {
                        id: id.clone(),
                        subject: text(input, "subject"),
                        active_form: text(input, "activeForm"),
                        description: text(input, "description"),
                        status: "pending".into(),
                        blocks: Vec::new(),
                        blocked_by: Vec::new(),
                    };
                    if items.insert(id.clone(), todo).is_none() {
                        order.push(id);
                    }
                }
                "TaskUpdate" => {
                    let id = text(input, "taskId");
                    if id.is_empty() {
                        continue;
                    }
                    let todo = items.entry(id.clone()).or_insert_with(|| {
                        order.push(id.clone());
                        Todo {
                            id: id.clone(),
                            status: "pending".into(),
                            ..Todo::default()
                        }
                    });
                    // Only the fields this call carried: an update that sets a
                    // status must not blank the subject written at creation.
                    for (key, field) in [
                        ("status", &mut todo.status),
                        ("activeForm", &mut todo.active_form),
                        ("description", &mut todo.description),
                        ("subject", &mut todo.subject),
                    ] {
                        if let Some(value) = input.get(key).and_then(Value::as_str) {
                            *field = value.to_string();
                        }
                    }
                }
                // Not seen in the wild on this harness, but it is the shape the
                // tool had before and it replaces the whole list rather than
                // editing one entry, so treat it as authoritative when present.
                "TodoWrite" => {
                    let Some(todos) = input.get("todos").and_then(Value::as_array) else {
                        continue;
                    };
                    items.clear();
                    order.clear();
                    for (index, todo) in todos.iter().enumerate() {
                        let Some(todo) = todo.as_object() else {
                            continue;
                        };
                        let id = (index + 1).to_string();
                        let mut subject = text(todo, "content");
                        if subject.is_empty() {
                            subject = text(todo, "subject");
                        }
                        items.insert(
                            id.clone(),
                            Todo {
                                id: id.clone(),
                                subject,
                                active_form: text(todo, "activeForm"),
                                description: String::new(),
                                status: text(todo, "status"),
                                blocks: Vec::new(),
                                blocked_by: Vec::new(),
                            },
                        );
                        order.push(id);
                    }
                }
                // TaskStop / TaskOutput / TaskList belong to background agent
                // runs, which are a different thing wearing a similar name.
                _ => {}
            }
        }
    }

    let mut items: Vec<Todo> = order
        .into_iter()
        .filter_map(|id| items.remove(&id))
        .collect();
    sort_by_id(&mut items);
    items
}

fn result_index(entries: &[Value]) -> HashMap<String, (bool, String)> {
    let mut map = HashMap::new();
    for entry in entries {
        let Some(blocks) = entry
            .get("message")
            .and_then(|message| message.get("content"))
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
            map.insert(id.to_string(), (!failed, block_text(block.get("content"))));
        }
    }
    map
}

/// `"Task #12 created successfully: …"` -> `"12"`.
fn created_id(result: &str) -> Option<String> {
    let rest = result.split_once(CREATED_MARKER)?.1;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    (!digits.is_empty()).then_some(digits)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same shape as `crate::memory::tests::temp_dir`, which is private to that
    /// module tree. Callers clean up after themselves.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "shep-{tag}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|since| since.as_nanos())
                .unwrap_or(0)
        ));
        let _ = std::fs::create_dir_all(&dir);
        dir
    }

    /// One assistant tool_use plus the user tool_result that answered it, which
    /// is the pair the fold has to read together.
    fn call(id: &str, name: &str, input: Value, result: &str, ok: bool) -> String {
        let use_line = json!({
            "type": "assistant",
            "message": {"content": [{"type": "tool_use", "id": id, "name": name, "input": input}]},
        });
        let result_line = json!({
            "type": "user",
            "message": {"content": [{
                "type": "tool_result", "tool_use_id": id, "is_error": !ok, "content": result,
            }]},
        });
        format!("{use_line}\n{result_line}")
    }

    fn created(id: &str, n: u32, subject: &str) -> String {
        call(
            id,
            "TaskCreate",
            json!({"subject": subject, "activeForm": format!("doing {subject}")}),
            &format!("Task #{n} created successfully: {subject}"),
            true,
        )
    }

    #[test]
    fn folds_create_and_update_into_one_checklist() {
        let raw = [
            created("a", 1, "first"),
            created("b", 2, "second"),
            call(
                "c",
                "TaskUpdate",
                json!({"taskId": "1", "status": "completed"}),
                "Updated task #1 status",
                true,
            ),
            call(
                "d",
                "TaskUpdate",
                json!({"taskId": "2", "status": "in_progress"}),
                "Updated task #2 status",
                true,
            ),
        ]
        .join("\n");

        let items = from_transcript(&raw);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[0].subject, "first");
        assert_eq!(items[0].status, "completed");
        assert_eq!(items[1].status, "in_progress");
    }

    /// The id lives in the result sentence, not the input, so a create whose
    /// result is missing still has to land somewhere sensible.
    #[test]
    fn a_create_without_a_result_still_gets_an_id() {
        let raw = json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "id": "x", "name": "TaskCreate",
                "input": {"subject": "orphan"},
            }]},
        })
        .to_string();

        let items = from_transcript(&raw);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].id, "1");
        assert_eq!(items[0].subject, "orphan");
    }

    #[test]
    fn a_rejected_update_changes_nothing() {
        let raw = [
            created("a", 1, "first"),
            call(
                "b",
                "TaskUpdate",
                json!({"tasks": "[{\"task_id\":\"1\"}]"}),
                "<tool_use_error>InputValidationError: the required parameter `taskId` is missing</tool_use_error>",
                false,
            ),
        ]
        .join("\n");

        let items = from_transcript(&raw);

        assert_eq!(items.len(), 1);
        assert_eq!(items[0].status, "pending");
    }

    #[test]
    fn an_update_only_touches_the_fields_it_carries() {
        let raw = [
            created("a", 1, "keep my subject"),
            call(
                "b",
                "TaskUpdate",
                json!({"taskId": "1", "status": "in_progress"}),
                "Updated task #1 status",
                true,
            ),
        ]
        .join("\n");

        let items = from_transcript(&raw);

        assert_eq!(items[0].subject, "keep my subject");
        assert_eq!(items[0].active_form, "doing keep my subject");
        assert_eq!(items[0].status, "in_progress");
    }

    #[test]
    fn a_subagents_checklist_is_not_this_panes_checklist() {
        let mut line: Value =
            serde_json::from_str(&created("a", 1, "theirs")).unwrap_or(Value::Null);
        if let Some(entry) = line.as_object_mut() {
            entry.insert("isSidechain".into(), Value::Bool(true));
        }

        assert!(from_transcript(&line.to_string()).is_empty());
    }

    #[test]
    fn todowrite_replaces_the_whole_list() {
        let raw = [
            created("a", 1, "from the task tool"),
            call(
                "b",
                "TodoWrite",
                json!({"todos": [
                    {"content": "one", "status": "completed", "activeForm": "doing one"},
                    {"content": "two", "status": "pending", "activeForm": "doing two"},
                ]}),
                "ok",
                true,
            ),
        ]
        .join("\n");

        let items = from_transcript(&raw);

        assert_eq!(items.len(), 2);
        assert_eq!(items[0].subject, "one");
        assert_eq!(items[1].status, "pending");
    }

    /// A plain string sort puts "10" before "2", which reads as a shuffled list.
    #[test]
    fn ids_sort_numerically_not_lexically() {
        let raw = (1..=11u32)
            .map(|n| created(&format!("c{n}"), n, &format!("task {n}")))
            .collect::<Vec<_>>()
            .join("\n");

        let items = from_transcript(&raw);

        let ids: Vec<&str> = items.iter().map(|item| item.id.as_str()).collect();
        assert_eq!(ids.first(), Some(&"1"));
        assert_eq!(ids.last(), Some(&"11"));
        assert_eq!(ids[1], "2");
    }

    #[test]
    fn reads_the_harness_store_when_it_has_files() {
        let dir = temp_dir("todo-store");
        for (name, body) in [
            (
                "2.json",
                json!({"id": "2", "subject": "second", "status": "pending"}),
            ),
            (
                "1.json",
                json!({
                    "id": "1", "subject": "first", "activeForm": "doing first",
                    "description": "why", "status": "completed",
                    "blocks": ["2"], "blockedBy": [],
                }),
            ),
            ("notes.txt", json!("ignored")),
        ] {
            let _ = std::fs::write(dir.join(name), body.to_string());
        }

        let items = from_store(&dir);
        let _ = std::fs::remove_dir_all(&dir);

        assert_eq!(items.len(), 2, "the non-json file must be skipped");
        assert_eq!(items[0].id, "1");
        assert_eq!(items[0].blocks, vec!["2".to_string()]);
        assert_eq!(items[1].id, "2");
    }

    #[test]
    fn an_absent_store_is_empty_not_an_error() {
        assert!(from_store(Path::new("/nonexistent/shep/todo/store")).is_empty());
    }

    #[test]
    fn only_answers_for_its_own_method() {
        let socket = Path::new("/tmp/shep-todo-test.sock");
        assert!(handle_local_method("pane.transcript", None, socket).is_none());
        assert!(handle_local_method("task.list", None, socket).is_none());
        assert!(handle_local_method("pane.todos", None, socket).is_some());
    }
}
