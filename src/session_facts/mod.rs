//! What an agent says about its own session, read from the agent's own session
//! file rather than guessed at from its screen.
//!
//! Claude Code writes a brief AI title, the `/rename` name, the permission mode
//! and a cost/churn tally into its transcript jsonl; shep's hook already
//! receives that transcript's path. These facts are **display hints** — the
//! same contract as `activity_lines`: never detection evidence, never
//! load-bearing, and always fail-soft to `None`.

pub(crate) mod manifest;

use std::{
    io::{Read, Seek, SeekFrom},
    path::Path,
};

use serde_json::Value;

/// Facts an agent publishes about the session it is running. Every field is
/// optional: a missing manifest, a missing record, a truncated line or an
/// unreadable file all mean "no fact", never a wrong one.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SessionFacts {
    /// A brief summary of the session, for the card's "what is this" row.
    pub title: Option<String>,
    /// The last thing the agent said it was doing, in its own words — the line
    /// it writes just before it acts. Fresher than `title`, which names the
    /// whole session, so a card prefers this when both are present.
    pub summary: Option<String>,
    /// The name the agent knows itself by (Claude's `/rename`).
    pub name: Option<String>,
    /// `normal` / `plan` / `acceptEdits` / `bypassPermissions`.
    pub permission_mode: Option<String>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
}

/// Where `agent` keeps the session file for `session_id`, when no hook has told
/// us the path.
///
/// The path arrives on a hook event, and an agent already running when the
/// server started has no such event left to send — so after a restart every
/// fact would stay blank until that agent restarted too. The session id is
/// persisted, and each agent's manifest says how to get from one to the other.
///
/// Returns `None` for an agent with no manifest, no `session_file` template, an
/// id that could climb out of the directory it names, or a file that is not
/// there.
pub fn locate(agent: &str, session_id: &str) -> Option<std::path::PathBuf> {
    let manifest = manifest::manifest_for(agent)?;
    let template = manifest.session_file.as_ref()?;
    // The id is the agent's own word for itself and lands in a path, so it may
    // not contain a separator or climb.
    if session_id.is_empty()
        || session_id.contains(std::path::MAIN_SEPARATOR)
        || session_id.contains('/')
        || session_id.contains("..")
    {
        return None;
    }
    resolve_session_file(template, session_id)
}

/// The template half of [`locate`], without the manifest lookup, so the path
/// rules can be tested against a real directory rather than the user's own.
fn resolve_session_file(template: &str, session_id: &str) -> Option<std::path::PathBuf> {
    let filled = expand_home(&template.replace("{session_id}", session_id))?;
    match filled.split_once("/*/") {
        None => {
            let path = std::path::PathBuf::from(filled);
            path.is_file().then_some(path)
        }
        Some((base, rest)) => std::fs::read_dir(base).ok()?.flatten().find_map(|entry| {
            let candidate = entry.path().join(rest);
            candidate.is_file().then_some(candidate)
        }),
    }
}

/// Expand a leading `~/`. Only leading, and only `~/` — a manifest naming a
/// path is naming one, not writing shell.
fn expand_home(path: &str) -> Option<String> {
    let Some(rest) = path.strip_prefix("~/") else {
        return Some(path.to_string());
    };
    let home = std::env::var_os("HOME")?;
    Some(
        std::path::Path::new(&home)
            .join(rest)
            .to_string_lossy()
            .into_owned(),
    )
}

/// Read the facts an agent has written about `path`'s session.
///
/// `agent` is the detected agent label; an agent with no manifest yields
/// `SessionFacts::default()` without touching the disk.
pub fn read(agent: &str, path: &Path) -> SessionFacts {
    let Some(manifest) = manifest::manifest_for(agent) else {
        return SessionFacts::default();
    };
    let Some(text) = read_tail(path, manifest.tail_bytes) else {
        return SessionFacts::default();
    };
    facts_from_jsonl_tail(&manifest, &text)
}

