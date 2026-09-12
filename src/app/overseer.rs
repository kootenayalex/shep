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
//! The board's chat with the overseer's headless runtime lives here too: a
//! `chat.jsonl` in the same dir (one [`ChatTurn`] per line, mirrored under
//! the same mtime guard), the prompt that carries the plugin's hard rules
//! and the last few turns, and the send path that spawns the runtime on a
//! thread and reports back through [`AppEvent::OverseerChatFinished`].
//!
//! Nothing on the read side creates the dir or any file. A missing dir is
//! simply an overseer that has not spoken yet; only a sent question writes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use super::state::{AppState, DocketSample};
use super::App;
use crate::events::AppEvent;

/// How stale the sample may get before the dir is stat'ed again. The
/// dashboard's interval: the overseer ticks on agent events, not per frame.
pub(crate) const OVERSEER_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// The most proposals the board lists, newest first.
pub(crate) const MAX_PROPOSALS: usize = 5;

const SITUATION_FILE: &str = "situation.json";
const SITUATION_MD_FILE: &str = "situation.md";
const CHAT_FILE: &str = "chat.jsonl";
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
    /// Whether the previous sample is still fresh enough to skip the stats.
    /// [`OverseerState::refresh_if_stale`] is the gate that uses it.
    pub fn within_ttl(&self, now: Instant) -> bool {
        self.sampled_at
            .is_some_and(|at| now.saturating_duration_since(at) < OVERSEER_SAMPLE_INTERVAL)
    }

    /// Look at the dir now, whatever the sample's age, re-reading only the
    /// files whose mtime moved. Returns whether anything changed.
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

    /// The narrative as the board reads it: every non-empty line of
    /// `BOARD.md` after the header the tick writes (`OVERSEER · <at> ·
    /// <source>`, or the older `# BOARD — …`), trimmed.
    pub fn narrative_lines(&self) -> Vec<String> {
        let Some(text) = self.narrative.as_deref() else {
            return Vec::new();
        };
        let mut lines = text
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .peekable();
        if lines
            .peek()
            .is_some_and(|line| line.starts_with("OVERSEER ·") || line.starts_with("# "))
        {
            lines.next();
        }
        lines.map(str::to_string).collect()
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
    pub fn brain_age(&self, now: SystemTime) -> Option<Duration> {
        now.duration_since(self.brain_mtime?).ok()
    }

    /// Whether the situation is older than `secs` — or was never written.
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

// ---------------------------------------------------------------------------
// Chat
// ---------------------------------------------------------------------------

/// Who said a chat line. The wire spelling is what `chat.jsonl` holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub(crate) enum ChatRole {
    You,
    Overseer,
}

/// One line of the chat with the overseer, as `chat.jsonl` keeps it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChatTurn {
    /// Unix seconds.
    pub at: u64,
    pub role: ChatRole,
    pub text: String,
}

/// How many lines of `chat.jsonl` are kept in memory: the tail.
pub(crate) const CHAT_KEEP_TURNS: usize = 200;

/// How many previous turns a question carries with it.
pub(crate) const CHAT_CONTEXT_TURNS: usize = 12;

/// How long the runtime gets to answer one question.
pub(crate) const CHAT_TIMEOUT: Duration = Duration::from_secs(120);

/// The board's answer when no runtime is configured; a row, not an error.
pub(crate) const CHAT_NO_RUNTIME: &str = "no headless runtime: set [plugins.overseer] runtime";

/// The rules the plugin's brain prompt opens with, copied here so the chat
/// answers under the same constraints as the narrative. A test holds this
/// text against `plugins/overseer/overseer-tick` so the two cannot drift.
pub(crate) const OVERSEER_HARD_RULES: &str = "\
Hard rules you must respect in what you write:
1. You never answer for an agent. A blocked agent is the person's to answer; you may draft a suggested answer with a reason.
2. You never touch the server, restart anything, or kill anything.
3. You never nudge or queue prompts.
4. Capture proposes, the person disposes: proposals are inbox items only, no dates, no repeats, and only for genuinely owed things visible in the situation.";

