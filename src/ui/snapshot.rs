//! Golden-screen tests for the desktop chrome.
//!
//! The existing UI tests assert single cells — `buffer[(2, 2)].symbol() == "┼"`.
//! That catches the thing the author was thinking about and nothing else, and it
//! encodes current padding in the coordinates, so any spacing change reds a test
//! that was never about spacing. Seven UI modules have no coverage at all.
//!
//! These render whole screens instead and compare them against checked-in text.
//! A snapshot has two blocks: the glyphs, which pin **layout**, and a style grid
//! plus legend, which pin **meaning** — the legend names palette roles
//! (`fg:red`, `fg:accent`) rather than hex, so re-theming churns nothing and a
//! blocked card turning from red to peach is a one-line diff.
//!
//! Re-bless with `SHEP_UPDATE_SNAPSHOTS=1 just test-one snapshot`, then read the
//! diff before committing it — an unreviewed re-bless is worse than no snapshot.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};

use crate::app::state::{AppState, Palette};

// ---------------------------------------------------------------------------
// Naming colors
// ---------------------------------------------------------------------------

/// The palette role a color came from, if any.
///
/// When a theme gives two roles the same color the label says so — `accent|blue`
/// rather than silently picking one. That is not hypothetical: `catppuccin` maps
/// `accent` and `blue` to the same RGB, and reporting only the first name made a
/// teal-to-blue change read as teal-to-accent in a diff.
fn palette_role(color: Color, p: &Palette) -> Option<String> {
    let names: Vec<&'static str> = [
        ("accent", p.accent),
        ("panel_bg", p.panel_bg),
        ("surface0", p.surface0),
        ("surface1", p.surface1),
        ("surface_dim", p.surface_dim),
        ("overlay0", p.overlay0),
        ("overlay1", p.overlay1),
        ("text", p.text),
        ("subtext0", p.subtext0),
        ("mauve", p.mauve),
        ("green", p.green),
        ("yellow", p.yellow),
        ("red", p.red),
        ("blue", p.blue),
        ("teal", p.teal),
        ("peach", p.peach),
    ]
    .into_iter()
    .filter(|(_, candidate)| *candidate == color)
    .map(|(name, _)| name)
    .collect();
    (!names.is_empty()).then(|| names.join("|"))
}

fn color_label(color: Option<Color>, p: &Palette) -> String {
    let Some(color) = color else {
        return "-".to_string();
    };
    if let Some(role) = palette_role(color, p) {
        return role;
    }
    match color {
        Color::Rgb(r, g, b) => format!("#{r:02x}{g:02x}{b:02x}"),
        Color::Indexed(i) => format!("idx{i}"),
        other => format!("{other:?}").to_lowercase(),
    }
}

fn modifier_label(modifier: Modifier) -> String {
    let mut parts = Vec::new();
    for (flag, name) in [
        (Modifier::BOLD, "bold"),
        (Modifier::DIM, "dim"),
        (Modifier::ITALIC, "italic"),
        (Modifier::UNDERLINED, "underlined"),
        (Modifier::REVERSED, "reversed"),
        (Modifier::CROSSED_OUT, "crossed_out"),
        (Modifier::SLOW_BLINK, "slow_blink"),
        (Modifier::RAPID_BLINK, "rapid_blink"),
        (Modifier::HIDDEN, "hidden"),
    ] {
        if modifier.contains(flag) {
            parts.push(name);
        }
    }
    parts.join("+")
}

/// A cell nobody styled.
///
/// `Buffer::empty` fills with `Color::Reset` rather than `None`, so an untouched
/// cell is not `Style::default()` — without this the style grid would be solid
/// letters and the blank regions of a screen would be invisible.
fn is_blank(style: Style) -> bool {
    let neutral = |c: Option<Color>| matches!(c, None | Some(Color::Reset));
    neutral(style.fg)
        && neutral(style.bg)
        && neutral(style.underline_color)
        && style.add_modifier.is_empty()
}

fn style_label(style: Style, p: &Palette) -> String {
    if is_blank(style) {
        return "(unstyled)".to_string();
    }
    let mut out = format!(
        "fg:{} bg:{}",
        color_label(style.fg, p),
        color_label(style.bg, p)
    );
    let modifiers = modifier_label(style.add_modifier);
    if !modifiers.is_empty() {
        out.push(' ');
        out.push_str(&modifiers);
    }
    out
}

