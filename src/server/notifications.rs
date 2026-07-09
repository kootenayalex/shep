use crate::app;
use crate::app::state::AppState;
use crate::config;
use crate::detect::AgentState;
use crate::layout::PaneId;
use crate::protocol;
use crate::terminal::TerminalRuntimeRegistry;

pub(crate) fn should_forward_toast_to_clients(delivery: config::ToastDelivery) -> bool {
    toast_notify_kind(delivery).is_some()
}

pub(crate) fn toast_notify_kind(delivery: config::ToastDelivery) -> Option<protocol::NotifyKind> {
    match delivery {
        config::ToastDelivery::Terminal => Some(protocol::NotifyKind::Toast),
        config::ToastDelivery::System => Some(protocol::NotifyKind::SystemToast),
        config::ToastDelivery::Off | config::ToastDelivery::Shep => None,
    }
}

pub(crate) fn toast_message_from_state_change(
    state: &AppState,
    terminal_runtimes: &TerminalRuntimeRegistry,
    pane_id: PaneId,
    suppress_active_tab_notifications: bool,
    prev_state: AgentState,
    new_state: AgentState,
    previous_agent_label: Option<&str>,
) -> Option<String> {
    state
        .workspaces
        .iter()
        .enumerate()
        .find_map(|(ws_idx, ws)| {
            ws.tabs.iter().find_map(|tab| {
                let pane = tab.panes.get(&pane_id)?;
                let agent_label = state
                    .terminals
                    .get(&pane.attached_terminal_id)
                    .and_then(|terminal| terminal.effective_agent_label())?;
                let kind = app::actions::notification_toast_for_state_change_with_agent_labels(
                    suppress_active_tab_notifications,
                    prev_state,
                    new_state,
                    previous_agent_label,
                    Some(agent_label),
                )?;
                let workspace_label = ws.display_name_from(&state.terminals, terminal_runtimes);
                Some(format!(
                    "{} {}: {}",
                    agent_label,
                    toast_event_text(kind),
                    app::actions::notification_context(ws, &workspace_label, ws_idx, pane_id)
                ))
            })
        })
}

/// Outcome of evaluating an effective agent-state transition against the
/// exec-bridge `[notifications]` policy. Pure so it is unit-testable without a
/// running server or spawned process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum NotifyExecDecision {
    /// Do not fire. `clear_debounce` asks the caller to forget any remembered
    /// fire for this pane so a later re-entry into a notify state fires again.
    Skip { clear_debounce: bool },
    /// Fire the exec-bridge and remember `new` as this pane's last fired state.
    Fire,
}

/// Decide whether an effective transition `prev -> new` should fire the
/// exec-bridge. `last_fired` is the state most recently fired for this pane (or
/// `None`). Debounce contract: at most one exec per pane per state-transition —
/// no repeat fire while the pane stays in the same notify state.
pub(crate) fn notify_exec_decision(
    notifications: &config::NotificationsConfig,
    last_fired: Option<AgentState>,
    prev: AgentState,
    new: AgentState,
) -> NotifyExecDecision {
    if prev == new {
        return NotifyExecDecision::Skip {
            clear_debounce: false,
        };
    }
    if !notifications.should_notify(new) {
        // The pane left the notify set; forget the last fire so the next entry
        // into a notify state is treated as a fresh transition.
        return NotifyExecDecision::Skip {
            clear_debounce: true,
        };
    }
    if last_fired == Some(new) {
        return NotifyExecDecision::Skip {
            clear_debounce: false,
        };
    }
    NotifyExecDecision::Fire
}