/// What the chat asks of the runtime, after the rules.
const CHAT_TASK: &str = "You are the overseer of this shep session; answer in at most 6 short lines, plain prose, no markdown headings.";

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The last [`CHAT_KEEP_TURNS`] lines of `chat.jsonl` in `dir`, oldest
/// first. A line that does not parse is skipped with a warning; a missing
/// file is an empty chat.
pub(crate) fn read_chat(dir: &Path) -> Vec<ChatTurn> {
    let path = dir.join(CHAT_FILE);
    let text = match std::fs::read_to_string(&path) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "overseer chat unreadable");
            return Vec::new();
        }
    };
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let start = lines.len().saturating_sub(CHAT_KEEP_TURNS);
    lines[start..]
        .iter()
        .filter_map(|line| match serde_json::from_str::<ChatTurn>(line) {
            Ok(turn) => Some(turn),
            Err(err) => {
                tracing::warn!(error = %err, path = %path.display(), "overseer chat line skipped");
                None
            }
        })
        .collect()
}

/// Append one turn to `chat.jsonl` in `dir`, creating the dir and the file
/// when they are missing.
pub(crate) fn append_chat(dir: &Path, turn: &ChatTurn) -> std::io::Result<()> {
    use std::io::Write;
    std::fs::create_dir_all(dir)?;
    let mut line = serde_json::to_string(turn).map_err(std::io::Error::other)?;
    line.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(CHAT_FILE))?;
    file.write_all(line.as_bytes())
}

/// The prompt for one question: the rules, the task, the situation the tick
/// last wrote, the last [`CHAT_CONTEXT_TURNS`] turns, and the question.
pub(crate) fn build_chat_prompt(situation_md: &str, turns: &[ChatTurn], question: &str) -> String {
    let mut prompt = String::new();
    prompt.push_str(OVERSEER_HARD_RULES);
    prompt.push_str("\n\n");
    prompt.push_str(CHAT_TASK);
    prompt.push_str("\n\nThe situation, as last sensed:\n");
    let situation = situation_md.trim();
    prompt.push_str(if situation.is_empty() {
        "(the overseer has not sensed the session yet)"
    } else {
        situation
    });
    prompt.push('\n');
    let start = turns.len().saturating_sub(CHAT_CONTEXT_TURNS);
    if start < turns.len() {
        prompt.push_str("\nThe conversation so far:\n");
        for turn in &turns[start..] {
            let who = match turn.role {
                ChatRole::You => "you",
                ChatRole::Overseer => "overseer",
            };
            prompt.push_str(&format!("{who}: {}\n", turn.text.trim()));
        }
    }
    prompt.push_str(&format!("\nyou: {}\noverseer:", question.trim()));
    prompt
}

