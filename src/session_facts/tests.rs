use super::*;

fn claude() -> manifest::SessionFactsManifest {
    manifest::manifest_for("claude").expect("claude ships a manifest")
}

fn temp_file(name: &str, body: &str) -> std::path::PathBuf {
    let path =
        std::env::temp_dir().join(format!("shep-session-facts-{name}-{}", std::process::id()));
    std::fs::write(&path, body).expect("write fixture");
    path
}

/// A line shaped like the bulk of a real transcript: enormous, and of no
/// interest to us. Present in these fixtures so the tests exercise the
/// cheap-rejection path rather than a file of nothing but facts.
fn noise(tag: &str) -> String {
    format!(
        r#"{{"type":"assistant","uuid":"{tag}","message":{{"content":"{}"}}}}"#,
        "x".repeat(4096)
    )
}

#[test]
fn the_last_record_of_each_kind_wins() {
    let body = [
        r#"{"type":"ai-title","aiTitle":"an early guess"}"#.to_string(),
        noise("a"),
        r#"{"type":"ai-title","aiTitle":"a later guess"}"#.to_string(),
        r#"{"type":"permission-mode","permissionMode":"plan"}"#.to_string(),
        noise("b"),
        r#"{"type":"permission-mode","permissionMode":"bypassPermissions"}"#.to_string(),
        r#"{"type":"agent-name","agentName":"board-lanes"}"#.to_string(),
    ]
    .join("\n");

    let facts = facts_from_tail(&claude(), &body);
    assert_eq!(facts.title.as_deref(), Some("a later guess"));
    assert_eq!(facts.permission_mode.as_deref(), Some("bypassPermissions"));
    assert_eq!(facts.name.as_deref(), Some("board-lanes"));
}

/// The reason precedence is by manifest order and not by recency. Claude
/// rewrites `ai-title` constantly, so a user's `custom-title` is nearly always
/// the *earlier* record — and must still win.
#[test]
fn a_typed_title_outranks_a_generated_one_even_when_it_is_older() {
    let body = [
        r#"{"type":"custom-title","customTitle":"shep-prototype"}"#,
        r#"{"type":"ai-title","aiTitle":"exploring the ui module"}"#,
        r#"{"type":"ai-title","aiTitle":"exploring the ui module again"}"#,
    ]
    .join("\n");

    assert_eq!(
        facts_from_tail(&claude(), &body).title.as_deref(),
        Some("shep-prototype")
    );
}

#[test]
fn cost_and_churn_come_off_one_record() {
    let body =
        r#"{"type":"cost-state","totalCostUSD":3.42,"totalLinesAdded":210,"totalLinesRemoved":18}"#;
    let facts = facts_from_tail(&claude(), body);
    assert_eq!(facts.cost_usd, Some(3.42));
    assert_eq!(facts.lines_added, Some(210));
    assert_eq!(facts.lines_removed, Some(18));
}

/// shep reads this file while the agent is appending to it, so the last line is
/// routinely half-written. That must cost the one record, not the whole read.
#[test]
fn a_half_written_final_line_does_not_lose_the_rest() {
    let body = [
        r#"{"type":"ai-title","aiTitle":"still here"}"#,
        r#"{"type":"cost-state","totalCostUS"#,
    ]
    .join("\n");

    let facts = facts_from_tail(&claude(), &body);
    assert_eq!(facts.title.as_deref(), Some("still here"));
    assert_eq!(facts.cost_usd, None);
}

#[test]
fn an_empty_value_is_not_an_answer() {
    let body = [
        r#"{"type":"ai-title","aiTitle":"a real title"}"#,
        r#"{"type":"ai-title","aiTitle":"   "}"#,
    ]
    .join("\n");
    // The blank is more recent, and is skipped rather than latched, so the
    // card keeps saying something true.
    assert_eq!(
        facts_from_tail(&claude(), &body).title.as_deref(),
        Some("a real title")
    );
}

#[test]
fn a_record_kind_we_do_not_ask_about_is_ignored() {
    let body = r#"{"type":"bridge-session","bridgeSessionId":"abc","aiTitle":"not ours"}"#;
    assert!(facts_from_tail(&claude(), body).is_empty());
}

