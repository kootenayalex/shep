//! What the overseer plugin has written down, sampled into state so the chrome
//! can read it without touching a file.
//!
//! The plugin's tick leaves four files in its state dir: `situation.json`
//! (the facts it sensed, with a timestamp and `shep doctor`'s findings),
//! `BOARD.md` (its narrative), `BOARD.md.source` (`brain` or
//! `deterministic`) and `last-brain` (an empty file whose mtime is the last
//! time a brain answered). [`OverseerSample`] mirrors them the way
//! `DocketSample` mirrors the docket: a TTL gates the whole sample, and inside
//! it a per-file mtime guard means a quiet dir costs three `stat`s and no reads.
//!
//! Nothing here creates the dir or any file. A missing dir is simply an
//! overseer that has not spoken yet.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

use super::state::DocketSample;

/// How stale the sample may get before the dir is stat'ed again. The
/// dashboard's interval: the overseer ticks on agent events, not per frame.
pub(crate) const OVERSEER_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// The most proposals the board lists, newest first.
pub(crate) const MAX_PROPOSALS: usize = 5;

const SITUATION_FILE: &str = "situation.json";
const BOARD_FILE: &str = "BOARD.md";
const BOARD_SOURCE_FILE: &str = "BOARD.md.source";
const BRAIN_STAMP_FILE: &str = "last-brain";

/// `shep doctor`'s verdict on one check, as the plugin copied it into
/// `situation.json`. The lowercase wire spelling matches `shep doctor --json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum HealthLevel {
    Ok,
    Warn,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub(crate) struct HealthFinding {
    pub level: HealthLevel,
    pub check: String,
    #[serde(default)]
    pub detail: String,
    #[serde(default)]
    pub fix: Option<String>,
}

/// Who wrote the narrative: a brain runtime, or the tick's own template.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum NarrativeSource {
    Brain,
    #[default]
    Deterministic,
}

/// The overseer's files, mirrored. Every field is optional: a dir that does
/// not exist yet renders as no strip and an empty health list, never as an
/// error.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct OverseerSample {
    /// `BOARD.md` whole, when there is one.
    pub narrative: Option<String>,
    pub source: NarrativeSource,
    /// `hh:mm` sliced out of the situation's `at`, for the strip's right edge.
    pub tick_at: Option<String>,
    pub health: Vec<HealthFinding>,
    pub situation_mtime: Option<SystemTime>,
    pub board_mtime: Option<SystemTime>,
    pub brain_mtime: Option<SystemTime>,
    /// `true` once the dir has been looked at, whatever was found.
    pub sampled: bool,
    pub sampled_at: Option<Instant>,
}

/// Only what the strip and the board need from `situation.json`; everything
/// else the plugin writes is its own business, so unknown keys are ignored
/// and a missing key is a default rather than a parse failure.
#[derive(Debug, Default, Deserialize)]
struct SituationFile {
    #[serde(default)]
    at: String,
    /// Raw values, so one malformed finding drops itself rather than the list.
    #[serde(default)]
    health: Vec<serde_json::Value>,
}

fn mtime(path: &Path) -> Option<SystemTime> {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
}

/// `2026-09-12 10:22 PDT` -> `10:22`. The first `hh:mm` token, wherever the
/// plugin put it; `None` when there is none.
fn clock_token(at: &str) -> Option<String> {
    at.split_whitespace()
        .map(|token| token.trim_end_matches(|c: char| !c.is_ascii_digit()))
        .find(|token| {
            let bytes = token.as_bytes();
            bytes.len() == 5
                && bytes[2] == b':'
                && bytes[..2].iter().all(u8::is_ascii_digit)
                && bytes[3..].iter().all(u8::is_ascii_digit)
        })
        .map(str::to_string)
}

impl OverseerSample {
    /// Look at the dir again if the previous sample has aged out, re-reading
    /// only the files whose mtime moved. Returns whether anything changed.
    pub fn refresh_if_stale(&mut self, now: Instant, dir: &Path) -> bool {
        if self
            .sampled_at
            .is_some_and(|at| now.saturating_duration_since(at) < OVERSEER_SAMPLE_INTERVAL)
        {
            return false;
        }
        self.sample(now, dir)
    }

    /// Look at the dir now, whatever the sample's age.
    // Read by the board's live open, which lands with the overseer view.
    #[allow(dead_code)]
    pub fn refresh(&mut self, now: Instant, dir: &Path) -> bool {
        self.sample(now, dir)
    }