fn toast_event_text(kind: app::state::ToastKind) -> &'static str {
    match kind {
        app::state::ToastKind::NeedsAttention => "needs attention",
        app::state::ToastKind::Finished => "finished",
        app::state::ToastKind::UpdateInstalled => "updated",
    }
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::*;
    #[cfg(unix)]
    use crate::detect::Agent;
    #[cfg(unix)]
    use crate::terminal::TerminalState;

    use super::{notify_exec_decision, NotifyExecDecision};
    use crate::config::{NotificationsConfig, NotifyState};
    use crate::detect::AgentState;

    fn notify_on(states: &[NotifyState]) -> NotificationsConfig {
        NotificationsConfig {
            notify_on: states.to_vec(),
            exec: None,
        }
    }

    #[test]
    fn notify_exec_skips_when_state_unchanged() {
        let config = notify_on(&[NotifyState::Blocked]);
        assert_eq!(
            notify_exec_decision(&config, None, AgentState::Blocked, AgentState::Blocked),
            NotifyExecDecision::Skip {
                clear_debounce: false
            }
        );
    }

    #[test]
    fn notify_exec_fires_on_transition_into_blocked() {
        let config = notify_on(&[NotifyState::Blocked]);
        assert_eq!(
            notify_exec_decision(&config, None, AgentState::Working, AgentState::Blocked),
            NotifyExecDecision::Fire
        );
    }

    #[test]
    fn notify_exec_does_not_refire_while_state_stays_blocked() {
        let config = notify_on(&[NotifyState::Blocked]);
        // Already fired for blocked; a redundant blocked report must not re-fire.
        assert_eq!(
            notify_exec_decision(
                &config,
                Some(AgentState::Blocked),
                AgentState::Working,
                AgentState::Blocked,
            ),
            NotifyExecDecision::Skip {
                clear_debounce: false
            }
        );
    }

    #[test]
    fn notify_exec_clears_debounce_when_leaving_notify_set() {
        let config = notify_on(&[NotifyState::Blocked]);
        assert_eq!(
            notify_exec_decision(
                &config,
                Some(AgentState::Blocked),
                AgentState::Blocked,
                AgentState::Working,
            ),
            NotifyExecDecision::Skip {
                clear_debounce: true
            }
        );
    }

    #[test]
    fn notify_exec_empty_filter_fires_on_every_transition() {
        let config = notify_on(&[]);
        assert_eq!(
            notify_exec_decision(&config, None, AgentState::Blocked, AgentState::Working),
            NotifyExecDecision::Fire
        );
        assert_eq!(
            notify_exec_decision(
                &config,
                Some(AgentState::Working),
                AgentState::Working,
                AgentState::Idle,
            ),
            NotifyExecDecision::Fire
        );
    }

    #[cfg(unix)]
    fn init_repo(path: &std::path::Path) {
        let status = std::process::Command::new("git")
            .args(["init", "-q"])
            .current_dir(path)
            .status()
            .unwrap();
        assert!(status.success(), "git init failed for {}", path.display());
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn toast_message_uses_live_root_runtime_cwd_label() {
        let mut state = AppState::test_new();
        state
            .workspaces
            .push(crate::workspace::Workspace::test_new("stale"));
        state.ensure_test_terminals();
        let root = state.workspaces[0].tabs[0].root_pane;
        let terminal_id = state.workspaces[0].terminal_id(root).cloned().unwrap();
        let temp_root = std::env::temp_dir().join(format!(
            "shep-forwarded-toast-context-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let stale_cwd = temp_root.join("__shep_original__");
        let live_cwd = temp_root.join("__shep_projects__");
        std::fs::create_dir_all(&stale_cwd).unwrap();
        std::fs::create_dir_all(&live_cwd).unwrap();
        init_repo(&stale_cwd);
        init_repo(&live_cwd);
        state.workspaces[0].custom_name = None;
        state.workspaces[0].identity_cwd = stale_cwd.clone();
        let mut terminal = TerminalState::new(terminal_id.clone(), stale_cwd);
        terminal.set_detected_state(Some(Agent::Codex), AgentState::Idle);
        state.terminals.insert(terminal_id.clone(), terminal);
        let (events, _) = tokio::sync::mpsc::channel(4);
        let runtime = crate::terminal::TerminalRuntime::spawn(
            root,
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
        let mut terminal_runtimes = TerminalRuntimeRegistry::new();
        terminal_runtimes.insert(terminal_id, runtime);

        let message = toast_message_from_state_change(
            &state,
            &terminal_runtimes,
            root,
            false,
            AgentState::Working,
            AgentState::Idle,
            Some("codex"),
        );

        assert_eq!(
            message.as_deref(),
            Some("codex finished: __shep_projects__ · 1")
        );

        for (_, runtime) in terminal_runtimes.drain() {
            runtime.shutdown();
        }
        let _ = std::fs::remove_dir_all(temp_root);
    }
}