// ---------------------------------------------------------------------------
// Rendering a buffer to text
// ---------------------------------------------------------------------------

/// Characters for the style grid, in assignment order. `.` is reserved for the
/// wholly-unstyled cell so untouched regions read as empty space.
const STYLE_CHARS: &[u8] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789+*%$@!?";

fn dump(name: &str, buffer: &Buffer, p: &Palette) -> String {
    let area = buffer.area;

    // Letters are assigned by sorted label, not by first appearance. Appearance
    // order looks natural but is unstable: adding one style anywhere re-letters
    // every style after it, so a pure layout change churns the whole grid and
    // buries the diff you actually wanted to read.
    let mut labels: BTreeMap<String, Style> = BTreeMap::new();
    for y in area.top()..area.bottom() {
        for x in area.left()..area.right() {
            let style = buffer[(x, y)].style();
            if !is_blank(style) {
                labels.insert(style_label(style, p), style);
            }
        }
    }
    // `Style` is Hash but not Ord, so the reverse index is a HashMap; the
    // ordering that matters lives in `labels`, which is sorted.
    let assigned: HashMap<Style, char> = labels
        .values()
        .zip(
            STYLE_CHARS
                .iter()
                .map(|b| *b as char)
                // More distinct styles than the pool: collapse the tail into one
                // char rather than panicking, and let the legend say so.
                .chain(std::iter::repeat('#')),
        )
        .map(|(style, ch)| (*style, ch))
        .collect();
    let legend: BTreeMap<char, String> = labels
        .iter()
        .map(|(label, style)| {
            (
                assigned.get(style).copied().unwrap_or('#'),
                label.to_string(),
            )
        })
        .collect();

    let mut glyphs = String::new();
    let mut styles = String::new();
    for y in area.top()..area.bottom() {
        let mut glyph_row = String::new();
        let mut style_row = String::new();
        for x in area.left()..area.right() {
            let cell = &buffer[(x, y)];
            glyph_row.push_str(cell.symbol());
            let style = cell.style();
            style_row.push(if is_blank(style) {
                '.'
            } else {
                assigned.get(&style).copied().unwrap_or('#')
            });
        }
        // Trailing blanks are noise in a diff; a change that reaches further
        // right lengthens the line, which is the signal worth seeing.
        glyphs.push_str(glyph_row.trim_end());
        glyphs.push('\n');
        styles.push_str(style_row.trim_end_matches('.'));
        styles.push('\n');
    }

    let mut out = String::new();
    let _ = writeln!(
        out,
        "# {name} {}x{} · {} styles",
        area.width,
        area.height,
        labels.len()
    );
    let _ = writeln!(out, "# glyphs pin layout, styles pin meaning");
    out.push_str("\n--- screen ---\n");
    out.push_str(&glyphs);
    out.push_str("\n--- styles ---\n");
    out.push_str(&styles);
    out.push_str("\n--- legend ---\n");
    let _ = writeln!(out, ". = (unstyled)");
    for (ch, label) in legend {
        let _ = writeln!(out, "{ch} = {label}");
    }
    out
}

// ---------------------------------------------------------------------------
// Comparing against the checked-in file
// ---------------------------------------------------------------------------

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/snapshots")
        .join(format!("{name}.txt"))
}

fn first_difference(expected: &str, actual: &str) -> String {
    let expected: Vec<&str> = expected.lines().collect();
    let actual: Vec<&str> = actual.lines().collect();
    let at = expected
        .iter()
        .zip(actual.iter())
        .position(|(a, b)| a != b)
        .unwrap_or(expected.len().min(actual.len()));
    let from = at.saturating_sub(3);
    let to = (at + 4).min(expected.len().max(actual.len()));
    let mut out = format!("first difference at line {}\n", at + 1);
    for i in from..to {
        let marker = if i == at { ">>" } else { "  " };
        let _ = writeln!(out, "{marker} expected |{}", expected.get(i).unwrap_or(&""));
        let _ = writeln!(out, "{marker} actual   |{}", actual.get(i).unwrap_or(&""));
    }
    out
}

