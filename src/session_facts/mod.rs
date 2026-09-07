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
    /// The name the agent knows itself by (Claude's `/rename`).
    pub name: Option<String>,
    /// `normal` / `plan` / `acceptEdits` / `bypassPermissions`.
    pub permission_mode: Option<String>,
    pub cost_usd: Option<f64>,
    pub lines_added: Option<u64>,
    pub lines_removed: Option<u64>,
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
            if rule.record == record && per_rule[idx].is_none() {
                per_rule[idx] = Some(value.clone());
                unresolved -= 1;
            }
        }
    }

    let string_field = |key: &dyn Fn(&manifest::FactRule) -> Option<&String>| {
        manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = key(rule)?;
            let value = per_rule[idx].as_ref()?.get(field)?.as_str()?.trim();
            (!value.is_empty()).then(|| value.to_string())
        })
    };
    let u64_field = |key: &dyn Fn(&manifest::FactRule) -> Option<&String>| {
        manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = key(rule)?;
            per_rule[idx].as_ref()?.get(field)?.as_u64()
        })
    };

    SessionFacts {
        title: string_field(&|rule| rule.title.as_ref()),
        name: string_field(&|rule| rule.name.as_ref()),
        permission_mode: string_field(&|rule| rule.permission_mode.as_ref()),
        cost_usd: manifest.facts.iter().enumerate().find_map(|(idx, rule)| {
            let field = rule.cost_usd.as_ref()?;
            per_rule[idx].as_ref()?.get(field)?.as_f64()
        }),
        lines_added: u64_field(&|rule| rule.lines_added.as_ref()),
        lines_removed: u64_field(&|rule| rule.lines_removed.as_ref()),
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
