//! `overseer.*` — the overseer over the JSON API, so a companion sees the
//! same read of the room the desktop board does.
//!
//! Three verbs: `overseer.sample` reads what the plugin last wrote (and never
//! creates anything doing it), `overseer.chat` puts one question to the
//! overseer's headless runtime, and `overseer.tick` asks the plugin for a
//! fresh situation when the last one has aged out. The answers to a question
//! and the files moving both arrive as events (`overseer.chat_turn`,
//! `overseer.updated`), so a client need not poll to stay current.

use crate::api::schema::{
    OverseerChatParams, OverseerSampleParams, OverseerTickParams, ResponseResult,
};
use crate::app::overseer::{CHAT_BUSY, STALE_SITUATION_SECS};

use super::responses::{encode_error, encode_success};

impl super::App {
    pub(super) fn handle_overseer_sample(
        &mut self,
        id: String,
        params: OverseerSampleParams,
    ) -> String {
        // An API client has no TUI tick behind it, so look at the dir here;
        // the TTL makes a polling client cost what the board costs. A look
        // that found something moved is news for every subscriber, not just
        // this caller.
        if self
            .state
            .refresh_overseer_if_stale(std::time::Instant::now())
        {
            self.emit_overseer_updated();
        }
        encode_success(
            id,
            ResponseResult::OverseerSample {
                sample: Box::new(self.overseer_sample_info(params.chat_turns)),
            },
        )
    }

    pub(super) fn handle_overseer_chat(
        &mut self,
        id: String,
        params: OverseerChatParams,
    ) -> String {
        match self.send_overseer_chat(params.text) {
            Ok(turn) => {
                let (session_id, started) = self.state.overseer.peek_session();
                encode_success(
                    id,
                    ResponseResult::OverseerChat {
                        turn,
                        session: crate::api::schema::OverseerSessionInfo {
                            id: session_id,
                            started,
                        },
                    },
                )
            }
            Err(reason) if reason == CHAT_BUSY => encode_error(id, "overseer_chat_busy", reason),
            Err(reason) => encode_error(id, "invalid_params", reason),
        }
    }