fn assert_snapshot(name: &str, actual: String) {
    let path = snapshot_path(name);
    if std::env::var_os("SHEP_UPDATE_SNAPSHOTS").is_some() {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("snapshot dir should be creatable");
        }
        std::fs::write(&path, &actual).expect("snapshot should be writable");
        return;
    }
    let Ok(expected) = std::fs::read_to_string(&path) else {
        panic!(
            "no snapshot at {}\n\
             create it with: SHEP_UPDATE_SNAPSHOTS=1 just test-one snapshot\n\n{actual}",
            path.display()
        );
    };
    assert!(
        expected == actual,
        "{} changed.\n\n{}\nIf the change is intended, re-bless with \
         SHEP_UPDATE_SNAPSHOTS=1 just test-one snapshot — and read the diff.",
        path.display(),
        first_difference(&expected, &actual)
    );
}

/// Render `state` at `width`x`height` and compare it to the stored screen.
fn assert_screen(state: &mut AppState, name: &str, width: u16, height: u16) {
    let palette = state.palette.clone();
    let area = Rect::new(0, 0, width, height);
    let (buffer, _cursor) = crate::server::render_stream::render_virtual(state, area, true);
    assert_snapshot(name, dump(name, &buffer, &palette));
}

// ---------------------------------------------------------------------------
// The fixture
// ---------------------------------------------------------------------------

mod fixture {
    use super::*;
    use crate::app::state::TaskQueueRow;
    use crate::detect::{Agent, AgentState};
    use crate::tasks::{TaskRuntime, TaskState};
    use crate::workspace::Workspace;
    use ratatui::layout::Direction;

    /// One agent's worth of facts, so each pane in the fixture is distinguishable
    /// on screen rather than five copies of "claude".
    pub(super) struct AgentFacts {
        pub state: AgentState,
        pub seen: bool,
        pub name: &'static str,
        pub cwd: &'static str,
        pub activity: Option<&'static str>,
        pub context: Option<u8>,
        /// Seconds ago the state last changed. Chosen away from the 60s/3600s
        /// boundaries in `format_event_age` so a slow test run cannot flip it.
        pub age_secs: u64,
        pub seq: u64,
    }

    fn apply(
        state: &mut AppState,
        ws: usize,
        tab: usize,
        pane: crate::layout::PaneId,
        f: &AgentFacts,
    ) {
        let terminal_id = state.workspaces[ws].tabs[tab]
            .panes
            .get(&pane)
            .expect("fixture pane should exist")
            .attached_terminal_id
            .clone();
        state.workspaces[ws].tabs[tab]
            .panes
            .get_mut(&pane)
            .expect("fixture pane should exist")
            .seen = f.seen;
        let terminal = state
            .terminals
            .get_mut(&terminal_id)
            .expect("fixture terminal should exist");
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = f.state;
        terminal.agent_name = Some(f.name.to_string());
        terminal.cwd = PathBuf::from(f.cwd);
        terminal.activity_lines = f.activity.map(str::to_string).into_iter().collect();
        terminal.context_percent = f.context;
        terminal.last_agent_state_change_seq = Some(f.seq);
        terminal.last_agent_state_change_at =
            Some(Instant::now() - Duration::from_secs(f.age_secs));
    }