    fn sample(&mut self, now: Instant, dir: &Path) -> bool {
        self.sampled_at = Some(now);
        self.sampled = true;
        let mut changed = false;

        let situation = mtime(&dir.join(SITUATION_FILE));
        if situation != self.situation_mtime {
            self.situation_mtime = situation;
            self.read_situation(&dir.join(SITUATION_FILE));
            changed = true;
        }

        let board = mtime(&dir.join(BOARD_FILE));
        if board != self.board_mtime {
            self.board_mtime = board;
            self.read_board(dir);
            changed = true;
        }

        let brain = mtime(&dir.join(BRAIN_STAMP_FILE));
        if brain != self.brain_mtime {
            self.brain_mtime = brain;
            changed = true;
        }

        changed
    }

    fn read_situation(&mut self, path: &Path) {
        let file = match std::fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str::<SituationFile>(&text) {
                Ok(file) => file,
                Err(err) => {
                    tracing::warn!(error = %err, path = %path.display(), "overseer situation unreadable");
                    SituationFile::default()
                }
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => SituationFile::default(),
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "overseer situation unreadable");
                SituationFile::default()
            }
        };
        self.tick_at = clock_token(&file.at);
        self.health = file
            .health
            .into_iter()
            .filter_map(|value| serde_json::from_value::<HealthFinding>(value).ok())
            .collect();
    }

    fn read_board(&mut self, dir: &Path) {
        self.narrative = std::fs::read_to_string(dir.join(BOARD_FILE))
            .ok()
            .map(|text| text.trim_end().to_string())
            .filter(|text| !text.is_empty());
        self.source = match std::fs::read_to_string(dir.join(BOARD_SOURCE_FILE)) {
            Ok(text) if text.trim() == "brain" => NarrativeSource::Brain,
            _ => NarrativeSource::Deterministic,
        };
    }

    /// The narrative's opening sentence — what the strip has room for.
    ///
    /// Skips a header line (`# BOARD — …` from the tick's template, or
    /// `OVERSEER · …`) and stops at the first sentence end or line break.
    pub fn first_sentence(&self) -> Option<String> {
        let text = self.narrative.as_deref()?;
        let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
        let mut line = lines.next()?;
        if line.starts_with("OVERSEER ·") || line.starts_with("# ") {
            line = lines.next()?;
        }
        let end = [". ", "! ", "? "]
            .iter()
            .filter_map(|mark| line.find(mark))
            .min()
            .map(|at| at + 1)
            .unwrap_or(line.len());
        let sentence = line[..end].trim();
        (!sentence.is_empty()).then(|| sentence.to_string())
    }

    /// How long since a brain last answered, when one ever has.
    // Read by the board header, which lands with the overseer view.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn brain_age(&self, now: SystemTime) -> Option<Duration> {
        now.duration_since(self.brain_mtime?).ok()
    }

    /// Whether the situation is older than `secs` — or was never written.
    // Read by the board's live open, which decides whether to ask for a tick.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn situation_older_than(&self, secs: u64) -> bool {
        match self.situation_mtime {
            Some(at) => SystemTime::now()
                .duration_since(at)
                .map(|age| age > Duration::from_secs(secs))
                .unwrap_or(false),
            None => true,
        }
    }

    /// A sample with every surface populated: a brain narrative with two
    /// sentences, a tick time, and a doctor report with two warnings.
    #[cfg(test)]
    pub(crate) fn test_fixture() -> Self {
        fn finding(level: HealthLevel, check: &str, detail: &str) -> HealthFinding {
            HealthFinding {
                level,
                check: check.to_string(),
                detail: detail.to_string(),
                fix: None,
            }
        }
        Self {
            narrative: Some(
                "workmayt's claude has been blocked 2m on a permission prompt. \
                 emberline is done and unseen; two proposals on the board."
                    .to_string(),
            ),
            source: NarrativeSource::Brain,
            tick_at: Some("07:08".to_string()),
            health: vec![
                finding(HealthLevel::Ok, "server", "running"),
                finding(HealthLevel::Ok, "bridge", "listening"),
                finding(HealthLevel::Ok, "hooks", "installed"),
                finding(HealthLevel::Ok, "push", "configured"),
                finding(HealthLevel::Warn, "disk", "9.8 G free"),
                finding(HealthLevel::Warn, "err-log", "written 23h ago"),
            ],
            situation_mtime: None,
            board_mtime: None,
            brain_mtime: None,
            sampled: true,
            sampled_at: Some(Instant::now()),
        }
    }
}

