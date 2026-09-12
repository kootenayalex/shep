use ratatui::{
    layout::{Alignment, Rect},
    style::{Modifier, Style},
    text::{Line, Span},
    widgets::Paragraph,
    Frame,
};

use super::glyphs;
use super::scrollbar::{render_scrollbar, should_show_scrollbar};
use super::status::{
    agent_icon, agent_icon_for, manual_state_appearance, state_label, state_label_color,
};
use super::text::{display_width, display_width_u16, fit_strip, truncate_end};
use crate::app::state::{AgentPanelSort, Palette};
use crate::app::{AppState, Mode};
use crate::detect::AgentState;
use crate::terminal::TerminalRuntimeRegistry;

const WORKSPACE_SECTION_HEADER_ROWS: u16 = 2;

/// At or under this many content columns the tree draws its narrow form:
/// one row per group and no state word on an agent row.
const NARROW_SIDEBAR_WIDTH: u16 = 20;

/// The header row carries the sort toggle only from this many content
/// columns — a 26-column sidebar, the auto width of the snapshot fixture.
/// Under it ` groups` and `grouped ≡ «` would sit on top of each other.
const SORT_TOGGLE_MIN_WIDTH: u16 = 25;

/// The footer's button says `+ new group` when it has this much room, and
/// `+ new` when it does not.
const NEW_GROUP_LABEL_MIN_WIDTH: u16 = 12;

/// The least room a branch keeps on a group's second row before the upstream
/// badges after it come off whole. `feat/d…` still names the branch.
const MIN_BRANCH_WIDTH: usize = 6;

/// The narrow tree: one row per group, glyph and name on an agent row.
///
/// `width` is the content width — the list without its separator. Under
/// twenty columns a branch row and a state word cost more than they say, and
/// the glyph and its colour already carry the state.
pub(crate) fn sidebar_narrow(width: u16) -> bool {
    width <= NARROW_SIDEBAR_WIDTH
}

/// The footer button's label for a footer this wide.
pub(crate) fn sidebar_new_button_label(width: u16) -> &'static str {
    if width >= NEW_GROUP_LABEL_MIN_WIDTH {
        "+ new group"
    } else {
        "+ new"
    }
}

pub(crate) struct AgentPanelEntry {
    pub ws_idx: usize,
    pub tab_idx: usize,
    pub pane_id: crate::layout::PaneId,
    pub primary_label: String,
    pub primary_tab_label: Option<String>,
    pub agent_label: Option<String>,
    pub state: AgentState,
    pub seen: bool,
    pub last_agent_state_change_seq: Option<u64>,
    pub custom_status: Option<String>,
    pub state_labels: std::collections::HashMap<String, String>,
    pub context_percent: Option<u8>,
    pub manual_state: Option<crate::api::schema::PaneManualState>,
}

/// The sidebar's content column — everything but the separator it keeps on its
/// right edge.
pub(crate) fn expanded_sidebar_content(area: Rect) -> Rect {
    Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height)
}

fn sidebar_sort_label(sort: AgentPanelSort) -> &'static str {
    match sort {
        AgentPanelSort::Grouped => "grouped",
        AgentPanelSort::Priority => "priority",
    }
}

/// The sort toggle on the list's own header row, left of the `≡ «` pair.
///
/// `area` is the list rect (`workspace_list_rect`). It shares the header with
/// ` groups` rather than owning a row: the tree has a single header now, and a
/// label that cost a whole row would be the widest thing on the panel. Under
/// `SORT_TOGGLE_MIN_WIDTH` columns it is not drawn at all — the sort still
/// applies, and the keyboard still flips it.
pub(crate) fn sidebar_sort_toggle_rect(area: Rect, sort: AgentPanelSort) -> Rect {
    if area.width < SORT_TOGGLE_MIN_WIDTH || area.height == 0 {
        return Rect::default();
    }

    let label = sidebar_sort_label(sort);
    let width = display_width_u16(label);
    // ` ≡ «`: the two glyph cells, the space between them and the one before.
    let right = area.x + area.width.saturating_sub(4);
    Rect::new(right.saturating_sub(width), area.y, width, 1)
}

/// The global menu's `≡`, on the header row two cells left of `«`.
///
/// `area` is the whole sidebar, separator included, like
/// `expanded_sidebar_toggle_rect`.
pub(crate) fn sidebar_menu_glyph_rect(area: Rect) -> Rect {
    let content = expanded_sidebar_content(area);
    if content.width < 3 || content.height == 0 {
        return Rect::default();
    }
    Rect::new(content.x + content.width.saturating_sub(3), content.y, 1, 1)
}

/// Drop the workspace's own name from the front of an agent's name.
///
/// A pane the human named `ShiftMayt Legal` inside the `ShiftMayt` workspace
/// says the group twice on every surface that already prints the group beside
/// it. The stored name keeps whatever they typed; only the redundant prefix is
/// dropped at render time, and only when something is left to show.
fn strip_workspace_prefix(agent_label: &str, workspace_label: &str) -> String {
    let workspace = workspace_label.trim();
    if workspace.is_empty() {
        return agent_label.to_string();
    }
    let Some(head) = agent_label.get(..workspace.len()) else {
        return agent_label.to_string();
    };
    if !head.eq_ignore_ascii_case(workspace) {
        return agent_label.to_string();
    }
    let tail = &agent_label[workspace.len()..];
    let rest = tail.trim_start_matches(|c: char| " -_:/".contains(c) || glyphs::SEP.contains(c));
    // Only a real word boundary counts: `ShiftMaytics` is its own name, not the
    // group plus a suffix.
    if !rest.is_empty() && rest.len() == tail.len() {
        return agent_label.to_string();
    }
    if rest.is_empty() {
        agent_label.to_string()
    } else {
        rest.to_string()
    }
}

pub(crate) fn agent_panel_entries(app: &AppState) -> Vec<AgentPanelEntry> {
    agent_panel_entries_with_runtimes(app, None)
}

pub(crate) fn agent_panel_entries_from(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
) -> Vec<AgentPanelEntry> {
    agent_panel_entries_with_runtimes(app, Some(terminal_runtimes))
}

fn agent_panel_entries_with_runtimes(
    app: &AppState,
    terminal_runtimes: Option<&TerminalRuntimeRegistry>,
) -> Vec<AgentPanelEntry> {
    let empty_runtimes;
    let terminal_runtimes = match terminal_runtimes {
        Some(terminal_runtimes) => terminal_runtimes,
        None => {
            empty_runtimes = TerminalRuntimeRegistry::new();
            &empty_runtimes
        }
    };

    let mut entries: Vec<_> = app
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| !ws.is_system())
        .flat_map(|(ws_idx, ws)| {
            let multi_tab = ws.tabs.len() > 1;
            let workspace_label = ws.display_name_from(&app.terminals, terminal_runtimes);
            ws.pane_details(&app.terminals)
                .into_iter()
                .map(move |detail| AgentPanelEntry {
                    ws_idx,
                    tab_idx: detail.tab_idx,
                    pane_id: detail.pane_id,
                    primary_label: workspace_label.clone(),
                    primary_tab_label: multi_tab
                        .then(|| strip_workspace_prefix(&detail.tab_label, &workspace_label)),
                    agent_label: Some(strip_workspace_prefix(
                        &detail.agent_label,
                        &workspace_label,
                    )),
                    state: detail.state,
                    seen: detail.seen,
                    last_agent_state_change_seq: detail.last_agent_state_change_seq,
                    custom_status: detail.custom_status,
                    state_labels: detail.state_labels,
                    context_percent: detail.context_percent,
                    manual_state: detail.manual_state,
                })
        })
        .collect();

    if matches!(app.agent_panel_sort, AgentPanelSort::Priority) {
        entries.sort_by_key(|entry| {
            (
                std::cmp::Reverse(workspace_attention_priority(entry.state, entry.seen)),
                std::cmp::Reverse(entry.last_agent_state_change_seq),
            )
        });
    }

    entries
}

pub(super) fn agent_panel_status_key(state: AgentState, seen: bool) -> &'static str {
    match (state, seen) {
        (AgentState::Idle, false) => "done",
        (AgentState::Idle, true) => "idle",
        (AgentState::Working, _) => "working",
        (AgentState::Blocked, _) => "blocked",
        (AgentState::Unknown, _) => "unknown",
    }
}

/// Format a duration since the last agent event as a compact age like "3s",
/// "2m", "4h", or "2d". Pure and unit-testable.
pub(crate) fn format_event_age(elapsed: std::time::Duration) -> String {
    let secs = elapsed.as_secs();
    if secs < 60 {
        format!("{secs}s")
    } else if secs < 3600 {
        format!("{}m", secs / 60)
    } else if secs < 86_400 {
        format!("{}h", secs / 3600)
    } else {
        format!("{}d", secs / 86_400)
    }
}

/// Two rows when the group has a branch to say and the room to say it in.
fn workspace_row_height(ws: &crate::workspace::Workspace, narrow: bool) -> u16 {
    if !narrow && ws.branch().is_some() {
        2
    } else {
        1
    }
}

fn workspace_attention_priority(state: AgentState, seen: bool) -> u8 {
    match (state, seen) {
        (AgentState::Blocked, _) => 4,
        (AgentState::Idle, false) => 3,
        (AgentState::Working, _) => 2,
        (AgentState::Idle, true) => 1,
        (AgentState::Unknown, _) => 0,
    }
}

fn space_aggregate_state(app: &AppState, key: &str) -> (AgentState, bool) {
    app.workspaces
        .iter()
        .filter(|ws| ws.worktree_space().is_some_and(|space| space.key == key))
        .map(|ws| ws.aggregate_state(&app.terminals))
        .max_by_key(|(state, seen)| workspace_attention_priority(*state, *seen))
        .unwrap_or((AgentState::Unknown, true))
}

