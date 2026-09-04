use std::path::PathBuf;

use crate::api::schema::{
    EventData, EventEnvelope, EventKind, ResponseResult, TabCreateParams, TabListParams,
    TabMoveParams, TabRenameParams, TabTarget,
};
use crate::app::{App, Mode};

use super::responses::{encode_error, encode_success};

impl App {
    pub(super) fn handle_tab_list(&mut self, id: String, params: TabListParams) -> String {
        let tabs = if let Some(workspace_id) = params.workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(&workspace_id) else {
                return workspace_not_found(id, &workspace_id);
            };
            let Some(_) = self.state.workspaces.get(ws_idx) else {
                return workspace_not_found(id, &workspace_id);
            };
            self.tab_list_info(ws_idx)
        } else {
            let mut tabs = Vec::new();
            for (ws_idx, ws) in self.state.workspaces.iter().enumerate() {
                for tab_idx in 0..ws.tabs.len() {
                    if let Some(tab) = self.tab_info(ws_idx, tab_idx) {
                        tabs.push(tab);
                    }
                }
            }
            tabs
        };

        encode_success(id, ResponseResult::TabList { tabs })
    }

    pub(super) fn handle_tab_get(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(tab) = self.tab_info(ws_idx, tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_create(&mut self, id: String, params: TabCreateParams) -> String {
        let TabCreateParams {
            workspace_id,
            cwd,
            focus,
            label,
            env,
        } = params;
        let ws_idx = if let Some(workspace_id) = workspace_id {
            let Some(ws_idx) = self.parse_workspace_id(&workspace_id) else {
                return workspace_not_found(id, &workspace_id);
            };
            ws_idx
        } else if let Some(active) = self.state.active {
            active
        } else {
            return encode_error(id, "workspace_not_found", "no active workspace");
        };
        let cwd = cwd.map(PathBuf::from).unwrap_or_else(|| {
            self.resolve_new_terminal_cwd(self.focused_pane_cwd_in_workspace(ws_idx))
        });
        let (rows, cols) = self.state.estimate_pane_size();
        let default_shell = self.state.default_shell.clone();
        let scrollback_limit_bytes = self.state.pane_scrollback_limit_bytes;
        let host_terminal_theme = self.state.host_terminal_theme;
        let extra_env = match super::env::normalize_launch_env(env) {
            Ok(env) => env,
            Err((code, message)) => return encode_error(id, &code, message),
        };
        let result = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .ok_or_else(|| std::io::Error::other("workspace disappeared"))
            .and_then(|ws| {
                ws.create_tab(
                    rows,
                    cols,
                    cwd,
                    scrollback_limit_bytes,
                    host_terminal_theme,
                    crate::pane::PaneShellConfig::new(&default_shell, self.state.shell_mode),
                    extra_env,
                )
            });
        match result {
            Ok((tab_idx, terminal, runtime)) => {
                self.terminal_runtimes.insert(terminal.id.clone(), runtime);
                self.state.terminals.insert(terminal.id.clone(), terminal);
                self.state.remove_alias_shadowed_by_new_pane(
                    self.state.workspaces[ws_idx].tabs[tab_idx].root_pane,
                );
                if let Some(label) = label {
                    let workspace_id = self.state.workspaces[ws_idx].id.clone();
                    let tab_id = self.public_tab_id(ws_idx, tab_idx).unwrap_or_else(|| {
                        crate::workspace::public_tab_id_for_number(&workspace_id, tab_idx + 1)
                    });
                    if let Some(tab) = self
                        .state
                        .workspaces
                        .get_mut(ws_idx)
                        .and_then(|ws| ws.tabs.get_mut(tab_idx))
                    {
                        tab.set_custom_name(label);
                        crate::logging::tab_renamed(&workspace_id, &tab_id);
                    }
                }
                if focus {
                    self.state.switch_workspace_tab(ws_idx, tab_idx);
                    self.state.mode = Mode::Terminal;
                }
                self.schedule_session_save();
                self.emit_tab_created_events(ws_idx, tab_idx);
                encode_success(
                    id,
                    self.tab_created_result(ws_idx, tab_idx)
                        .expect("new tab should produce a complete create response"),
                )
            }
            Err(err) => encode_error(id, "tab_create_failed", err.to_string()),
        }
    }

    pub(super) fn handle_tab_focus(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        self.state.switch_workspace_tab(ws_idx, tab_idx);
        let tab = self.tab_info(ws_idx, tab_idx).unwrap();

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_rename(&mut self, id: String, params: TabRenameParams) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&params.tab_id) else {
            return tab_not_found(id, &params.tab_id);
        };
        let workspace_id = self.state.workspaces[ws_idx].id.clone();
        let tab_id = self.public_tab_id(ws_idx, tab_idx).unwrap_or_else(|| {
            crate::workspace::public_tab_id_for_number(&workspace_id, tab_idx + 1)
        });
        let Some(tab) = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .and_then(|ws| ws.tabs.get_mut(tab_idx))
        else {
            return tab_not_found(id, &params.tab_id);
        };
        tab.set_custom_name(params.label.clone());
        crate::logging::tab_renamed(&workspace_id, &tab_id);
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::TabRenamed,
            data: EventData::TabRenamed {
                tab_id: self.public_tab_id(ws_idx, tab_idx).unwrap(),
                workspace_id: self.public_workspace_id(ws_idx),
                label: params.label,
            },
        });
        let tab = self.tab_info(ws_idx, tab_idx).unwrap();

        encode_success(id, ResponseResult::TabInfo { tab })
    }

    pub(super) fn handle_tab_move(&mut self, id: String, params: TabMoveParams) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&params.tab_id) else {
            return tab_not_found(id, &params.tab_id);
        };
        if params.new_workspace && params.workspace_id.is_some() {
            return encode_error(
                id,
                "tab_move_failed",
                "new_workspace conflicts with workspace_id",
            );
        }
        let target_ws_idx = match &params.workspace_id {
            Some(workspace_id) => match self.parse_workspace_id(workspace_id) {
                Some(idx) => Some(idx),
                None => return workspace_not_found(id, workspace_id),
            },
            None => None,
        };
        if params.new_workspace || target_ws_idx.is_some_and(|idx| idx != ws_idx) {
            return self.handle_tab_move_across_workspaces(
                id,
                ws_idx,
                tab_idx,
                target_ws_idx,
                params.insert_index,
            );
        }
        let Some(insert_index) = params.insert_index else {
            // Same workspace and no position asked for: nothing to change.
            let tabs = self.tab_list_info(ws_idx);
            return encode_success(id, ResponseResult::TabList { tabs });
        };
        let Some(ws) = self.state.workspaces.get(ws_idx) else {
            return tab_not_found(id, &params.tab_id);
        };
        if insert_index > ws.tabs.len() {
            return encode_error(
                id,
                "tab_move_failed",
                format!("insert_index {insert_index} is out of bounds"),
            );
        }

        let tab_id = self
            .public_tab_id(ws_idx, tab_idx)
            .unwrap_or_else(|| crate::workspace::public_tab_id_for_number(&ws.id, tab_idx + 1));
        let workspace_id = self.public_workspace_id(ws_idx);
        let moved = self
            .state
            .workspaces
            .get_mut(ws_idx)
            .is_some_and(|ws| ws.move_tab(tab_idx, insert_index));
        let tabs = self.tab_list_info(ws_idx);
        if moved {
            self.schedule_session_save();
            if self.state.active == Some(ws_idx) {
                self.state.tab_scroll_follow_active = true;
                self.state.refresh_tab_bar_view();
            }
            self.emit_event(EventEnvelope {
                event: EventKind::TabMoved,
                data: EventData::TabMoved {
                    tab_id,
                    workspace_id,
                    insert_index,
                    tabs: tabs.clone(),
                },
            });
        }

        encode_success(id, ResponseResult::TabList { tabs })
    }

    /// Move a whole tab into another workspace (or a fresh one).
    ///
    /// Built on `pane.move`, one pane at a time, so every invariant that path
    /// keeps — public-id aliases, the empty source workspace closing, the
    /// recovery on a failed take — holds here too. The first pane opens the
    /// tab at the destination and carries the tab's name; the rest split in
    /// beside it, in layout order. Ratios are not preserved: a move keeps
    /// which panes are together, not how wide each was.
    fn handle_tab_move_across_workspaces(
        &mut self,
        id: String,
        ws_idx: usize,
        tab_idx: usize,
        target_ws_idx: Option<usize>,
        insert_index: Option<usize>,
    ) -> String {
        use crate::api::schema::{
            PaneMoveDestination, PaneMoveParams, SplitDirection, SuccessResponse,
        };

        let (zoomed, label, raw_pane_ids) = {
            let Some(tab) = self
                .state
                .workspaces
                .get(ws_idx)
                .and_then(|ws| ws.tabs.get(tab_idx))
            else {
                return encode_error(id, "tab_not_found", "tab not found");
            };
            (tab.zoomed, tab.custom_name.clone(), tab.layout.pane_ids())
        };
        if zoomed {
            return encode_error(
                id,
                "tab_move_failed",
                "unzoom the tab before moving it to another workspace",
            );
        }
        let pane_ids: Vec<String> = raw_pane_ids
            .iter()
            .filter_map(|pane_id| self.public_pane_id(ws_idx, *pane_id))
            .collect();
        let Some(first) = pane_ids.first().cloned() else {
            return encode_error(id, "tab_move_failed", "tab has no panes to move");
        };
        let destination = match target_ws_idx {
            Some(target_ws_idx) => PaneMoveDestination::NewTab {
                workspace_id: Some(self.public_workspace_id(target_ws_idx)),
                label: label.clone(),
            },
            None => PaneMoveDestination::NewWorkspace {
                label: None,
                tab_label: label,
            },
        };
        let response = self.handle_pane_move(
            id.clone(),
            PaneMoveParams {
                pane_id: first,
                destination,
                focus: false,
            },
        );
        let Ok(success) = serde_json::from_str::<SuccessResponse>(&response) else {
            // A pane.move error already names the reason; pass it through.
            return response;
        };
        let ResponseResult::PaneMove { move_result } = success.result else {
            return encode_error(id, "tab_move_failed", "unexpected pane.move response");
        };
        let Some(new_tab) = move_result.created_tab else {
            return encode_error(id, "tab_move_failed", "moving the tab produced no tab");
        };
        for pane_id in pane_ids.into_iter().skip(1) {
            let response = self.handle_pane_move(
                id.clone(),
                PaneMoveParams {
                    pane_id,
                    destination: PaneMoveDestination::Tab {
                        tab_id: new_tab.tab_id.clone(),
                        target_pane_id: None,
                        split: SplitDirection::Right,
                        ratio: None,
                    },
                    focus: false,
                },
            );
            if serde_json::from_str::<SuccessResponse>(&response).is_err() {
                return response;
            }
        }
        let Some((new_ws_idx, new_tab_idx)) = self.parse_tab_id(&new_tab.tab_id) else {
            return encode_error(id, "tab_move_failed", "moved tab disappeared");
        };
        let mut landed = new_tab_idx;
        if let Some(insert_index) = insert_index {
            let len = self.state.workspaces[new_ws_idx].tabs.len();
            if insert_index > len {
                return encode_error(
                    id,
                    "tab_move_failed",
                    format!("insert_index {insert_index} is out of bounds"),
                );
            }
            if self.state.workspaces[new_ws_idx].move_tab(new_tab_idx, insert_index) {
                landed = self
                    .parse_tab_id(&new_tab.tab_id)
                    .map(|(_, tab_idx)| tab_idx)
                    .unwrap_or(new_tab_idx);
            }
        }
        self.schedule_session_save();
        if self.state.active == Some(new_ws_idx) {
            self.state.tab_scroll_follow_active = true;
            self.state.refresh_tab_bar_view();
        }
        let workspace_id = self.public_workspace_id(new_ws_idx);
        let tabs = self.tab_list_info(new_ws_idx);
        self.emit_event(EventEnvelope {
            event: EventKind::TabMoved,
            data: EventData::TabMoved {
                tab_id: new_tab.tab_id,
                workspace_id,
                insert_index: landed,
                tabs: tabs.clone(),
            },
        });
        encode_success(id, ResponseResult::TabList { tabs })
    }

    pub(super) fn handle_tab_close(&mut self, id: String, target: TabTarget) -> String {
        let Some((ws_idx, tab_idx)) = self.parse_tab_id(&target.tab_id) else {
            return tab_not_found(id, &target.tab_id);
        };
        let Some(tab_id) = self.public_tab_id(ws_idx, tab_idx) else {
            return tab_not_found(id, &target.tab_id);
        };
        let workspace_id = self.public_workspace_id(ws_idx);
        let terminal_ids = self.state.terminal_ids_for_tab(ws_idx, tab_idx);
        let pane_ids = self
            .state
            .workspaces
            .get(ws_idx)
            .and_then(|ws| ws.tabs.get(tab_idx))
            .map(|tab| tab.layout.pane_ids())
            .unwrap_or_default();
        let Some(ws) = self.state.workspaces.get_mut(ws_idx) else {
            return tab_not_found(id, &target.tab_id);
        };
        if ws.tabs.len() <= 1 {
            return encode_error(
                id,
                "tab_close_failed",
                "cannot close the last tab in a workspace",
            );
        }
        if !ws.close_tab(tab_idx) {
            return encode_error(
                id,
                "tab_close_failed",
                format!("tab {} could not be closed", target.tab_id),
            );
        }
        self.state.remove_plugin_pane_records(pane_ids);
        self.state.remove_unattached_terminal_ids(terminal_ids);
        self.shutdown_detached_terminal_runtimes();
        self.schedule_session_save();
        self.emit_event(EventEnvelope {
            event: EventKind::TabClosed,
            data: EventData::TabClosed {
                tab_id,
                workspace_id,
            },
        });

        encode_success(id, ResponseResult::Ok {})
    }

    fn tab_list_info(&self, ws_idx: usize) -> Vec<crate::api::schema::TabInfo> {
        self.state
            .workspaces
            .get(ws_idx)
            .map(|ws| {
                (0..ws.tabs.len())
                    .filter_map(|idx| self.tab_info(ws_idx, idx))
                    .collect()
            })
            .unwrap_or_default()
    }
}