/// The sample plus where it comes from. The dir is a field so a test can
/// point it at scratch space, the way `docket_db` does.
#[derive(Debug, Clone)]
pub(crate) struct OverseerState {
    pub sample: OverseerSample,
    pub state_dir: PathBuf,
}

impl OverseerState {
    pub(crate) fn new(state_dir: PathBuf) -> Self {
        Self {
            sample: OverseerSample::default(),
            state_dir,
        }
    }
}

/// One inbox item the overseer proposed, as the board lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProposalCard {
    pub id: i64,
    pub title: String,
    /// The first line of the notes, when there are any.
    pub notes_line: Option<String>,
    /// What the proposal points at (`source.ref`), when it says.
    pub reference: Option<String>,
    /// ISO 8601 UTC, as the store returns it.
    pub updated: String,
}

fn is_situation_sourced(source: Option<&serde_json::Value>) -> bool {
    source
        .and_then(|value| value.get("kind"))
        .and_then(serde_json::Value::as_str)
        == Some("situation")
}

/// The proposals waiting on the board: inbox items the overseer captured
/// (`source.kind == "situation"`), newest first, at most [`MAX_PROPOSALS`].
pub(crate) fn proposals(sample: &DocketSample) -> Vec<ProposalCard> {
    use crate::api::schema::DocketStatus;
    let mut cards: Vec<ProposalCard> = sample
        .rows
        .iter()
        .filter(|row| {
            row.status == DocketStatus::Inbox && is_situation_sourced(row.source.as_ref())
        })
        .map(|row| ProposalCard {
            id: row.id,
            title: row.title.clone(),
            notes_line: row
                .notes
                .as_deref()
                .and_then(|notes| notes.lines().next())
                .map(str::trim)
                .filter(|line| !line.is_empty())
                .map(str::to_string),
            reference: row
                .source
                .as_ref()
                .and_then(|value| value.get("ref"))
                .and_then(serde_json::Value::as_str)
                .map(str::to_string),
            updated: row.updated.clone(),
        })
        .collect();
    cards.sort_by(|a, b| b.updated.cmp(&a.updated).then(b.id.cmp(&a.id)));
    cards.truncate(MAX_PROPOSALS);
    cards
}

