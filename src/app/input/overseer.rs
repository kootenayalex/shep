//! Keys on the overseer view.
//!
//! One flat selection walks needs-you, agents, docket and proposals; enter
//! does the obvious thing for the row it is on (focus the pane, open the
//! docket item, keep the proposal); `a` and `x` keep or drop a proposal and
//! otherwise `a` goes round the views. The docket verbs go through the same
//! store path as the docket board's, by id.
//!
//! `tab` hands the keys to the chat input; while it has them, printable
//! keys type, enter sends and esc gives them back — to the board, not to
//! the desktop. The switch chord is checked before any of this.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::app::{
    state::{AppState, BoardView},
    App,
};
use crate::ui::board::BoardDir;
use crate::ui::overseer::{overseer_model, OverseerRow};

use super::modal::{leave_modal, open_keybind_help};

impl AppState {
    /// The row the overseer view treats as selected.
    pub(crate) fn overseer_selection(&self) -> Option<OverseerRow> {
        overseer_model(self).effective_selection(self.board.overseer_selected)
    }

    /// Move the overseer selection in `dir`.
    pub(crate) fn overseer_move_selection(&mut self, dir: BoardDir) {
        let model = overseer_model(self);
        let current = model.effective_selection(self.board.overseer_selected);
        if let Some(next) = model.next(current, dir) {
            self.board.overseer_selected = Some(next);
        }
    }

    /// After a proposal is kept or dropped its card is gone: land on the one
    /// that took its place, or the last proposal, or wherever the view puts
    /// a selection that no longer resolves.
    fn overseer_reseat_after_proposal(&mut self, was_at: usize) {
        let model = overseer_model(self);
        self.board.overseer_selected = model
            .proposals
            .get(was_at.min(model.proposals.len().saturating_sub(1)))
            .map(|card| OverseerRow::Proposal(card.id))
            .or_else(|| model.effective_selection(self.board.overseer_selected));
    }

    /// `a` on a proposal: promote it as slated, the docket's `p`.
    pub(crate) fn overseer_keep_proposal(&mut self, id: i64) {
        let at = self.proposal_index(id);
        self.docket_promote_slated_id(id);
        self.overseer_reseat_after_proposal(at);
    }

    /// `x` on a proposal: discard it.
    pub(crate) fn overseer_drop_proposal(&mut self, id: i64) {
        let at = self.proposal_index(id);
        self.docket_discard_id(id);
        self.overseer_reseat_after_proposal(at);
    }

    fn proposal_index(&self, id: i64) -> usize {
        overseer_model(self)
            .proposals
            .iter()
            .position(|card| card.id == id)
            .unwrap_or(0)
    }
}

impl App {
    pub(crate) fn handle_board_overseer_key(&mut self, key: KeyEvent) {
        if self.state.overseer.chat_focused {
            self.handle_overseer_chat_key(key);
            return;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => leave_modal(&mut self.state),
            KeyCode::Up | KeyCode::Char('k') => self.state.overseer_move_selection(BoardDir::Up),
            KeyCode::Down | KeyCode::Char('j') => {
                self.state.overseer_move_selection(BoardDir::Down)
            }
            KeyCode::Enter => self.overseer_enter(),
            KeyCode::Char('a') => match self.state.overseer_selection() {
                Some(OverseerRow::Proposal(id)) => self.state.overseer_keep_proposal(id),
                _ => self.state.cycle_board_view(),
            },
            KeyCode::Char('x') => {
                if let Some(OverseerRow::Proposal(id)) = self.state.overseer_selection() {
                    self.state.overseer_drop_proposal(id);
                }
            }
            KeyCode::Char('?') => {
                // Help over the board, and back to the board when it closes.
                self.state.board.suspended = true;
                open_keybind_help(&mut self.state);
            }
            KeyCode::Tab => self.state.overseer.chat_focused = true,
            _ => {}
        }
    }