fn workspace_not_found(id: String, workspace_id: &str) -> String {
    encode_error(
        id,
        "workspace_not_found",
        format!("workspace {workspace_id} not found"),
    )
}

fn tab_not_found(id: String, tab_id: &str) -> String {
    encode_error(id, "tab_not_found", format!("tab {tab_id} not found"))
}

#[cfg(test)]
mod tests {
    use super::super::test_support::{exiting_test_command, shutdown_test_runtimes};
    use super::*;
    use crate::{
        api::schema::SuccessResponse,
        config::{Config, ShellModeConfig},
        workspace::Workspace,
    };

    #[test]
    fn api_tab_move_reorders_tabs_in_target_workspace() {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(&Config::default(), true, None, api_rx, event_hub.clone());
        let mut workspace = Workspace::test_new("tabs");
        workspace.test_add_tab(Some("two"));
        workspace.test_add_tab(Some("three"));
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.selected = 0;
        let moved_root = app.state.workspaces[0].tabs[0].root_pane;
        let moved_id = app.public_tab_id(0, 0).unwrap();

        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id.clone(),
                workspace_id: None,
                new_workspace: false,
                insert_index: Some(3),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabList { tabs } = success.result else {
            panic!("expected tab list");
        };
        assert_eq!(app.state.workspaces[0].tabs[2].root_pane, moved_root);
        assert_eq!(tabs[2].tab_id, app.public_tab_id(0, 2).unwrap());
        let events = event_hub.events_after(0);
        assert!(events.iter().any(|(_, event)| {
            matches!(
                &event.data,
                EventData::TabMoved {
                    tab_id,
                    workspace_id,
                    insert_index: 3,
                    tabs,
                } if tab_id == &moved_id
                    && workspace_id == &app.public_workspace_id(0)
                    && tabs[2].tab_id == moved_id
            )
        }));
    }

