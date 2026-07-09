//! Claude-code Stop-hook reflection: force exactly one memory-review turn when a
//! session tries to finish. Ported from damon-ade's `reflect-on-stop.mjs`
//! (`agent-scaffold.ts`), adapted to shep's `shep memory` CLI.
//!
//! The hook reads the claude-code Stop-hook JSON payload from stdin and, unless
//! it is already inside the injected reflection turn (`stop_hook_active`), emits
//! `{"decision":"block","reason":"<prompt>"}` so claude feeds the prompt back for
//! one review turn. The `stop_hook_active` guard means that review turn itself
//! stops cleanly instead of looping.
//!
//! FAIL-OPEN: any stdin parse error, missing field, or internal problem yields
//! [`Decision::Passthrough`] — the hook prints nothing and exits 0. A bug in
//! shep must never wedge a user's claude session.

use serde_json::Value;

/// The reflection prompt claude receives as the `reason` of the block decision.
/// Adapted from damon-ade's reflect prompt: points the agent at `shep memory`
/// rather than raw file edits, and carries an explicit do-NOT-capture list.
pub(crate) const REFLECTION_PROMPT: &str = "[session reflection] Before you finish, \
review this conversation for durable, memory-worthy facts and record them with the `shep memory` \
CLI. Save stable preferences, corrections, and personal details about the user with \
`shep memory add --user \"<fact>\"`; save stable environment, convention, build/test, and tooling \
facts about this repo with `shep memory add \"<fact>\"` (repo memory is the default target). Use \
`shep memory replace \"<old>\" \"<new>\"` to update an existing entry and \
`shep memory remove \"<substring>\"` to drop a stale one. Run `shep memory status` to check usage \
first, and consolidate (merge overlapping, drop the stalest) rather than append when a file is near \
its cap — shep errors on overflow instead of auto-compacting. One fact per entry, present tense, \
absolute dates. Do NOT capture: secrets, tokens, or credentials; transient task state, progress \
logs, or one-off task narratives; environment-dependent failures (missing binaries, \
\"command not found\", unconfigured credentials); negative claims about tools (record the fix \
instead); or anything trivially re-derivable from the repo itself. If nothing durable came up, make \
no changes and finish.";

/// What the Stop hook should do for a given payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Print nothing, exit 0 — either already inside the reflection turn, or a
    /// fail-open on malformed input.
    Passthrough,
    /// Emit the block decision so claude runs one reflection turn.
    Block,
}

/// Pure decision logic over the raw stdin payload. Never panics; any parse
/// failure fails open to [`Decision::Passthrough`].
pub(crate) fn decide(raw: &str) -> Decision {
    // A payload we cannot parse must not block the user's session.
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Decision::Passthrough;
    };
    // Already inside the reflection turn we injected: let it stop (no loop).
    if value
        .get("stop_hook_active")
        .and_then(Value::as_bool)
        .unwrap_or(false)
    {
        return Decision::Passthrough;
    }
    Decision::Block
}

/// Render the stdout line for a [`Decision`], if any. `None` means print nothing.
pub(crate) fn output_for(decision: Decision) -> Option<String> {
    match decision {
        Decision::Passthrough => None,
        Decision::Block => Some(
            serde_json::json!({
                "decision": "block",
                "reason": REFLECTION_PROMPT,
            })
            .to_string(),
        ),
    }
}

/// Read the Stop-hook payload from stdin and print the block decision when a
/// reflection turn is warranted. Always exits 0 (fail-open): a read error, parse
/// error, or serialization glitch prints nothing rather than blocking claude.
pub(crate) fn run_reflect_hook() -> i32 {
    use std::io::Read;
    let mut raw = String::new();
    // A stdin read error is fail-open too: print nothing, exit 0.
    if std::io::stdin().read_to_string(&mut raw).is_err() {
        return 0;
    }
    if let Some(line) = output_for(decide(&raw)) {
        println!("{line}");
    }
    0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocks_a_normal_stop_payload() {
        let raw = r#"{"session_id":"abc","stop_hook_active":false}"#;
        assert_eq!(decide(raw), Decision::Block);
    }

    #[test]
    fn missing_stop_hook_active_defaults_to_block() {
        // A first-turn Stop payload may omit the field entirely.
        let raw = r#"{"session_id":"abc"}"#;
        assert_eq!(decide(raw), Decision::Block);
    }

    #[test]
    fn passes_through_when_already_reflecting() {
        let raw = r#"{"session_id":"abc","stop_hook_active":true}"#;
        assert_eq!(decide(raw), Decision::Passthrough);
    }

    #[test]
    fn fails_open_on_unparseable_input() {
        assert_eq!(decide("not json at all"), Decision::Passthrough);
        assert_eq!(decide(""), Decision::Passthrough);
    }

    #[test]
    fn fails_open_when_stop_hook_active_is_wrong_type() {
        // A non-bool value must not be treated as true; but it also must not
        // crash — unwrap_or(false) means we still block, which is safe.
        let raw = r#"{"stop_hook_active":"yes"}"#;
        assert_eq!(decide(raw), Decision::Block);
    }

    #[test]
    fn block_output_is_exact_decision_json() {
        let out = output_for(Decision::Block).expect("block emits output");
        let value: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(value["decision"], "block");
        assert_eq!(value["reason"], REFLECTION_PROMPT);
        // Exactly two keys, no stray fields.
        assert_eq!(value.as_object().unwrap().len(), 2);
    }

    #[test]
    fn passthrough_emits_no_output() {
        assert_eq!(output_for(Decision::Passthrough), None);
    }

    #[test]
    fn reflection_prompt_names_the_cli_and_forbids_secrets() {
        assert!(REFLECTION_PROMPT.contains("shep memory add"));
        assert!(REFLECTION_PROMPT.contains("shep memory status"));
        assert!(REFLECTION_PROMPT.contains("secrets"));
    }
}