    /// Keys while the chat input has them. Esc and tab give them back;
    /// nothing here leaves the board.
    fn handle_overseer_chat_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc | KeyCode::Tab => self.state.overseer.chat_focused = false,
            KeyCode::Enter => {
                // A question already out is refused: take nothing, so the
                // text stays in the input to send when the answer lands.
                if self.state.overseer.chat_pending {
                    return;
                }
                let question = std::mem::take(&mut self.state.overseer.chat_input);
                if let Err(reason) = self.send_overseer_chat(question) {
                    // Only a blank question reaches this: the busy case was
                    // refused above, with the text still in the input.
                    tracing::debug!(reason, "overseer question not sent");
                }
            }
            KeyCode::Backspace => {
                self.state.overseer.chat_input.pop();
            }
            KeyCode::Char('u') if key.modifiers == KeyModifiers::CONTROL => {
                self.state.overseer.chat_input.clear();
            }
            KeyCode::Char(c)
                if key.modifiers.is_empty() || key.modifiers == KeyModifiers::SHIFT =>
            {
                self.state.overseer.chat_input.push(c);
            }
            _ => {}
        }
    }

    /// Enter on the selected row: a pane row focuses its pane and leaves the
    /// board; a docket row opens the item; a proposal is kept.
    pub(crate) fn overseer_enter(&mut self) {
        let model = overseer_model(&self.state);
        let Some(row) = model.effective_selection(self.state.board.overseer_selected) else {
            // Nothing to select: the header's button is the only thing
            // enter can mean.
            self.open_overseer_session();
            return;
        };
        match row {
            OverseerRow::NeedsYou(_) | OverseerRow::Agent(_) => {
                if let Some((ws_idx, pane_id)) = model.pane_target(row) {
                    self.board_focus_pane(ws_idx, pane_id);
                }
            }
            OverseerRow::Docket(id) => {
                self.state.board.docket_selected = Some(id);
                self.state.set_board_view(BoardView::DocketItem);
            }
            OverseerRow::Proposal(id) => self.state.overseer_keep_proposal(id),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::app::state::{AppState, Mode};
    use crate::detect::{Agent, AgentState};
    use crate::workspace::Workspace;
    use crossterm::event::KeyModifiers;
    use ratatui::layout::Direction;

    fn key(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    fn set_state(state: &mut AppState, ws_idx: usize, pane: crate::layout::PaneId, s: AgentState) {
        let terminal_id = state.workspaces[ws_idx].tabs[0]
            .panes
            .get(&pane)
            .expect("pane")
            .attached_terminal_id
            .clone();
        let terminal = state.terminals.get_mut(&terminal_id).expect("terminal");
        terminal.detected_agent = Some(Agent::Claude);
        terminal.state = s;
    }

    /// An app on the overseer view: two agents in one group (one blocked),
    /// and a scratch docket with an inbox item, a due item and a proposal.
    pub(crate) fn overseer_app() -> (App, std::path::PathBuf, crate::layout::PaneId) {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        let mut ws = Workspace::test_new("one");
        let root = ws.tabs[0].root_pane;
        let second = ws.test_split(Direction::Horizontal);
        ws.tabs[0].layout.focus_pane(root);
        let mut state = AppState::test_new();
        state.workspaces = vec![ws];
        state.ensure_test_terminals();
        state.active = Some(0);
        state.selected = 0;
        set_state(&mut state, 0, root, AgentState::Working);
        set_state(&mut state, 0, second, AgentState::Blocked);
        let path = crate::app::state::test_docket_db_path();
        state.docket_db = path.clone();
        {
            let conn = crate::docket::open_store(&path).expect("scratch docket");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "captured thing".into(),
                    ..Default::default()
                },
            )
            .expect("inbox item");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "due thing".into(),
                    kind: Some(crate::api::schema::DocketKind::Slated),
                    due: Some("2020-01-01".into()),
                    ..Default::default()
                },
            )
            .expect("due item");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "proposed thing".into(),
                    source: Some(serde_json::json!({"kind": "situation", "ref": "pane p2"})),
                    ..Default::default()
                },
            )
            .expect("proposal");
        }
        state.refresh_docket();
        app.state = state;
        app.state.open_board();
        (app, path, second)
    }

    pub(crate) fn cleanup(path: &std::path::Path) {
        if let Some(dir) = path.parent() {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    fn status_of(app: &App, id: i64) -> Option<crate::api::schema::DocketStatus> {
        app.state
            .docket_sample
            .rows
            .iter()
            .find(|row| row.id == id)
            .map(|row| row.status)
    }

    #[tokio::test]
    async fn enter_on_agent_focuses_and_leaves() {
        let (mut app, path, blocked) = overseer_app();
        assert_eq!(app.state.board.view, BoardView::Overseer);
        // The blocked agent leads the needs-you region and is selected.
        assert_eq!(
            app.state.overseer_selection(),
            Some(OverseerRow::NeedsYou(blocked))
        );
        // Walk down to the agents table and pick the blocked one there too.
        app.handle_board_overseer_key(key(KeyCode::Char('j')));
        assert_eq!(
            app.state.overseer_selection(),
            Some(OverseerRow::Agent(blocked))
        );
        app.handle_board_overseer_key(key(KeyCode::Enter));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(
            app.state.workspaces[0].focused_pane_id(),
            Some(blocked),
            "enter focused the selected pane"
        );
        cleanup(&path);
    }

    #[tokio::test]
    async fn enter_on_docket_row_opens_the_item() {
        let (mut app, path, _) = overseer_app();
        let model = overseer_model(&app.state);
        let due_id = model.docket.first().expect("due row").id;
        app.state.board.overseer_selected = Some(OverseerRow::Docket(due_id));
        app.handle_board_overseer_key(key(KeyCode::Enter));
        assert_eq!(app.state.board.view, BoardView::DocketItem);
        assert_eq!(app.state.board.docket_selected, Some(due_id));
        assert_eq!(app.state.mode, Mode::Board);
        // Esc from the item steps back to the docket lanes, as it always did.
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.board.view, BoardView::Docket);
        cleanup(&path);
    }

    #[tokio::test]
    async fn keep_slates_and_drop_discards_a_proposal() {
        let (mut app, path, _) = overseer_app();
        let proposals = overseer_model(&app.state).proposals;
        assert_eq!(proposals.len(), 1);
        let id = proposals[0].id;
        app.state.board.overseer_selected = Some(OverseerRow::Proposal(id));
        app.handle_board_overseer_key(key(KeyCode::Char('a')));
        assert_eq!(
            status_of(&app, id),
            Some(crate::api::schema::DocketStatus::Open)
        );
        assert!(app.state.board.docket_notice.is_none());
        // No proposals left: the selection lands somewhere real.
        let sel = app.state.overseer_selection().expect("a selection");
        assert!(!matches!(sel, OverseerRow::Proposal(_)));

        // A second proposal, dropped this time.
        {
            let conn = crate::docket::open_store(&path).expect("scratch docket");
            crate::docket::add(
                &conn,
                crate::docket::NewItem {
                    title: "another".into(),
                    source: Some(serde_json::json!({"kind": "situation", "ref": "disk"})),
                    ..Default::default()
                },
            )
            .expect("proposal");
        }
        app.state.refresh_docket();
        let id = overseer_model(&app.state).proposals[0].id;
        app.state.board.overseer_selected = Some(OverseerRow::Proposal(id));
        app.handle_board_overseer_key(key(KeyCode::Char('x')));
        assert_eq!(
            status_of(&app, id),
            Some(crate::api::schema::DocketStatus::Discarded)
        );
        // `a` with nothing proposed selected goes round the views instead.
        app.state.board.overseer_selected = None;
        app.handle_board_overseer_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Columns);
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_cycles_overseer_columns_docket() {
        let (mut app, path, _) = overseer_app();
        app.state.board.overseer_selected = None;
        // From the first agent row, not a proposal, `a` is the view cycle.
        let first_agent = overseer_model(&app.state)
            .rows()
            .into_iter()
            .find(|row| matches!(row, OverseerRow::Agent(_)))
            .expect("an agent row");
        app.state.board.overseer_selected = Some(first_agent);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Columns);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Docket);
        app.handle_board_key(key(KeyCode::Char('a')));
        assert_eq!(app.state.board.view, BoardView::Overseer);
        cleanup(&path);
    }

    #[tokio::test]
    async fn help_from_the_board_returns_to_the_board() {
        let (mut app, path, _) = overseer_app();
        app.handle_board_key(key(KeyCode::Char('?')));
        assert_eq!(app.state.mode, Mode::KeybindHelp);
        assert!(app.state.board.suspended);
        assert!(app.state.board_underlay());
        super::super::modal::handle_keybind_help_key(&mut app.state, key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Board);
        assert!(!app.state.board.suspended);
        assert_eq!(app.state.board.view, BoardView::Overseer);
        // And esc from the board itself leaves it.
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Terminal);
        cleanup(&path);
    }

    #[tokio::test]
    async fn opening_a_stale_board_invokes_tick_once() {
        let (mut app, path, _) = overseer_app();
        app.state.mode = Mode::Terminal;
        // A fake overseer plugin whose tick is `true`.
        let plugin_root = crate::app::overseer::test_state_dir();
        std::fs::create_dir_all(&plugin_root).expect("plugin root");
        let manifest_path = plugin_root.join("shep-plugin.toml");
        std::fs::write(&manifest_path, "id = 'overseer'\n").expect("manifest");
        app.state.installed_plugins.insert(
            "overseer".into(),
            crate::api::schema::InstalledPluginInfo {
                plugin_id: "overseer".into(),
                name: "Overseer".into(),
                version: "0.1.0".into(),
                min_shep_version: "0.7.3".into(),
                description: None,
                manifest_path: manifest_path.display().to_string(),
                plugin_root: plugin_root.display().to_string(),
                enabled: true,
                platforms: None,
                build: Vec::new(),
                actions: vec![crate::api::schema::PluginManifestAction {
                    id: "tick".into(),
                    title: "tick".into(),
                    description: None,
                    contexts: Vec::new(),
                    platforms: None,
                    command: vec!["true".into()],
                }],
                events: Vec::new(),
                panes: Vec::new(),
                link_handlers: Vec::new(),
                source: crate::api::schema::PluginSourceInfo::default(),
                warnings: Vec::new(),
            },
        );
        // The situation was never written: stale by definition.
        app.open_board_live();
        assert_eq!(app.state.mode, Mode::Board);
        let ticks = |app: &App| {
            app.state
                .plugin_command_logs
                .iter()
                .filter(|log| log.action_id.as_deref() == Some("tick"))
                .count()
        };
        assert_eq!(ticks(&app), 1);
        let log_id = app
            .state
            .overseer
            .tick_in_flight
            .clone()
            .expect("in flight");

        // Reopening while the tick runs does not stack another.
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(ticks(&app), 1);

        // The tick finishing clears the guard and re-reads the dir.
        app.handle_internal_event(crate::events::AppEvent::PluginCommandFinished {
            log_id,
            finished_unix_ms: 0,
            exit_code: Some(0),
            stdout: String::new(),
            stderr: String::new(),
            error: None,
        });
        assert!(app.state.overseer.tick_in_flight.is_none());

        // Still stale (nothing wrote a situation): a third open asks again.
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(ticks(&app), 2);
        let _ = std::fs::remove_dir_all(&plugin_root);
        cleanup(&path);
    }

    #[tokio::test]
    async fn opening_without_the_plugin_is_quiet() {
        let (mut app, path, _) = overseer_app();
        app.state.mode = Mode::Terminal;
        app.open_board_live();
        assert_eq!(app.state.mode, Mode::Board);
        assert!(app.state.overseer.tick_in_flight.is_none());
        assert!(app.state.plugin_command_logs.is_empty());
        assert!(app.state.board.docket_notice.is_none());
        cleanup(&path);
    }

    /// A `[runtimes.fake-brain]` whose headless recipe is a shell script
    /// that prints `answer`, wired as the overseer's runtime, with the chat
    /// pointed at a scratch state dir. Returns the dir.
    fn fake_brain(app: &mut App, script: &str) -> std::path::PathBuf {
        crate::env_compat::remove_process_env_for_test("SHEP_OVERSEER_RUNTIME");
        let dir = crate::app::overseer::test_state_dir();
        std::fs::create_dir_all(&dir).expect("state dir");
        let bin = dir.join("fake-brain.sh");
        std::fs::write(&bin, format!("#!/bin/sh\n{script}\n")).expect("script");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        }
        app.state.runtimes_config.insert(
            "fake-brain".into(),
            crate::config::RuntimeOverrideConfig {
                headless_argv: vec![bin.display().to_string()],
                ..Default::default()
            },
        );
        app.state.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"fake-brain\"").expect("table"),
        );
        app.state.overseer.state_dir = dir.clone();
        dir
    }

    /// [`fake_brain`] whose recipe also names conversations
    /// (`--session-id {session_id}` / `--resume {session_id}`), so the chat
    /// shares one with the session pane. The script sees them as `$@` and
    /// is expected to record them in `argv.log` in the dir.
    fn fake_brain_with_session(app: &mut App, script: &str) -> std::path::PathBuf {
        let dir = fake_brain(app, script);
        let over = app
            .state
            .runtimes_config
            .get_mut("fake-brain")
            .expect("the fake brain");
        over.headless_session_new_args = Some(vec!["--session-id".into(), "{session_id}".into()]);
        over.headless_session_resume_args = Some(vec!["--resume".into(), "{session_id}".into()]);
        dir
    }

    /// The lines of `argv.log`: what each headless run was called with.
    fn argv_log(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("argv.log"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    /// A script body that logs `$@` and the cwd to `argv.log`, then reads
    /// the prompt and answers `text` — unless `$1` is `--resume`, when
    /// `on_resume` runs instead (an empty string: answer as usual).
    fn recording_script(text: &str, on_resume: &str) -> String {
        format!(
            "printf '%s' \"$PWD\" >> \"$(dirname \"$0\")/argv.log\"; printf ' %s' \"$@\" >> \"$(dirname \"$0\")/argv.log\"; echo >> \"$(dirname \"$0\")/argv.log\"; \
             prompt=$(cat); case \"$prompt\" in *'Hard rules'*) ;; *) echo 'no rules' >&2; exit 9;; esac; \
             case \"$prompt\" in *'The conversation so far'*) echo 'replayed' >&2; exit 8;; esac; \
             if [ \"$1\" = --resume ]; then :; {on_resume} fi; echo '{text}'"
        )
    }

    async fn pump_chat(app: &mut App) {
        let event = tokio::time::timeout(std::time::Duration::from_secs(20), app.event_rx.recv())
            .await
            .expect("the runtime answers in time")
            .expect("the channel is open");
        assert!(
            matches!(event, crate::events::AppEvent::OverseerChatFinished { .. }),
            "{event:?}"
        );
        app.handle_internal_event(event);
    }

    fn chat_lines(dir: &std::path::Path) -> Vec<String> {
        std::fs::read_to_string(dir.join("chat.jsonl"))
            .unwrap_or_default()
            .lines()
            .map(str::to_string)
            .collect()
    }

    #[tokio::test]
    async fn send_then_finished_appends_both_turns() {
        let (mut app, path, _) = overseer_app();
        // The prompt arrives on stdin; the script proves it saw the question
        // and the rules by echoing a fixed answer only then.
        let dir = fake_brain(
            &mut app,
            "prompt=$(cat); case \"$prompt\" in *'Hard rules'*'you: what first?'*) echo 'answer workmayt first.';; *) echo unexpected; exit 1;; esac",
        );
        std::fs::write(dir.join("situation.md"), "claude is blocked.\n").expect("situation");
        app.state.overseer.chat_focused = true;
        for c in "what first?".chars() {
            app.handle_board_key(key(KeyCode::Char(c)));
        }
        assert_eq!(app.state.overseer.chat_input, "what first?");
        app.handle_board_key(key(KeyCode::Enter));
        assert!(app.state.overseer.chat_pending);
        assert_eq!(app.state.overseer.chat_input, "");
        assert_eq!(app.state.overseer.chat.len(), 1);
        assert_eq!(
            app.state.mode,
            Mode::Board,
            "sending never leaves the board"
        );

        pump_chat(&mut app).await;
        assert!(!app.state.overseer.chat_pending);
        let turns = &app.state.overseer.chat;
        assert_eq!(turns.len(), 2);
        assert_eq!(turns[0].role, crate::app::overseer::ChatRole::You);
        assert_eq!(turns[1].role, crate::app::overseer::ChatRole::Overseer);
        assert_eq!(turns[1].text, "answer workmayt first.");
        let lines = chat_lines(&dir);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(lines[1].contains(r#""text":"answer workmayt first.""#));

        // A refresh past the TTL re-reads nothing new and keeps the thread.
        app.state
            .overseer
            .refresh_if_stale(std::time::Instant::now() + std::time::Duration::from_secs(5));
        assert_eq!(app.state.overseer.chat.len(), 2);

        // Blank questions are not questions.
        app.state.overseer.chat_input = "   ".into();
        app.handle_board_key(key(KeyCode::Enter));
        assert!(!app.state.overseer.chat_pending);
        assert_eq!(chat_lines(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&path);
    }

    /// The chat only writes the client config when the runtime's recipe says
    /// how to attach one, and then the words reach the child unchanged — the
    /// brain's tools are the recipe's business, not the chat's.
    #[tokio::test]
    async fn chat_writes_mcp_json_and_passes_it_when_the_recipe_asks() {
        let (mut app, path, _) = overseer_app();
        let dir = fake_brain(&mut app, &recording_script("looked.", ""));
        std::fs::write(dir.join("situation.md"), "all quiet.\n").expect("situation");

        // Without `mcp_config_args` nothing is written and nothing spliced.
        let _ = app.send_overseer_chat("anything?".into());
        pump_chat(&mut app).await;
        assert!(
            !dir.join("mcp.json").exists(),
            "a runtime that cannot take tools is not handed a config"
        );
        let cwd = std::fs::canonicalize(&dir)
            .expect("dir")
            .display()
            .to_string();
        // The script logs the cwd and then one `printf ' %s'` per argument,
        // so "no arguments" is the cwd and the format's single space.
        assert_eq!(argv_log(&dir), vec![format!("{cwd} ")], "no extra words");

        // With it, the config is written and its path is the recipe's.
        app.state
            .runtimes_config
            .get_mut("fake-brain")
            .expect("the fake brain")
            .headless_mcp_config_args = Some(vec![
            "--mcp-config".into(),
            "{mcp_config}".into(),
            "--allowedTools".into(),
            "mcp__{mcp_server}".into(),
        ]);
        let _ = app.send_overseer_chat("and now?".into());
        pump_chat(&mut app).await;
        let config = dir.join("mcp.json");
        assert!(config.exists(), "the config is written before the run");
        let written: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&config).expect("config")).expect("json");
        assert!(
            written["mcpServers"]["shep"]["args"]
                .as_array()
                .is_some_and(|args| args.contains(&serde_json::Value::from("overseer"))),
            "the overseer profile, not everything: {written}"
        );
        assert_eq!(
            argv_log(&dir).last().map(String::as_str),
            Some(
                format!(
                    "{cwd} --mcp-config {} --allowedTools mcp__shep",
                    config.display()
                )
                .as_str()
            )
        );
        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn first_chat_creates_then_resumes() {
        let (mut app, path, _) = overseer_app();
        let dir = fake_brain_with_session(&mut app, &recording_script("fine.", ""));
        std::fs::write(dir.join("situation.md"), "all quiet.\n").expect("situation");
        assert!(!dir.join("session-id").exists());

        let _ = app.send_overseer_chat("first?".into());
        pump_chat(&mut app).await;
        let (id, started) = app.state.overseer.overseer_session();
        assert!(started, "a successful answer begins the session");
        assert_eq!(
            app.state.overseer.chat.last().map(|t| t.text.as_str()),
            Some("fine.")
        );

        let _ = app.send_overseer_chat("second?".into());
        pump_chat(&mut app).await;
        assert_eq!(app.state.overseer.overseer_session(), (id.clone(), true));
        let cwd = std::fs::canonicalize(&dir)
            .expect("dir")
            .display()
            .to_string();
        assert_eq!(
            argv_log(&dir),
            vec![
                format!("{cwd} --session-id {id}"),
                format!("{cwd} --resume {id}"),
            ],
            "new first, resumed after, both in the session's cwd"
        );
        assert_eq!(
            chat_lines(&dir).len(),
            4,
            "the board still draws every turn"
        );
        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_failed_resume_retries_once_as_new() {
        let (mut app, path, _) = overseer_app();
        let dir = fake_brain_with_session(
            &mut app,
            &recording_script("started over.", "echo 'No conversation found' >&2; exit 1;"),
        );
        // Something began the session (the pane, say) but the runtime has
        // lost it: the resume fails, the same id starts over, once.
        app.state.overseer.mark_session_started();
        let (id, _) = app.state.overseer.overseer_session();
        let _ = app.send_overseer_chat("still there?".into());
        pump_chat(&mut app).await;
        assert_eq!(
            app.state.overseer.chat.last().map(|t| t.text.as_str()),
            Some("started over.")
        );
        let log = argv_log(&dir);
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log[0].ends_with(&format!(" --resume {id}")), "{log:?}");
        assert!(log[1].ends_with(&format!(" --session-id {id}")), "{log:?}");
        assert_eq!(app.state.overseer.overseer_session(), (id.clone(), true));

        // When starting over fails too, the marker stays cleared and the
        // last failure is the row; a new one is not retried again.
        let dir2 = fake_brain_with_session(
            &mut app,
            "printf ' %s' \"$@\" >> \"$(dirname \"$0\")/argv.log\"; echo >> \"$(dirname \"$0\")/argv.log\"; cat >/dev/null; echo \"gone $1\" >&2; exit 2",
        );
        app.state.overseer.mark_session_started();
        let _ = app.send_overseer_chat("anyone?".into());
        pump_chat(&mut app).await;
        assert_eq!(
            app.state.overseer.chat.last().map(|t| t.text.as_str()),
            Some("the overseer did not answer: fake-brain exited 2: gone --session-id")
        );
        assert_eq!(argv_log(&dir2).len(), 2);
        assert!(
            !app.state.overseer.overseer_session().1,
            "nothing has begun it"
        );
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_failed_runtime_becomes_an_overseer_row() {
        let (mut app, path, _) = overseer_app();
        let dir = fake_brain(&mut app, "echo 'no credits' >&2; exit 7");
        let _ = app.send_overseer_chat("hello?".into());
        pump_chat(&mut app).await;
        assert!(!app.state.overseer.chat_pending);
        let last = app.state.overseer.chat.last().expect("an overseer row");
        assert_eq!(last.role, crate::app::overseer::ChatRole::Overseer);
        assert_eq!(
            last.text,
            "the overseer did not answer: fake-brain exited 7: no credits"
        );
        assert_eq!(chat_lines(&dir).len(), 2);
        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn no_runtime_configured_says_so_without_spawning() {
        let (mut app, path, _) = overseer_app();
        crate::env_compat::remove_process_env_for_test("SHEP_OVERSEER_RUNTIME");
        let dir = crate::app::overseer::test_state_dir();
        app.state.overseer.state_dir = dir.clone();
        let _ = app.send_overseer_chat("anyone there?".into());
        assert!(!app.state.overseer.chat_pending, "answered on the spot");
        assert_eq!(app.state.overseer.chat.len(), 2);
        assert_eq!(
            app.state.overseer.chat[1].text,
            crate::app::overseer::CHAT_NO_RUNTIME
        );
        assert!(app.event_rx.try_recv().is_err(), "nothing was spawned");
        assert_eq!(
            chat_lines(&dir).len(),
            2,
            "the dir was created for the file"
        );

        // A configured name that resolves to nothing is the same row, with
        // the reason.
        app.state.plugins_config.insert(
            "overseer".into(),
            toml::from_str("runtime = \"no-such-runtime\"").expect("table"),
        );
        let _ = app.send_overseer_chat("still there?".into());
        assert!(!app.state.overseer.chat_pending);
        let last = app.state.overseer.chat.last().expect("row");
        assert!(last.text.starts_with(crate::app::overseer::CHAT_NO_RUNTIME));
        assert!(
            last.text.contains("unknown runtime no-such-runtime"),
            "{:?}",
            last.text
        );
        assert!(app.event_rx.try_recv().is_err());
        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_second_question_while_pending_keeps_the_input() {
        let (mut app, path, _) = overseer_app();
        let dir = crate::app::overseer::test_state_dir();
        app.state.overseer.state_dir = dir.clone();
        // A question is already out with the runtime.
        app.state.overseer.chat_pending = true;
        app.state.overseer.chat_focused = true;
        for c in "and another thing".chars() {
            app.handle_board_key(key(KeyCode::Char(c)));
        }

        app.handle_board_key(key(KeyCode::Enter));

        assert_eq!(
            app.state.overseer.chat_input, "and another thing",
            "a refused question stays where it was typed"
        );
        assert!(app.state.overseer.chat.is_empty(), "nothing was recorded");
        assert!(app.event_rx.try_recv().is_err(), "nothing was spawned");
        assert_eq!(
            app.send_overseer_chat("and another thing".into()),
            Err(crate::app::overseer::CHAT_BUSY)
        );
        assert!(!dir.exists(), "a refusal writes nothing");
        cleanup(&path);
    }

    #[tokio::test]
    async fn tab_toggles_chat_focus() {
        let (mut app, path, _) = overseer_app();
        assert!(!app.state.overseer.chat_focused);
        app.handle_board_key(key(KeyCode::Tab));
        assert!(app.state.overseer.chat_focused);
        // With the keys, `j` types rather than moves, and `q` does not quit.
        let before = app.state.overseer_selection();
        app.handle_board_key(key(KeyCode::Char('j')));
        app.handle_board_key(key(KeyCode::Char('q')));
        assert_eq!(app.state.overseer.chat_input, "jq");
        assert_eq!(app.state.overseer_selection(), before);
        assert_eq!(app.state.mode, Mode::Board);
        app.handle_board_key(key(KeyCode::Backspace));
        assert_eq!(app.state.overseer.chat_input, "j");
        app.handle_board_key(key(KeyCode::Tab));
        assert!(!app.state.overseer.chat_focused);
        assert_eq!(
            app.state.overseer.chat_input, "j",
            "unfocus keeps the draft"
        );
        // Back on the board, `j` moves again.
        app.handle_board_key(key(KeyCode::Char('j')));
        assert_eq!(app.state.overseer.chat_input, "j");
        assert_ne!(app.state.overseer_selection(), before);
        cleanup(&path);
    }

    #[tokio::test]
    async fn esc_leaves_the_input_not_the_board() {
        let (mut app, path, _) = overseer_app();
        app.handle_board_key(key(KeyCode::Tab));
        app.handle_board_key(key(KeyCode::Char('x')));
        app.handle_board_key(key(KeyCode::Esc));
        assert!(!app.state.overseer.chat_focused);
        assert_eq!(app.state.mode, Mode::Board);
        assert_eq!(app.state.board.view, BoardView::Overseer);
        // The second esc is the board's.
        app.handle_board_key(key(KeyCode::Esc));
        assert_eq!(app.state.mode, Mode::Terminal);
        // Leaving with the keys on the input and coming back: the board has
        // them again, the draft is still there.
        app.state.open_board();
        app.handle_board_key(key(KeyCode::Tab));
        app.handle_board_key(key(KeyCode::Char('y')));
        leave_modal(&mut app.state);
        app.state.open_board();
        assert!(!app.state.overseer.chat_focused);
        assert_eq!(app.state.overseer.chat_input, "xy");
        cleanup(&path);
    }

    #[tokio::test]
    async fn paste_lands_in_the_chat_input() {
        let (mut app, path, _) = overseer_app();
        assert!(
            !app.paste_into_active_text_input("nope"),
            "an unfocused chat takes no paste"
        );
        app.handle_board_key(key(KeyCode::Tab));
        assert!(app.paste_into_active_text_input("two\nlines"));
        assert_eq!(app.state.overseer.chat_input, "two lines");
        cleanup(&path);
    }

    #[tokio::test]
    async fn a_click_on_the_input_focuses_and_elsewhere_unfocuses() {
        use crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
        let (mut app, path, _) = overseer_app();
        crate::ui::compute_view(&mut app.state, ratatui::layout::Rect::new(0, 0, 160, 45));
        let model = overseer_model(&app.state);
        let layout = crate::ui::overseer::overseer_layout(
            &app.state,
            &model,
            crate::ui::board::board_area(&app.state),
        );
        let click = |col: u16, row: u16| MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: col,
            row,
            modifiers: KeyModifiers::NONE,
        };
        assert!(app.handle_overlay_mouse(click(layout.chat_input.x + 2, layout.chat_input.y)));
        assert!(app.state.overseer.chat_focused);
        // A click on a heading: nobody's row, but the keys go back.
        assert!(
            app.handle_overlay_mouse(click(layout.agents.heading.x + 2, layout.agents.heading.y))
        );
        assert!(!app.state.overseer.chat_focused);
        assert_eq!(app.state.mode, Mode::Board);
        cleanup(&path);
    }

    // --- the session button ------------------------------------------------

    /// An installed, enabled overseer plugin whose `session` pane runs
    /// `command`. Returns the plugin root to remove afterwards.
    fn install_fake_overseer(app: &mut App, command: &[&str]) -> std::path::PathBuf {
        let plugin_root = crate::app::overseer::test_state_dir();
        std::fs::create_dir_all(&plugin_root).expect("plugin root");
        let manifest_path = plugin_root.join("shep-plugin.toml");
        std::fs::write(&manifest_path, "id = 'overseer'\n").expect("manifest");
        app.state.installed_plugins.insert(
            "overseer".into(),
            crate::api::schema::InstalledPluginInfo {
                plugin_id: "overseer".into(),
                name: "Overseer".into(),
                version: "0.1.0".into(),
                min_shep_version: "0.7.3".into(),
                description: None,
                manifest_path: manifest_path.display().to_string(),
                plugin_root: plugin_root.display().to_string(),
                enabled: true,
                platforms: None,
                build: Vec::new(),
                actions: Vec::new(),
                events: Vec::new(),
                panes: vec![crate::api::schema::PluginManifestPane {
                    id: "session".into(),
                    title: "overseer session".into(),
                    description: None,
                    platforms: None,
                    placement: crate::api::schema::PluginPanePlacement::Tab,
                    command: command.iter().map(|s| s.to_string()).collect(),
                }],
                link_handlers: Vec::new(),
                source: crate::api::schema::PluginSourceInfo::default(),
                warnings: Vec::new(),
            },
        );
        plugin_root
    }

    fn system_workspaces(app: &App) -> Vec<usize> {
        app.state
            .workspaces
            .iter()
            .enumerate()
            .filter(|(_, ws)| ws.is_system())
            .map(|(idx, _)| idx)
            .collect()
    }

    #[tokio::test]
    async fn open_overseer_session_without_the_plugin_says_so() {
        let (mut app, path, _) = overseer_app();
        assert_eq!(app.state.mode, Mode::Board);
        let before = app.state.workspaces.len();
        app.open_overseer_session();
        assert_eq!(app.state.mode, Mode::Board, "stays on the board");
        assert_eq!(app.state.workspaces.len(), before);
        assert_eq!(
            app.state.board.docket_notice.as_deref(),
            Some(crate::app::overseer::SESSION_NEEDS_PLUGIN)
        );
        assert!(app.overseer_session_workspace().is_none());
        // A disabled plugin is as good as none.
        let root = install_fake_overseer(&mut app, &["true"]);
        app.state
            .installed_plugins
            .get_mut("overseer")
            .expect("plugin")
            .enabled = false;
        app.state.board.docket_notice = None;
        app.open_overseer_session();
        assert_eq!(
            app.state.board.docket_notice.as_deref(),
            Some(crate::app::overseer::SESSION_NEEDS_PLUGIN)
        );
        assert_eq!(app.state.workspaces.len(), before);
        let _ = std::fs::remove_dir_all(&root);
        cleanup(&path);
    }

    #[tokio::test]
    async fn enter_with_nothing_selected_is_the_session_button() {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        // No agents, no docket: the overseer view has no rows at all.
        app.state = AppState::test_new();
        app.state.open_board();
        assert_eq!(app.state.overseer_selection(), None);
        app.handle_board_overseer_key(key(KeyCode::Enter));
        assert_eq!(
            app.state.board.docket_notice.as_deref(),
            Some(crate::app::overseer::SESSION_NEEDS_PLUGIN)
        );
    }

    #[tokio::test]
    async fn open_overseer_session_creates_one_from_the_plugin() {
        let (mut app, path, _) = overseer_app();
        let event_hub = crate::api::EventHub::default();
        app.event_hub = event_hub.clone();
        let root = install_fake_overseer(&mut app, &["sh", "-c", "sleep 30"]);
        let user_groups = app.state.workspaces.len();
        let listed_before = crate::ui::workspace_list_entries(&app.state);

        app.open_overseer_session();

        let system = system_workspaces(&app);
        assert_eq!(system.len(), 1, "exactly one system workspace");
        let idx = system[0];
        assert_eq!(app.state.active, Some(idx));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(app.state.board.docket_notice.is_none());
        assert_eq!(app.overseer_session_workspace(), Some(idx));
        let ws = &app.state.workspaces[idx];
        assert_eq!(
            ws.system,
            Some(crate::workspace::SystemRole::OverseerSession)
        );
        assert_eq!(ws.custom_name.as_deref(), Some("overseer"));
        assert_eq!(ws.tabs.len(), 1);
        ws.assert_invariants_for_test();
        app.state.assert_invariants_for_test();
        let pane = ws.tabs[0].root_pane;
        let terminal = app
            .state
            .terminals
            .get(ws.terminal_id(pane).expect("terminal id"))
            .expect("terminal");
        assert_eq!(terminal.manual_label.as_deref(), Some("overseer session"));
        assert!(app.terminal_runtimes.get(&terminal.id).is_some());
        assert_eq!(
            app.state
                .plugin_panes
                .get(&pane)
                .map(|r| r.entrypoint.as_str()),
            Some("session")
        );
        // Not a group: the sidebar and every other list read as before.
        assert_eq!(app.state.workspaces.len(), user_groups + 1);
        assert_eq!(crate::ui::workspace_list_entries(&app.state), listed_before);
        assert!(!crate::ui::board::board_model(&app.state)
            .lanes
            .iter()
            .any(|lane| lane.ws_idx == idx));
        let events = event_hub.events_after(0);
        assert!(events
            .iter()
            .any(|(_, e)| e.event == crate::api::schema::EventKind::WorkspaceCreated));
        assert!(events
            .iter()
            .any(|(_, e)| e.event == crate::api::schema::EventKind::PaneCreated));

        // The button a second time focuses the same one, from the board.
        app.state.switch_workspace(0);
        app.state.open_board();
        app.open_overseer_session();
        assert_eq!(system_workspaces(&app), vec![idx]);
        assert_eq!(app.state.active, Some(idx));
        assert_eq!(app.state.mode, Mode::Terminal);

        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(&root);
        cleanup(&path);
    }

    #[tokio::test]
    async fn session_pane_env_carries_id_resume_and_cwd() {
        let (mut app, path, _) = overseer_app();
        let state_dir = crate::app::overseer::test_state_dir();
        app.state.overseer.state_dir = state_dir.clone();
        let cwd = crate::app::overseer::test_state_dir();
        std::fs::create_dir_all(&cwd).expect("cwd");
        app.state.plugins_config.insert(
            "overseer".into(),
            toml::from_str(&format!("session_cwd = \"{}\"", cwd.display())).expect("table"),
        );
        let out = state_dir.join("pane-env");
        let script = format!(
            "printf '%s\\n%s\\n%s\\n' \"$SHEP_OVERSEER_SESSION_ID\" \"$SHEP_OVERSEER_SESSION_RESUME\" \"$SHEP_OVERSEER_SESSION_CWD\" > '{}'; sleep 30",
            out.display()
        );
        let root = install_fake_overseer(&mut app, &["sh", "-c", &script]);
        assert!(!state_dir.join("session-id").exists());

        app.open_overseer_session();
        assert!(app.overseer_session_workspace().is_some());
        let (id, started) = app.state.overseer.overseer_session();
        assert!(started, "opening the pane begins the session");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let text = loop {
            if let Ok(text) = std::fs::read_to_string(&out) {
                if text.lines().count() == 3 {
                    break text;
                }
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the pane never wrote its env"
            );
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        };
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            vec![id.as_str(), "0", &cwd.display().to_string()],
            "a fresh id, not yet begun when the pane was launched, in the configured cwd"
        );

        crate::app::api::test_support::shutdown_test_runtimes(&mut app);
        let _ = std::fs::remove_dir_all(&root);
        let _ = std::fs::remove_dir_all(&state_dir);
        let _ = std::fs::remove_dir_all(&cwd);
        cleanup(&path);
    }

    #[tokio::test]
    async fn open_overseer_session_focuses_an_existing_one() {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        app.state = AppState::test_with_system_workspace();
        let system = AppState::TEST_SYSTEM_WS;
        app.state.open_board();
        app.state.board.suspended = true;
        let before = app.state.workspaces.len();
        app.open_overseer_session();
        assert_eq!(app.state.workspaces.len(), before, "nothing new is spawned");
        assert_eq!(app.state.active, Some(system));
        assert_eq!(app.state.mode, Mode::Terminal);
        assert!(!app.state.board.suspended);
        assert!(app.state.board.docket_notice.is_none());
    }

    #[tokio::test]
    async fn session_pane_exit_returns_active_to_a_user_group() {
        let mut app = App::new(
            &crate::config::Config::default(),
            true,
            None,
            tokio::sync::mpsc::unbounded_channel().1,
            crate::api::EventHub::default(),
        );
        app.state = AppState::test_with_system_workspace();
        let system = AppState::TEST_SYSTEM_WS;
        app.state.switch_workspace(system);
        app.state.mode = Mode::Terminal;
        let listed_before = crate::ui::workspace_list_entries(&app.state);
        let rows_before = crate::ui::sidebar_rows(&app.state);
        let pane = app.state.workspaces[system].tabs[0].root_pane;

        app.handle_internal_event(crate::events::AppEvent::PaneDied { pane_id: pane });

        assert!(system_workspaces(&app).is_empty());
        assert!(app.overseer_session_workspace().is_none());
        let active = app.state.active.expect("a user group is active");
        assert!(!app.state.workspaces[active].is_system());
        assert_eq!(app.state.mode, Mode::Terminal);
        assert_eq!(crate::ui::workspace_list_entries(&app.state), listed_before);
        assert_eq!(crate::ui::sidebar_rows(&app.state), rows_before);
        app.state.assert_invariants_for_test();
    }
}