#[test]
fn an_agent_with_no_manifest_reads_nothing() {
    let path = temp_file("no-manifest", r#"{"type":"ai-title","aiTitle":"x"}"#);
    assert!(read("codex", &path).is_empty());
    std::fs::remove_file(path).ok();
}

#[test]
fn a_missing_file_is_silence_not_an_error() {
    let path = std::env::temp_dir().join("shep-session-facts-does-not-exist");
    std::fs::remove_file(&path).ok();
    assert!(read("claude", &path).is_empty());
}

/// The window is the whole point: a fact that has scrolled out of it reads as
/// absent. Absent is a card row that does not draw; a *stale* value would be a
/// card that lies.
#[test]
fn a_fact_beyond_the_tail_window_reads_as_absent() {
    let body = format!(
        "{}\n{}\n{}",
        r#"{"type":"ai-title","aiTitle":"scrolled out of the window"}"#,
        noise("filler").repeat(80),
        r#"{"type":"cost-state","totalCostUSD":1.0}"#
    );
    let path = temp_file("window", &body);

    // Read it whole first, to prove the fixture really does contain the title.
    let facts = facts_from_tail(&claude(), &body);
    assert_eq!(facts.title.as_deref(), Some("scrolled out of the window"));

    // Then through a window far too small to reach it.
    let tail = read_tail(&path, 4096).expect("tail");
    let windowed = facts_from_tail(&claude(), &tail);
    assert_eq!(windowed.title, None);
    assert_eq!(windowed.cost_usd, Some(1.0));
    std::fs::remove_file(path).ok();
}

/// Seeking from the end lands mid-line, and half a JSON object is not a record.
#[test]
fn the_partial_line_a_window_starts_on_is_dropped() {
    let body = format!(
        "{}\n{}\n",
        "y".repeat(500),
        r#"{"type":"ai-title","aiTitle":"kept"}"#
    );
    let path = temp_file("partial", &body);
    let tail = read_tail(&path, 200).expect("tail");
    assert!(
        !tail.starts_with('y'),
        "the window kept a partial line: {tail:?}"
    );
    assert_eq!(
        facts_from_tail(&claude(), &tail).title.as_deref(),
        Some("kept")
    );
    std::fs::remove_file(path).ok();
}

/// A file smaller than the window has no partial line to drop, and cutting at
/// its first newline anyway would eat its only record.
#[test]
fn a_short_file_is_read_whole() {
    let path = temp_file("short", "{\"type\":\"ai-title\",\"aiTitle\":\"tiny\"}\n");
    let tail = read_tail(&path, 1024).expect("tail");
    assert_eq!(
        facts_from_tail(&claude(), &tail).title.as_deref(),
        Some("tiny")
    );
    std::fs::remove_file(path).ok();
}

#[test]
fn reading_a_real_transcript_agrees_with_the_file() {
    // Point at whatever live transcript exists on this machine, if any, and
    // check the reader against the file rather than against a fixture the
    // reader itself shaped. Skipped where there is no Claude history — this
    // must not fail CI on a fresh box.
    let Some(home) = std::env::var_os("HOME") else {
        return;
    };
    let projects = std::path::Path::new(&home).join(".claude/projects");
    let Some(found) = newest_transcript(&projects) else {
        return;
    };
    let facts = read("claude", &found);
    let body = std::fs::read_to_string(&found).unwrap_or_default();
    if let Some(title) = &facts.title {
        assert!(
            body.contains(title.as_str()),
            "{} reports a title that is not in the file: {title:?}",
            found.display()
        );
    }
    if let Some(name) = &facts.name {
        assert!(
            body.contains(name.as_str()),
            "{} reports a name that is not in the file: {name:?}",
            found.display()
        );
    }
    if let Some(mode) = &facts.permission_mode {
        assert!(
            ["normal", "plan", "acceptEdits", "bypassPermissions"].contains(&mode.as_str()),
            "unexpected permission mode {mode:?} in {}",
            found.display()
        );
    }
}

fn newest_transcript(root: &std::path::Path) -> Option<std::path::PathBuf> {
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    for project in std::fs::read_dir(root).ok()? {
        let Ok(project) = project else { continue };
        let Ok(entries) = std::fs::read_dir(project.path()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
                continue;
            };
            if best.as_ref().is_none_or(|(seen, _)| modified > *seen) {
                best = Some((modified, path));
            }
        }
    }
    best.map(|(_, path)| path)
}

#[test]
fn probe_live_file() {
    let path = std::path::PathBuf::from(
        "/Users/alex/.claude/projects/-private-tmp-shep-live-rename/6b236fa6-c09f-455f-a408-b36a75ec08a5.jsonl",
    );
    if !path.exists() {
        return;
    }
    let facts = super::read("claude", &path);
    panic!("{facts:?}");
}
