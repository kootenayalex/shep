//! Per-agent manifests describing where an agent writes facts about its own
//! session, and how to read them.
//!
//! Deliberately separate from `src/detect/manifests/`: detection reads a screen
//! snapshot and nothing else, so a file reader must never enter
//! `DetectionInput`. These manifests only ever produce display hints.

use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{OnceLock, RwLock},
};

use serde::Deserialize;

/// Bundled manifests, by agent label. An agent that is absent here degrades to
/// the pre-session-facts behaviour; adding one later is a TOML file, not code.
const BUNDLED_MANIFESTS: &[(&str, &str)] = &[("claude", include_str!("manifests/claude.toml"))];

/// The one format implemented today: a JSON-lines file, scanned backwards, with
/// the last record of each type winning.
pub(crate) const FORMAT_JSONL_TAIL: &str = "jsonl-tail";

fn default_tail_bytes() -> u64 {
    1_048_576
}

fn default_discriminator() -> String {
    "type".to_string()
}

#[derive(Debug, Clone, Deserialize)]
pub(crate) struct SessionFactsManifest {
    pub(crate) id: String,
    pub(crate) format: String,
    /// How far back from the end of the file to read. A record older than this
    /// counts as absent — a bounded read is what keeps a 1.7 MB transcript off
    /// the sampling path.
    #[serde(default = "default_tail_bytes")]
    pub(crate) tail_bytes: u64,
    /// The JSON field naming a record's kind.
    #[serde(default = "default_discriminator")]
    pub(crate) discriminator: String,
    /// Block order is precedence: the first block that resolves a field wins.
    #[serde(default)]
    pub(crate) facts: Vec<FactRule>,
}

/// One record type, and which of its JSON fields carry which facts.
///
/// A field name is a dotted path, not just a key: `message.content.text`
/// descends `message`, then `content`, and — because `content` is an array —
/// takes the first element that has a `text` of its own. That is what lets a
/// fact live inside a block list without teaching this module the shape of any
/// particular agent's messages.
#[derive(Debug, Clone, Deserialize)]
pub(crate) struct FactRule {
    pub(crate) record: String,
    pub(crate) title: Option<String>,
    /// What the agent last said it was doing, in its own words.
    pub(crate) summary: Option<String>,
    pub(crate) name: Option<String>,
    pub(crate) permission_mode: Option<String>,
    pub(crate) cost_usd: Option<String>,
    pub(crate) lines_added: Option<String>,
    pub(crate) lines_removed: Option<String>,
}

pub(crate) fn parse_manifest(text: &str) -> Result<SessionFactsManifest, String> {
    let manifest: SessionFactsManifest = toml::from_str(text).map_err(|err| err.to_string())?;
    if manifest.format != FORMAT_JSONL_TAIL {
        return Err(format!("unsupported format {}", manifest.format));
    }
    Ok(manifest)
}

fn override_path(agent: &str) -> PathBuf {
    crate::config::config_dir()
        .join("session-facts")
        .join(format!("{agent}.toml"))
}

fn load_uncached(agent: &str) -> Option<SessionFactsManifest> {
    let path = override_path(agent);
    if let Ok(text) = std::fs::read_to_string(&path) {
        // A bad override falls back to the bundled manifest rather than to
        // nothing: a typo must not silently blank the board.
        if let Ok(manifest) = parse_manifest(&text) {
            if manifest.id == agent {
                return Some(manifest);
            }
        }
    }
    let bundled = BUNDLED_MANIFESTS
        .iter()
        .find(|(label, _)| *label == agent)
        .map(|(_, text)| *text)?;
    parse_manifest(bundled).ok()
}

type Cache = RwLock<HashMap<String, Option<SessionFactsManifest>>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

/// The manifest for an agent label, or `None` when that agent has none.
pub(crate) fn manifest_for(agent: &str) -> Option<SessionFactsManifest> {
    if let Ok(cache) = cache().read() {
        if let Some(entry) = cache.get(agent) {
            return entry.clone();
        }
    }
    let loaded = load_uncached(agent);
    if let Ok(mut cache) = cache().write() {
        cache.insert(agent.to_string(), loaded.clone());
    }
    loaded
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bundled_claude_manifest_parses() {
        let manifest = manifest_for("claude").expect("claude ships a manifest");
        assert_eq!(manifest.id, "claude");
        assert_eq!(manifest.format, FORMAT_JSONL_TAIL);
        assert_eq!(manifest.discriminator, "type");
        assert_eq!(manifest.facts.len(), 6);
        // Precedence: a hand-set title outranks the generated one.
        assert_eq!(manifest.facts[0].record, "custom-title");
        assert_eq!(manifest.facts[1].record, "ai-title");
        // The narration reads out of the message's content blocks, so its path
        // has to survive the round trip through TOML as a path, not a key.
        let assistant = manifest
            .facts
            .iter()
            .find(|rule| rule.record == "assistant")
            .expect("claude publishes what it is doing");
        assert_eq!(assistant.summary.as_deref(), Some("message.content.text"));
    }

    #[test]
    fn an_agent_without_a_manifest_has_none() {
        assert!(manifest_for("codex").is_none());
    }

    #[test]
    fn an_unsupported_format_is_rejected() {
        let err = parse_manifest("id = \"x\"\nformat = \"sqlite\"\n").unwrap_err();
        assert!(err.contains("unsupported format"), "{err}");
    }
}