/// The last `tail_bytes` of a file, as lossy UTF-8. Returns `None` when the
/// file cannot be read at all.
fn read_tail(path: &Path, tail_bytes: u64) -> Option<String> {
    let mut file = std::fs::File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    let start = len.saturating_sub(tail_bytes);
    if start > 0 {
        file.seek(SeekFrom::Start(start)).ok()?;
    }
    let mut buf = Vec::with_capacity(tail_bytes.min(len) as usize);
    file.take(tail_bytes).read_to_end(&mut buf).ok()?;
    let mut text = String::from_utf8_lossy(&buf).into_owned();
    if start > 0 {
        // The first line of a mid-file read is a fragment; drop it rather than
        // let a half-record parse into a half-fact.
        match text.find('\n') {
            Some(idx) => text = text[idx + 1..].to_string(),
            None => return Some(String::new()),
        }
    }
    Some(text)
}

/// Scan backwards so the last record of each type wins, then resolve each field
/// through the manifest's block order.
fn facts_from_jsonl_tail(manifest: &manifest::SessionFactsManifest, text: &str) -> SessionFacts {
    let mut per_rule: Vec<Option<Value>> = vec![None; manifest.facts.len()];
    let mut unresolved = manifest.facts.len();

    for line in text.lines().rev() {
        if unresolved == 0 {
            break;
        }
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        // A live transcript is being appended to while we read it, so a
        // malformed final line is normal, not an error.
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        let Some(record) = value.get(&manifest.discriminator).and_then(Value::as_str) else {
            continue;
        };
        for (idx, rule) in manifest.facts.iter().enumerate() {
            // A record of the right kind that carries none of the rule's fields
            // is not the record we want. Most `assistant` records are a tool
            // call and nothing else; latching onto the newest one would mean
            // reading no summary at all rather than the newest one that has
            // words in it.
            if rule.record == record && per_rule[idx].is_none() && rule_resolves(rule, &value) {
                per_rule[idx] = Some(value.clone());
                unresolved -= 1;
            }
        }
    }

    let string_field = |key: &dyn Fn(&manifest::FactRule) -> Option<&String>| {
        manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = key(rule)?;
            let value = resolve(per_rule[idx].as_ref()?, field)?.as_str()?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let u64_field = |key: &dyn Fn(&manifest::FactRule) -> Option<&String>| {
        manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = key(rule)?;
            resolve(per_rule[idx].as_ref()?, field)?.as_u64()
        })
    };

    SessionFacts {
        title: string_field(&|rule| rule.title.as_ref()),
        summary: string_field(&|rule| rule.summary.as_ref()).map(|text| first_line(&text)),
        name: string_field(&|rule| rule.name.as_ref()),
        permission_mode: string_field(&|rule| rule.permission_mode.as_ref()),
        cost_usd: manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = rule.cost_usd.as_ref()?;
            resolve(per_rule[idx].as_ref()?, field)?.as_f64()
        }),
        lines_added: u64_field(&|rule| rule.lines_added.as_ref()),
        lines_removed: u64_field(&|rule| rule.lines_removed.as_ref()),
    }
}

/// Whether this record actually carries any of the fields the rule reads.
fn rule_resolves(rule: &manifest::FactRule, value: &Value) -> bool {
    [
        rule.title.as_ref(),
        rule.summary.as_ref(),
        rule.name.as_ref(),
        rule.permission_mode.as_ref(),
        rule.cost_usd.as_ref(),
        rule.lines_added.as_ref(),
        rule.lines_removed.as_ref(),
    ]
    .into_iter()
    .flatten()
    .any(|field| resolve(value, field).is_some_and(|found| !found.is_null()))
}

/// The most a summary may be. An agent's own words are a card line, and the
/// same field rides `session.overview` to the phone; a whole closing report is
/// neither.
const MAX_SUMMARY: usize = 200;

/// The first line of `text`, bounded. What an agent writes before it acts is
/// one line; what it writes when it is done is a report whose first line is
/// still the sentence that summarises it.
fn first_line(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    match line.char_indices().nth(MAX_SUMMARY) {
        Some((end, _)) => format!("{}…", line[..end].trim_end()),
        None => line.to_string(),
    }
}