pub(crate) fn workspace_parent_group_state(
    app: &AppState,
    ws_idx: usize,
) -> Option<(String, bool)> {
    let space = app.workspaces.get(ws_idx)?.worktree_space()?;
    if space.is_linked_worktree {
        return None;
    }
    let member_count = app
        .workspaces
        .iter()
        .filter(|ws| {
            ws.worktree_space()
                .is_some_and(|member| member.key == space.key)
        })
        .count();
    (member_count >= 2).then(|| {
        (
            space.key.clone(),
            app.collapsed_space_keys.contains(&space.key),
        )
    })
}

pub(crate) fn grouped_child_display_label(
    label: &str,
    branch: Option<&str>,
    has_custom_name: bool,
) -> String {
    if has_custom_name {
        return label.to_string();
    }
    let Some(branch) = branch else {
        return label.to_string();
    };
    branch
        .strip_prefix("worktree/")
        .unwrap_or(branch)
        .to_string()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum WorkspaceListEntry {
    Workspace { ws_idx: usize, indented: bool },
}

pub(crate) fn next_entry_is_indented_workspace(entries: &[WorkspaceListEntry], idx: usize) -> bool {
    matches!(
        entries.get(idx.saturating_add(1)),
        Some(WorkspaceListEntry::Workspace { indented: true, .. })
    )
}

pub(crate) fn normalized_workspace_scroll(app: &AppState, area: Rect, requested: usize) -> usize {
    let ws_area = workspace_list_rect(area);
    let body = workspace_list_body_rect(ws_area, false);
    if body.height == 0 {
        return requested;
    }

    let entry_count = sidebar_rows(app).len();
    if entry_count == 0 {
        0
    } else {
        requested.min(entry_count.saturating_sub(1))
    }
}

pub(crate) fn workspace_list_entries(app: &AppState) -> Vec<WorkspaceListEntry> {
    workspace_list_entries_inner(app, false)
}

/// Like [`workspace_list_entries`] but always expands worktree groups, ignoring
/// `collapsed_space_keys`. The mobile switcher has no collapse affordance and
/// always shows the full worktree tree.
pub(crate) fn workspace_list_entries_expanded(app: &AppState) -> Vec<WorkspaceListEntry> {
    workspace_list_entries_inner(app, true)
}

fn workspace_list_entries_inner(app: &AppState, force_expanded: bool) -> Vec<WorkspaceListEntry> {
    let mut members_by_key = std::collections::HashMap::<String, Vec<usize>>::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        if ws.is_system() {
            continue;
        }
        if let Some(space) = ws.worktree_space() {
            members_by_key
                .entry(space.key.clone())
                .or_default()
                .push(ws_idx);
        }
    }
    let grouped_keys = members_by_key
        .iter()
        .filter(|(_, members)| {
            members.len() >= 2
                && members.iter().any(|idx| {
                    app.workspaces
                        .get(*idx)
                        .and_then(|ws| ws.worktree_space())
                        .is_some_and(|space| !space.is_linked_worktree)
                })
        })
        .map(|(key, _)| key.clone())
        .collect::<std::collections::HashSet<_>>();

    let visible_group_idx = if matches!(app.mode, Mode::Navigate) {
        Some(app.selected)
    } else {
        app.active
    };
    let active_group = visible_group_idx.and_then(|idx| {
        app.workspaces
            .get(idx)
            .and_then(|ws| ws.worktree_space())
            .map(|space| space.key.clone())
    });

    let mut emitted_groups = std::collections::HashSet::<String>::new();
    let mut entries = Vec::new();
    for (ws_idx, ws) in app.workspaces.iter().enumerate() {
        // A system workspace (the overseer's session) is never a group in
        // the tree; `ws_idx` stays a real index into `app.workspaces`.
        if ws.is_system() {
            continue;
        }
        let Some(space) = ws
            .worktree_space()
            .filter(|space| grouped_keys.contains(&space.key))
        else {
            entries.push(WorkspaceListEntry::Workspace {
                ws_idx,
                indented: false,
            });
            continue;
        };

        if !emitted_groups.insert(space.key.clone()) {
            continue;
        }

        let Some(members) = members_by_key.get(&space.key) else {
            continue;
        };
        let Some(parent_idx) = members.iter().copied().find(|idx| {
            app.workspaces
                .get(*idx)
                .and_then(|member| member.worktree_space())
                .is_some_and(|member_space| !member_space.is_linked_worktree)
        }) else {
            entries.push(WorkspaceListEntry::Workspace {
                ws_idx,
                indented: false,
            });
            continue;
        };
        let collapsed = !force_expanded && app.collapsed_space_keys.contains(&space.key);
        entries.push(WorkspaceListEntry::Workspace {
            ws_idx: parent_idx,
            indented: false,
        });

        if collapsed {
            if let Some(active_idx) = visible_group_idx
                .filter(|idx| *idx != parent_idx)
                .filter(|_| active_group.as_deref() == Some(space.key.as_str()))
            {
                entries.push(WorkspaceListEntry::Workspace {
                    ws_idx: active_idx,
                    indented: true,
                });
            }
        } else {
            for member_idx in members {
                if *member_idx == parent_idx {
                    continue;
                }
                entries.push(WorkspaceListEntry::Workspace {
                    ws_idx: *member_idx,
                    indented: true,
                });
            }
        }
    }
    sort_workspace_blocks_blocked_first(app, entries)
}

/// Reorder the workspace list so blocked spaces surface first (then done,
/// working, idle, unknown), while keeping each worktree group's parent+children
/// block contiguous and preserving the original order within equal-priority
/// blocks so the list does not thrash. Presentation-only client ordering.
fn sort_workspace_blocks_blocked_first(
    app: &AppState,
    entries: Vec<WorkspaceListEntry>,
) -> Vec<WorkspaceListEntry> {
    // Partition the flat entry list into top-level blocks. A block begins at a
    // non-indented entry and absorbs the indented children that follow it.
    let mut blocks: Vec<Vec<WorkspaceListEntry>> = Vec::new();
    for entry in entries {
        match entry {
            WorkspaceListEntry::Workspace {
                indented: false, ..
            } => blocks.push(vec![entry]),
            WorkspaceListEntry::Workspace { indented: true, .. } => {
                if let Some(block) = blocks.last_mut() {
                    block.push(entry);
                } else {
                    blocks.push(vec![entry]);
                }
            }
        }
    }

    // Priority of a block is the max attention across its member workspaces.
    let block_priority = |block: &[WorkspaceListEntry]| -> u8 {
        block
            .iter()
            .map(|entry| match entry {
                WorkspaceListEntry::Workspace { ws_idx, .. } => app
                    .workspaces
                    .get(*ws_idx)
                    .map(|ws| {
                        let (state, seen) = ws.aggregate_state(&app.terminals);
                        workspace_attention_priority(state, seen)
                    })
                    .unwrap_or(0),
            })
            .max()
            .unwrap_or(0)
    };

    // Stable sort keeps within-group order and, for equal priority, the
    // original block order (i.e. by workspace number).
    blocks.sort_by_key(|block| std::cmp::Reverse(block_priority(block)));
    blocks.into_iter().flatten().collect()
}

/// One drawn block in the sidebar tree: a group card, or one agent nested
/// under it.
///
/// The tree is the whole left panel now. A group list and an agent list were
/// two panels before, which meant every name was printed twice — once as a
/// group, once as the label of each of that group's agents — and the group's
/// own glyph was never more than a summary of rows already on screen below it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SidebarRow {
    Group {
        ws_idx: usize,
        indented: bool,
    },
    Agent {
        ws_idx: usize,
        tab_idx: usize,
        pane_id: crate::layout::PaneId,
    },
}

/// The sidebar tree: every group in list order, each followed by its agents.
pub(crate) fn sidebar_rows(app: &AppState) -> Vec<SidebarRow> {
    let entries = workspace_list_entries(app);
    let mut rows = Vec::with_capacity(entries.len() * 2);
    for entry in entries {
        let WorkspaceListEntry::Workspace { ws_idx, indented } = entry;
        rows.push(SidebarRow::Group { ws_idx, indented });
        let Some(ws) = app.workspaces.get(ws_idx) else {
            continue;
        };
        let mut details = ws.pane_details(&app.terminals);
        // `priority` puts the agent that wants something first inside its own
        // group; the groups themselves are already ordered blocked-first.
        if matches!(app.agent_panel_sort, AgentPanelSort::Priority) {
            details.sort_by_key(|detail| {
                (
                    std::cmp::Reverse(workspace_attention_priority(detail.state, detail.seen)),
                    std::cmp::Reverse(detail.last_agent_state_change_seq),
                )
            });
        }
        for detail in details {
            rows.push(SidebarRow::Agent {
                ws_idx,
                tab_idx: detail.tab_idx,
                pane_id: detail.pane_id,
            });
        }
    }
    rows
}

/// How many rows this entry draws, and the blank row that follows it.
///
/// Agents sit flush under their group; the gap belongs between groups, which is
/// what makes a group and its agents read as one block.
fn sidebar_row_height(app: &AppState, rows: &[SidebarRow], idx: usize, narrow: bool) -> (u16, u16) {
    let height = match rows.get(idx) {
        Some(SidebarRow::Group { ws_idx, indented }) => match app.workspaces.get(*ws_idx) {
            Some(ws) if !*indented => workspace_row_height(ws, narrow),
            Some(_) => 1,
            None => return (0, 0),
        },
        Some(SidebarRow::Agent { .. }) => 1,
        None => return (0, 0),
    };
    let gap = u16::from(matches!(
        rows.get(idx.saturating_add(1)),
        Some(SidebarRow::Group { .. })
    ));
    (height, gap)
}

pub(crate) fn workspace_list_rect(area: Rect) -> Rect {
    expanded_sidebar_content(area)
}

pub(crate) fn workspace_list_body_rect(area: Rect, has_scrollbar: bool) -> Rect {
    if area.width == 0 || area.height <= WORKSPACE_SECTION_HEADER_ROWS {
        return Rect::default();
    }

    let body_y = area.y.saturating_add(WORKSPACE_SECTION_HEADER_ROWS);
    let footer_y = area.y + area.height.saturating_sub(1);
    let body_height = footer_y.saturating_sub(body_y);
    let body_width = area.width.saturating_sub(u16::from(has_scrollbar));
    Rect::new(area.x, body_y, body_width, body_height)
}

