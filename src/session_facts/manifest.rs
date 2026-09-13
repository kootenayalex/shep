//! Per-agent configuration describing where an agent records facts about its
//! own session, and which keys in that file mean what.
//!
//! This is deliberately a *separate* registry from `src/detect/manifests/`.
//! Detection reads a screen snapshot and nothing else (see the "detection is
//! decoupled" principle in `CLAUDE.md`); a file reader has no business inside
//! `DetectionInput`. The two share only a shape: bundled TOML is the source of
//! truth, and a file under `<config>/session-facts/<agent>.toml` shadows it for
//! local experiments.
//!
//! An agent with no manifest is not an error. It simply has no facts to read,
//! and every surface falls back to what it did before. Teaching shep about a
//! new agent's session store is a TOML file, not a code change.

use std::collections::HashMap;
use std::sync::{OnceLock, RwLock};

use serde::Deserialize;

/// Bundled manifests, keyed by the agent label `detect` uses.
const BUNDLED: &[(&str, &str)] = &[("claude", include_str!("manifests/claude.toml"))];

/// A generous default: the records cluster at the end of the file, but one
/// line can be a large tool result, so a few hundred kilobytes of tail is the
/// difference between "reliably found" and "sometimes missing".
const DEFAULT_TAIL_BYTES: u64 = 256 * 1024;

/// An upper bound that holds even if a manifest asks for more. The read happens
/// off a live, growing file on every sample tick; it must stay cheap.
const MAX_TAIL_BYTES: u64 = 4 * 1024 * 1024;

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SessionFactsManifest {
    #[allow(dead_code)] // Parsed for symmetry with detection manifests and to
    // catch a copy-pasted file whose id disagrees with its name.
    pub id: String,
    pub format: ManifestFormat,
    /// The JSON key whose value names the kind of record on a line.
    pub discriminator: String,
    #[serde(default)]
    tail_bytes: Option<u64>,
    /// In precedence order: the first block that resolves a field owns it.
    #[serde(default)]
    pub facts: Vec<FactBlock>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub(crate) enum ManifestFormat {
    /// One JSON object per line, appended; the last record of a kind wins.
    JsonlTail,
}

/// One record kind, and which of its keys carry which facts.
///
/// Every field is optional because a record kind usually answers exactly one
/// question — `ai-title` says nothing about cost, and asking it to would be a
/// misconfiguration rather than a missing value.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FactBlock {
    /// The `discriminator` value that selects this block.
    pub record: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub permission_mode: Option<String>,
    #[serde(default)]
    pub cost_usd: Option<String>,
    #[serde(default)]
    pub lines_added: Option<String>,
    #[serde(default)]
    pub lines_removed: Option<String>,
}

impl SessionFactsManifest {
    /// How many bytes off the end of the file to read, clamped so a typo in a
    /// local override cannot ask shep to slurp a gigabyte every few seconds.
    pub(crate) fn tail_bytes(&self) -> u64 {
        self.tail_bytes
            .unwrap_or(DEFAULT_TAIL_BYTES)
            .clamp(1, MAX_TAIL_BYTES)
    }

    /// The record kinds this manifest cares about. Every other line in the
    /// file is skipped without being parsed as JSON at all.
    pub(crate) fn wanted_records(&self) -> Vec<&str> {
        self.facts
            .iter()
            .map(|block| block.record.as_str())
            .collect()
    }
}

static CACHE: OnceLock<RwLock<HashMap<String, Option<SessionFactsManifest>>>> = OnceLock::new();

fn cache() -> &'static RwLock<HashMap<String, Option<SessionFactsManifest>>> {
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// The manifest for `agent`, or `None` when shep has not been taught about that
/// agent's session store.
///
/// A malformed manifest is a `None` with a warning, never a panic and never a
/// hard error: these facts are a display hint, and a bad local override should
/// cost the user a line on a card, not their session.
pub(crate) fn manifest_for(agent: &str) -> Option<SessionFactsManifest> {
    if let Ok(guard) = cache().read() {
        if let Some(hit) = guard.get(agent) {
            return hit.clone();
        }
    }
    let loaded = load_uncached(agent);
    if let Ok(mut guard) = cache().write() {
        guard.insert(agent.to_string(), loaded.clone());
    }
    loaded
}