/// The runtime the chat asks: `[plugins.overseer] runtime`, else
/// `SHEP_OVERSEER_RUNTIME` in the server's environment.
pub(crate) fn overseer_runtime_name(app: &AppState) -> Option<String> {
    app.plugins_config
        .get("overseer")
        .and_then(|table| table.get("runtime"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .or_else(|| {
            crate::env_compat::var("SHEP_OVERSEER_RUNTIME")
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
        })
}

/// The sample plus where it comes from, and the chat beside it. The dir is a
/// field so a test can point it at scratch space, the way `docket_db` does.
#[derive(Debug, Clone)]
pub(crate) struct OverseerState {
    pub sample: OverseerSample,
    pub state_dir: PathBuf,
    /// `chat.jsonl`, mirrored: the tail, oldest first.
    pub chat: Vec<ChatTurn>,
    pub chat_mtime: Option<SystemTime>,
    /// What is typed into the chat and not yet sent.
    pub chat_input: String,
    /// Whether keys go to the chat input rather than the board.
    pub chat_focused: bool,
    /// A question is out with the runtime; its answer is an event away.
    pub chat_pending: bool,
}

impl OverseerState {
    pub(crate) fn new(state_dir: PathBuf) -> Self {
        Self {
            sample: OverseerSample::default(),
            state_dir,
            chat: Vec::new(),
            chat_mtime: None,
            chat_input: String::new(),
            chat_focused: false,
            chat_pending: false,
        }
    }

    /// Look at the dir again if the sample has aged out — the files the
    /// sample mirrors and the chat, one mtime guard each. Returns whether
    /// anything changed.
    pub fn refresh_if_stale(&mut self, now: Instant) -> bool {
        if self.sample.within_ttl(now) {
            return false;
        }
        self.refresh(now)
    }

    /// Look at the dir now, whatever the sample's age.
    pub fn refresh(&mut self, now: Instant) -> bool {
        let dir = self.state_dir.clone();
        let changed = self.sample.refresh(now, &dir);
        self.sample_chat(&dir) || changed
    }

    fn sample_chat(&mut self, dir: &Path) -> bool {
        let stamp = mtime(&dir.join(CHAT_FILE));
        if stamp == self.chat_mtime {
            return false;
        }
        self.chat_mtime = stamp;
        self.chat = read_chat(dir);
        true
    }

    /// Append a turn to the file and the mirror together. The mirror's
    /// stamp follows the file so the next refresh does not re-read what it
    /// already holds; a write that fails is logged and the turn stays on
    /// screen for this session.
    fn record_turn(&mut self, role: ChatRole, text: String) {
        let turn = ChatTurn {
            at: unix_now(),
            role,
            text,
        };
        if let Err(err) = append_chat(&self.state_dir, &turn) {
            tracing::warn!(error = %err, dir = %self.state_dir.display(), "overseer chat not written");
        }
        self.chat_mtime = mtime(&self.state_dir.join(CHAT_FILE));
        self.chat.push(turn);
        if self.chat.len() > CHAT_KEEP_TURNS {
            let drop = self.chat.len() - CHAT_KEEP_TURNS;
            self.chat.drain(..drop);
        }
    }

    /// The chat's fixture: a question, an answer, and a follow-up.
    #[cfg(test)]
    pub(crate) fn test_chat_fixture(now: u64) -> Vec<ChatTurn> {
        vec![
            ChatTurn {
                at: now - 6 * 60,
                role: ChatRole::You,
                text: "what should I do first?".into(),
            },
            ChatTurn {
                at: now - 5 * 60,
                role: ChatRole::Overseer,
                text: "answer workmayt's claude: it wants to run the stripe integration tests, \
                       and the fix branch is waiting on that. then look at emberline's push."
                    .into(),
            },
            ChatTurn {
                at: now - 3 * 60,
                role: ChatRole::You,
                text: "is the disk warning urgent?".into(),
            },
        ]
    }
}

impl App {
    /// Send `question` to the overseer's runtime. The `you` turn lands on
    /// the file and the screen at once; the answer comes back as an
    /// [`AppEvent::OverseerChatFinished`] from a thread that owns nothing of
    /// the app. Without a runtime the overseer answers with the fact.
    pub(crate) fn send_overseer_chat(&mut self, question: String) {
        let question = question.trim().to_string();
        if question.is_empty() {
            return;
        }
        let overseer = &mut self.state.overseer;
        overseer.chat_input.clear();
        overseer.record_turn(ChatRole::You, question.clone());
        overseer.chat_pending = true;
        self.mark_render_dirty();

        let Some(name) = overseer_runtime_name(&self.state) else {
            self.finish_overseer_chat(CHAT_NO_RUNTIME.to_string());
            return;
        };
        let spec = match crate::runtimes::resolve_headless(&name, &self.state.runtimes_config) {
            Ok((spec, _)) => spec,
            Err(err) => {
                tracing::warn!(runtime = %name, error = %err, "overseer chat runtime unresolvable");
                self.finish_overseer_chat(format!("{CHAT_NO_RUNTIME} ({err})"));
                return;
            }
        };
        let state_dir = self.state.overseer.state_dir.clone();
        let situation =
            std::fs::read_to_string(state_dir.join(SITUATION_MD_FILE)).unwrap_or_default();
        // The question is already the last turn; the prompt names it once.
        let turns = &self.state.overseer.chat;
        let earlier = &turns[..turns.len().saturating_sub(1)];
        let prompt = build_chat_prompt(&situation, earlier, &question);
        let event_tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let answer = match crate::runtimes::run_headless_captured(
                &spec,
                &prompt,
                CHAT_TIMEOUT,
                Some(&state_dir),
            ) {
                Ok(capture) if capture.outcome.timed_out => Err(format!(
                    "{name} did not answer within {}s",
                    CHAT_TIMEOUT.as_secs()
                )),
                Ok(capture) if capture.outcome.exit_code != Some(0) => {
                    let tail = capture
                        .stderr
                        .lines()
                        .rev()
                        .find(|line| !line.trim().is_empty())
                        .unwrap_or("")
                        .trim()
                        .to_string();
                    Err(match capture.outcome.exit_code {
                        Some(code) if tail.is_empty() => format!("{name} exited {code}"),
                        Some(code) => format!("{name} exited {code}: {tail}"),
                        None => format!("{name} was killed"),
                    })
                }
                Ok(capture) => {
                    let text = capture.stdout.trim().to_string();
                    if text.is_empty() {
                        Err(format!("{name} said nothing"))
                    } else {
                        Ok(text)
                    }
                }
                Err(err) => Err(format!("{name} could not start: {err}")),
            };
            let _ = event_tx.blocking_send(AppEvent::OverseerChatFinished { answer });
        });
    }

    /// The runtime's answer, or why there is none, becomes the overseer's
    /// turn.
    pub(crate) fn handle_overseer_chat_finished(&mut self, answer: Result<String, String>) {
        let text = match answer {
            Ok(text) => text,
            Err(reason) => format!("the overseer did not answer: {reason}"),
        };
        self.finish_overseer_chat(text);
    }

    fn finish_overseer_chat(&mut self, text: String) {
        let overseer = &mut self.state.overseer;
        overseer.record_turn(ChatRole::Overseer, text);
        overseer.chat_pending = false;
        self.mark_render_dirty();
    }

    fn mark_render_dirty(&self) {
        self.render_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        self.render_notify.notify_one();
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
        assert!(sample.refresh(Instant::now(), &dir));
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
        let mut state = OverseerState::new(dir.clone());
        let t0 = Instant::now();
        assert!(state.refresh_if_stale(t0));
        // Inside the TTL: nothing is even stat'ed.
        assert!(!state.refresh_if_stale(t0 + Duration::from_millis(500)));
        // An unconditional look still reports nothing moved.
        assert!(!state.refresh(t0 + Duration::from_millis(600)));
        // Past it, with nothing moved: a stat, no change.
        assert!(!state.refresh_if_stale(t0 + Duration::from_secs(3)));
        // Rewrite the narrative and bump the mtime past the filesystem's
        // granularity so the guard can see it.
        let later = std::time::SystemTime::now() + Duration::from_secs(5);
        write(&dir, "BOARD.md", "another line.\n");
        std::fs::File::open(dir.join("BOARD.md"))
            .and_then(|file| file.set_modified(later))
            .expect("set mtime");
        assert!(state.refresh_if_stale(t0 + Duration::from_secs(6)));
        assert_eq!(
            state.sample.first_sentence().as_deref(),
            Some("another line.")
        );
        // The chat is under the same guard: a line appended is seen on the
        // next look past the TTL, and only then.
        append_chat(
            &dir,
            &ChatTurn {
                at: 1,
                role: ChatRole::You,
                text: "hi".into(),
            },
        )
        .expect("append");
        assert!(!state.refresh_if_stale(t0 + Duration::from_secs(7)));
        assert!(state.refresh_if_stale(t0 + Duration::from_secs(9)));
        assert_eq!(state.chat.len(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_dir_is_a_silent_empty_sample() {
        let dir = test_state_dir();
        let mut state = OverseerState::new(dir.clone());
        // Nothing there is nothing to redraw, first look or fiftieth.
        assert!(!state.refresh_if_stale(Instant::now()));
        assert!(state.sample.sampled);
        assert_eq!(state.sample.narrative, None);
        assert!(state.sample.health.is_empty());
        assert!(state.chat.is_empty());
        assert!(state.sample.situation_older_than(0));
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
    fn narrative_lines_skip_the_header_and_blank_lines() {
        let mut sample = OverseerSample {
            narrative: Some("OVERSEER · 07:08 · brain\n\nfirst.\n  second.  \n".into()),
            ..Default::default()
        };
        assert_eq!(sample.narrative_lines(), vec!["first.", "second."]);
        sample.narrative = Some("# BOARD — now\nonly.".into());
        assert_eq!(sample.narrative_lines(), vec!["only."]);
        sample.narrative = Some("no header".into());
        assert_eq!(sample.narrative_lines(), vec!["no header"]);
        sample.narrative = None;
        assert!(sample.narrative_lines().is_empty());
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

    #[test]
    fn chat_jsonl_round_trips_and_skips_bad_lines() {
        let dir = test_state_dir();
        assert!(read_chat(&dir).is_empty(), "no file is an empty chat");
        let you = ChatTurn {
            at: 1_700_000_000,
            role: ChatRole::You,
            text: "what first?".into(),
        };
        let overseer = ChatTurn {
            at: 1_700_000_005,
            role: ChatRole::Overseer,
            text: "answer claude.".into(),
        };
        append_chat(&dir, &you).expect("creates the dir and the file");
        append_chat(&dir, &overseer).expect("appends");
        {
            use std::io::Write;
            let mut file = std::fs::OpenOptions::new()
                .append(true)
                .open(dir.join("chat.jsonl"))
                .expect("open");
            writeln!(file, "not json").expect("write");
            writeln!(file).expect("blank line");
        }
        let text = std::fs::read_to_string(dir.join("chat.jsonl")).expect("read");
        assert!(text.starts_with(r#"{"at":1700000000,"role":"you","text":"what first?"}"#));
        assert!(text.contains(r#""role":"overseer""#));
        assert_eq!(read_chat(&dir), vec![you.clone(), overseer.clone()]);

        // Only the tail is kept.
        for i in 0..(CHAT_KEEP_TURNS + 10) {
            append_chat(
                &dir,
                &ChatTurn {
                    at: i as u64,
                    role: ChatRole::You,
                    text: format!("turn {i}"),
                },
            )
            .expect("append");
        }
        let chat = read_chat(&dir);
        assert_eq!(chat.len(), CHAT_KEEP_TURNS);
        assert_eq!(chat[0].text, "turn 10");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_chat_prompt_has_rules_situation_last_12_turns_and_question() {
        let turns: Vec<ChatTurn> = (0..15)
            .map(|i| ChatTurn {
                at: i,
                role: if i % 2 == 0 {
                    ChatRole::You
                } else {
                    ChatRole::Overseer
                },
                text: format!("turn {i}"),
            })
            .collect();
        let prompt = build_chat_prompt("# situation\nclaude is blocked.", &turns, "and now?");
        assert!(prompt.starts_with(OVERSEER_HARD_RULES), "rules lead");
        let rules_end = prompt
            .find("You are the overseer of this shep session")
            .expect("task");
        let situation_at = prompt.find("claude is blocked.").expect("situation");
        let first_turn = prompt
            .find("overseer: turn 3\n")
            .expect("the 12th-newest turn");
        let question_at = prompt.find("you: and now?").expect("question");
        assert!(rules_end < situation_at && situation_at < first_turn && first_turn < question_at);
        assert!(!prompt.contains("turn 2\n"), "older turns are left out");
        assert!(prompt.contains("overseer: turn 13\n"));
        assert!(prompt.contains("you: turn 14\n"));
        assert!(prompt.ends_with("you: and now?\noverseer:"));
        assert!(prompt.contains("at most 6 short lines"));

        // No situation and no turns: the prompt says so rather than going
        // blank, and carries no conversation header.
        let bare = build_chat_prompt("", &[], "hello?");
        assert!(bare.contains("has not sensed the session yet"));
        assert!(!bare.contains("The conversation so far"));
        assert!(bare.ends_with("you: hello?\noverseer:"));
    }

    /// The rules are the plugin's words. Rule 4 in the script carries a
    /// `{max_proposals}` count the chat has no use for, so each rule is
    /// held clause by clause.
    #[test]
    fn hard_rules_match_the_plugin_text() {
        let script = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("plugins/overseer/overseer-tick"),
        )
        .expect("the overseer plugin's tick script");
        let mut rules = 0;
        for line in OVERSEER_HARD_RULES.lines() {
            if line.starts_with(|c: char| c.is_ascii_digit()) {
                rules += 1;
            }
            for clause in line.split(", ") {
                assert!(
                    script.contains(clause),
                    "the plugin no longer says {clause:?}; update OVERSEER_HARD_RULES"
                );
            }
        }
        assert_eq!(rules, 4);
    }

    #[test]
    fn runtime_name_is_config_then_env() {
        crate::env_compat::remove_process_env_for_test("SHEP_OVERSEER_RUNTIME");
        let mut app = AppState::test_new();
        assert_eq!(overseer_runtime_name(&app), None);
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \" codex \"").expect("table"),
        );
        assert_eq!(overseer_runtime_name(&app).as_deref(), Some("codex"));
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"\"").expect("table"),
        );
        assert_eq!(overseer_runtime_name(&app), None, "an empty name is none");
    }
}