    /// A session with every state represented, chrome on, and enough real text
    /// that truncation and column budgets are actually exercised.
    ///
    /// Deliberately deterministic: the spinner tick is pinned, host vitals are
    /// literals rather than a live read, and every age sits mid-bucket.
    pub(super) fn session() -> AppState {
        let mut billing = Workspace::test_new("workmayt");
        let billing_root = billing.tabs[0].root_pane;
        let billing_second = billing.test_split(Direction::Horizontal);
        billing.tabs[0].layout.focus_pane(billing_root);
        billing.cached_git_branch = Some("fix/stripe-webhook".to_string());
        billing.cached_git_ahead_behind = Some((2, 1));
        billing.cached_memory_usage_percent = Some(87);
        billing.review_state = crate::api::schema::ReviewState::NeedsReview;

        let mut site = Workspace::test_new("emberline");
        site.cached_git_branch = Some("master".to_string());
        let site_root = site.tabs[0].root_pane;

        let mut tools = Workspace::test_new("shep");
        tools.cached_git_branch = Some("feat/design-pass".to_string());
        tools.cached_git_ahead_behind = Some((0, 4));
        let tools_root = tools.tabs[0].root_pane;
        let tools_second = tools.test_split(Direction::Vertical);

        let mut state = AppState::test_new();
        state.workspaces = vec![billing, site, tools];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;

        // Chrome ships on; `test_new` keeps it off so older minimal-baseline
        // tests stay small, but a screen snapshot with no titlebar or hint bar
        // is not the screen anyone uses.
        state.titlebar = true;
        state.hint_bar = true;
        // The shipped default. `test_new` still starts on catppuccin, which is
        // not what anyone runs — and worse for a snapshot, catppuccin maps
        // `accent` and `blue` to the same RGB, so the two roles are
        // indistinguishable in a legend.
        state.palette = crate::app::state::Palette::shep();
        // Frame 2 of the braille spinner: pinned so a working agent's icon is a
        // fact about the fixture and not about when the suite ran.
        state.spinner_tick = 16;

        apply(
            &mut state,
            0,
            0,
            billing_root,
            &AgentFacts {
                state: AgentState::Blocked,
                seen: true,
                name: "claude",
                cwd: "~/vault/dev/workmayt",
                activity: Some("may I run the stripe integration tests?"),
                context: Some(72),
                age_secs: 130,
                seq: 60,
            },
        );
        apply(
            &mut state,
            0,
            0,
            billing_second,
            &AgentFacts {
                state: AgentState::Working,
                seen: true,
                name: "opencode",
                cwd: "~/vault/dev/workmayt/apps/site",
                activity: Some("Metamorphosing…"),
                context: Some(41),
                age_secs: 20,
                seq: 50,
            },
        );
        apply(
            &mut state,
            1,
            0,
            site_root,
            &AgentFacts {
                state: AgentState::Idle,
                seen: false,
                name: "claude",
                cwd: "~/vault/dev/emberline",
                activity: Some("pushed 3 commits to master"),
                context: Some(88),
                age_secs: 400,
                seq: 40,
            },
        );
        apply(
            &mut state,
            2,
            0,
            tools_root,
            &AgentFacts {
                state: AgentState::Idle,
                seen: true,
                name: "claude",
                cwd: "~/vault/dev/shep",
                activity: None,
                context: Some(12),
                age_secs: 4000,
                seq: 30,
            },
        );
        apply(
            &mut state,
            2,
            0,
            tools_second,
            &AgentFacts {
                state: AgentState::Working,
                seen: true,
                name: "codex",
                cwd: "~/vault/dev/shep-worktrees/design-pass",
                activity: Some("reading src/ui/board.rs"),
                context: Some(55),
                age_secs: 130,
                seq: 20,
            },
        );

        // Queued input on the blocked pane: the teal ⇥N badge is one of the
        // things this pass is going to move, so it belongs in the baseline.
        state.queued_pane_input.insert(
            billing_root,
            vec!["run the tests".into(), "then ship it".into()],
        );

        state.dashboard_sample.vitals = crate::platform::HostVitals {
            load_percent: Some(38),
            cores: Some(12),
            memory_percent: Some(64),
            memory_total_bytes: Some(64 * 1024 * 1024 * 1024),
            memory_used_bytes: Some(41 * 1024 * 1024 * 1024),
        };
        state.dashboard_sample.pending_tasks = Some(2);
        state.dashboard_sample.sampled_at = Some(Instant::now());

        state.task_queue.sampled = true;
        state.task_queue.sampled_at = Some(Instant::now());
        state.task_queue.rows = vec![
            TaskQueueRow {
                id: 1,
                prompt: "harden the stripe webhook signature check".into(),
                state: TaskState::Running,
                repo_label: "workmayt".into(),
                runtime: TaskRuntime::Claude,
                use_worktree: true,
                dispatched: true,
                age_secs: 240,
            },
            TaskQueueRow {
                id: 2,
                prompt: "draft the 0.9.0 release notes".into(),
                state: TaskState::Todo,
                repo_label: "shep".into(),
                runtime: TaskRuntime::Opencode,
                use_worktree: false,
                dispatched: false,
                age_secs: 1140,
            },
            TaskQueueRow {
                id: 3,
                prompt: "unblock the gitea 403 on emberline".into(),
                state: TaskState::Blocked,
                repo_label: "emberline".into(),
                runtime: TaskRuntime::Claude,
                use_worktree: false,
                dispatched: true,
                age_secs: 7200,
            },
        ];

        state
    }
}

// ---------------------------------------------------------------------------
// The snapshots
// ---------------------------------------------------------------------------

use crate::app::state::{BoardView, Mode};

/// Sizes worth pinning: a wide desktop, a half-screen split, and the 80x24 that
/// every overlay in this codebase is one guard away from refusing to draw.
const WIDE: (u16, u16) = (200, 55);
const MID: (u16, u16) = (120, 40);
const SMALL: (u16, u16) = (80, 24);

