pub mod client;
mod event_hub;
pub mod schema;
mod server;
mod status;
mod subscriptions;
mod wait;

pub use event_hub::EventHub;
pub use server::{start_server, start_server_with_capabilities, ServerHandle};
pub use status::{read_runtime_status_at, RuntimeStatus};

use std::path::PathBuf;

use tokio::sync::mpsc;

use crate::api::schema::{Method, Request};

pub const SOCKET_PATH_ENV_VAR: &str = "SHEP_SOCKET_PATH";

pub(crate) fn request_changes_ui(request: &Request) -> bool {
    matches!(
        &request.method,
        Method::ServerReloadConfig(_)
            | Method::ServerReloadAgentManifests(_)
            | Method::NotificationShow(_)
            | Method::WorkspaceCreate(_)
            | Method::WorkspaceFocus(_)
            | Method::WorkspaceRename(_)
            | Method::WorkspaceMove(_)
            | Method::WorkspaceClose(_)
            | Method::WorktreeCreate(_)
            | Method::WorktreeOpen(_)
            | Method::WorktreeRemove(_)
            | Method::TabCreate(_)
            | Method::TabFocus(_)
            | Method::TabRename(_)
            | Method::TabMove(_)
            | Method::TabClose(_)
            | Method::LayoutApply(_)
            | Method::LayoutSetSplitRatio(_)
            | Method::AgentRename(_)
            | Method::AgentFocus(_)
            | Method::AgentStart(_)
            | Method::PaneSplit(_)
            | Method::PaneSwap(_)
            | Method::PaneMove(_)
            | Method::PaneZoom(_)
            | Method::PaneFocusDirection(_)
            | Method::PaneResize(_)
            | Method::PaneFocus(_)
            | Method::PaneRename(_)
            | Method::PaneReportAgent(_)
            | Method::PaneReportAgentSession(_)
            | Method::PaneReportMetadata(_)
            | Method::PaneClearAgentAuthority(_)
            | Method::PaneReleaseAgent(_)
            | Method::PaneClose(_)
            | Method::PluginActionInvoke(_)
            | Method::PluginPaneOpen(_)
            | Method::PluginPaneFocus(_)
            | Method::PluginPaneClose(_)
            // The docket and the overseer are on the board, so a phone
            // keeping or dropping a proposal must repaint the desktop.
            | Method::DocketAdd(_)
            | Method::DocketUpdate(_)
            | Method::DocketPromote(_)
            | Method::DocketComplete(_)
            | Method::DocketDiscard(_)
            | Method::OverseerChat(_)
            | Method::OverseerTick(_)
    )
}

pub struct ApiRequestMessage {
    pub request: Request,
    pub respond_to: std::sync::mpsc::Sender<String>,
}

pub type ApiRequestSender = mpsc::UnboundedSender<ApiRequestMessage>;

pub fn socket_path() -> PathBuf {
    crate::session::active_api_socket_path()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::schema::{
        DocketListParams, DocketTarget, EmptyParams, OverseerChatParams, OverseerSampleParams,
        OverseerTickParams,
    };

    fn changes_ui(method: Method) -> bool {
        request_changes_ui(&Request {
            id: "req".into(),
            method,
        })
    }

    #[test]
    fn mutating_docket_and_overseer_methods_repaint_the_ui() {
        // The board draws the docket and the overseer, so anything that
        // moves them has to wake the desktop's render.
        assert!(changes_ui(Method::DocketAdd(
            crate::api::schema::DocketAddParams {
                title: "a thing".into(),
                kind: None,
                status: None,
                due: None,
                repeat: None,
                source: None,
                notes: None,
            }
        )));
        assert!(changes_ui(Method::DocketUpdate(
            crate::api::schema::DocketUpdateParams {
                id: 1,
                title: None,
                notes: None,
                due: None,
                repeat: None,
                kind: None,
            }
        )));
        assert!(changes_ui(Method::DocketPromote(
            crate::api::schema::DocketPromoteParams {
                id: 1,
                kind: crate::api::schema::DocketKind::Slated,
                due: None,
                repeat: None,
            }
        )));
        assert!(changes_ui(Method::DocketComplete(DocketTarget { id: 1 })));
        assert!(changes_ui(Method::DocketDiscard(DocketTarget { id: 1 })));
        assert!(changes_ui(Method::OverseerChat(OverseerChatParams {
            text: "what needs me?".into(),
        })));
        assert!(changes_ui(Method::OverseerTick(
            OverseerTickParams::default()
        )));
    }

    #[test]
    fn reads_do_not_repaint_the_ui() {
        assert!(!changes_ui(Method::DocketList(DocketListParams::default())));
        assert!(!changes_ui(Method::OverseerSample(
            OverseerSampleParams::default()
        )));
        assert!(!changes_ui(Method::SessionSnapshot(EmptyParams::default())));
    }
}