    #[test]
    fn api_tab_move_into_another_workspace_carries_every_pane_and_the_name() {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(&Config::default(), true, None, api_rx, event_hub.clone());
        let mut source = Workspace::test_new("source");
        source.active_tab = source.test_add_tab(Some("review"));
        source.test_split(ratatui::layout::Direction::Horizontal);
        app.state.workspaces = vec![source, Workspace::test_new("target")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let moved_terminals: Vec<_> = app.state.workspaces[0].tabs[1]
            .panes
            .values()
            .map(|pane| pane.attached_terminal_id.to_string())
            .collect();
        assert_eq!(moved_terminals.len(), 2, "the moved tab holds two panes");
        let moved_id = app.public_tab_id(0, 1).unwrap();
        let target_workspace_id = app.public_workspace_id(1);

        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id,
                workspace_id: Some(target_workspace_id.clone()),
                new_workspace: false,
                insert_index: None,
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabList { tabs } = success.result else {
            panic!("expected tab list");
        };
        assert_eq!(tabs.len(), 2, "target gained one tab: {tabs:?}");
        assert_eq!(tabs[1].workspace_id, target_workspace_id);
        assert_eq!(tabs[1].label, "review");
        assert_eq!(
            app.state.workspaces[0].tabs.len(),
            1,
            "source kept its other tab"
        );
        let landed = &app.state.workspaces[1].tabs[1];
        assert_eq!(landed.custom_name.as_deref(), Some("review"));
        let mut landed_terminals: Vec<_> = landed
            .panes
            .values()
            .map(|pane| pane.attached_terminal_id.to_string())
            .collect();
        let mut expected = moved_terminals;
        landed_terminals.sort();
        expected.sort();
        assert_eq!(landed_terminals, expected, "both panes travelled");
        app.state.assert_invariants_for_test();
        let events = event_hub.events_after(0);
        assert!(events.iter().any(|(_, event)| {
            matches!(
                &event.data,
                EventData::TabMoved { workspace_id, insert_index: 1, .. }
                    if workspace_id == &target_workspace_id
            )
        }));
    }

    #[test]
    fn api_tab_move_into_a_new_workspace_and_at_an_index() {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(&Config::default(), true, None, api_rx, event_hub);
        let mut source = Workspace::test_new("source");
        source.test_add_tab(Some("solo"));
        let mut target = Workspace::test_new("target");
        target.test_add_tab(Some("second"));
        app.state.workspaces = vec![source, target];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;

        // Into a fresh workspace of its own.
        let moved_id = app.public_tab_id(0, 1).unwrap();
        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id,
                workspace_id: None,
                new_workspace: true,
                insert_index: None,
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabList { tabs } = success.result else {
            panic!("expected tab list");
        };
        assert_eq!(app.state.workspaces.len(), 3);
        assert_eq!(tabs.len(), 1);
        assert_eq!(tabs[0].label, "solo");
        assert_eq!(tabs[0].workspace_id, app.public_workspace_id(2));

        // Into the front of an existing strip.
        let moved_id = app.public_tab_id(2, 0).unwrap();
        let target_workspace_id = app.public_workspace_id(1);
        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id,
                workspace_id: Some(target_workspace_id),
                new_workspace: false,
                insert_index: Some(0),
            },
        );
        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        let ResponseResult::TabList { tabs } = success.result else {
            panic!("expected tab list");
        };
        assert_eq!(tabs.len(), 3, "{tabs:?}");
        assert_eq!(tabs[0].label, "solo", "landed at the front");
        assert_eq!(tabs[2].label, "second");
        assert_eq!(
            app.state.workspaces.len(),
            2,
            "the emptied workspace closed behind the tab"
        );
        app.state.assert_invariants_for_test();
    }

    #[test]
    fn api_tab_move_rejects_conflicting_destinations_and_zoomed_tabs() {
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(
            &Config::default(),
            true,
            None,
            api_rx,
            crate::api::EventHub::default(),
        );
        let mut source = Workspace::test_new("source");
        source.test_add_tab(Some("zoomed"));
        app.state.workspaces = vec![source, Workspace::test_new("target")];
        app.state.ensure_test_terminals();
        app.state.active = Some(0);
        app.state.selected = 0;
        let moved_id = app.public_tab_id(0, 1).unwrap();
        let target_workspace_id = app.public_workspace_id(1);

        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id.clone(),
                workspace_id: Some(target_workspace_id.clone()),
                new_workspace: true,
                insert_index: None,
            },
        );
        assert!(response.contains("new_workspace conflicts"), "{response}");

        app.state.workspaces[0].tabs[1].zoomed = true;
        let response = app.handle_tab_move(
            "req".into(),
            TabMoveParams {
                tab_id: moved_id,
                workspace_id: Some(target_workspace_id),
                new_workspace: false,
                insert_index: None,
            },
        );
        assert!(response.contains("unzoom"), "{response}");
        assert_eq!(app.state.workspaces[0].tabs.len(), 2, "nothing moved");
    }

    #[tokio::test]
    async fn tab_create_follows_cached_focused_pane_cwd_without_runtime() {
        let event_hub = crate::api::EventHub::default();
        let (_api_tx, api_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut app = App::new(&Config::default(), true, None, api_rx, event_hub);
        app.state.default_shell = exiting_test_command().into();
        app.state.shell_mode = ShellModeConfig::NonLogin;
        let workspace = Workspace::test_new("tabs");
        let focused_pane = workspace.tabs[0].root_pane;
        app.state.workspaces = vec![workspace];
        app.state.active = Some(0);
        app.state.selected = 0;
        app.state.ensure_test_terminals();
        let cached_cwd = std::env::temp_dir();
        let terminal_id = app.state.workspaces[0]
            .terminal_id(focused_pane)
            .cloned()
            .unwrap();
        app.state.terminals.get_mut(&terminal_id).unwrap().cwd = cached_cwd.clone();

        let response = app.handle_tab_create(
            "req".into(),
            TabCreateParams {
                workspace_id: None,
                cwd: None,
                focus: false,
                label: None,
                env: Default::default(),
            },
        );

        let success: SuccessResponse = serde_json::from_str(&response).unwrap();
        assert!(matches!(success.result, ResponseResult::TabCreated { .. }));
        let created = &app.state.workspaces[0].tabs[1];
        let created_terminal_id = created.terminal_id(created.root_pane).unwrap();
        let created_cwd = &app.state.terminals.get(created_terminal_id).unwrap().cwd;
        assert_eq!(
            crate::worktree::canonical_or_original(created_cwd),
            crate::worktree::canonical_or_original(&cached_cwd)
        );
        shutdown_test_runtimes(&mut app);
    }
}