fn workspace_list_visible_count(app: &AppState, area: Rect, scroll: usize) -> usize {
    let body = workspace_list_body_rect(area, false);
    if body.width == 0 || body.height == 0 {
        return 0;
    }

    let mut used_rows = 0u16;
    let mut visible = 0usize;
    let rows = sidebar_rows(app);
    let narrow = sidebar_narrow(area.width);
    for idx in scroll..rows.len() {
        let (height, gap) = sidebar_row_height(app, &rows, idx, narrow);
        if height == 0 {
            continue;
        }
        if used_rows.saturating_add(height).saturating_add(gap) > body.height {
            break;
        }
        used_rows = used_rows.saturating_add(height).saturating_add(gap);
        visible += 1;
    }
    visible
}

pub(crate) fn workspace_list_scroll_metrics(
    app: &AppState,
    area: Rect,
) -> crate::pane::ScrollMetrics {
    let total_rows = sidebar_rows(app).len();
    let scroll = app.workspace_scroll.min(total_rows.saturating_sub(1));
    let viewport_rows = workspace_list_visible_count(app, area, scroll);
    let max_offset_from_bottom = total_rows.saturating_sub(viewport_rows);
    let offset_from_bottom = total_rows
        .saturating_sub(scroll)
        .saturating_sub(viewport_rows);

    crate::pane::ScrollMetrics {
        offset_from_bottom,
        max_offset_from_bottom,
        viewport_rows,
    }
}

pub(crate) fn workspace_list_scrollbar_rect(app: &AppState, area: Rect) -> Option<Rect> {
    let metrics = workspace_list_scroll_metrics(app, area);
    let body = workspace_list_body_rect(area, true);
    (should_show_scrollbar(metrics) && body.width > 0 && body.height > 0).then_some(Rect::new(
        area.x + area.width.saturating_sub(1),
        body.y,
        1,
        body.height,
    ))
}

pub(crate) fn compute_workspace_list_areas(
    app: &AppState,
    area: Rect,
) -> (Vec<crate::app::state::WorkspaceCardArea>, Vec<()>) {
    let ws_area = workspace_list_rect(area);
    if ws_area == Rect::default() {
        return (Vec::new(), Vec::new());
    }

    let metrics = workspace_list_scroll_metrics(app, ws_area);
    let body = workspace_list_body_rect(ws_area, should_show_scrollbar(metrics));
    if body.width == 0 || body.height == 0 {
        return (Vec::new(), Vec::new());
    }

    let mut row_y = body.y;
    let body_bottom = body.y + body.height;
    let mut cards = Vec::new();
    let headers = Vec::new();

    let rows = sidebar_rows(app);
    let narrow = sidebar_narrow(ws_area.width);
    for idx in app.workspace_scroll..rows.len() {
        let (height, gap) = sidebar_row_height(app, &rows, idx, narrow);
        if height == 0 {
            continue;
        }
        if row_y.saturating_add(height).saturating_add(gap) > body_bottom {
            break;
        }
        let (ws_idx, indented, kind) = match rows[idx] {
            SidebarRow::Group { ws_idx, indented } => (
                ws_idx,
                indented,
                crate::app::state::WorkspaceCardKind::Group,
            ),
            SidebarRow::Agent {
                ws_idx,
                tab_idx,
                pane_id,
            } => (
                ws_idx,
                matches!(
                    rows.get(idx.saturating_sub(1)),
                    Some(SidebarRow::Group { indented: true, .. })
                ),
                crate::app::state::WorkspaceCardKind::Agent { tab_idx, pane_id },
            ),
        };
        cards.push(crate::app::state::WorkspaceCardArea {
            ws_idx,
            rect: Rect::new(body.x, row_y, body.width, height),
            indented,
            kind,
        });
        row_y = row_y.saturating_add(height + gap);
    }

    (cards, headers)
}

pub(crate) fn compute_workspace_card_areas(
    app: &AppState,
    area: Rect,
) -> Vec<crate::app::state::WorkspaceCardArea> {
    compute_workspace_list_areas(app, area).0
}

/// Auto-scale sidebar width based on workspace identity + agent summary.
pub(crate) fn collapsed_sidebar_sections(area: Rect) -> (Rect, Option<u16>, Rect) {
    let content = Rect::new(area.x, area.y, area.width.saturating_sub(1), area.height);
    if content.width == 0 || content.height == 0 {
        return (Rect::default(), None, Rect::default());
    }

    if content.height < 7 {
        return (content, None, Rect::default());
    }

    let total_h = content.height as usize;
    let ws_h = total_h.div_ceil(2);
    let detail_h = total_h.saturating_sub(ws_h + 1);
    if ws_h == 0 || detail_h == 0 {
        return (content, None, Rect::default());
    }

    let divider_y = content.y + ws_h as u16;
    let ws_area = Rect::new(content.x, content.y, content.width, ws_h as u16);
    let detail_area = Rect::new(content.x, divider_y + 1, content.width, detail_h as u16);
    (ws_area, Some(divider_y), detail_area)
}

/// Collapsed sidebar: workspace glance on top, compact agent list below.
pub(super) fn render_sidebar_collapsed(app: &AppState, frame: &mut Frame, area: Rect) {
    if area.width == 0 || area.height == 0 {
        return;
    }

    let is_navigating = matches!(app.mode, Mode::Navigate);

    let p = &app.palette;
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };
    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    let (ws_area, divider_y, detail_area) = collapsed_sidebar_sections(area);
    if ws_area == Rect::default() {
        render_sidebar_toggle(app, frame, area, true, p);
        return;
    }

    for (row, (visible_idx, ws)) in app
        .workspaces
        .iter()
        .enumerate()
        .filter(|(_, ws)| !ws.is_system())
        .enumerate()
    {
        let y = ws_area.y + row as u16;
        if y >= ws_area.y + ws_area.height {
            break;
        }
        let (agg_state, agg_seen) = ws.aggregate_state(&app.terminals);
        let (icon, icon_style) = agent_icon(agg_state, agg_seen, app.spinner_tick, p);
        let is_selected = visible_idx == app.selected && is_navigating;
        let is_active = Some(visible_idx) == app.active;
        let row_style = if is_selected {
            Style::default().bg(p.surface0)
        } else if is_active {
            Style::default().bg(p.surface_dim)
        } else {
            Style::default()
        };
        let num_style = if is_selected {
            Style::default().fg(p.overlay1).bg(p.surface0)
        } else if is_active {
            Style::default().fg(p.text).bg(p.surface_dim)
        } else {
            Style::default().fg(p.overlay0)
        };

        if is_selected || is_active {
            let buf = frame.buffer_mut();
            for x in ws_area.x..ws_area.x + ws_area.width {
                buf[(x, y)].set_style(row_style);
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(format!("{}", visible_idx + 1), num_style),
                Span::styled(" ", row_style),
                Span::styled(icon, icon_style),
            ])),
            Rect::new(ws_area.x, y, ws_area.width, 1),
        );
    }

    if let Some(divider_y) = divider_y {
        let buf = frame.buffer_mut();
        for x in ws_area.x..ws_area.x + ws_area.width {
            buf[(x, divider_y)].set_symbol("─");
            buf[(x, divider_y)].set_style(Style::default().fg(p.surface_dim));
        }
    }

    let detail_content_area = Rect::new(
        detail_area.x,
        detail_area.y,
        detail_area.width,
        detail_area.height.saturating_sub(1),
    );
    if detail_content_area != Rect::default() {
        for (detail_idx, detail) in agent_panel_entries(app).iter().enumerate() {
            let y = detail_content_area.y + detail_idx as u16;
            if y >= detail_content_area.y + detail_content_area.height {
                break;
            }
            let pane_num = app
                .workspaces
                .get(detail.ws_idx)
                .and_then(|ws| ws.public_pane_number(detail.pane_id))
                .unwrap_or(detail_idx + 1);
            let pane_style = Style::default().fg(p.overlay0);
            let (icon, icon_style) = agent_icon_for(
                detail.state,
                detail.seen,
                detail.manual_state.as_ref(),
                app.spinner_tick,
                p,
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(format!("{pane_num}"), pane_style),
                    Span::styled(" ", pane_style),
                    Span::styled(icon, icon_style),
                ])),
                Rect::new(detail_content_area.x, y, detail_content_area.width, 1),
            );
        }
    }

    render_sidebar_toggle(app, frame, area, true, p);
}

pub(crate) fn workspace_drop_indicator_row(
    cards: &[crate::app::state::WorkspaceCardArea],
    area: Rect,
    insert_idx: usize,
) -> Option<u16> {
    if area.height == 0 {
        return None;
    }
    let list_bottom = area.y + area.height.saturating_sub(1);

    let first = cards.first()?;
    if insert_idx == first.ws_idx {
        return first.rect.y.checked_sub(1).filter(|y| *y < list_bottom);
    }

    if let Some(row) = cards
        .last()
        .filter(|card| insert_idx == card.ws_idx.saturating_add(1))
        .map(|card| card.rect.y.saturating_add(card.rect.height))
        .filter(|y| *y < list_bottom)
    {
        return Some(row);
    }

    if let Some(card) = cards.iter().find(|card| card.ws_idx == insert_idx) {
        return card.rect.y.checked_sub(1).filter(|y| *y < list_bottom);
    }

    None
}

pub(super) fn render_sidebar(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
) {
    let p = &app.palette;
    let is_navigating = matches!(app.mode, Mode::Navigate);
    let sep_style = if is_navigating {
        Style::default().fg(p.accent)
    } else {
        Style::default().fg(p.surface_dim)
    };

    let sep_x = area.x + area.width.saturating_sub(1);
    let buf = frame.buffer_mut();
    for y in area.y..area.y + area.height {
        buf[(sep_x, y)].set_symbol("│");
        buf[(sep_x, y)].set_style(sep_style);
    }

    render_workspace_list(
        app,
        terminal_runtimes,
        frame,
        workspace_list_rect(area),
        is_navigating,
    );
    render_sidebar_menu_glyph(app, frame, area, p);
    render_sidebar_toggle(app, frame, area, false, p);
}