/// Resolve a dotted path against a JSON value.
///
/// Objects descend by key. An array descends by *search*: the first element
/// whose remaining path resolves wins, which is how `message.content.text`
/// finds the text block in a list that also holds tool calls and thinking. A
/// path with no dots is an ordinary field lookup, so every manifest written
/// before this existed reads exactly as it did.
fn resolve<'a>(value: &'a Value, path: &str) -> Option<&'a Value> {
    let mut segments = path.splitn(2, '.');
    let head = segments.next()?;
    let rest = segments.next();
    let next = match value {
        Value::Array(items) => {
            return items.iter().find_map(|item| resolve(item, path));
        }
        other => other.get(head)?,
    };
    match rest {
        Some(rest) => resolve(next, rest),
        None => Some(next),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn claude() -> manifest::SessionFactsManifest {
        manifest::manifest_for("claude").expect("claude ships a manifest")
    }

    fn write_temp(name: &str, body: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("shep-session-facts-{name}.jsonl"));
        std::fs::write(&path, body).expect("write fixture");
        path
    }

    #[test]
    fn last_record_of_each_type_wins_and_custom_title_beats_ai_title() {
        let text = r#"
{"type":"ai-title","aiTitle":"first guess"}
{"type":"cost-state","totalCostUSD":1.5,"totalLinesAdded":3,"totalLinesRemoved":1}
{"type":"agent-name","agentName":"stale"}
{"type":"permission-mode","permissionMode":"plan"}
{"type":"ai-title","aiTitle":"board card layout pass"}
{"type":"agent-name","agentName":"board-redesign"}
{"type":"cost-state","totalCostUSD":3.42,"totalLinesAdded":210,"totalLinesRemoved":18}
{"type":"custom-title","customTitle":"the one Alex typed"}
"#;
        let facts = facts_from_jsonl_tail(&claude(), text);
        assert_eq!(facts.title.as_deref(), Some("the one Alex typed"));
        assert_eq!(facts.name.as_deref(), Some("board-redesign"));
        assert_eq!(facts.permission_mode.as_deref(), Some("plan"));
        assert_eq!(facts.cost_usd, Some(3.42));
        assert_eq!(facts.lines_added, Some(210));
        assert_eq!(facts.lines_removed, Some(18));
    }

    #[test]
    fn the_summary_is_the_last_thing_the_agent_said_it_was_doing() {
        // Real record shapes: a turn is one assistant record carrying the text
        // and a second carrying only the tool call. Scanning backwards hits the
        // tool-call record first, and it has no words in it — the newest record
        // that actually says something is the one to read.
        let text = r#"
{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"text","text":"Now carry the age through the session snapshot:"}]}}
{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"cargo fmt"}}]}}
{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"thinking","thinking":"weighing two shapes"},{"type":"text","text":"Now the pane screen's header age:\nand then the tests."},{"type":"tool_use","id":"toolu_2","name":"Edit","input":{}}]}}
{"type":"assistant","message":{"role":"assistant","stop_reason":"tool_use","content":[{"type":"tool_use","id":"toolu_3","name":"Read","input":{}}]}}
"#;
        let facts = facts_from_jsonl_tail(&claude(), text);
        // Thinking is skipped (no `text` of its own), the first line only, and
        // the tool-call-only records above and below are passed over.
        assert_eq!(
            facts.summary.as_deref(),
            Some("Now the pane screen's header age:")
        );
    }

    #[test]
    fn a_transcript_of_nothing_but_tool_calls_has_no_summary() {
        let text = r#"{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t","name":"Bash","input":{}}]}}"#;
        assert_eq!(facts_from_jsonl_tail(&claude(), text).summary, None);
    }

    #[test]
    fn a_closing_report_is_cut_to_its_first_sentence_worth() {
        let long = "x".repeat(400);
        let text = format!(
            r#"{{"type":"assistant","message":{{"content":[{{"type":"text","text":"{long}"}}]}}}}"#
        );
        let summary = facts_from_jsonl_tail(&claude(), &text)
            .summary
            .expect("a report is still a summary");
        assert_eq!(summary.chars().count(), MAX_SUMMARY + 1, "{summary}");
        assert!(summary.ends_with('\u{2026}'));
    }

    #[test]
    fn a_session_file_is_found_from_the_id_when_no_hook_said_where() {
        // The shape the claude manifest names: a directory shep does not know
        // the name of, and the session id doing the identifying.
        let root = std::env::temp_dir().join(format!("shep-locate-{}", std::process::id()));
        let project = root.join("projects").join("-Users-alex");
        std::fs::create_dir_all(project.join("-Users-alex-other")).expect("fixture dirs");
        std::fs::create_dir_all(&project).expect("fixture dirs");
        let id = "11111111-2222-3333-4444-555555555555";
        let wanted = project.join(format!("{id}.jsonl"));
        std::fs::write(&wanted, "{}\n").expect("fixture file");

        let template = format!("{}/projects/*/{{session_id}}.jsonl", root.display());
        assert_eq!(resolve_session_file(&template, id).as_ref(), Some(&wanted));
        // A session that is not there yields nothing rather than a path that
        // would fail to open later.
        assert_eq!(
            resolve_session_file(&template, "99999999-0000-0000-0000-000000000000"),
            None
        );
        // The claude manifest is what makes this reachable at all.
        let claude = claude();
        assert_eq!(
            claude.session_file.as_deref(),
            Some("~/.claude/projects/*/{session_id}.jsonl")
        );

        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_id_that_could_climb_out_of_its_directory_finds_nothing() {
        assert_eq!(locate("claude", "../../etc/passwd"), None);
        assert_eq!(locate("claude", "sub/dir"), None);
        assert_eq!(locate("claude", ""), None);
        // An agent with no manifest has nowhere to look.
        assert_eq!(
            locate("codex", "11111111-2222-3333-4444-555555555555"),
            None
        );
    }

    #[test]
    fn ai_title_stands_in_when_nothing_was_typed() {
        let text = r#"{"type":"ai-title","aiTitle":"board card layout pass"}"#;
        let facts = facts_from_jsonl_tail(&claude(), text);
        assert_eq!(facts.title.as_deref(), Some("board card layout pass"));
    }

    #[test]
    fn a_truncated_final_line_does_not_lose_the_rest() {
        let text = "{\"type\":\"ai-title\",\"aiTitle\":\"a real title\"}\n{\"type\":\"cost-st";
        let facts = facts_from_jsonl_tail(&claude(), text);
        assert_eq!(facts.title.as_deref(), Some("a real title"));
        assert_eq!(facts.cost_usd, None);
    }

    #[test]
    fn a_record_older_than_the_tail_reads_as_absent() {
        let filler = "{\"type\":\"message\",\"pad\":\"".to_string() + &"x".repeat(4_000) + "\"}\n";
        let mut body = String::from("{\"type\":\"ai-title\",\"aiTitle\":\"scrolled away\"}\n");
        for _ in 0..4 {
            body.push_str(&filler);
        }
        let path = write_temp("tail-bound", &body);
        // A tail smaller than the filler cannot reach the title.
        let manifest = manifest::SessionFactsManifest {
            tail_bytes: 4_096,
            ..claude()
        };
        let text = read_tail(&path, manifest.tail_bytes).expect("read tail");
        let facts = facts_from_jsonl_tail(&manifest, &text);
        assert_eq!(facts.title, None);
        // The same file read whole does find it.
        let text = read_tail(&path, 1_000_000).expect("read tail");
        assert_eq!(
            facts_from_jsonl_tail(&claude(), &text).title.as_deref(),
            Some("scrolled away")
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn an_agent_without_a_manifest_yields_nothing() {
        let path = write_temp("no-manifest", "{\"type\":\"ai-title\",\"aiTitle\":\"x\"}\n");
        assert_eq!(read("codex", &path), SessionFacts::default());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn a_missing_file_yields_nothing() {
        let path = std::env::temp_dir().join("shep-session-facts-does-not-exist.jsonl");
        assert_eq!(read("claude", &path), SessionFacts::default());
    }
}
