//! Docket API types: a personal assistant's list of recurring, slated and
//! captured items, owned by the server in `<state dir>/docket.db` and served
//! over `docket.*`. Nothing here dispatches work — the docket is a reminder
//! surface, not a queue.

use serde::{Deserialize, Serialize};

/// Where an item came from and how it is meant to be handled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocketKind {
    /// Proposed from a session or a memory file; waits in the inbox until
    /// promoted or discarded.
    Captured,
    /// Dated, one-off.
    Slated,
    /// Fires by `due`; completing it rolls `due` forward by `repeat`.
    Recurring,
}

impl DocketKind {
    pub fn as_str(self) -> &'static str {
        match self {
            DocketKind::Captured => "captured",
            DocketKind::Slated => "slated",
            DocketKind::Recurring => "recurring",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "captured" => DocketKind::Captured,
            "slated" => DocketKind::Slated,
            "recurring" => DocketKind::Recurring,
            _ => return None,
        })
    }
}

/// Lifecycle of an item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum DocketStatus {
    Inbox,
    Open,
    Done,
    Discarded,
}

impl DocketStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            DocketStatus::Inbox => "inbox",
            DocketStatus::Open => "open",
            DocketStatus::Done => "done",
            DocketStatus::Discarded => "discarded",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "inbox" => DocketStatus::Inbox,
            "open" => DocketStatus::Open,
            "done" => DocketStatus::Done,
            "discarded" => DocketStatus::Discarded,
            _ => return None,
        })
    }
}

/// How far `due` moves when a recurring item is completed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum DocketRepeat {
    #[serde(rename = "1d")]
    Daily,
    #[serde(rename = "1w")]
    Weekly,
    #[serde(rename = "2w")]
    Fortnightly,
    #[serde(rename = "1m")]
    Monthly,
}

impl DocketRepeat {
    pub fn as_str(self) -> &'static str {
        match self {
            DocketRepeat::Daily => "1d",
            DocketRepeat::Weekly => "1w",
            DocketRepeat::Fortnightly => "2w",
            DocketRepeat::Monthly => "1m",
        }
    }

    pub fn parse(raw: &str) -> Option<Self> {
        Some(match raw {
            "1d" => DocketRepeat::Daily,
            "1w" => DocketRepeat::Weekly,
            "2w" => DocketRepeat::Fortnightly,
            "1m" => DocketRepeat::Monthly,
            _ => return None,
        })
    }
}

/// One docket row as the API returns it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocketItem {
    pub id: i64,
    pub title: String,
    pub kind: DocketKind,
    pub status: DocketStatus,
    /// `YYYY-MM-DD`, local calendar.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<DocketRepeat>,
    /// Free-form provenance (`{file, line}` or `{pane, session}`); opaque to
    /// the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    /// ISO 8601 UTC.
    pub created: String,
    /// ISO 8601 UTC.
    pub updated: String,
    /// ISO 8601 UTC; the last time a recurring item was completed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_fired: Option<String>,
    /// `status == open && due < today`.
    #[serde(default)]
    pub overdue: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(default)]
pub struct DocketListParams {
    /// Only items in this status; omitted = every item.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<DocketStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocketAddParams {
    pub title: String,
    /// Defaults to `captured`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DocketKind>,
    /// Defaults to `inbox` for captured items and `open` otherwise.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<DocketStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<DocketRepeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
}

/// Fields omitted are left unchanged.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocketUpdateParams {
    pub id: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub notes: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<DocketRepeat>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<DocketKind>,
}

/// Move an inbox item into the docket proper as a slated or recurring item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocketPromoteParams {
    pub id: i64,
    pub kind: DocketKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub due: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repeat: Option<DocketRepeat>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DocketTarget {
    pub id: i64,
}