/// The global menu's `≡` on the header row.
///
/// A click target only, so it goes with the mouse the way `«` beside it does.
/// It carries the attention badge the old `menu` button wore: peach — the
/// warning tier, an update is waiting — in the one cell it has, since a dot
/// before it would cost a second.
fn render_sidebar_menu_glyph(app: &AppState, frame: &mut Frame, area: Rect, p: &Palette) {
    if !app.mouse_capture {
        return;
    }
    let rect = sidebar_menu_glyph_rect(area);
    if rect == Rect::default() {
        return;
    }
    let style = if app.global_menu_attention_badge_visible() {
        Style::default().fg(p.peach).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(Paragraph::new(Span::styled(glyphs::MENU, style)), rect);
}

fn render_workspace_list(
    app: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    frame: &mut Frame,
    area: Rect,
    is_navigating: bool,
) {
    let p = &app.palette;
    let dragged_ws_idx = match app.drag.as_ref().map(|drag| &drag.target) {
        Some(crate::app::state::DragTarget::WorkspaceReorder { source_ws_idx, .. }) => {
            Some(*source_ws_idx)
        }
        _ => None,
    };
    let insertion_row = match app.drag.as_ref().map(|drag| &drag.target) {
        Some(crate::app::state::DragTarget::WorkspaceReorder {
            insert_idx: Some(insert_idx),
            ..
        }) => workspace_drop_indicator_row(&app.view.workspace_card_areas, area, *insert_idx),
        _ => None,
    };

    let list_bottom = area.y + area.height.saturating_sub(1);
    if area.height > 0 {
        frame.render_widget(
            Paragraph::new(Line::from(vec![Span::styled(
                " groups",
                Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
            )])),
            Rect::new(area.x, area.y, area.width, 1),
        );
        let toggle_rect = sidebar_sort_toggle_rect(area, app.agent_panel_sort);
        if toggle_rect != Rect::default() {
            frame.render_widget(
                Paragraph::new(Span::styled(
                    sidebar_sort_label(app.agent_panel_sort),
                    Style::default().fg(p.overlay0).add_modifier(Modifier::BOLD),
                ))
                .alignment(Alignment::Right),
                toggle_rect,
            );
        }
    }

    // Agents are looked up by pane so the tree can draw them in group order
    // regardless of how the flat agent ordering is sorted.
    let entries_by_pane: std::collections::HashMap<_, _> =
        agent_panel_entries_from(app, terminal_runtimes)
            .into_iter()
            .map(|entry| (entry.pane_id, entry))
            .collect();

    let metrics = workspace_list_scroll_metrics(app, area);
    let scrollbar_rect = workspace_list_scrollbar_rect(app, area);
    let cards = &app.view.workspace_card_areas;
    let narrow = sidebar_narrow(area.width);

    for card in cards {
        let i = card.ws_idx;
        let ws = &app.workspaces[i];
        let row_y = card.rect.y;
        let row_height = card.rect.height;
        let selected = i == app.selected && is_navigating;
        let is_active = Some(i) == app.active;
        let is_dragged = dragged_ws_idx == Some(i);
        let highlighted = selected || is_active || is_dragged;
        let (agg_state, agg_seen) = ws.aggregate_state(&app.terminals);

        let bg = highlighted.then_some(if selected {
            p.surface0
        } else if is_dragged {
            p.surface1
        } else {
            p.surface_dim
        });
        if let Some(bg) = bg {
            let buf = frame.buffer_mut();
            for y in row_y..row_y + row_height {
                if y >= list_bottom {
                    break;
                }
                for x in card.rect.x..card.rect.x + card.rect.width {
                    buf[(x, y)].set_style(Style::default().bg(bg));
                }
            }
            // Selection edge marker (matches the board's card marker); it
            // replaces the leading pad cell so row content never shifts.
            if selected && card.is_group() && row_y < list_bottom {
                buf[(card.rect.x, row_y)]
                    .set_symbol(glyphs::MARKER)
                    .set_style(Style::default().fg(p.accent).bg(bg));
            }
        }
        let row_style = bg.map(|bg| Style::default().bg(bg)).unwrap_or_default();

        if let Some((_, _, pane_id)) = card.agent() {
            if row_y < list_bottom {
                if let Some(entry) = entries_by_pane.get(&pane_id) {
                    render_agent_row(
                        app,
                        frame,
                        entry,
                        card.rect,
                        card.indented,
                        narrow,
                        row_style,
                    );
                }
            }
            continue;
        }

        let name_style = if selected || is_active || is_dragged {
            Style::default().fg(p.text).add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(p.subtext0)
        };

        let (icon, icon_style) = agent_icon(agg_state, agg_seen, app.spinner_tick, p);
        let label = ws.display_name_from(&app.terminals, terminal_runtimes);
        let mut line1 = Vec::new();
        let mut show_workspace_icon = true;
        if card.indented {
            line1.push(Span::styled("   ", Style::default()));
        } else if let Some((key, collapsed)) = workspace_parent_group_state(app, i) {
            let icon = if collapsed {
                glyphs::COLLAPSED
            } else {
                glyphs::EXPANDED
            };
            let (state_icon, state_style) = if collapsed {
                let (state, seen) = space_aggregate_state(app, &key);
                agent_icon(state, seen, app.spinner_tick, p)
            } else {
                (icon, Style::default().fg(p.accent))
            };
            line1.push(Span::styled(icon, Style::default().fg(p.accent)));
            if collapsed {
                line1.push(Span::styled(" ", Style::default()));
                line1.push(Span::styled(state_icon, state_style));
                show_workspace_icon = false;
            }
            line1.push(Span::styled(" ", Style::default()));
        } else {
            line1.push(Span::styled(" ", Style::default()));
        }
        if show_workspace_icon {
            line1.push(Span::styled(icon, icon_style));
            line1.push(Span::styled(" ", Style::default()));
        }
        if card.indented {
            let display_label = grouped_child_display_label(
                &label,
                ws.branch().as_deref(),
                ws.custom_name.is_some(),
            );
            line1.push(Span::styled(display_label, name_style));
        } else {
            line1.push(Span::styled(label, name_style));
        }
        // Review-state badge (M3): compact glyph after the name.
        {
            if let Some((glyph, style)) = super::status::review_badge(ws.review_state, p) {
                line1.push(Span::styled(" ", Style::default()));
                line1.push(Span::styled(glyph, style));
            }
        }
        // Queued-input badge (M5 tab-to-queue): prompts waiting for idle.
        {
            let queued = app.queued_input_count_for_workspace(i);
            if queued > 0 {
                line1.push(Span::styled(" ", Style::default()));
                line1.push(Span::styled(
                    format!("{}{queued}", glyphs::QUEUED),
                    Style::default().fg(p.teal),
                ));
            }
        }

        frame.render_widget(
            Paragraph::new(Line::from(line1)),
            Rect::new(card.rect.x, row_y, card.rect.width, 1),
        );

        if row_height > 1 && row_y + 1 < list_bottom {
            if let Some(branch) = ws.branch() {
                // The branch and where it stands against upstream, and nothing
                // else: the event age and the memory nudge were host facts on a
                // group row, and they live on the board now. The branch is
                // the one fact here that truncates; the two badges after it
                // come off whole, the ahead one last.
                let branch_indent = if card.indented { "     " } else { "   " };
                let branch_color = if selected || is_active {
                    p.mauve
                } else {
                    p.overlay0
                };
                let mut spans = vec![Span::styled(branch_indent, Style::default())];
                spans.extend(group_branch_row(
                    &branch,
                    ws.git_ahead_behind(),
                    (card.rect.width as usize).saturating_sub(display_width(branch_indent) + 1),
                    branch_color,
                    p,
                ));
                frame.render_widget(
                    Paragraph::new(Line::from(spans)),
                    Rect::new(card.rect.x, row_y + 1, card.rect.width, 1),
                );
            }
        }
    }

    if let Some(y) = insertion_row.filter(|y| *y < list_bottom) {
        let indicator_right = scrollbar_rect
            .map(|rect| rect.x)
            .unwrap_or(area.x + area.width);
        let buf = frame.buffer_mut();
        for x in area.x..indicator_right {
            buf[(x, y)].set_symbol("─");
            buf[(x, y)].set_style(Style::default().fg(p.accent));
        }
    }

    if let Some(track) = scrollbar_rect {
        render_scrollbar(
            frame,
            metrics,
            track,
            p.surface_dim,
            p.overlay0,
            glyphs::RULE_LEFT,
        );
    }

    if app.mouse_capture && list_bottom > area.y {
        // One button, the width of the footer: `«` moved up to the header
        // and the menu is the `≡` beside it, so the row is the new-group
        // affordance and nothing else.
        let new_rect = app.sidebar_new_button_rect();
        frame.render_widget(
            Paragraph::new(Span::styled(
                format!(" {}", sidebar_new_button_label(new_rect.width)),
                Style::default().fg(p.overlay0),
            )),
            new_rect,
        );
    }
}

/// A group's second row: the branch, then `↑N` and `↓N` when the group
/// stands ahead of or behind its upstream.
///
/// `width` is the room after the indent. The branch truncates to what the
/// badges leave it, down to `MIN_BRANCH_WIDTH`; past that the badges drop
/// whole, behind before ahead, rather than clipping.
fn group_branch_row<'a>(
    branch: &str,
    ahead_behind: Option<(usize, usize)>,
    width: usize,
    branch_color: ratatui::style::Color,
    p: &Palette,
) -> Vec<Span<'a>> {
    let mut badges: Vec<Vec<Span<'a>>> = Vec::new();
    if let Some((ahead, behind)) = ahead_behind {
        if ahead > 0 {
            badges.push(vec![Span::styled(
                format!("{}{ahead}", glyphs::AHEAD),
                Style::default().fg(p.green),
            )]);
        }
        if behind > 0 {
            badges.push(vec![Span::styled(
                format!("{}{behind}", glyphs::BEHIND),
                Style::default().fg(p.peach),
            )]);
        }
    }
    let badges_reserved: usize = badges
        .iter()
        .map(|badge| super::text::spans_width(badge) + 1)
        .sum();
    let branch_budget = width
        .saturating_sub(badges_reserved)
        .max(MIN_BRANCH_WIDTH.min(width));
    let mut facts = vec![vec![Span::styled(
        truncate_end(branch, branch_budget),
        Style::default().fg(branch_color),
    )]];
    facts.extend(badges);
    fit_strip(facts, &Span::raw(" "), width)
}

