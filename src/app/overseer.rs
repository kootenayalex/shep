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
//! the same mtime guard), the prompt that carries the plugin's hard rules,
//! and the send path that spawns the runtime on a thread and reports back
//! through [`AppEvent::OverseerChatFinished`].
//!
//! One session, two faces: when the runtime's headless recipe can name a
//! conversation (`session_new_args` / `session_resume_args`), the chat and
//! the board's interactive session pane are the same conversation — the id
//! lives in `session-id`, `session-started` says whether anything has begun
//! it yet, and the prompt carries no turn replay because the session is the
//! memory. A runtime without those recipes gets the last few turns replayed
//! instead. Ticks are stateless either way.
//!
//! Nothing on the read side creates the dir or any file. A missing dir is
//! simply an overseer that has not spoken yet; only a sent question or an
//! opened session writes.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};

use serde::Deserialize;

use super::state::{AppState, Mode};
use super::App;
use crate::events::AppEvent;
use crate::workspace::SystemRole;

/// How stale the sample may get before the dir is stat'ed again. The
/// dashboard's interval: the overseer ticks on agent events, not per frame.
pub(crate) const OVERSEER_SAMPLE_INTERVAL: Duration = Duration::from_secs(2);

/// How old the overseer's situation may be before a caller that wants a
/// current read of the room — the board opening, or an `overseer.tick` with
/// no `max_age_seconds` — asks the plugin for a fresh tick.
pub(crate) const STALE_SITUATION_SECS: u64 = 60;

/// How many chat turns [`crate::api::schema::OverseerSample`] carries when the
/// caller does not say.
pub(crate) const DEFAULT_SAMPLE_CHAT_TURNS: u32 = 20;

/// Why a question was refused: one is already out, or it was blank.
pub(crate) const CHAT_BUSY: &str = "a question is already out";
pub(crate) const CHAT_EMPTY: &str = "empty question";

const SITUATION_FILE: &str = "situation.json";
const SITUATION_MD_FILE: &str = "situation.md";
/// The MCP client config both overseer faces point their runtime at, written
/// into the plugin's state dir beside `situation.md` so the chat, the session
/// pane and anyone reading the dir see the same file.
const MCP_CONFIG_FILE: &str = "mcp.json";
/// The `shep mcp` profile the overseer gets: read, inbox capture and paging.
/// The tool list is the boundary — see `[plugins.overseer] tools`.
pub(crate) const MCP_PROFILE: &str = "overseer";
/// What `overseer-session` execs when nothing is configured, and so whose
/// `[launch]` recipe says how to attach the config for the pane.
const SESSION_DEFAULT_RUNTIME: &str = "claude";
const CHAT_FILE: &str = "chat.jsonl";
const BOARD_FILE: &str = "BOARD.md";
const BOARD_SOURCE_FILE: &str = "BOARD.md.source";
const BRAIN_STAMP_FILE: &str = "last-brain";
/// The shared conversation's id (a v4 uuid), created on the first chat or
/// the first session open and never changed.
const SESSION_ID_FILE: &str = "session-id";
/// Present once something has begun the conversation under that id — a
/// headless answer that succeeded, or the interactive pane being opened —
/// so the next face resumes instead of starting anew.
const SESSION_STARTED_FILE: &str = "session-started";

/// The wire types, under the names this module has always used: they are the
/// shapes of the plugin's own files, so `api::schema::overseer` owns them and
/// every reader here — the board, the chat, the `overseer.*` handlers — sees
/// exactly what a client does.
pub(crate) use crate::api::schema::{
    OverseerChatRole as ChatRole, OverseerChatTurn as ChatTurn,
    OverseerHealthFinding as HealthFinding, OverseerHealthLevel as HealthLevel,
    OverseerNarrativeSection as NarrativeSection, OverseerNarrativeSource as NarrativeSource,
};

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