fn board_at(name: &str, size: (u16, u16)) {
    let mut state = fixture::session();
    state.mode = Mode::Board;
    assert_screen(&mut state, name, size.0, size.1);
}

#[test]
fn snapshot_board_wide() {
    board_at("board-wide", WIDE);
}

#[test]
fn snapshot_board_mid() {
    board_at("board-mid", MID);
}

#[test]
fn snapshot_board_small() {
    board_at("board-small", SMALL);
}

#[test]
fn snapshot_board_agent_detail() {
    let mut state = fixture::session();
    state.mode = Mode::Board;
    state.board.view = BoardView::Agent;
    assert_screen(&mut state, "board-agent-detail", MID.0, MID.1);
}

#[test]
fn snapshot_board_task_queue() {
    let mut state = fixture::session();
    state.mode = Mode::Board;
    state.board.view = BoardView::Tasks;
    assert_screen(&mut state, "board-task-queue", MID.0, MID.1);
}

/// The two detail screens at 80x24, where their key/value columns have the
/// least room to be wrong quietly.
#[test]
fn snapshot_board_agent_detail_small() {
    let mut state = fixture::session();
    state.mode = Mode::Board;
    state.board.view = BoardView::Agent;
    assert_screen(&mut state, "board-agent-detail-small", SMALL.0, SMALL.1);
}

#[test]
fn snapshot_board_task_queue_small() {
    let mut state = fixture::session();
    state.mode = Mode::Board;
    state.board.view = BoardView::Tasks;
    assert_screen(&mut state, "board-task-queue-small", SMALL.0, SMALL.1);
}

/// The normal working screen: titlebar, sidebar, tab bar, pane grid, hint bar.
///
/// `Mode::Terminal` explicitly, because `AppState::test_new` starts in
/// `Navigate` — which replaces the hint bar with a mode overlay, so the default
/// would quietly snapshot a screen nobody works in.
#[test]
fn snapshot_desktop_wide() {
    let mut state = fixture::session();
    state.mode = Mode::Terminal;
    assert_screen(&mut state, "desktop-wide", WIDE.0, WIDE.1);
}

#[test]
fn snapshot_desktop_small() {
    let mut state = fixture::session();
    state.mode = Mode::Terminal;
    assert_screen(&mut state, "desktop-small", SMALL.0, SMALL.1);
}

/// The mode overlays draw into the reserved hint-bar row. Pinned at 80 columns
/// because that is where their entry-truncation budget actually bites.
#[test]
fn snapshot_navigate_overlay_small() {
    let mut state = fixture::session();
    state.mode = Mode::Navigate;
    assert_screen(&mut state, "navigate-overlay-small", SMALL.0, SMALL.1);
}

#[test]
fn snapshot_prefix_overlay_small() {
    let mut state = fixture::session();
    state.mode = Mode::Prefix;
    assert_screen(&mut state, "prefix-overlay-small", SMALL.0, SMALL.1);
}

#[test]
fn snapshot_settings_mid() {
    let mut state = fixture::session();
    state.mode = Mode::Settings;
    assert_screen(&mut state, "settings-mid", MID.0, MID.1);
}

/// The 76-wide settings modal on an 80-wide terminal — four columns of margin.
#[test]
fn snapshot_settings_small() {
    let mut state = fixture::session();
    state.mode = Mode::Settings;
    assert_screen(&mut state, "settings-small", SMALL.0, SMALL.1);
}

/// A terminal too small for the modal it was asked to draw.
///
/// This used to render *nothing*: `centered_popup_rect` returned `None`, the
/// caller returned, and the screen did not change — which from the outside is
/// exactly what a keybinding that does not exist looks like. The 88-column
/// announcement modal was already in that state on a standard 80x24.
#[test]
fn snapshot_settings_too_small() {
    let mut state = fixture::session();
    state.mode = Mode::Settings;
    assert_screen(&mut state, "settings-too-small", 24, 7);
}

#[test]
fn snapshot_keybind_help_small() {
    let mut state = fixture::session();
    state.mode = Mode::KeybindHelp;
    assert_screen(&mut state, "keybind-help-small", SMALL.0, SMALL.1);
}