/// One agent, on one row, under the group it belongs to.
///
/// The group above it already says where it lives, so this row says only what
/// the group cannot: which agent and how it is doing. The context gauge is
/// the pane title's and the board's; it was a bare `72%` here, a number with
/// no meter, on the one surface that could least afford the columns. Facts
/// trail to the right edge and drop whole rather than truncate; in the narrow
/// tree there are none, and the glyph and its colour carry the state.
fn render_agent_row(
    app: &AppState,
    frame: &mut Frame,
    entry: &AgentPanelEntry,
    rect: Rect,
    indented: bool,
    narrow: bool,
    row_style: Style,
) {
    let p = &app.palette;
    let is_active = app.is_active_pane(entry.ws_idx, entry.tab_idx, entry.pane_id);

    let (icon, icon_style) = agent_icon_for(
        entry.state,
        entry.seen,
        entry.manual_state.as_ref(),
        app.spinner_tick,
        p,
    );
    let status_color = entry
        .manual_state
        .as_ref()
        .map(|manual| manual_state_appearance(manual, 0).color(p))
        .unwrap_or_else(|| state_label_color(entry.state, entry.seen, p));
    let status = entry
        .state_labels
        .get(agent_panel_status_key(entry.state, entry.seen))
        .map(String::as_str)
        .unwrap_or_else(|| state_label(entry.state, entry.seen));

    let name = entry
        .primary_tab_label
        .as_deref()
        .filter(|label| !label.trim().is_empty())
        .or(entry.agent_label.as_deref())
        .filter(|label| !label.trim().is_empty())
        .unwrap_or("agent");
    let name_style = if is_active {
        Style::default().fg(p.text).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.subtext0)
    };
    let muted = Style::default().fg(p.overlay0).add_modifier(Modifier::DIM);

    let indent = if indented { "     " } else { "   " };
    let fixed = display_width(indent) + display_width(icon) + 1;
    let width = rect.width as usize;
    // Trailing facts drop whole, cheapest first, until the name has room to
    // say which agent this is.
    let mut trailing: Vec<(String, Style)> = Vec::new();
    if !narrow {
        trailing.push((status.to_string(), Style::default().fg(status_color)));
        if let Some(custom_status) = &entry.custom_status {
            trailing.push((custom_status.clone(), muted));
        }
    }

    let facts_width = |facts: &[(String, Style)]| -> usize {
        facts
            .iter()
            .map(|(text, _)| display_width(text))
            .sum::<usize>()
            + facts.len().saturating_sub(1)
    };

    // The name says which agent this is, so facts give way to it: drop the
    // cheapest whole fact until the name fits, and only truncate once the state
    // word is all that is left.
    let name_width = display_width(name);
    while trailing.len() > 1 && fixed + name_width + 1 + facts_width(&trailing) + 1 > width {
        trailing.pop();
    }
    let facts = facts_width(&trailing);
    let name_budget = if trailing.is_empty() {
        width.saturating_sub(fixed + 1)
    } else {
        width.saturating_sub(fixed + facts + 2)
    }
    .max(1);
    let name = truncate_end(name, name_budget);

    let mut spans = vec![
        Span::styled(indent, Style::default()),
        Span::styled(icon, icon_style),
        Span::styled(" ", Style::default()),
        Span::styled(name.clone(), name_style),
    ];
    if !trailing.is_empty() {
        // Pin the facts one column in from the right edge, as every other
        // strip does.
        let used = fixed + display_width(&name);
        let pad = width
            .saturating_sub(1)
            .saturating_sub(facts)
            .saturating_sub(used)
            .max(1);
        spans.push(Span::styled(" ".repeat(pad), Style::default()));
        for (idx, (text, style)) in trailing.into_iter().enumerate() {
            if idx > 0 {
                spans.push(Span::styled(" ", Style::default()));
            }
            spans.push(Span::styled(text, style));
        }
    }

    frame.render_widget(Paragraph::new(Line::from(spans)).style(row_style), rect);
}

pub(crate) fn collapsed_sidebar_toggle_rect(area: Rect) -> Rect {
    let bottom_y = area.y + area.height.saturating_sub(1);
    let content_w = area.width.saturating_sub(1);
    if content_w == 0 || area.height == 0 {
        return Rect::default();
    }
    let x = area.x + content_w / 2;
    Rect::new(x, bottom_y, 1, 1)
}

/// The expanded sidebar's `«`: the header row's last content column, beside
/// the `≡` of `sidebar_menu_glyph_rect`. `area` is the whole sidebar.
pub(crate) fn expanded_sidebar_toggle_rect(area: Rect) -> Rect {
    if area.width <= 1 || area.height == 0 {
        return Rect::default();
    }
    Rect::new(area.x + area.width.saturating_sub(2), area.y, 1, 1)
}