/// The title of a `## <title>` section heading, when `line` is one.
pub(crate) fn section_title(line: &str) -> Option<&str> {
    line.strip_prefix("## ")
        .map(str::trim)
        .filter(|title| !title.is_empty())
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

    /// The body of `BOARD.md`: every non-empty line after the header the
    /// tick writes (`OVERSEER · <at> · <source>`, or the older `# BOARD — …`),
    /// trimmed, with section headings still carrying their `## `.
    fn body_lines(&self) -> Vec<&str> {
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
        lines.collect()
    }

    /// The narrative as a flat list: every non-empty line of `BOARD.md`
    /// after the header, a section heading reduced to its bare title.
    pub fn narrative_lines(&self) -> Vec<String> {
        self.body_lines()
            .into_iter()
            .map(|line| section_title(line).unwrap_or(line).to_string())
            .collect()
    }

    /// The narrative by section: the tick writes `## <agent>` over each
    /// agent's paragraph and `## room` over what cuts across them. Prose
    /// before the first heading — the whole board, for one written before
    /// the sections — is a section with no title.
    pub fn narrative_sections(&self) -> Vec<NarrativeSection> {
        let mut sections: Vec<NarrativeSection> = Vec::new();
        for line in self.body_lines() {
            if let Some(title) = section_title(line) {
                sections.push(NarrativeSection {
                    title: Some(title.to_string()),
                    lines: Vec::new(),
                });
                continue;
            }
            if sections.is_empty() {
                sections.push(NarrativeSection::default());
            }
            sections
                .last_mut()
                .expect("a section was just pushed")
                .lines
                .push(line.to_string());
        }
        sections
    }

    /// The narrative's opening sentence — what the strip has room for.
    ///
    /// The `room` section's first sentence when the board has one (what
    /// cuts across the agents is the strip's business; each agent's own
    /// read sits under its row on the board), else the first line that is
    /// not a heading. Skips the header, stops at the first sentence end or
    /// line break.
    pub fn first_sentence(&self) -> Option<String> {
        let lines = self.body_lines();
        let first_prose = |from: usize| {
            lines[from.min(lines.len())..]
                .iter()
                .copied()
                .find(|line| section_title(line).is_none())
        };
        let room = lines
            .iter()
            .position(|line| section_title(line).is_some_and(|t| t.eq_ignore_ascii_case("room")));
        let line = room
            .and_then(|at| first_prose(at + 1))
            .or_else(|| first_prose(0))?;
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

    /// How long since the tick last wrote its situation, when one exists.
    pub fn situation_age(&self, now: SystemTime) -> Option<Duration> {
        now.duration_since(self.situation_mtime?).ok()
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
                "OVERSEER · 07:08 · brain\n\
                 ## claude · workmayt\n\
                 workmayt's claude has been blocked 2m on a permission prompt. \
                 Say yes: it is the push it was asked for.\n\
                 ## claude · emberline\n\
                 done and unseen; its push is waiting.\n\
                 ## room\n\
                 nothing owed; disk is low."
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
4. You never write to the docket: what is owed is said on the board, and the person keeps their own list.";

/// What the chat asks of the runtime, after the rules.
const CHAT_TASK: &str = "You are the overseer of this shep session; answer in at most 6 short lines, plain prose, no markdown headings.";

/// What the chat adds when the runtime resumes one conversation: the
/// situation is re-sent every time and supersedes what earlier turns said.
const CHAT_TASK_SHARED: &str =
    "The situation below is current and replaces any earlier one in this conversation.";

/// Where a question's memory of the thread comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChatMemory<'a> {
    /// The runtime forgets between calls: the last
    /// [`CHAT_CONTEXT_TURNS`] of these ride along in the prompt.
    Replay(&'a [ChatTurn]),
    /// The runtime resumes one conversation: nothing is replayed.
    Session,
}

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
/// last wrote, the memory (the last [`CHAT_CONTEXT_TURNS`] turns when the
/// runtime needs them replayed; nothing when it resumes a session), and
/// the question.
pub(crate) fn build_chat_prompt(
    situation_md: &str,
    memory: ChatMemory<'_>,
    question: &str,
) -> String {
    let mut prompt = String::new();
    prompt.push_str(OVERSEER_HARD_RULES);
    prompt.push_str("\n\n");
    prompt.push_str(CHAT_TASK);
    if memory == ChatMemory::Session {
        prompt.push(' ');
        prompt.push_str(CHAT_TASK_SHARED);
    }
    prompt.push_str("\n\nThe situation, as last sensed:\n");
    let situation = situation_md.trim();
    prompt.push_str(if situation.is_empty() {
        "(the overseer has not sensed the session yet)"
    } else {
        situation
    });
    prompt.push('\n');
    if let ChatMemory::Replay(turns) = memory {
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

/// Where the shared session runs: `[plugins.overseer] session_cwd` (`~`
/// expands) when that directory exists, else the overseer's state dir. The
/// one resolution both faces use — claude keys its transcripts by cwd, so
/// the headless question and the interactive pane must agree — handed to
/// the session pane as `SHEP_OVERSEER_SESSION_CWD`.
pub(crate) fn overseer_session_cwd(app: &AppState) -> PathBuf {
    app.plugins_config
        .get("overseer")
        .and_then(|table| table.get("session_cwd"))
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|path| !path.is_empty())
        .map(crate::worktree::expand_tilde_path)
        .filter(|path| path.is_dir())
        .unwrap_or_else(|| app.overseer.state_dir.clone())
}

/// Write the overseer's MCP client config into `state_dir` and return its
/// path. `None` when it cannot be written — the runtime then runs without
/// tools rather than not at all.
fn write_overseer_mcp_config(state_dir: &Path) -> Option<PathBuf> {
    let path = state_dir.join(MCP_CONFIG_FILE);
    match crate::mcp_config::write_client_config(&path, MCP_PROFILE) {
        Ok(()) => Some(path),
        Err(err) => {
            tracing::warn!(path = %path.display(), error = %err, "overseer mcp config not written");
            None
        }
    }
}

/// The env the session pane gets so it runs the same conversation the chat
/// does: the id, whether to resume it, where — and how to reach shep's own
/// tools, as the config's path plus the launch recipe's arguments already
/// substituted, so the plugin script appends words rather than inventing a
/// runtime's flag spelling. The arguments are `[]` when the runtime declares
/// no way to attach a config.
pub(crate) fn overseer_session_env(app: &AppState) -> Vec<(String, String)> {
    let (id, started) = app.overseer.overseer_session();
    let config = app.overseer.state_dir.join(MCP_CONFIG_FILE);
    let runtime = overseer_runtime_name(app).unwrap_or_else(|| SESSION_DEFAULT_RUNTIME.to_string());
    let args = crate::runtimes::launch_mcp_config_args(&runtime, &app.runtimes_config)
        .and_then(|args| crate::runtimes::substitute_mcp_config(&args, &config))
        .unwrap_or_default();
    vec![
        ("SHEP_OVERSEER_SESSION_ID".to_string(), id),
        (
            "SHEP_OVERSEER_SESSION_RESUME".to_string(),
            if started { "1" } else { "0" }.to_string(),
        ),
        (
            "SHEP_OVERSEER_SESSION_CWD".to_string(),
            overseer_session_cwd(app).display().to_string(),
        ),
        (
            "SHEP_OVERSEER_MCP_CONFIG".to_string(),
            config.display().to_string(),
        ),
        (
            "SHEP_OVERSEER_MCP_ARGS".to_string(),
            serde_json::to_string(&args).unwrap_or_else(|_| "[]".to_string()),
        ),
    ]
}

/// A fresh v4 uuid, formatted the way `claude --session-id` insists on
/// (`8-4-4-4-12` lowercase hex with the version and variant bits set).
/// Sixteen bytes from `/dev/urandom`, the way the bridge mints its tokens;
/// when that is unreadable, a hash of the clock, the pid and a counter.
fn new_session_id() -> String {
    use std::io::Read;
    let mut bytes = [0u8; 16];
    let random = std::fs::File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .is_ok();
    if !random {
        use sha2::Digest;
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let seq = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let digest = sha2::Sha256::digest(format!("{nanos}-{}-{seq}", std::process::id()));
        bytes.copy_from_slice(&digest[..16]);
    }
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    let hex: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
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
    /// The overseer plugin's `tick` the server asked for, by its command log
    /// id, until it finishes — so a board reopening and an `overseer.tick`
    /// from the phone do not stack ticks on each other. A server fact, not
    /// the board's: the API asks for ticks too.
    pub tick_in_flight: Option<String>,
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
            tick_in_flight: None,
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
    fn record_turn(&mut self, role: ChatRole, text: String) -> ChatTurn {
        let turn = ChatTurn {
            at: unix_now(),
            role,
            text,
        };
        if let Err(err) = append_chat(&self.state_dir, &turn) {
            tracing::warn!(error = %err, dir = %self.state_dir.display(), "overseer chat not written");
        }
        self.chat_mtime = mtime(&self.state_dir.join(CHAT_FILE));
        self.chat.push(turn.clone());
        if self.chat.len() > CHAT_KEEP_TURNS {
            let drop = self.chat.len() - CHAT_KEEP_TURNS;
            self.chat.drain(..drop);
        }
        turn
    }

    /// The shared conversation: its id and whether anything has begun it.
    /// The id is minted and written on the first call (creating the dir);
    /// every later call reads the same one back. Called from input paths
    /// only — never from render, which must not touch the dir.
    pub(crate) fn overseer_session(&self) -> (String, bool) {
        let path = self.state_dir.join(SESSION_ID_FILE);
        let existing = std::fs::read_to_string(&path)
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|id| !id.is_empty());
        let id = match existing {
            Some(id) => id,
            None => {
                let id = new_session_id();
                let written = std::fs::create_dir_all(&self.state_dir)
                    .and_then(|_| std::fs::write(&path, format!("{id}\n")));
                match written {
                    Ok(()) => tracing::info!(id = %id, "overseer session id created"),
                    Err(err) => {
                        tracing::warn!(error = %err, path = %path.display(), "overseer session id not written")
                    }
                }
                id
            }
        };
        (id, self.state_dir.join(SESSION_STARTED_FILE).exists())
    }

    /// The shared conversation as a reader sees it: the id if one has been
    /// minted, and whether anything has begun it. Unlike
    /// [`OverseerState::overseer_session`] this mints nothing and creates no
    /// dir, so render and the read-only API may call it.
    pub(crate) fn peek_session(&self) -> (Option<String>, bool) {
        let id = std::fs::read_to_string(self.state_dir.join(SESSION_ID_FILE))
            .ok()
            .map(|text| text.trim().to_string())
            .filter(|id| !id.is_empty());
        (id, self.state_dir.join(SESSION_STARTED_FILE).exists())
    }

    /// Record that the conversation under the current id has begun.
    pub(crate) fn mark_session_started(&self) {
        mark_session_started_in(&self.state_dir);
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

fn mark_session_started_in(dir: &Path) {
    let path = dir.join(SESSION_STARTED_FILE);
    if let Err(err) = std::fs::create_dir_all(dir).and_then(|_| std::fs::write(&path, "")) {
        tracing::warn!(error = %err, path = %path.display(), "overseer session marker not written");
    }
}

fn clear_session_started_in(dir: &Path) {
    let path = dir.join(SESSION_STARTED_FILE);
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
        Err(err) => {
            tracing::warn!(error = %err, path = %path.display(), "overseer session marker not cleared")
        }
    }
}

/// One headless run, judged: the answer text, or why there is none.
fn judge_capture(
    name: &str,
    run: std::io::Result<crate::runtimes::HeadlessCapture>,
) -> Result<String, String> {
    match run {
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
    }
}

impl App {
    /// Send `question` to the overseer's runtime. The `you` turn lands on
    /// the file and the screen at once; the answer comes back as an
    /// [`AppEvent::OverseerChatFinished`] from a thread that owns nothing of
    /// the app. Without a runtime the overseer answers with the fact.
    ///
    /// One question at a time: a second while the first is out would spawn a
    /// second thread and a second `you` turn — two runs of the same resumed
    /// conversation at once — so it is refused with [`CHAT_BUSY`] and the
    /// caller keeps the text.
    pub(crate) fn send_overseer_chat(
        &mut self,
        question: String,
    ) -> Result<ChatTurn, &'static str> {
        let question = question.trim().to_string();
        if question.is_empty() {
            return Err(CHAT_EMPTY);
        }
        if self.state.overseer.chat_pending {
            return Err(CHAT_BUSY);
        }
        let overseer = &mut self.state.overseer;
        overseer.chat_input.clear();
        let recorded = overseer.record_turn(ChatRole::You, question.clone());
        overseer.chat_pending = true;
        self.emit_chat_turn(recorded.clone());
        self.mark_render_dirty();

        let Some(name) = overseer_runtime_name(&self.state) else {
            self.finish_overseer_chat(CHAT_NO_RUNTIME.to_string());
            return Ok(recorded);
        };
        let spec = match crate::runtimes::resolve_headless(&name, &self.state.runtimes_config) {
            Ok((spec, _)) => spec,
            Err(err) => {
                tracing::warn!(runtime = %name, error = %err, "overseer chat runtime unresolvable");
                self.finish_overseer_chat(format!("{CHAT_NO_RUNTIME} ({err})"));
                return Ok(recorded);
            }
        };
        let state_dir = self.state.overseer.state_dir.clone();
        let situation =
            std::fs::read_to_string(state_dir.join(SITUATION_MD_FILE)).unwrap_or_default();
        // Only a recipe that says how to attach one gets a config written:
        // an answer from a runtime that cannot take tools should not leave a
        // file claiming it could.
        let mcp = spec
            .mcp_config_args
            .is_some()
            .then(|| write_overseer_mcp_config(&state_dir))
            .flatten();
        // A runtime that can name conversations shares one with the session
        // pane: the session is the memory, and the question runs where the
        // pane would. Any other runtime gets the last turns replayed and
        // runs in the state dir.
        let (session, cwd, prompt) = if spec.shares_session() {
            let (id, started) = self.state.overseer.overseer_session();
            (
                Some(crate::runtimes::HeadlessSession {
                    id,
                    resume: started,
                }),
                overseer_session_cwd(&self.state),
                build_chat_prompt(&situation, ChatMemory::Session, &question),
            )
        } else {
            // The question is already the last turn; the prompt names it once.
            let turns = &self.state.overseer.chat;
            let earlier = &turns[..turns.len().saturating_sub(1)];
            (
                None,
                state_dir.clone(),
                build_chat_prompt(&situation, ChatMemory::Replay(earlier), &question),
            )
        };
        let event_tx = self.event_tx.clone();
        std::thread::spawn(move || {
            let run = |session: Option<&crate::runtimes::HeadlessSession>| {
                judge_capture(
                    &name,
                    crate::runtimes::run_headless_captured(
                        &spec,
                        &prompt,
                        CHAT_TIMEOUT,
                        Some(&cwd),
                        session,
                        mcp.as_deref(),
                    ),
                )
            };
            let mut answer = run(session.as_ref());
            if let Some(session) = session {
                // A resume that fails is taken as a conversation the runtime
                // no longer has (its transcript gone, its cwd moved): the
                // marker is dropped and the same id starts over, once.
                if answer.is_err() && session.resume {
                    tracing::warn!(
                        runtime = %name,
                        id = %session.id,
                        error = answer.as_ref().err().map(String::as_str).unwrap_or(""),
                        "overseer chat resume failed; starting the session anew"
                    );
                    clear_session_started_in(&state_dir);
                    answer = run(Some(&crate::runtimes::HeadlessSession {
                        id: session.id.clone(),
                        resume: false,
                    }));
                }
                if answer.is_ok() {
                    mark_session_started_in(&state_dir);
                }
            }
            let _ = event_tx.blocking_send(AppEvent::OverseerChatFinished { answer });
        });
        Ok(recorded)
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
        let recorded = overseer.record_turn(ChatRole::Overseer, text);
        overseer.chat_pending = false;
        self.emit_chat_turn(recorded);
        self.mark_render_dirty();
    }

    /// Every recorded turn reaches subscribers the moment it is written, so a
    /// client that is not polling still sees the question go out and the
    /// answer come back.
    fn emit_chat_turn(&mut self, turn: ChatTurn) {
        let pending = self.state.overseer.chat_pending;
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::OverseerChatTurn,
            data: crate::api::schema::EventData::OverseerChatTurn { turn, pending },
        });
    }

    /// The overseer's files moved: what a client needs to know it should
    /// re-read them.
    pub(crate) fn emit_overseer_updated(&mut self) {
        let sample = &self.state.overseer.sample;
        let data = crate::api::schema::EventData::OverseerUpdated {
            tick_at: sample.tick_at.clone(),
            source: sample.source,
            situation_age_seconds: sample
                .situation_age(SystemTime::now())
                .map(|age| age.as_secs()),
        };
        self.emit_event(crate::api::schema::EventEnvelope {
            event: crate::api::schema::EventKind::OverseerUpdated,
            data,
        });
    }

    /// Ask the overseer plugin for a fresh `tick` unless the situation is
    /// younger than `max_age_seconds` or a tick is already out. The one
    /// implementation behind the board opening and `overseer.tick`; it
    /// reports whether it started one, whether one is running, and the
    /// command log id of whichever tick that is.
    pub(crate) fn overseer_tick(
        &mut self,
        max_age_seconds: u64,
        source: &str,
    ) -> Result<(bool, bool, Option<String>), String> {
        if let Some(log_id) = self.state.overseer.tick_in_flight.clone() {
            return Ok((false, true, Some(log_id)));
        }
        if !self
            .state
            .overseer
            .sample
            .situation_older_than(max_age_seconds)
        {
            return Ok((false, false, None));
        }
        let log = self.invoke_plugin_action_quietly(Some(OVERSEER_PLUGIN_ID), "tick", source)?;
        self.state.overseer.tick_in_flight = Some(log.log_id.clone());
        Ok((true, true, Some(log.log_id)))
    }

    /// Everything `overseer.sample` reports, read from what the server
    /// already holds. Pure: no file is created, no session id is minted.
    pub(crate) fn overseer_sample_info(
        &self,
        chat_turns: Option<u32>,
    ) -> crate::api::schema::OverseerSample {
        let overseer = &self.state.overseer;
        let now = SystemTime::now();
        let wanted = chat_turns
            .unwrap_or(DEFAULT_SAMPLE_CHAT_TURNS)
            .min(CHAT_KEEP_TURNS as u32) as usize;
        let start = overseer.chat.len().saturating_sub(wanted);
        let (session_id, session_started) = overseer.peek_session();
        crate::api::schema::OverseerSample {
            plugin_linked: self
                .state
                .installed_plugins
                .contains_key(OVERSEER_PLUGIN_ID),
            sampled: overseer.sample.sampled,
            narrative: overseer.sample.narrative_lines(),
            sections: overseer.sample.narrative_sections(),
            source: overseer.sample.source,
            tick_at: overseer.sample.tick_at.clone(),
            situation_age_seconds: overseer.sample.situation_age(now).map(|age| age.as_secs()),
            brain_age_seconds: overseer.sample.brain_age(now).map(|age| age.as_secs()),
            runtime: overseer_runtime_name(&self.state),
            tick_in_flight: overseer.tick_in_flight.is_some(),
            health: overseer.sample.health.clone(),
            chat: overseer.chat[start..].to_vec(),
            chat_total: overseer.chat.len() as u64,
            chat_pending: overseer.chat_pending,
            session: crate::api::schema::OverseerSessionInfo {
                id: session_id,
                started: session_started,
            },
        }
    }

    fn mark_render_dirty(&self) {
        self.render_dirty
            .store(true, std::sync::atomic::Ordering::Release);
        self.render_notify.notify_one();
    }

    /// The system workspace holding the overseer's session, if one is open.
    /// Its pane's exit removes the workspace, so an index here is a live
    /// session.
    pub(crate) fn overseer_session_workspace(&self) -> Option<usize> {
        self.state
            .workspaces
            .iter()
            .position(|ws| ws.system == Some(SystemRole::OverseerSession))
    }

    /// The board header's one button: show the overseer's session pane.
    ///
    /// An open session is focused; otherwise one is started from the
    /// overseer plugin's `session` pane in a workspace flagged as the
    /// system's own, which no list will ever show. Without the plugin the
    /// board says so where the docket notice goes and nothing is created.
    pub(crate) fn open_overseer_session(&mut self) {
        if let Some(idx) = self.overseer_session_workspace() {
            self.show_overseer_session(idx);
            return;
        }
        let Some((plugin, pane)) = self.overseer_session_pane() else {
            self.state.board.docket_notice = Some(SESSION_NEEDS_PLUGIN.to_string());
            return;
        };
        let context = self.current_plugin_context("overseer-session");
        // The env names the config, so it has to exist before the pane does.
        let _ = write_overseer_mcp_config(&self.state.overseer.state_dir);
        // The pane runs the same conversation the chat does; opening it is
        // what begins that conversation when nothing has yet.
        let session_env: std::collections::HashMap<String, String> =
            overseer_session_env(&self.state).into_iter().collect();
        let extra_env = match self.plugin_pane_launch_env(&plugin, &pane.id, session_env, &context)
        {
            Ok(env) => env,
            Err((code, message)) => {
                tracing::warn!(code, message, "overseer session env failed");
                self.state.board.docket_notice = Some(format!("overseer session: {message}"));
                return;
            }
        };
        let (rows, cols) = self.state.estimate_pane_size();
        let created = crate::workspace::Workspace::new_argv_command_with_extra_env(
            PathBuf::from(&plugin.plugin_root),
            rows.max(4),
            cols.max(10),
            &pane.command,
            self.state.pane_scrollback_limit_bytes,
            self.state.host_terminal_theme,
            self.event_tx.clone(),
            self.render_notify.clone(),
            self.render_dirty.clone(),
            extra_env,
        );
        let (mut ws, mut terminal, runtime) = match created {
            Ok(created) => created,
            Err(err) => {
                tracing::warn!(error = %err, "overseer session failed to start");
                self.state.board.docket_notice = Some(format!("overseer session: {err}"));
                return;
            }
        };
        self.state.overseer.mark_session_started();
        ws.system = Some(SystemRole::OverseerSession);
        ws.custom_name = Some(SESSION_GROUP_NAME.to_string());
        terminal.set_manual_label(pane.title.clone());
        let pane_id = ws.tabs[0].root_pane;
        let terminal_id = terminal.id.clone();
        self.terminal_runtimes.insert(terminal_id.clone(), runtime);
        self.state.terminals.insert(terminal_id, terminal);
        self.state.workspaces.push(ws);
        let idx = self.state.workspaces.len() - 1;
        self.state.remove_alias_shadowed_by_new_pane(pane_id);
        self.state.plugin_panes.insert(
            pane_id,
            crate::app::state::PluginPaneRecord {
                plugin_id: plugin.plugin_id.clone(),
                entrypoint: pane.id.clone(),
            },
        );
        crate::logging::workspace_created(&self.state.workspaces[idx].id, pane_id.raw());
        // Clients re-read their lists on these, and the lists omit it; the
        // events keep the plugin hooks and the pane's own lifecycle honest.
        self.emit_workspace_open_events(idx);
        self.show_overseer_session(idx);
        self.schedule_session_save();
    }

    /// The installed, enabled overseer plugin and its `session` pane.
    fn overseer_session_pane(
        &self,
    ) -> Option<(
        crate::api::schema::InstalledPluginInfo,
        crate::api::schema::PluginManifestPane,
    )> {
        let plugin = self.state.installed_plugins.get(OVERSEER_PLUGIN_ID)?;
        if !plugin.enabled {
            return None;
        }
        let pane = plugin
            .panes
            .iter()
            .find(|pane| pane.id == SESSION_PANE_ID)?
            .clone();
        Some((plugin.clone(), pane))
    }

    /// Put the session's workspace on the desktop, whatever screen or
    /// board-opened modal was up.
    fn show_overseer_session(&mut self, idx: usize) {
        self.state.switch_workspace(idx);
        self.state.board.suspended = false;
        self.state.mode = Mode::Terminal;
    }
}

/// The plugin the session comes from, and its pane entrypoint.
pub(crate) const OVERSEER_PLUGIN_ID: &str = "overseer";
pub(crate) const SESSION_PANE_ID: &str = "session";
/// The system workspace's name, as the titlebar and a degraded (unflagged)
/// fallback would show it.
pub(crate) const SESSION_GROUP_NAME: &str = "overseer";
/// What the board says when the button has nothing to open.
pub(crate) const SESSION_NEEDS_PLUGIN: &str =
    "link the overseer plugin: shep plugin link plugins/overseer";

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
            (
                "OVERSEER · 07:08\n## claude\nblocked 2m. say yes.\n## room",
                Some("blocked 2m."),
            ),
            (
                "OVERSEER · 07:08\n## claude\nblocked 2m. say yes.\n## room\nnothing owed. disk low.",
                Some("nothing owed."),
            ),
            ("## room", None),
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
        sample.narrative = Some("## claude\nblocked.\n## room\nquiet.".into());
        assert_eq!(
            sample.narrative_lines(),
            vec!["claude", "blocked.", "room", "quiet."],
            "a heading is its bare title"
        );
        sample.narrative = None;
        assert!(sample.narrative_lines().is_empty());
    }

    #[test]
    fn narrative_sections_split_on_headings_and_keep_untitled_prose() {
        let section = |title: Option<&str>, lines: &[&str]| NarrativeSection {
            title: title.map(str::to_string),
            lines: lines.iter().map(|l| l.to_string()).collect(),
        };
        let sample = OverseerSample::test_fixture();
        assert_eq!(
            sample.narrative_sections(),
            vec![
                section(
                    Some("claude · workmayt"),
                    &["workmayt's claude has been blocked 2m on a permission prompt. Say yes: it is the push it was asked for."]
                ),
                section(Some("claude · emberline"), &["done and unseen; its push is waiting."]),
                section(Some("room"), &["nothing owed; disk is low."]),
            ]
        );
        assert_eq!(
            sample.first_sentence().as_deref(),
            Some("nothing owed; disk is low."),
            "the strip quotes the room, not an agent's own read"
        );

        // A board from before the sections: one untitled section, whole.
        let old = OverseerSample {
            narrative: Some("OVERSEER · 07:08 · brain\nall quiet.\nnothing owed.".into()),
            ..Default::default()
        };
        assert_eq!(
            old.narrative_sections(),
            vec![section(None, &["all quiet.", "nothing owed."])]
        );
        // Prose before the first heading keeps its own untitled section; an
        // empty heading (`## `) is prose, not a section.
        let mixed = OverseerSample {
            narrative: Some("lead.\n## \n## codex\nworking.".into()),
            ..Default::default()
        };
        assert_eq!(
            mixed.narrative_sections(),
            vec![
                section(None, &["lead.", "##"]),
                section(Some("codex"), &["working."])
            ]
        );
        assert!(OverseerSample::default().narrative_sections().is_empty());
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
        let prompt = build_chat_prompt(
            "# situation\nclaude is blocked.",
            ChatMemory::Replay(&turns),
            "and now?",
        );
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
        let bare = build_chat_prompt("", ChatMemory::Replay(&[]), "hello?");
        assert!(bare.contains("has not sensed the session yet"));
        assert!(!bare.contains("The conversation so far"));
        assert!(bare.ends_with("you: hello?\noverseer:"));
        assert!(!bare.contains(CHAT_TASK_SHARED));
    }

    #[test]
    fn chat_prompt_has_no_turn_replay_when_the_session_is_shared() {
        let turns = OverseerState::test_chat_fixture(1_700_000_000);
        let replayed =
            build_chat_prompt("claude is blocked.", ChatMemory::Replay(&turns), "and now?");
        assert!(replayed.contains("The conversation so far"));
        assert!(replayed.contains("you: what should I do first?"));

        let shared = build_chat_prompt("claude is blocked.", ChatMemory::Session, "and now?");
        assert!(shared.starts_with(OVERSEER_HARD_RULES));
        assert!(shared.contains("at most 6 short lines"));
        assert!(
            shared.contains(CHAT_TASK_SHARED),
            "the situation is declared current"
        );
        assert!(shared.contains("claude is blocked."));
        assert!(
            !shared.contains("The conversation so far"),
            "the session is the memory"
        );
        for turn in &turns {
            assert!(
                !shared.contains(turn.text.as_str()),
                "{:?} was replayed",
                turn.text
            );
        }
        assert!(shared.ends_with("you: and now?\noverseer:"));
    }

    #[test]
    fn session_id_is_created_once_and_reused() {
        let dir = test_state_dir();
        let state = OverseerState::new(dir.clone());
        assert!(!dir.exists());
        let (id, started) = state.overseer_session();
        assert!(!started, "nothing has begun it");
        assert!(dir.join("session-id").exists(), "the first ask writes it");
        assert_eq!(id.len(), 36);
        assert!(id
            .bytes()
            .all(|b| b == b'-' || b.is_ascii_hexdigit() && !b.is_ascii_uppercase()));
        assert_eq!(
            id.split('-').map(str::len).collect::<Vec<_>>(),
            vec![8, 4, 4, 4, 12]
        );
        assert_eq!(&id[14..15], "4", "a v4 uuid");
        assert!(matches!(&id[19..20], "8" | "9" | "a" | "b"), "{id}");
        assert_eq!(
            std::fs::read_to_string(dir.join("session-id"))
                .expect("file")
                .trim(),
            id
        );

        let again = OverseerState::new(dir.clone());
        assert_eq!(
            again.overseer_session(),
            (id.clone(), false),
            "read back, not minted"
        );
        assert_ne!(new_session_id(), new_session_id(), "ids are random");

        again.mark_session_started();
        assert_eq!(again.overseer_session(), (id.clone(), true));
        clear_session_started_in(&dir);
        assert_eq!(again.overseer_session(), (id, false));
        clear_session_started_in(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_cwd_resolution_prefers_config_then_state_dir() {
        let mut app = AppState::test_new();
        let dir = test_state_dir();
        app.overseer.state_dir = dir.clone();
        assert_eq!(overseer_session_cwd(&app), dir, "nothing configured");

        let configured = test_state_dir();
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str(&format!("session_cwd = \"{}\"", configured.display())).expect("table"),
        );
        assert_eq!(
            overseer_session_cwd(&app),
            dir,
            "a configured dir that does not exist is passed over"
        );
        std::fs::create_dir_all(&configured).expect("cwd");
        assert_eq!(overseer_session_cwd(&app), configured);

        // `~` expands.
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .filter(|home| home.is_dir());
        if let Some(home) = home {
            app.plugins_config.insert(
                "overseer".into(),
                toml::from_str("session_cwd = \"~\"").expect("table"),
            );
            assert_eq!(overseer_session_cwd(&app), home);
        }
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str("session_cwd = \"  \"").expect("table"),
        );
        assert_eq!(overseer_session_cwd(&app), dir, "blank is unset");

        // The env the pane gets is the same resolution, plus the id and
        // whether to resume.
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str(&format!("session_cwd = \"{}\"", configured.display())).expect("table"),
        );
        let env = overseer_session_env(&app);
        let (id, _) = app.overseer.overseer_session();
        assert_eq!(
            &env[..3],
            &[
                ("SHEP_OVERSEER_SESSION_ID".to_string(), id.clone()),
                ("SHEP_OVERSEER_SESSION_RESUME".to_string(), "0".to_string()),
                (
                    "SHEP_OVERSEER_SESSION_CWD".to_string(),
                    configured.display().to_string()
                ),
            ][..]
        );
        app.overseer.mark_session_started();
        assert_eq!(overseer_session_env(&app)[1].1, "1");
        assert!(
            !dir.join("chat.jsonl").exists(),
            "asking for the session writes no chat"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&configured);
    }

    /// The pane is told where the config is and what to append to reach it,
    /// so the plugin script never has to know a runtime's flag spelling.
    #[test]
    fn session_env_carries_mcp_config_and_args() {
        crate::env_compat::remove_process_env_for_test("SHEP_OVERSEER_RUNTIME");
        let mut app = AppState::test_new();
        let dir = test_state_dir();
        app.overseer.state_dir = dir.clone();
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"claude\"").expect("table"),
        );
        let config = dir.join("mcp.json");
        let env: std::collections::BTreeMap<String, String> =
            overseer_session_env(&app).into_iter().collect();
        assert_eq!(
            env.get("SHEP_OVERSEER_MCP_CONFIG").map(String::as_str),
            Some(config.display().to_string().as_str())
        );
        // The bundled claude `[launch]` recipe, already substituted.
        let args: Vec<String> =
            serde_json::from_str(env.get("SHEP_OVERSEER_MCP_ARGS").expect("args")).expect("json");
        assert_eq!(
            args,
            vec!["--mcp-config".to_string(), config.display().to_string()]
        );
        assert!(
            !config.exists(),
            "asking for the env writes nothing; opening the pane does"
        );

        // A runtime whose launch recipe cannot take a config says so with an
        // empty list rather than by leaving the variable out.
        app.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"pi\"").expect("table"),
        );
        let env: std::collections::BTreeMap<String, String> =
            overseer_session_env(&app).into_iter().collect();
        assert_eq!(
            env.get("SHEP_OVERSEER_MCP_ARGS").map(String::as_str),
            Some("[]")
        );
        assert!(env.contains_key("SHEP_OVERSEER_MCP_CONFIG"));
        let _ = std::fs::remove_dir_all(&dir);
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