/// The whole app with `mouse_capture = false`.
///
/// Shep receives no mouse events in this mode, so anything that can only be
/// clicked is unreachable — the tab-scroll arrows, the `+` new-tab button and
/// the sidebar's `new`/`menu` buttons all correctly disappear. The footers that
/// advertise scrolling are the interesting part: this pins that they name keys
/// rather than a wheel that is not listening.
#[test]
fn snapshot_desktop_no_mouse() {
    let mut state = fixture::session();
    state.mode = Mode::Terminal;
    state.mouse_capture = false;
    assert_screen(&mut state, "desktop-no-mouse", MID.0, MID.1);
}

#[test]
fn snapshot_keybind_help_no_mouse() {
    let mut state = fixture::session();
    state.mode = Mode::KeybindHelp;
    state.mouse_capture = false;
    assert_screen(&mut state, "keybind-help-no-mouse", SMALL.0, SMALL.1);
}

#[test]
fn snapshot_navigator_mid() {
    let mut state = fixture::session();
    state.mode = Mode::Navigator;
    assert_screen(&mut state, "navigator-mid", MID.0, MID.1);
}

// ---------------------------------------------------------------------------
// Tests for the harness itself
// ---------------------------------------------------------------------------

#[cfg(test)]
mod harness_tests {
    use super::*;

    fn palette() -> Palette {
        Palette::shep()
    }

    /// Two roles sharing one color must both be named, or a change between
    /// them is invisible in a diff. `catppuccin` really does this.
    #[test]
    fn a_shared_color_names_every_role_that_claims_it() {
        let p = Palette::catppuccin();
        assert_eq!(p.accent, p.blue, "fixture assumes catppuccin shares these");
        assert_eq!(color_label(Some(p.blue), &p), "accent|blue");
    }

    #[test]
    fn legend_names_palette_roles_not_hex() {
        let p = palette();
        assert_eq!(color_label(Some(p.red), &p), "red");
        assert_eq!(color_label(Some(p.accent), &p), "accent");
        // A color that is not in the palette still has to render as something
        // readable, or an off-palette regression would be invisible.
        assert_eq!(color_label(Some(Color::Rgb(1, 2, 3)), &p), "#010203");
        assert_eq!(color_label(None, &p), "-");
    }

    #[test]
    fn a_color_change_moves_the_legend_and_not_the_grid() {
        let p = palette();
        let area = Rect::new(0, 0, 3, 1);
        let mut before = Buffer::empty(area);
        before[(0, 0)].set_symbol("x");
        before[(0, 0)].set_style(Style::default().fg(p.teal));
        let mut after = Buffer::empty(area);
        after[(0, 0)].set_symbol("x");
        after[(0, 0)].set_style(Style::default().fg(p.blue));

        let before = dump("t", &before, &p);
        let after = dump("t", &after, &p);
        assert!(before.contains("a = fg:teal bg:reset"), "{before}");
        assert!(after.contains("a = fg:blue bg:reset"), "{after}");
        // Same glyphs, same style grid — only the legend moved. That is the
        // whole point of splitting them.
        let grid = |s: &str| {
            s.split("--- legend ---")
                .next()
                .expect("dump has a legend")
                .to_string()
        };
        assert_eq!(grid(&before), grid(&after));
    }

    #[test]
    fn distinct_styles_get_distinct_characters_by_sorted_label() {
        let p = palette();
        let area = Rect::new(0, 0, 3, 1);
        let mut buffer = Buffer::empty(area);
        buffer[(0, 0)].set_symbol("a");
        buffer[(0, 0)].set_style(Style::default().fg(p.red));
        buffer[(1, 0)].set_symbol("b");
        buffer[(1, 0)].set_style(Style::default().fg(p.green));
        buffer[(2, 0)].set_symbol("c");
        buffer[(2, 0)].set_style(Style::default().fg(p.red));
        let dumped = dump("t", &buffer, &p);
        // Letters follow the sorted label, not the reading order: `fg:green`
        // sorts before `fg:red`, so green is `a` even though red is drawn
        // first. That is what keeps the grid stable when a style is added.
        assert!(
            dumped.contains("\nbab\n"),
            "style grid should reuse b: {dumped}"
        );
        assert!(dumped.contains("a = fg:green bg:reset"), "{dumped}");
        assert!(dumped.contains("b = fg:red bg:reset"), "{dumped}");
    }

    #[test]
    fn modifiers_are_named_so_a_lost_bold_is_visible() {
        assert_eq!(modifier_label(Modifier::empty()), "");
        assert_eq!(modifier_label(Modifier::BOLD), "bold");
        assert_eq!(
            modifier_label(Modifier::BOLD | Modifier::ITALIC),
            "bold+italic"
        );
    }
}