fn render_sidebar_toggle(
    app: &AppState,
    frame: &mut Frame,
    area: Rect,
    collapsed: bool,
    p: &Palette,
) {
    // Expanded, `«` is purely a click target, and with `mouse_capture = false`
    // shep never hears the click — so it goes, the same way the `≡` beside it
    // and the `+ new group` footer do. Collapsed, `»` stays whatever the mouse
    // is doing: it is the only remaining evidence that a sidebar exists, and it
    // is where the attention badge lights up.
    if !collapsed && !app.mouse_capture {
        return;
    }
    let toggle_area = if collapsed {
        collapsed_sidebar_toggle_rect(area)
    } else {
        expanded_sidebar_toggle_rect(area)
    };
    if toggle_area == Rect::default() {
        return;
    }
    let icon = if collapsed {
        glyphs::SIDEBAR_EXPAND
    } else {
        glyphs::SIDEBAR_COLLAPSE
    };
    let icon_style = if collapsed && app.global_menu_attention_badge_visible() {
        Style::default().fg(p.accent).add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(p.overlay0)
    };
    frame.render_widget(Paragraph::new(Span::styled(icon, icon_style)), toggle_area);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{detect::Agent, workspace::Workspace};
    use ratatui::{backend::TestBackend, Terminal};

    #[test]
    fn strip_workspace_prefix_drops_the_group_name_it_repeats() {
        assert_eq!(
            strip_workspace_prefix("ShiftMayt Legal", "ShiftMayt"),
            "Legal"
        );
        assert_eq!(strip_workspace_prefix("shiftmayt-dev", "ShiftMayt"), "dev");
        assert_eq!(
            strip_workspace_prefix(
                &format!("ShiftMayt{}Legal", glyphs::SEP_SPACED),
                "ShiftMayt"
            ),
            "Legal"
        );
    }

    #[test]
    fn strip_workspace_prefix_keeps_names_that_only_look_like_the_group() {
        // A longer word that merely starts with the group name is its own name.
        assert_eq!(
            strip_workspace_prefix("ShiftMaytics", "ShiftMayt"),
            "ShiftMaytics"
        );
        // Nothing would be left, so nothing is dropped.
        assert_eq!(
            strip_workspace_prefix("ShiftMayt", "ShiftMayt"),
            "ShiftMayt"
        );
        assert_eq!(strip_workspace_prefix("claude", "ShiftMayt"), "claude");
        assert_eq!(strip_workspace_prefix("claude", "   "), "claude");
    }

    #[test]
    fn render_sidebar_toggle_draws_expanded_collapse_icon() {
        let app = crate::app::state::AppState::test_new();
        let area = Rect::new(0, 0, 26, 20);
        let mut terminal =
            Terminal::new(TestBackend::new(26, 20)).expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_toggle(&app, frame, area, false, &app.palette))
            .expect("sidebar toggle should render");

        let toggle = expanded_sidebar_toggle_rect(area);
        assert_eq!(
            terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
            "«"
        );
    }

    /// With no mouse, the expanded collapse button is not drawn — but the
    /// collapsed one still is, because it is the only thing left saying there
    /// is a sidebar, and it carries the attention badge.
    #[test]
    fn the_collapse_button_needs_a_mouse_but_the_reopen_hint_does_not() {
        let mut app = crate::app::state::AppState::test_new();
        app.mouse_capture = false;
        let area = Rect::new(0, 0, 26, 20);

        for (collapsed, expected) in [(false, " "), (true, "\u{bb}")] {
            let mut terminal =
                Terminal::new(TestBackend::new(26, 20)).expect("test terminal should initialize");
            terminal
                .draw(|frame| render_sidebar_toggle(&app, frame, area, collapsed, &app.palette))
                .expect("sidebar toggle should render");
            let toggle = if collapsed {
                collapsed_sidebar_toggle_rect(area)
            } else {
                expanded_sidebar_toggle_rect(area)
            };
            assert_eq!(
                terminal.backend().buffer()[(toggle.x, toggle.y)].symbol(),
                expected,
                "collapsed={collapsed}"
            );
        }
    }

    /// `≡` and `«` share the header row: the last two content cells with a
    /// space between, and neither touches the separator.
    #[test]
    fn menu_glyph_and_collapse_share_row_zero() {
        let area = Rect::new(0, 3, 26, 20);
        let toggle = expanded_sidebar_toggle_rect(area);
        let menu = sidebar_menu_glyph_rect(area);

        assert_eq!(toggle, Rect::new(24, 3, 1, 1));
        assert_eq!(menu, Rect::new(22, 3, 1, 1));
        assert_eq!(toggle.y, area.y, "the toggle sits on the header row");
        assert_eq!(menu.y, area.y);
        assert!(menu.x + 1 < toggle.x, "a space between the two glyphs");
        assert!(toggle.x < area.x + area.width - 1, "inside the content");

        let rows = sidebar_screen(26, 20, true);
        assert_eq!(&rows[0][22..], "≡ «", "{:?}", rows[0]);
        // Without a mouse neither is drawn: both are click targets only.
        let rows = sidebar_screen(26, 20, false);
        assert_eq!(&rows[0][22..], "   ", "{:?}", rows[0]);
    }

    /// The `≡` wears the attention badge the old `menu` button did, in its
    /// one cell: peach, the warning tier, an update is waiting.
    #[test]
    fn menu_glyph_carries_the_attention_badge_in_peach() {
        let mut app = crate::app::state::AppState::test_new();
        app.update_available = Some("0.9.0".into());
        let area = Rect::new(0, 0, 26, 20);
        let mut terminal =
            Terminal::new(TestBackend::new(26, 20)).expect("test terminal should initialize");
        terminal
            .draw(|frame| render_sidebar_menu_glyph(&app, frame, area, &app.palette))
            .expect("menu glyph should render");
        let menu = sidebar_menu_glyph_rect(area);
        let cell = &terminal.backend().buffer()[(menu.x, menu.y)];
        assert_eq!(cell.symbol(), glyphs::MENU);
        assert_eq!(cell.fg, app.palette.peach);
        assert!(cell.modifier.contains(Modifier::BOLD));

        app.update_available = None;
        terminal
            .draw(|frame| render_sidebar_menu_glyph(&app, frame, area, &app.palette))
            .expect("menu glyph should render");
        let cell = &terminal.backend().buffer()[(menu.x, menu.y)];
        assert_eq!(cell.fg, app.palette.overlay0);
    }

    /// A 26-column sidebar keeps its sort toggle, left of ` ≡ «`; a narrower
    /// one has no room for it beside ` groups` and drops it whole.
    #[test]
    fn sort_toggle_hidden_below_26() {
        let list = workspace_list_rect(Rect::new(0, 0, 26, 20));
        let toggle = sidebar_sort_toggle_rect(list, AgentPanelSort::Grouped);
        assert_eq!(toggle, Rect::new(14, 0, 7, 1));
        assert_eq!(
            sidebar_sort_toggle_rect(list, AgentPanelSort::Priority),
            Rect::new(13, 0, 8, 1)
        );
        let rows = sidebar_screen(26, 20, true);
        assert_eq!(rows[0], " groups       grouped ≡ «");

        let list = workspace_list_rect(Rect::new(0, 0, 25, 20));
        assert_eq!(
            sidebar_sort_toggle_rect(list, AgentPanelSort::Grouped),
            Rect::default()
        );
        let rows = sidebar_screen(25, 20, true);
        assert!(!rows[0].contains("grouped"), "{:?}", rows[0]);
        assert!(rows[0].contains("≡ «"), "{:?}", rows[0]);
    }

    /// The sidebar drawn from the snapshot fixture at a given size, one
    /// string per row, without the separator column.
    fn sidebar_screen(width: u16, height: u16, mouse: bool) -> Vec<String> {
        let mut app = crate::ui::snapshot::fixture::session();
        app.mouse_capture = mouse;
        let area = Rect::new(0, 0, width, height);
        app.view.sidebar_rect = area;
        app.view.workspace_card_areas = compute_workspace_card_areas(&app, area);
        let mut terminal = Terminal::new(TestBackend::new(width, height))
            .expect("test terminal should initialize");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| render_sidebar(&app, &runtimes, frame, area))
            .expect("sidebar should render");
        let buffer = terminal.backend().buffer();
        (0..height)
            .map(|y| {
                (0..width.saturating_sub(1))
                    .map(|x| buffer[(x, y)].symbol())
                    .collect()
            })
            .collect()
    }

    /// A group's second row is the branch and where it stands against
    /// upstream, and nothing else: the fixture's `workmayt` is 2 ahead, 1
    /// behind, at 87% of its memory cap, with an agent event 20s ago, and
    /// only the first two of those are on the row.
    #[test]
    fn group_row_shows_branch_and_upstream_only() {
        let rows = sidebar_screen(26, 24, true);
        assert_eq!(rows[3].trim_end(), "   fix/stripe-web… ↑2 ↓1");
        assert!(!rows[3].contains("mem"), "{:?}", rows[3]);
        assert!(!rows[3].contains("20s"), "{:?}", rows[3]);
        // `shep` is only behind: one badge, and the branch keeps its room.
        let shep = rows
            .iter()
            .find(|row| row.contains("feat/design-pass"))
            .expect("the shep group's branch row");
        assert_eq!(shep.trim_end(), "   feat/design-pass ↓4");
        // Wide enough, the branch is whole and the badges follow it.
        let wide = sidebar_screen(36, 24, true);
        assert_eq!(wide[3].trim_end(), "   fix/stripe-webhook ↑2 ↓1");
    }

    /// The branch truncates; the badges come off whole, behind before ahead.
    #[test]
    fn group_branch_row_drops_badges_whole_before_the_branch_floor() {
        let p = Palette::shep();
        let text = |width: usize| -> String {
            group_branch_row("fix/stripe-webhook", Some((2, 1)), width, p.mauve, &p)
                .iter()
                .map(|s| s.content.as_ref())
                .collect()
        };
        assert_eq!(text(30), "fix/stripe-webhook ↑2 ↓1");
        assert_eq!(text(15), "fix/stri… ↑2 ↓1");
        // Under the branch's floor the badges go rather than the branch.
        assert_eq!(text(9), "fix/s… ↑2");
        assert_eq!(text(6), "fix/s…");
        for width in 0..40 {
            let row = text(width);
            assert!(display_width(&row) <= width, "{width}: {row:?}");
            assert!(!row.ends_with(' '), "{width}: {row:?}");
        }
    }

    /// An agent row is glyph, name and state word — the context percentage
    /// is the pane title's and the board's now.
    #[test]
    fn agent_row_drops_the_context_percent() {
        let rows = sidebar_screen(26, 24, true);
        assert_eq!(rows[4].trim_end(), "   ◉ claude      blocked");
        assert!(
            rows.iter().all(|row| !row.contains('%')),
            "no row carries a percentage: {rows:#?}"
        );
    }

    /// At twenty content columns and under, a group is one row and an agent
    /// row is glyph and name: the glyph and its colour carry the state.
    #[test]
    fn narrow_sidebar_is_one_row_per_group_and_no_state_word() {
        assert!(sidebar_narrow(20));
        assert!(!sidebar_narrow(21));

        // 21 columns: 20 of content, narrow.
        let rows = sidebar_screen(21, 24, true);
        assert_eq!(rows[2].trim_end(), " ◉ workmayt ◆ ⇥2");
        assert_eq!(rows[3].trim_end(), "   ◉ claude");
        assert_eq!(rows[4].trim_end(), "   ⠹ opencode");
        assert_eq!(rows[5].trim_end(), "");
        assert_eq!(rows[6].trim_end(), " ● emberline");
        assert!(
            rows.iter()
                .all(|row| !row.contains("blocked") && !row.contains("fix/")),
            "{rows:#?}"
        );
        assert_eq!(rows[23].trim_end(), " + new group");

        // One column more and the branch rows and state words are back.
        let rows = sidebar_screen(22, 24, true);
        assert!(rows[3].contains("fix/"), "{:?}", rows[3]);
        assert!(rows[4].contains("blocked"), "{:?}", rows[4]);
    }

    fn workspace_visible_order(app: &AppState) -> Vec<usize> {
        workspace_list_entries(app)
            .into_iter()
            .map(|entry| match entry {
                WorkspaceListEntry::Workspace { ws_idx, .. } => ws_idx,
            })
            .collect()
    }

    fn set_pane_state(app: &mut AppState, ws_idx: usize, state: AgentState) {
        let pane = app.workspaces[ws_idx].tabs[0].root_pane;
        let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = state;
    }

    #[test]
    fn format_event_age_uses_compact_units() {
        use std::time::Duration;
        assert_eq!(format_event_age(Duration::from_secs(0)), "0s");
        assert_eq!(format_event_age(Duration::from_secs(3)), "3s");
        assert_eq!(format_event_age(Duration::from_secs(125)), "2m");
        assert_eq!(format_event_age(Duration::from_secs(3 * 3600 + 5)), "3h");
        assert_eq!(format_event_age(Duration::from_secs(2 * 86_400 + 60)), "2d");
    }

    #[test]
    fn workspace_list_surfaces_blocked_space_first() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![
            Workspace::test_new("one"),
            Workspace::test_new("two"),
            Workspace::test_new("three"),
        ];
        app.ensure_test_terminals();
        set_pane_state(&mut app, 0, AgentState::Working);
        set_pane_state(&mut app, 1, AgentState::Blocked);
        set_pane_state(&mut app, 2, AgentState::Idle);

        // Blocked (1) surfaces first, then working (0), then idle (2).
        assert_eq!(workspace_visible_order(&app), vec![1, 0, 2]);
    }

    #[test]
    fn workspace_list_ordering_is_stable_within_equal_priority() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![
            Workspace::test_new("one"),
            Workspace::test_new("two"),
            Workspace::test_new("three"),
        ];
        app.ensure_test_terminals();
        // All unknown -> equal priority -> original order preserved (no thrash).
        assert_eq!(workspace_visible_order(&app), vec![0, 1, 2]);

        // Two blocked spaces keep their relative order; both float above idle.
        set_pane_state(&mut app, 0, AgentState::Blocked);
        set_pane_state(&mut app, 2, AgentState::Blocked);
        assert_eq!(workspace_visible_order(&app), vec![0, 2, 1]);
    }

    #[test]
    fn all_workspaces_agent_panel_entries_use_workspace_and_optional_tab_labels() {
        let mut app = crate::app::state::AppState::test_new();
        let first = Workspace::test_new("one");
        let first_pane = first.tabs[0].root_pane;
        let mut second = Workspace::test_new("two");
        let second_tab = second.test_add_tab(Some("logs"));
        let second_pane = second.tabs[second_tab].root_pane;

        app.workspaces = vec![first, second];
        app.ensure_test_terminals();
        let first_terminal_id = app.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        let second_terminal_id = app.workspaces[1].tabs[second_tab].panes[&second_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&second_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Claude);
        app.active = Some(0);
        app.selected = 0;

        let entries = agent_panel_entries(&app);
        assert_eq!(entries[0].primary_label, "one");
        assert!(entries[0].primary_tab_label.is_none());
        assert_eq!(entries[0].agent_label.as_deref(), Some("pi"));
        assert_eq!(entries[1].primary_label, "two");
        assert_eq!(entries[1].primary_tab_label.as_deref(), Some("logs"));
        assert_eq!(entries[1].agent_label.as_deref(), Some("claude"));
    }

    #[test]
    fn priority_agent_panel_sort_uses_attention_then_space_order() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![
            Workspace::test_new("one"),
            Workspace::test_new("two"),
            Workspace::test_new("three"),
            Workspace::test_new("four"),
        ];
        app.ensure_test_terminals();
        app.active = Some(0);
        app.selected = 0;
        app.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;

        let set_state = |app: &mut crate::app::state::AppState, ws_idx: usize, state| {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            let terminal = app.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = state;
        };
        set_state(&mut app, 0, AgentState::Working);
        set_state(&mut app, 1, AgentState::Idle);
        set_state(&mut app, 2, AgentState::Working);
        set_state(&mut app, 3, AgentState::Blocked);

        let done_pane = app.workspaces[1].tabs[0].root_pane;
        app.workspaces[1].tabs[0]
            .panes
            .get_mut(&done_pane)
            .unwrap()
            .seen = false;

        let labels: Vec<String> = agent_panel_entries(&app)
            .into_iter()
            .map(|entry| entry.primary_label)
            .collect();

        assert_eq!(labels, ["four", "two", "one", "three"]);
    }

    #[test]
    fn collapsed_sidebar_uses_all_workspaces_agent_panel_order() {
        let mut app = crate::app::state::AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one"), Workspace::test_new("two")];
        app.ensure_test_terminals();
        app.active = Some(0);
        app.selected = 0;
        app.agent_panel_sort = crate::app::state::AgentPanelSort::Priority;

        let set_state = |app: &mut crate::app::state::AppState, ws_idx: usize, state| {
            let pane = app.workspaces[ws_idx].tabs[0].root_pane;
            let terminal_id = app.workspaces[ws_idx].tabs[0].panes[&pane]
                .attached_terminal_id
                .clone();
            let terminal = app.terminals.get_mut(&terminal_id).unwrap();
            terminal.detected_agent = Some(Agent::Claude);
            terminal.state = state;
        };
        set_state(&mut app, 0, AgentState::Working);
        set_state(&mut app, 1, AgentState::Blocked);

        let area = Rect::new(0, 0, 5, 12);
        let (_, _, detail_area) = collapsed_sidebar_sections(area);
        let first_detail_y = detail_area.y;
        let mut terminal = Terminal::new(TestBackend::new(area.width, area.height))
            .expect("test terminal should initialize");

        terminal
            .draw(|frame| render_sidebar_collapsed(&app, frame, area))
            .expect("collapsed sidebar should render");

        let buffer = terminal.backend().buffer();
        assert_eq!(buffer[(detail_area.x + 2, first_detail_y)].symbol(), "◉");
        assert_eq!(
            buffer[(detail_area.x + 2, first_detail_y)].style().fg,
            Some(app.palette.red)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn all_workspaces_agent_panel_entries_use_live_root_runtime_cwd_for_workspace_label() {
        let unique = format!(
            "shep-agent-panel-runtime-cwd-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );
        let root = std::env::temp_dir().join(unique);
        let stale_cwd = root.join("issue-264-nix-support");
        let live_cwd = root.join("shep");
        std::fs::create_dir_all(stale_cwd.join(".git")).unwrap();
        std::fs::create_dir_all(live_cwd.join(".git")).unwrap();

        let mut app = crate::app::state::AppState::test_new();
        let mut workspace = Workspace::test_new("stale-name");
        workspace.custom_name = None;
        workspace.identity_cwd = stale_cwd.clone();
        let pane = workspace.tabs[0].root_pane;

        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let terminal_id = app.workspaces[0].tabs[0].panes[&pane]
            .attached_terminal_id
            .clone();
        let terminal = app.terminals.get_mut(&terminal_id).unwrap();
        terminal.cwd = stale_cwd;
        terminal.detected_agent = Some(Agent::Pi);
        app.active = Some(0);
        app.selected = 0;

        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            pane,
            24,
            80,
            live_cwd.clone(),
            0,
            crate::terminal_theme::TerminalTheme::default(),
            crate::pane::PaneShellConfig::new("/bin/sh", crate::config::ShellModeConfig::NonLogin),
            &crate::pane::PaneLaunchEnv::default(),
            events,
            std::sync::Arc::new(tokio::sync::Notify::new()),
            std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        )
        .unwrap();

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while runtime.cwd() != Some(live_cwd.clone()) && std::time::Instant::now() < deadline {
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }

        let mut runtime_registry = TerminalRuntimeRegistry::new();
        runtime_registry.insert(terminal_id, runtime);
        let entries = agent_panel_entries_from(&app, &runtime_registry);
        let primary_label = entries[0].primary_label.clone();

        for (_, runtime) in runtime_registry.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(root);

        assert_eq!(primary_label, "shep");
    }

    #[test]
    fn all_workspaces_agent_panel_entries_prefer_agent_names_for_agent_identity() {
        let mut app = crate::app::state::AppState::test_new();
        let workspace = Workspace::test_new("bridge");
        let first_pane = workspace.tabs[0].root_pane;

        app.workspaces = vec![workspace];
        app.ensure_test_terminals();
        let first_terminal_id = app.workspaces[0].tabs[0].panes[&first_pane]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .detected_agent = Some(Agent::Pi);
        app.terminals
            .get_mut(&first_terminal_id)
            .unwrap()
            .set_agent_name("planner".into());
        app.active = Some(0);
        app.selected = 0;

        let entries = agent_panel_entries(&app);
        assert_eq!(entries[0].primary_label, "bridge");
        assert_eq!(entries[0].agent_label.as_deref(), Some("planner"));
    }

    /// The tree owns the whole panel; only the separator column is not its own.
    #[test]
    fn the_list_is_the_whole_sidebar_less_its_separator() {
        assert_eq!(
            workspace_list_rect(Rect::new(0, 0, 20, 5)),
            Rect::new(0, 0, 19, 5)
        );
    }

    #[test]
    fn grouped_child_label_keeps_custom_workspace_name() {
        assert_eq!(
            grouped_child_display_label("renamed issue", Some("worktree/issue-137"), true),
            "renamed issue"
        );
    }

    #[test]
    fn grouped_child_label_uses_short_branch_for_auto_named_workspace() {
        assert_eq!(
            grouped_child_display_label("shep-issue", Some("worktree/issue-137"), false),
            "issue-137"
        );
    }

    #[test]
    fn workspace_list_truncates_cjk_branch_without_panic() {
        let mut app = crate::app::state::AppState::test_new();
        let mut ws = Workspace::test_new("repo");
        ws.cached_git_branch = Some("feature/中文-分支-644".into());
        app.workspaces = vec![ws];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Terminal;
        app.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            kind: crate::app::state::WorkspaceCardKind::Group,
            ws_idx: 0,
            rect: Rect::new(0, 1, 15, 2),
            indented: false,
        }];

        let mut terminal = Terminal::new(TestBackend::new(15, 6)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();

        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 15, 6), false)
            })
            .expect("workspace list should render");
    }

    #[test]
    fn workspace_row_shows_queued_input_badge() {
        let mut app = crate::app::state::AppState::test_new();
        let ws = Workspace::test_new("repo");
        let root = ws.tabs[0].root_pane;
        app.workspaces = vec![ws];
        app.active = Some(0);
        app.selected = 0;
        app.mode = Mode::Terminal;
        app.queued_pane_input
            .insert(root, vec!["one".into(), "two".into()]);
        app.view.workspace_card_areas = vec![crate::app::state::WorkspaceCardArea {
            kind: crate::app::state::WorkspaceCardKind::Group,
            ws_idx: 0,
            rect: Rect::new(0, 1, 20, 2),
            indented: false,
        }];

        let mut terminal = Terminal::new(TestBackend::new(20, 6)).expect("test terminal");
        let runtimes = crate::terminal::TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| {
                render_workspace_list(&app, &runtimes, frame, Rect::new(0, 0, 20, 6), false)
            })
            .expect("workspace list should render");

        let buffer = terminal.backend().buffer();
        let row: String = (0..20).map(|x| buffer[(x, 1)].symbol()).collect();
        assert!(
            row.contains("\u{21e5}2"),
            "queued badge should render: {row:?}"
        );
    }

    fn workspace_with_worktree_space(
        name: &str,
        key: Option<&str>,
        checkout_key: &str,
    ) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_new(name);
        if let Some(key) = key {
            ws.worktree_space = Some(crate::workspace::WorktreeSpaceMembership {
                key: key.into(),
                label: "shep".into(),
                repo_root: std::path::PathBuf::from("/repo/shep"),
                checkout_path: std::path::PathBuf::from(checkout_key),
                is_linked_worktree: name != "main",
            });
        }
        ws
    }

    fn workspace_with_git_space(name: &str, key: &str) -> crate::workspace::Workspace {
        let mut ws = crate::workspace::Workspace::test_new(name);
        ws.cached_git_space = Some(crate::workspace::GitSpaceMetadata {
            key: key.into(),
            checkout_key: format!("/repo/{name}"),
            label: "shep".into(),
            repo_root: std::path::PathBuf::from(format!("/repo/{name}")),
            is_linked_worktree: false,
        });
        ws
    }

    #[test]
    fn parent_workspace_row_stays_clickable_when_grouped() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];

        let (cards, headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 20));

        assert!(headers.is_empty());
        assert_eq!(cards[0].ws_idx, 0);
        assert!(!cards[0].indented);
        assert_eq!(cards[1].ws_idx, 1);
        assert!(cards[1].indented);
        assert_eq!(cards[1].rect.y, cards[0].rect.y + cards[0].rect.height + 1);
    }

    #[test]
    fn linked_only_worktree_members_do_not_form_parentless_group() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
            workspace_with_worktree_space("review", Some("repo-key"), "/repo/shep-review"),
        ];

        let entries = workspace_list_entries(&app);

        assert_eq!(
            entries,
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false
                },
            ]
        );
    }

    fn detect_agent(
        app: &mut AppState,
        ws_idx: usize,
        tab_idx: usize,
        pane_id: crate::layout::PaneId,
        agent: Agent,
    ) {
        let terminal_id = app.workspaces[ws_idx].tabs[tab_idx].panes[&pane_id]
            .attached_terminal_id
            .clone();
        app.terminals
            .get_mut(&terminal_id)
            .expect("test terminal")
            .detected_agent = Some(agent);
    }

    /// The redundancy the tree exists to remove: a group and its agents were two
    /// lists, so every name was drawn twice.
    #[test]
    fn every_group_is_followed_by_its_own_agents() {
        let mut app = AppState::test_new();
        let mut first = Workspace::test_new("one");
        let second_tab = first.test_add_tab(Some("logs"));
        let second_pane = first.tabs[second_tab].root_pane;
        let first_pane = first.tabs[0].root_pane;
        app.workspaces = vec![first, Workspace::test_new("two")];
        app.ensure_test_terminals();
        let other_pane = app.workspaces[1].tabs[0].root_pane;
        detect_agent(&mut app, 0, 0, first_pane, Agent::Claude);
        detect_agent(&mut app, 0, second_tab, second_pane, Agent::Codex);
        detect_agent(&mut app, 1, 0, other_pane, Agent::Claude);

        assert_eq!(
            sidebar_rows(&app),
            vec![
                SidebarRow::Group {
                    ws_idx: 0,
                    indented: false
                },
                SidebarRow::Agent {
                    ws_idx: 0,
                    tab_idx: 0,
                    pane_id: first_pane
                },
                SidebarRow::Agent {
                    ws_idx: 0,
                    tab_idx: second_tab,
                    pane_id: second_pane
                },
                SidebarRow::Group {
                    ws_idx: 1,
                    indented: false
                },
                SidebarRow::Agent {
                    ws_idx: 1,
                    tab_idx: 0,
                    pane_id: other_pane
                },
            ]
        );
    }

    /// An agent row is a card in its own right, so clicking one can focus that
    /// pane without the sidebar keeping a second hit-test table for it.
    #[test]
    fn agent_rows_carry_the_pane_they_stand_for() {
        let mut app = AppState::test_new();
        app.workspaces = vec![Workspace::test_new("one")];
        app.ensure_test_terminals();
        let pane_id = app.workspaces[0].tabs[0].root_pane;
        detect_agent(&mut app, 0, 0, pane_id, Agent::Claude);

        let cards = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 12)).0;

        assert_eq!(
            cards
                .iter()
                .filter_map(|card| card.agent())
                .collect::<Vec<_>>(),
            vec![(0, 0, pane_id)]
        );
        assert_eq!(cards.iter().filter(|card| card.is_group()).count(), 1);
    }

    #[test]
    fn compact_space_group_scroll_offset_can_start_inside_group() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("one", Some("repo-key"), "/repo/shep-one"),
            workspace_with_worktree_space("two", Some("repo-key"), "/repo/shep-two"),
        ];
        let area = Rect::new(0, 0, 30, 20);
        app.workspace_scroll = normalized_workspace_scroll(&app, area, 2);

        let (cards, headers) = compute_workspace_list_areas(&app, area);

        assert!(headers.is_empty());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].ws_idx, 2);
    }

    #[test]
    fn workspace_scroll_metrics_count_display_entries_not_raw_workspaces() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
            Workspace::test_new("notes"),
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;

        let ws_area = Rect::new(0, 0, 30, 6);
        let metrics = workspace_list_scroll_metrics(&app, ws_area);

        assert_eq!(metrics.viewport_rows, 1);
        assert_eq!(metrics.max_offset_from_bottom, 1);
        assert_eq!(metrics.offset_from_bottom, 1);
    }

    #[test]
    fn workspace_scroll_offset_applies_to_group_children() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
            Workspace::test_new("notes"),
        ];
        app.collapsed_space_keys.insert("repo-key".into());
        app.active = None;
        app.mode = Mode::Terminal;
        app.workspace_scroll = 1;

        let (cards, headers) = compute_workspace_list_areas(&app, Rect::new(0, 0, 30, 12));

        assert!(headers.is_empty());
        assert_eq!(cards.len(), 1);
        assert_eq!(cards[0].ws_idx, 2);
    }

    #[test]
    fn workspace_list_entries_group_multiple_workspaces_in_same_git_space() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_group_non_contiguous_explicit_members() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_git_space("normal", "other-key"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_do_not_group_normal_git_workspaces() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_git_space("one", "repo-key"),
            workspace_with_git_space("two", "repo-key"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_do_not_auto_attach_normal_git_workspace_to_group() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_git_space("scratch", "repo-key"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 2,
                    indented: true,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                },
            ]
        );
    }

    #[test]
    fn workspace_list_entries_leave_single_git_and_non_git_workspaces_flat() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_git_space("one", "repo-key"),
            workspace_with_worktree_space("notes", None, "/notes"),
        ];

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: false,
                },
            ]
        );
    }

    #[test]
    fn collapsed_group_hides_inactive_children_but_keeps_active_visible() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];
        app.active = Some(1);
        app.mode = Mode::Terminal;
        app.collapsed_space_keys.insert("repo-key".into());

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                },
            ]
        );

        app.active = None;
        app.mode = Mode::Terminal;
        assert_eq!(
            workspace_list_entries(&app),
            vec![WorkspaceListEntry::Workspace {
                ws_idx: 0,
                indented: false,
            }]
        );
    }

    #[test]
    fn collapsed_group_keeps_selected_child_visible_in_navigate_mode() {
        let mut app = AppState::test_new();
        app.workspaces = vec![
            workspace_with_worktree_space("main", Some("repo-key"), "/repo/shep"),
            workspace_with_worktree_space("issue", Some("repo-key"), "/repo/shep-issue"),
        ];
        app.mode = Mode::Navigate;
        app.selected = 1;
        app.active = Some(1);
        app.collapsed_space_keys.insert("repo-key".into());

        assert_eq!(
            workspace_list_entries(&app),
            vec![
                WorkspaceListEntry::Workspace {
                    ws_idx: 0,
                    indented: false,
                },
                WorkspaceListEntry::Workspace {
                    ws_idx: 1,
                    indented: true,
                },
            ]
        );
    }

    // --- system workspaces -------------------------------------------------
    // Characterized first (every workspace listed, the blocked system group
    // sorted first), then the predicate landed and these flipped.

    #[test]
    fn sidebar_rows_skip_system_workspaces() {
        let app = AppState::test_with_system_workspace();
        let system = AppState::TEST_SYSTEM_WS;
        let listed: Vec<usize> = workspace_list_entries(&app)
            .into_iter()
            .map(|WorkspaceListEntry::Workspace { ws_idx, .. }| ws_idx)
            .collect();
        // The system pane is blocked, which used to sort its group first;
        // now it is simply not a group.
        assert_eq!(listed, vec![0, 1]);
        let expanded: Vec<usize> = workspace_list_entries_expanded(&app)
            .into_iter()
            .map(|WorkspaceListEntry::Workspace { ws_idx, .. }| ws_idx)
            .collect();
        assert_eq!(expanded, vec![0, 1]);
        assert!(!sidebar_rows(&app).iter().any(|row| match row {
            SidebarRow::Group { ws_idx, .. } | SidebarRow::Agent { ws_idx, .. } => {
                *ws_idx == system
            }
        }));
        assert!(!agent_panel_entries(&app)
            .iter()
            .any(|entry| entry.ws_idx == system));
        // The two user groups keep their real indices.
        assert_eq!(
            agent_panel_entries(&app)
                .iter()
                .map(|entry| entry.ws_idx)
                .collect::<Vec<_>>(),
            vec![0, 1]
        );
    }

    /// `active` may point at the system workspace while its pane is on
    /// screen; the tree then highlights nothing and hit-tests no row for it.
    #[test]
    fn sidebar_copes_with_an_unlisted_active_workspace() {
        let mut app = AppState::test_with_system_workspace();
        app.switch_workspace(AppState::TEST_SYSTEM_WS);
        assert_eq!(app.active, Some(AppState::TEST_SYSTEM_WS));
        assert_eq!(app.selected, AppState::TEST_SYSTEM_WS);
        let area = Rect::new(0, 0, 30, 20);
        let cards = compute_workspace_card_areas(&app, area);
        assert!(cards
            .iter()
            .all(|card| card.ws_idx != AppState::TEST_SYSTEM_WS));
        // Rendering with an unlisted active/selected workspace neither
        // panics nor paints a selection on a user group.
        let mut terminal =
            ratatui::Terminal::new(ratatui::backend::TestBackend::new(30, 20)).expect("terminal");
        let runtimes = TerminalRuntimeRegistry::new();
        terminal
            .draw(|frame| {
                render_sidebar(&app, &runtimes, frame, area);
            })
            .expect("draw");
        let buffer = terminal.backend().buffer().clone();
        let selected_bg = app.palette.surface0;
        let active_bg = app.palette.surface_dim;
        let painted = cards.iter().filter(|card| card.is_group()).any(|card| {
            let cell = &buffer[(card.rect.x + 1, card.rect.y)];
            cell.style().bg == Some(selected_bg) || cell.style().bg == Some(active_bg)
        });
        assert!(
            !painted,
            "no user group is highlighted for the system workspace"
        );
        // The collapsed strip maps rows to user workspaces only.
        app.sidebar_collapsed = true;
        app.view.sidebar_rect = Rect::new(0, 0, 4, 20);
        let (ws_area, _, _) = collapsed_sidebar_sections(app.view.sidebar_rect);
        assert_eq!(app.collapsed_workspace_at_row(ws_area.y), Some(0));
        assert_eq!(app.collapsed_workspace_at_row(ws_area.y + 1), Some(1));
        assert_eq!(app.collapsed_workspace_at_row(ws_area.y + 2), None);
    }
}