    pub(super) fn handle_overseer_tick(
        &mut self,
        id: String,
        params: OverseerTickParams,
    ) -> String {
        let max_age = params.max_age_seconds.unwrap_or(STALE_SITUATION_SECS);
        match self.overseer_tick(max_age, "api") {
            Ok((invoked, in_flight, log_id)) => encode_success(
                id,
                ResponseResult::OverseerTick {
                    invoked,
                    in_flight,
                    situation_age_seconds: self
                        .state
                        .overseer
                        .sample
                        .situation_age(std::time::SystemTime::now())
                        .map(|age| age.as_secs()),
                    log_id,
                },
            ),
            Err(err) => encode_error(id, "overseer_tick_failed", err),
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::api::schema::{
        EventData, EventKind, Method, OverseerChatParams, OverseerSampleParams, OverseerTickParams,
        Request,
    };
    use crate::app::input::overseer::tests::{cleanup, overseer_app};
    use crate::app::overseer::test_state_dir;

    fn call(app: &mut crate::app::App, method: Method) -> serde_json::Value {
        let encoded = app.handle_api_request(Request {
            id: "req".into(),
            method,
        });
        serde_json::from_str(&encoded).expect("a json response")
    }

    /// The events this app has pushed, oldest first.
    fn events(app: &crate::app::App) -> Vec<crate::api::schema::EventEnvelope> {
        app.event_hub
            .events_after(0)
            .into_iter()
            .map(|(_, envelope)| envelope)
            .collect()
    }

    fn overseer_events(app: &crate::app::App, kind: EventKind) -> Vec<EventData> {
        events(app)
            .into_iter()
            .filter(|envelope| envelope.event == kind)
            .map(|envelope| envelope.data)
            .collect()
    }

    #[tokio::test]
    async fn sample_reads_a_dir_it_never_creates() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        app.state.overseer.state_dir = dir.clone();

        let value = call(
            &mut app,
            Method::OverseerSample(OverseerSampleParams::default()),
        );
        let sample = &value["result"]["sample"];
        assert_eq!(value["result"]["type"], "overseer_sample");
        assert_eq!(sample["sampled"], true);
        assert_eq!(sample["plugin_linked"], false, "nothing is linked");
        assert_eq!(sample["source"], "deterministic");
        assert!(sample["narrative"].as_array().unwrap().is_empty());
        assert!(sample["session"]["id"].is_null(), "reading mints nothing");
        assert_eq!(sample["session"]["started"], false);
        assert_eq!(sample["chat_total"], 0);
        assert_eq!(sample["tick_in_flight"], false);
        assert!(!dir.exists(), "a read created the state dir");

        cleanup(&docket);
    }

    #[tokio::test]
    async fn chat_records_the_question_and_announces_it() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        app.state.overseer.state_dir = dir.clone();
        // No runtime: the overseer answers with the fact, synchronously.
        crate::env_compat::remove_process_env_for_test("SHEP_OVERSEER_RUNTIME");

        let value = call(
            &mut app,
            Method::OverseerChat(OverseerChatParams {
                text: "  what needs me?  ".into(),
            }),
        );
        assert_eq!(value["result"]["type"], "overseer_chat");
        assert_eq!(value["result"]["turn"]["role"], "you");
        assert_eq!(value["result"]["turn"]["text"], "what needs me?");

        let turns = overseer_events(&app, EventKind::OverseerChatTurn);
        assert_eq!(turns.len(), 2, "{turns:?}");
        match (&turns[0], &turns[1]) {
            (
                EventData::OverseerChatTurn {
                    turn: asked,
                    pending: true,
                },
                EventData::OverseerChatTurn {
                    turn: answered,
                    pending: false,
                },
            ) => {
                assert_eq!(asked.role, crate::api::schema::OverseerChatRole::You);
                assert_eq!(
                    answered.role,
                    crate::api::schema::OverseerChatRole::Overseer
                );
                assert_eq!(answered.text, crate::app::overseer::CHAT_NO_RUNTIME);
            }
            other => panic!("{other:?}"),
        }
        assert!(!app.state.overseer.chat_pending);

        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&docket);
    }

    #[tokio::test]
    async fn a_question_while_one_is_out_is_refused() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        app.state.overseer.state_dir = dir.clone();
        app.state.overseer.chat_pending = true;

        let value = call(
            &mut app,
            Method::OverseerChat(OverseerChatParams {
                text: "and another thing".into(),
            }),
        );
        assert_eq!(value["error"]["code"], "overseer_chat_busy");
        assert!(app.state.overseer.chat.is_empty());

        let blank = {
            app.state.overseer.chat_pending = false;
            call(
                &mut app,
                Method::OverseerChat(OverseerChatParams { text: "   ".into() }),
            )
        };
        assert_eq!(blank["error"]["code"], "invalid_params");

        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&docket);
    }

    #[tokio::test]
    async fn a_finished_answer_announces_the_overseer_turn() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        app.state.overseer.state_dir = dir.clone();
        app.state.overseer.chat_pending = true;

        app.handle_internal_event(crate::events::AppEvent::OverseerChatFinished {
            answer: Ok("answer workmayt first.".into()),
        });

        let turns = overseer_events(&app, EventKind::OverseerChatTurn);
        assert_eq!(turns.len(), 1, "{turns:?}");
        match &turns[0] {
            EventData::OverseerChatTurn { turn, pending } => {
                assert_eq!(turn.role, crate::api::schema::OverseerChatRole::Overseer);
                assert_eq!(turn.text, "answer workmayt first.");
                assert!(!pending, "the question is answered");
            }
            other => panic!("{other:?}"),
        }

        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&docket);
    }

    #[tokio::test]
    async fn a_sample_that_finds_new_files_announces_them_once() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        std::fs::create_dir_all(&dir).expect("state dir");
        std::fs::write(dir.join("BOARD.md"), "claude is blocked.\n").expect("board");
        app.state.overseer.state_dir = dir.clone();

        let value = call(
            &mut app,
            Method::OverseerSample(OverseerSampleParams::default()),
        );
        assert_eq!(
            value["result"]["sample"]["narrative"][0],
            "claude is blocked."
        );
        assert_eq!(overseer_events(&app, EventKind::OverseerUpdated).len(), 1);

        // Inside the TTL nothing is even stat'ed, so nothing is announced.
        let _ = call(
            &mut app,
            Method::OverseerSample(OverseerSampleParams::default()),
        );
        assert_eq!(overseer_events(&app, EventKind::OverseerUpdated).len(), 1);

        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&docket);
    }

    #[tokio::test]
    async fn tick_skips_a_fresh_situation_and_reports_one_in_flight() {
        let (mut app, docket, _) = overseer_app();
        let dir = test_state_dir();
        std::fs::create_dir_all(&dir).expect("state dir");
        std::fs::write(dir.join("situation.json"), "{}").expect("situation");
        app.state.overseer.state_dir = dir.clone();
        app.state.overseer.refresh(std::time::Instant::now());

        let fresh = call(
            &mut app,
            Method::OverseerTick(OverseerTickParams::default()),
        );
        assert_eq!(fresh["result"]["type"], "overseer_tick");
        assert_eq!(fresh["result"]["invoked"], false);
        assert_eq!(fresh["result"]["in_flight"], false);
        assert!(fresh["result"]["log_id"].is_null());

        // A tick already out is reported, never stacked on.
        app.state.overseer.tick_in_flight = Some("log-7".into());
        let busy = call(
            &mut app,
            Method::OverseerTick(OverseerTickParams {
                max_age_seconds: Some(0),
            }),
        );
        assert_eq!(busy["result"]["invoked"], false);
        assert_eq!(busy["result"]["in_flight"], true);
        assert_eq!(busy["result"]["log_id"], "log-7");

        let _ = std::fs::remove_dir_all(&dir);
        cleanup(&docket);
    }

    #[tokio::test]
    async fn tick_without_the_plugin_says_so() {
        let (mut app, docket, _) = overseer_app();
        app.state.overseer.state_dir = test_state_dir();

        let value = call(
            &mut app,
            Method::OverseerTick(OverseerTickParams {
                max_age_seconds: Some(0),
            }),
        );
        assert_eq!(value["error"]["code"], "overseer_tick_failed");
        assert!(app.state.overseer.tick_in_flight.is_none());

        cleanup(&docket);
    }

    #[test]
    fn a_docket_mutation_repaints_the_desktop() {
        // The board draws the docket, so a phone keeping a proposal has to
        // wake the desktop's render loop.
        assert!(crate::api::request_changes_ui(&Request {
            id: "req".into(),
            method: Method::DocketDiscard(crate::api::schema::DocketTarget { id: 1 }),
        }));
    }
}