/// A state dir no test shares with another or with the real one. Nothing is
/// created until a test writes into it.
#[cfg(test)]
pub(crate) fn test_state_dir() -> PathBuf {
    static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "shep-test-overseer-{}-{seq}-{nanos}",
        std::process::id()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{DocketItem, DocketKind, DocketStatus};

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::create_dir_all(dir).expect("scratch dir");
        std::fs::write(dir.join(name), text).expect("scratch file");
    }

    #[test]
    fn refresh_reads_narrative_health_and_tick() {
        let dir = test_state_dir();
        write(
            &dir,
            "situation.json",
            r#"{"at":"2026-09-12 10:22 PDT","health":[
                {"level":"ok","check":"server","detail":"running v0.7.3"},
                {"level":"warn","check":"disk","detail":"9.8 G free","fix":"free some"},
                {"level":"bogus","check":"x","detail":"dropped"}
            ],"agents":[{"name":"ignored"}]}"#,
        );
        write(
            &dir,
            "BOARD.md",
            "# BOARD — 2026-09-12 10:22 PDT\nclaude is blocked. emberline is done.\n",
        );
        write(&dir, "BOARD.md.source", "brain\n");
        write(&dir, "last-brain", "");

        let mut sample = OverseerSample::default();
        assert!(sample.refresh_if_stale(Instant::now(), &dir));
        assert_eq!(sample.tick_at.as_deref(), Some("10:22"));
        assert_eq!(sample.health.len(), 2);
        assert_eq!(sample.health[1].level, HealthLevel::Warn);
        assert_eq!(sample.health[1].fix.as_deref(), Some("free some"));
        assert_eq!(sample.source, NarrativeSource::Brain);
        assert_eq!(
            sample.first_sentence().as_deref(),
            Some("claude is blocked.")
        );
        assert!(sample.brain_age(SystemTime::now()).is_some());
        assert!(!sample.situation_older_than(60));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn refresh_is_a_stat_when_nothing_moved() {
        let dir = test_state_dir();
        write(&dir, "BOARD.md", "one line.\n");
        let mut sample = OverseerSample::default();
        let t0 = Instant::now();
        assert!(sample.refresh_if_stale(t0, &dir));
        // Inside the TTL: nothing is even stat'ed.
        assert!(!sample.refresh_if_stale(t0 + Duration::from_millis(500), &dir));
        // An unconditional look still reports nothing moved.
        assert!(!sample.refresh(t0 + Duration::from_millis(600), &dir));
        // Past it, with nothing moved: a stat, no change.
        assert!(!sample.refresh_if_stale(t0 + Duration::from_secs(3), &dir));
        // Rewrite the narrative and bump the mtime past the filesystem's
        // granularity so the guard can see it.
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        write(&dir, "BOARD.md", "another line.\n");
        std::fs::File::open(dir.join("BOARD.md"))
            .and_then(|file| file.set_modified(later))
            .expect("set mtime");
        assert!(sample.refresh_if_stale(t0 + Duration::from_secs(6), &dir));
        assert_eq!(sample.first_sentence().as_deref(), Some("another line."));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dir_is_a_silent_empty_sample() {
        let dir = test_state_dir();
        let mut sample = OverseerSample::default();
        // Nothing there is nothing to redraw, first look or fiftieth.
        assert!(!sample.refresh_if_stale(Instant::now(), &dir));
        assert!(sample.sampled);
        assert_eq!(sample.narrative, None);
        assert!(sample.health.is_empty());
        assert!(sample.situation_older_than(0));
        assert!(!dir.exists(), "sampling must not create the state dir");
    }

    #[test]
    fn first_sentence_skips_the_header() {
        let mut sample = OverseerSample::default();
        for (text, want) in [
            (
                "# BOARD — now\nfirst thing. second thing.",
                Some("first thing."),
            ),
            (
                "OVERSEER · 07:08\n\nall quiet! nothing owed",
                Some("all quiet!"),
            ),
            ("no header here\nsecond line", Some("no header here")),
            (
                "one sentence with no stop",
                Some("one sentence with no stop"),
            ),
            ("# only a header", None),
            ("", None),
        ] {
            sample.narrative = (!text.is_empty()).then(|| text.to_string());
            assert_eq!(sample.first_sentence().as_deref(), want, "{text:?}");
        }
    }

    #[test]
    fn clock_token_finds_hh_mm() {
        assert_eq!(
            clock_token("2026-09-12 10:22 PDT").as_deref(),
            Some("10:22")
        );
        assert_eq!(clock_token("07:08").as_deref(), Some("07:08"));
        assert_eq!(clock_token("2026-09-12T07:08:09Z"), None);
        assert_eq!(clock_token(""), None);
    }

    fn inbox(id: i64, updated: &str, source: Option<serde_json::Value>) -> DocketItem {
        DocketItem {
            id,
            title: format!("item {id}"),
            kind: DocketKind::Captured,
            status: DocketStatus::Inbox,
            due: None,
            repeat: None,
            source,
            notes: Some(format!("note {id}\nmore")),
            created: updated.to_string(),
            updated: updated.to_string(),
            last_fired: None,
            overdue: false,
        }
    }

    #[test]
    fn proposals_are_situation_sourced_newest_first_max_five() {
        let situation = |r: &str| Some(serde_json::json!({"kind": "situation", "ref": r}));
        let mut rows = vec![
            inbox(1, "2026-09-10T10:00:00Z", situation("pane p1")),
            inbox(2, "2026-09-12T10:00:00Z", situation("pane p2")),
            inbox(
                3,
                "2026-09-11T10:00:00Z",
                Some(serde_json::json!({"pane": "p3"})),
            ),
            inbox(4, "2026-09-11T10:00:00Z", None),
            inbox(5, "2026-09-11T12:00:00Z", situation("pane p5")),
            inbox(6, "2026-09-11T11:00:00Z", situation("pane p6")),
            inbox(7, "2026-09-11T09:00:00Z", situation("pane p7")),
            inbox(8, "2026-09-11T08:00:00Z", situation("pane p8")),
        ];
        let mut done = inbox(9, "2026-09-13T00:00:00Z", situation("pane p9"));
        done.status = DocketStatus::Done;
        rows.push(done);
        let sample = DocketSample {
            rows,
            today: None,
            sampled: true,
            sampled_at: None,
        };
        let cards = proposals(&sample);
        assert_eq!(
            cards.iter().map(|c| c.id).collect::<Vec<_>>(),
            vec![2, 5, 6, 7, 8]
        );
        assert_eq!(cards[0].reference.as_deref(), Some("pane p2"));
        assert_eq!(cards[0].notes_line.as_deref(), Some("note 2"));
    }
}