/// Drop the cache so a local override is picked up without a restart, matching
/// what `reload_manifests` does for detection.
pub(crate) fn reload() {
    if let Ok(mut guard) = cache().write() {
        guard.clear();
    }
}

fn load_uncached(agent: &str) -> Option<SessionFactsManifest> {
    if let Some(path) = override_path(agent) {
        if let Ok(text) = std::fs::read_to_string(&path) {
            match parse(&text) {
                Ok(manifest) => return Some(manifest),
                Err(err) => {
                    tracing::warn!(
                        agent,
                        path = %path.display(),
                        %err,
                        "ignoring malformed session-facts override"
                    );
                }
            }
        }
    }
    let bundled = BUNDLED
        .iter()
        .find(|(id, _)| *id == agent)
        .map(|(_, text)| *text)?;
    match parse(bundled) {
        Ok(manifest) => Some(manifest),
        Err(err) => {
            // A bundled manifest that does not parse is a build-time mistake,
            // and there is a test below that fails on exactly this.
            tracing::error!(agent, %err, "bundled session-facts manifest is malformed");
            None
        }
    }
}

pub(crate) fn parse(text: &str) -> Result<SessionFactsManifest, String> {
    let manifest: SessionFactsManifest = toml::from_str(text).map_err(|err| err.to_string())?;
    if manifest.discriminator.is_empty() {
        return Err("discriminator must name a JSON key".to_string());
    }
    Ok(manifest)
}

fn override_path(agent: &str) -> Option<std::path::PathBuf> {
    Some(
        crate::config::config_dir()
            .join("session-facts")
            .join(format!("{agent}.toml")),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_bundled_manifest_parses() {
        for (agent, text) in BUNDLED {
            let manifest = parse(text)
                .unwrap_or_else(|err| panic!("bundled manifest for {agent} is malformed: {err}"));
            assert_eq!(
                &manifest.id, agent,
                "bundled manifest filed under {agent} calls itself {}",
                manifest.id
            );
            assert!(
                !manifest.facts.is_empty(),
                "{agent} manifest reads no facts, so it does nothing"
            );
        }
    }

    #[test]
    fn claude_reads_the_records_we_verified_on_disk() {
        let manifest = parse(BUNDLED[0].1).expect("parses");
        let records = manifest.wanted_records();
        for expected in [
            "custom-title",
            "ai-title",
            "agent-name",
            "permission-mode",
            "cost-state",
        ] {
            assert!(
                records.contains(&expected),
                "claude manifest stopped reading {expected}"
            );
        }
        // Precedence is positional, so a reordering that lets Claude's own
        // title beat the user's is a silent regression.
        let custom = records.iter().position(|r| *r == "custom-title");
        let ai = records.iter().position(|r| *r == "ai-title");
        assert!(custom < ai, "a user's title must outrank the generated one");
    }

    #[test]
    fn an_unknown_agent_has_no_manifest() {
        assert!(manifest_for("no-such-agent").is_none());
    }

    #[test]
    fn a_wild_tail_size_is_clamped() {
        let manifest = parse(
            r#"
id = "x"
format = "jsonl-tail"
discriminator = "type"
tail_bytes = 999999999999
[[facts]]
record = "r"
title = "t"
"#,
        )
        .expect("parses");
        assert_eq!(manifest.tail_bytes(), MAX_TAIL_BYTES);
    }

    #[test]
    fn a_manifest_without_a_discriminator_is_refused() {
        let err = parse(
            r#"
id = "x"
format = "jsonl-tail"
discriminator = ""
"#,
        )
        .expect_err("empty discriminator");
        assert!(err.contains("discriminator"), "unhelpful error: {err}");
    }
}
