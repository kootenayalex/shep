//! `docket.*` — the personal docket over the JSON API. The store is opened per
//! request (sqlite opens are cheap and the docket is edited by hand, not in a
//! loop), which keeps `AppState` free of a connection handle and means a
//! store failure is one request's error, never a server-wide state.

use crate::api::schema::{
    DocketAddParams, DocketListParams, DocketPromoteParams, DocketTarget, DocketUpdateParams,
    ResponseResult,
};
use crate::docket::{self, DocketError};

use super::responses::{encode_error, encode_success};

impl super::App {
    pub(super) fn handle_docket_list(&mut self, id: String, params: DocketListParams) -> String {
        match with_store(|conn| docket::list(conn, params.status).map_err(DocketError::from)) {
            Ok((today, items)) => encode_success(
                id,
                ResponseResult::DocketList {
                    today: today.format(),
                    items,
                },
            ),
            Err(err) => docket_error(id, err),
        }
    }

    pub(super) fn handle_docket_add(&mut self, id: String, params: DocketAddParams) -> String {
        let item = docket::NewItem {
            title: params.title,
            kind: params.kind,
            status: params.status,
            due: params.due,
            repeat: params.repeat,
            source: params.source,
            notes: params.notes,
        };
        docket_item_response(id, with_store(|conn| docket::add(conn, item)))
    }

    pub(super) fn handle_docket_update(
        &mut self,
        id: String,
        params: DocketUpdateParams,
    ) -> String {
        let patch = docket::ItemPatch {
            title: params.title,
            notes: params.notes,
            due: params.due,
            repeat: params.repeat,
            kind: params.kind,
        };
        docket_item_response(
            id,
            with_store(|conn| docket::update(conn, params.id, patch)),
        )
    }

    pub(super) fn handle_docket_promote(
        &mut self,
        id: String,
        params: DocketPromoteParams,
    ) -> String {
        docket_item_response(
            id,
            with_store(|conn| {
                docket::promote(conn, params.id, params.kind, params.due, params.repeat)
            }),
        )
    }

    pub(super) fn handle_docket_complete(&mut self, id: String, target: DocketTarget) -> String {
        docket_item_response(id, with_store(|conn| docket::complete(conn, target.id)))
    }

    pub(super) fn handle_docket_discard(&mut self, id: String, target: DocketTarget) -> String {
        docket_item_response(id, with_store(|conn| docket::discard(conn, target.id)))
    }
}

fn with_store<T>(
    op: impl FnOnce(&rusqlite::Connection) -> Result<T, DocketError>,
) -> Result<T, DocketError> {
    let conn = docket::open_store(&docket::docket_db_path())?;
    op(&conn)
}

fn docket_item_response(
    id: String,
    result: Result<crate::api::schema::DocketItem, DocketError>,
) -> String {
    match result {
        Ok(item) => encode_success(id, ResponseResult::DocketItem { item }),
        Err(err) => docket_error(id, err),
    }
}

fn docket_error(id: String, err: DocketError) -> String {
    if let DocketError::Store(io_err) = &err {
        tracing::warn!(error = %io_err, "docket store request failed");
    }
    encode_error(id, err.code(), err.to_string())
}
