//! The tools `shep mcp` serves, and the profile that bounds them.
//!
//! A [`Tool`] is a name, the [`Group`]s that grant it, what answers it
//! ([`Backing`]: an API method over the session socket, a bridge-local method,
//! or an in-process probe), a JSON Schema for its arguments, and the mapping
//! from those arguments to the backing method's params.
//!
//! The mapping is where a [`Profile`] *narrows* a tool rather than removing
//! it: with `docket-inbox` but not `docket`, `docket_add` still takes a title
//! and its provenance, but can only ever capture into the inbox. That is the
//! whole point of this file — the tool list, not a prompt, is the boundary, so
//! an unlisted tool is absent from `tools/list` and refused on call, and a
//! listed one cannot be talked into more authority than its profile has.

use std::collections::BTreeSet;
use std::sync::OnceLock;

use serde_json::{json, Map, Value};

/// A capability group. Profiles are built out of these, not out of tool names,
/// so a new tool joins an existing grant instead of needing every config
/// updated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Group {
    /// Read the session, the docket, memory and the overseer's last sample.
    Read,
    /// Capture into the docket inbox and nothing else.
    DocketInbox,
    /// The whole docket: promote, complete, discard, update, and add with a
    /// kind and a due date.
    Docket,
    /// Send text to an agent, queued behind its current turn.
    Nudge,
    /// Send text to an agent immediately, interrupting it.
    Send,
    /// Pin or clear an agent's displayed state.
    Mark,
    /// Mark a pane seen.
    Seen,
    /// Page the registered devices.
    Push,
    /// Talk to the overseer's own headless runtime.
    Overseer,
    /// Ask the overseer plugin for a fresh situation.
    Tick,
}

impl Group {
    pub(crate) const ALL: &'static [Group] = &[
        Group::Read,
        Group::DocketInbox,
        Group::Docket,
        Group::Nudge,
        Group::Send,
        Group::Mark,
        Group::Seen,
        Group::Push,
        Group::Overseer,
        Group::Tick,
    ];

    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Group::Read => "read",
            Group::DocketInbox => "docket-inbox",
            Group::Docket => "docket",
            Group::Nudge => "nudge",
            Group::Send => "send",
            Group::Mark => "mark",
            Group::Seen => "seen",
            Group::Push => "push",
            Group::Overseer => "overseer",
            Group::Tick => "tick",
        }
    }

    pub(crate) fn parse(raw: &str) -> Option<Self> {
        Group::ALL.iter().copied().find(|g| g.as_str() == raw)
    }
}

/// What actually answers a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Backing {
    /// A `Method` on the running server, over its socket.
    Api(&'static str),
    /// A method the bridge answers itself
    /// ([`crate::cli::bridge::BRIDGE_LOCAL_METHODS`]) — files on this machine,
    /// no server round trip and no protocol version.
    Local(&'static str),
    /// Answered inside this process; the name is for diagnostics only.
    InProcess(&'static str),
}

/// Maps validated tool arguments to the backing method's params, applying
/// whatever the profile's authority forces.
type Mapper = fn(&Profile, Map<String, Value>) -> Map<String, Value>;

pub(crate) struct Tool {
    pub(crate) name: &'static str,
    pub(crate) groups: &'static [Group],
    pub(crate) backing: Backing,
    pub(crate) description: &'static str,
    pub(crate) input_schema: Value,
    pub(crate) call: Mapper,
}

// ---------------------------------------------------------------------------
// Schema helpers. Hand-rolled: the schemas are small, and a tool's arguments
// are a different shape from its backing method's params often enough that
// deriving them from the API types would lie.
// ---------------------------------------------------------------------------

fn prop(kind: &str, description: &str) -> Value {
    json!({"type": kind, "description": description})
}

fn enum_prop(description: &str, values: &[&str]) -> Value {
    json!({"type": "string", "description": description, "enum": values})
}

fn schema(properties: &[(&str, Value)], required: &[&str]) -> Value {
    let mut props = Map::new();
    for (name, value) in properties {
        props.insert((*name).to_string(), value.clone());
    }
    let mut root = Map::new();
    root.insert("type".to_string(), Value::from("object"));
    root.insert("properties".to_string(), Value::Object(props));
    if !required.is_empty() {
        root.insert(
            "required".to_string(),
            Value::Array(required.iter().map(|name| Value::from(*name)).collect()),
        );
    }
    Value::Object(root)
}

// ---------------------------------------------------------------------------
// Argument mappers.
// ---------------------------------------------------------------------------

fn passthrough(_: &Profile, args: Map<String, Value>) -> Map<String, Value> {
    args
}

/// `agent.read` insists on a source; a tool call that does not say wants the
/// visible screen.
fn map_agent_read(_: &Profile, mut args: Map<String, Value>) -> Map<String, Value> {
    args.entry("source".to_string())
        .or_insert_with(|| Value::from("visible"));
    args
}

/// Without the `docket` group this is a capture tool and nothing else: it
/// lands in the inbox as `captured`, with no date and no repeat, for a person
/// to dispose of. `source` and `notes` survive — the overseer dedupes its own
/// captures by `source`, so dropping it would make it propose the same item
/// on every tick.
fn map_docket_add(profile: &Profile, mut args: Map<String, Value>) -> Map<String, Value> {
    if !profile.grants(Group::Docket) {
        args.insert("kind".to_string(), Value::from("captured"));
        args.insert("status".to_string(), Value::from("inbox"));
        args.remove("due");
        args.remove("repeat");
    }
    args
}

/// With `nudge` but not `send`, text waits for the agent's next idle moment
/// instead of interrupting whatever it is doing.
fn map_agent_send(profile: &Profile, mut args: Map<String, Value>) -> Map<String, Value> {
    if !profile.grants(Group::Send) {
        args.insert("queue".to_string(), Value::Bool(true));
    }
    args
}

/// Every other tool names a pane with `target`; `pane.mark_seen` calls the
/// same thing `pane_id`.
fn map_pane_mark_seen(_: &Profile, mut args: Map<String, Value>) -> Map<String, Value> {
    if let Some(target) = args.remove("target") {
        args.insert("pane_id".to_string(), target);
    }
    args
}

// ---------------------------------------------------------------------------
// The table.
// ---------------------------------------------------------------------------

const READ: &[Group] = &[Group::Read];
const CAPTURE: &[Group] = &[Group::DocketInbox, Group::Docket];
const DOCKET: &[Group] = &[Group::Docket];
const SEND: &[Group] = &[Group::Nudge, Group::Send];
const MARK: &[Group] = &[Group::Mark];
const SEEN: &[Group] = &[Group::Seen];
const PUSH: &[Group] = &[Group::Push];
const OVERSEER: &[Group] = &[Group::Overseer];
const TICK: &[Group] = &[Group::Tick];

fn table() -> Vec<Tool> {
    vec![
        Tool {
            name: "session_overview",
            groups: READ,
            backing: Backing::Api("session.overview"),
            description: "Everything the session looks like right now: every agent with its \
                           state, age, group, branch and context use, plus host vitals. The one \
                           call to make first.",
            input_schema: schema(&[], &[]),
            call: passthrough,
        },
        Tool {
            name: "session_snapshot",
            groups: READ,
            backing: Backing::Api("session.snapshot"),
            description: "The full structural snapshot: workspaces, tabs and panes as the server \
                           holds them. Use session_overview unless you need the tree.",
            input_schema: schema(&[], &[]),
            call: passthrough,
        },
        Tool {
            name: "agent_get",
            groups: READ,
            backing: Backing::Api("agent.get"),
            description: "One agent's identity, state and placement.",
            input_schema: schema(
                &[(
                    "target",
                    prop("string", "Agent name, label, pane id or terminal id."),
                )],
                &["target"],
            ),
            call: passthrough,
        },
        Tool {
            name: "agent_read",
            groups: READ,
            backing: Backing::Api("agent.read"),
            description: "Read an agent's screen. Expensive in context — reach for it only when \
                           a blocked agent's exact words matter; session_overview already says \
                           what state everything is in.",
            input_schema: schema(
                &[
                    (
                        "target",
                        prop("string", "Agent name, label, pane id or terminal id."),
                    ),
                    (
                        "source",
                        enum_prop(
                            "Which buffer to read; defaults to the visible screen.",
                            &["visible", "recent", "recent_unwrapped", "detection"],
                        ),
                    ),
                    ("lines", prop("integer", "How many lines to return.")),
                    (
                        "format",
                        enum_prop("Plain text (default) or raw ANSI.", &["text", "ansi"]),
                    ),
                ],
                &["target"],
            ),
            call: map_agent_read,
        },
        Tool {
            name: "docket_list",
            groups: READ,
            backing: Backing::Api("docket.list"),
            description: "The docket, overdue first. Omit status for everything; \
                           status=\"inbox\" is the proposal queue.",
            input_schema: schema(
                &[(
                    "status",
                    enum_prop(
                        "Only items in this status.",
                        &["inbox", "open", "done", "discarded"],
                    ),
                )],
                &[],
            ),
            call: passthrough,
        },
        Tool {
            name: "runtime_list",
            groups: READ,
            backing: Backing::Api("runtime.list"),
            description: "Which runtimes this server can name, and whether each can be launched \
                           or asked headlessly.",
            input_schema: schema(&[], &[]),
            call: passthrough,
        },
        Tool {
            name: "overseer_sample",
            groups: READ,
            backing: Backing::Api("overseer.sample"),
            description: "What the overseer last sensed and said: narrative, health findings, \
                           tick time, the headless runtime and the chat tail. Reads state; \
                           creates nothing.",
            input_schema: schema(
                &[(
                    "chat_turns",
                    prop("integer", "Trailing chat turns to include; 0 for none."),
                )],
                &[],
            ),
            call: passthrough,
        },
        Tool {
            name: "pane_todos",
            groups: READ,
            backing: Backing::Local("pane.todos"),
            description: "The todo list a claude pane is working through, folded out of its \
                           session transcript on this machine.",
            input_schema: schema(
                &[("target", prop("string", "Pane id, agent name or label."))],
                &["target"],
            ),
            call: passthrough,
        },
        Tool {
            name: "pane_transcript",
            groups: READ,
            backing: Backing::Local("pane.transcript"),
            description: "The tail of a claude pane's conversation, read from its transcript \
                           file rather than its screen.",
            input_schema: schema(
                &[
                    ("target", prop("string", "Pane id, agent name or label.")),
                    (
                        "limit",
                        prop("integer", "How many trailing turns to return."),
                    ),
                ],
                &["target"],
            ),
            call: passthrough,
        },
        Tool {
            name: "memory_search",
            groups: READ,
            backing: Backing::Local("memory.search"),
            description: "Search prior sessions' prompts and replies in the local history \
                           sidecar. Use it before asking a person something they may have \
                           already answered.",
            input_schema: schema(
                &[
                    (
                        "query",
                        prop("string", "Words to match; ANDed, matched as words."),
                    ),
                    (
                        "limit",
                        prop("integer", "How many hits; 20 by default, 100 at most."),
                    ),
                ],
                &["query"],
            ),
            call: passthrough,
        },
        Tool {
            name: "memory_show",
            groups: READ,
            backing: Backing::Local("memory.show"),
            description: "The shared memory file's entries and how full it is: the user profile \
                           by default, or a repo's own memory when repo names its absolute path.",
            input_schema: schema(
                &[(
                    "repo",
                    prop(
                        "string",
                        "Absolute path inside a repo; omit for the user profile.",
                    ),
                )],
                &[],
            ),
            call: passthrough,
        },
        Tool {
            name: "doctor",
            groups: READ,
            backing: Backing::InProcess("doctor"),
            description: "One deterministic pass over the install: server, socket, launchd, \
                           bridge and token, hooks, push, leftover state, error log, disk and \
                           docket. Returns the same findings as `shep doctor --json`.",
            input_schema: schema(&[], &[]),
            call: passthrough,
        },
        Tool {
            name: "docket_add",
            groups: CAPTURE,
            backing: Backing::Api("docket.add"),
            description: "Add an item to the docket. With docket-inbox but not docket this is a \
                           capture tool: kind is forced to \"captured\", status to \"inbox\", and \
                           due and repeat are dropped — a person disposes of it. Always pass \
                           source when the item came from somewhere ({\"file\":…,\"line\":…} or \
                           {\"pane\":…}); captures are deduped by it.",
            input_schema: schema(
                &[
                    ("title", prop("string", "One line, imperative.")),
                    (
                        "source",
                        json!({
                            "type": "object",
                            "description": "Free-form provenance, e.g. {\"file\":…,\"line\":…}.",
                        }),
                    ),
                    (
                        "notes",
                        prop("string", "Anything that will not fit in the title."),
                    ),
                    (
                        "kind",
                        enum_prop(
                            "Ignored without the docket group.",
                            &["captured", "slated", "recurring"],
                        ),
                    ),
                    (
                        "status",
                        enum_prop(
                            "Ignored without the docket group.",
                            &["inbox", "open", "done", "discarded"],
                        ),
                    ),
                    ("due", prop("string", "YYYY-MM-DD; needs the docket group.")),
                    (
                        "repeat",
                        enum_prop("Needs the docket group.", &["1d", "1w", "2w", "1m"]),
                    ),
                ],
                &["title"],
            ),
            call: map_docket_add,
        },
        Tool {
            name: "docket_promote",
            groups: DOCKET,
            backing: Backing::Api("docket.promote"),
            description: "Move an inbox item into the docket proper as slated or recurring.",
            input_schema: schema(
                &[
                    ("id", prop("integer", "Docket item id.")),
                    (
                        "kind",
                        enum_prop("What it becomes.", &["slated", "recurring"]),
                    ),
                    ("due", prop("string", "YYYY-MM-DD.")),
                    ("repeat", enum_prop("Cadence.", &["1d", "1w", "2w", "1m"])),
                ],
                &["id", "kind"],
            ),
            call: passthrough,
        },
        Tool {
            name: "docket_complete",
            groups: DOCKET,
            backing: Backing::Api("docket.complete"),
            description: "Complete an open item; a recurring one rolls its due date forward.",
            input_schema: schema(&[("id", prop("integer", "Docket item id."))], &["id"]),
            call: passthrough,
        },
        Tool {
            name: "docket_discard",
            groups: DOCKET,
            backing: Backing::Api("docket.discard"),
            description: "Discard an inbox or open item.",
            input_schema: schema(&[("id", prop("integer", "Docket item id."))], &["id"]),
            call: passthrough,
        },
        Tool {
            name: "docket_update",
            groups: DOCKET,
            backing: Backing::Api("docket.update"),
            description: "Change an item's title, notes, due date, repeat or kind. Omitted \
                           fields are left alone.",
            input_schema: schema(
                &[
                    ("id", prop("integer", "Docket item id.")),
                    ("title", prop("string", "New title.")),
                    ("notes", prop("string", "New notes.")),
                    ("due", prop("string", "YYYY-MM-DD.")),
                    ("repeat", enum_prop("Cadence.", &["1d", "1w", "2w", "1m"])),
                    (
                        "kind",
                        enum_prop("New kind.", &["captured", "slated", "recurring"]),
                    ),
                ],
                &["id"],
            ),
            call: passthrough,
        },
        Tool {
            name: "agent_send",
            groups: SEND,
            backing: Backing::Api("agent.send"),
            description: "Put text to an agent. With nudge but not send, queue is forced true: \
                           the text waits for the agent's next idle moment instead of \
                           interrupting its turn.",
            input_schema: schema(
                &[
                    ("target", prop("string", "Agent name, label or pane id.")),
                    ("text", prop("string", "What to say.")),
                    (
                        "queue",
                        prop(
                            "boolean",
                            "Hold until idle; forced true without the send group.",
                        ),
                    ),
                ],
                &["target", "text"],
            ),
            call: map_agent_send,
        },
        Tool {
            name: "agent_set_state",
            groups: MARK,
            backing: Backing::Api("agent.set_state"),
            description: "Pin an agent's displayed state by hand — how it shows on every \
                           surface, never anything typed into it. Name exactly one of state or \
                           custom.",
            input_schema: schema(
                &[
                    ("target", prop("string", "Agent name, label or pane id.")),
                    (
                        "state",
                        enum_prop(
                            "A built-in state.",
                            &["idle", "working", "blocked", "unknown"],
                        ),
                    ),
                    ("custom", prop("string", "A [[states.custom]] name.")),
                ],
                &["target"],
            ),
            call: passthrough,
        },
        Tool {
            name: "agent_clear_state",
            groups: MARK,
            backing: Backing::Api("agent.clear_state"),
            description: "Drop a hand-pinned state and let detection speak again.",
            input_schema: schema(
                &[("target", prop("string", "Agent name, label or pane id."))],
                &["target"],
            ),
            call: passthrough,
        },
        Tool {
            name: "pane_mark_seen",
            groups: SEEN,
            backing: Backing::Api("pane.mark_seen"),
            description: "Clear a pane's unseen marker, as looking at it would.",
            input_schema: schema(&[("target", prop("string", "Pane id."))], &["target"]),
            call: map_pane_mark_seen,
        },
        Tool {
            name: "push_page",
            groups: PUSH,
            backing: Backing::Local("push.send"),
            description: "Page every registered device with one composed notification. Reports \
                           what was delivered, skipped and what failed, so a dead push setup is \
                           visible instead of silent.",
            input_schema: schema(
                &[
                    ("title", prop("string", "Notification title.")),
                    (
                        "message",
                        prop("string", "Body; truncated at 400 characters."),
                    ),
                    (
                        "kind",
                        prop("string", "Notification kind, for per-device filtering."),
                    ),
                    (
                        "state",
                        prop("string", "Agent state this is about, when it is one."),
                    ),
                    (
                        "agent",
                        prop("string", "Who is talking; \"shep\" when omitted."),
                    ),
                    ("workspace", prop("string", "Group label to show.")),
                    ("pane_id", prop("string", "Pane the phone should open.")),
                ],
                &["title", "message"],
            ),
            call: passthrough,
        },
        Tool {
            name: "overseer_chat",
            groups: OVERSEER,
            backing: Backing::Api("overseer.chat"),
            description: "Put one question to the overseer's own headless runtime. Deliberately \
                           outside the overseer profile: the overseer calling this would resume \
                           its own conversation from inside itself.",
            input_schema: schema(&[("text", prop("string", "The question."))], &["text"]),
            call: passthrough,
        },
        Tool {
            name: "overseer_tick",
            groups: TICK,
            backing: Backing::Api("overseer.tick"),
            description: "Ask the overseer plugin for a fresh situation when the last one has \
                           aged out. Deliberately outside the overseer profile: a tick calling \
                           this would nest ticks inside itself.",
            input_schema: schema(
                &[(
                    "max_age_seconds",
                    prop(
                        "integer",
                        "Skip while the situation is younger than this; 0 forces.",
                    ),
                )],
                &[],
            ),
            call: passthrough,
        },
    ]
}

/// Every tool shep knows how to serve, regardless of profile.
pub(crate) fn tools() -> &'static [Tool] {
    static TOOLS: OnceLock<Vec<Tool>> = OnceLock::new();
    TOOLS.get_or_init(table)
}

pub(crate) fn tool(name: &str) -> Option<&'static Tool> {
    tools().iter().find(|tool| tool.name == name)
}

// ---------------------------------------------------------------------------
// Profiles.
// ---------------------------------------------------------------------------

/// The set of tools one `shep mcp` process will serve.
///
/// Two fields, and they mean different things: `groups` are grants held
/// wholesale (a tool added to `read` later is served without anyone editing a
/// config), `tools` are individual names allowed on top. [`deny`](Self::deny)
/// keeps that honest by *demoting* — denying one tool of a granted group drops
/// the group and re-adds its other tools by name, so [`grants`](Self::grants),
/// which decides what `docket_add` and `agent_send` are allowed to do, never
/// claims an authority the profile no longer has in full.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct Profile {
    groups: BTreeSet<Group>,
    tools: BTreeSet<String>,
}

impl Profile {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn from_groups(groups: &[Group]) -> Self {
        Self {
            groups: groups.iter().copied().collect(),
            tools: BTreeSet::new(),
        }
    }

    /// The built-in profiles. `overseer` is stricter than the overseer's own
    /// charter allows in prose — no nudges at all, capture only — because the
    /// tool list is the rule now and the prose was advisory.
    pub(crate) fn named(name: &str) -> Option<Self> {
        Some(match name {
            "overseer" => Self::from_groups(&[Group::Read, Group::DocketInbox, Group::Push]),
            "read" => Self::from_groups(&[Group::Read]),
            "all" => Self::from_groups(Group::ALL),
            _ => return None,
        })
    }

    /// `[plugins.overseer] tools = [...]` — group or tool names, replacing the
    /// built-in `overseer` set entirely. `None` when the table says nothing,
    /// so the built-in stands.
    pub(crate) fn from_config(table: &toml::Table) -> Option<Result<Self, String>> {
        let value = table.get("tools")?;
        let Some(entries) = value.as_array() else {
            return Some(Err("[plugins.overseer] tools must be an array".to_string()));
        };
        let mut names = Vec::with_capacity(entries.len());
        for entry in entries {
            match entry.as_str() {
                Some(name) => names.push(name.to_string()),
                None => return Some(Err("[plugins.overseer] tools must be strings".to_string())),
            }
        }
        let mut profile = Self::empty();
        Some(profile.allow(&names).map(|()| profile))
    }

    /// Add groups or individual tools by name.
    pub(crate) fn allow(&mut self, names: &[String]) -> Result<(), String> {
        for name in names {
            match selector(name)? {
                Selector::Group(group) => {
                    self.groups.insert(group);
                }
                Selector::Tool(tool) => {
                    self.tools.insert(tool.to_string());
                }
            }
        }
        Ok(())
    }

    /// Remove groups or individual tools by name, demoting any group that
    /// would otherwise keep granting a denied tool.
    pub(crate) fn deny(&mut self, names: &[String]) -> Result<(), String> {
        for name in names {
            match selector(name)? {
                Selector::Group(group) => {
                    self.groups.remove(&group);
                    self.tools
                        .retain(|name| !tool(name).is_some_and(|t| t.groups.contains(&group)));
                }
                Selector::Tool(denied) => {
                    let groups = tool(denied).map(|t| t.groups).unwrap_or(&[]);
                    for group in groups {
                        if self.groups.remove(group) {
                            for other in tools() {
                                if other.name != denied && other.groups.contains(group) {
                                    self.tools.insert(other.name.to_string());
                                }
                            }
                        }
                    }
                    self.tools.remove(denied);
                }
            }
        }
        Ok(())
    }

    /// Whether this profile holds a group's authority in full. What the
    /// narrowing mappers ask.
    pub(crate) fn grants(&self, group: Group) -> bool {
        self.groups.contains(&group)
    }

    pub(crate) fn permits(&self, tool: &Tool) -> bool {
        tool.groups.iter().any(|group| self.groups.contains(group))
            || self.tools.contains(tool.name)
    }

    /// The tools this profile serves, in table order.
    pub(crate) fn permitted(&self) -> Vec<&'static Tool> {
        tools().iter().filter(|tool| self.permits(tool)).collect()
    }
}

enum Selector {
    Group(Group),
    Tool(&'static str),
}

fn selector(name: &str) -> Result<Selector, String> {
    let name = name.trim();
    if let Some(group) = Group::parse(name) {
        return Ok(Selector::Group(group));
    }
    if let Some(tool) = tool(name) {
        return Ok(Selector::Tool(tool.name));
    }
    Err(format!("unknown tool or group `{name}`"))
}

// ---------------------------------------------------------------------------
// Argument validation.
// ---------------------------------------------------------------------------

/// Check `arguments` against the tool's own schema, then map them to the
/// backing method's params.
///
/// Validation comes first and against the *tool's* schema so the refusal names
/// the argument the caller actually wrote; for API-backed tools the mapped
/// params are then deserialised as a real [`crate::api::schema::Request`], so a
/// bad enum or a missing field the schema does not model still comes back as
/// one `-32602` rather than reaching the server.
pub(crate) fn prepare(tool: &Tool, profile: &Profile, arguments: Value) -> Result<Value, String> {
    let args =
        validate(&tool.input_schema, arguments).map_err(|err| format!("{}: {err}", tool.name))?;
    let mapped = Value::Object((tool.call)(profile, args));
    if let Backing::Api(method) = tool.backing {
        let request = json!({"id": "mcp:check", "method": method, "params": mapped.clone()});
        serde_json::from_value::<crate::api::schema::Request>(request)
            .map_err(|err| format!("{}: {err}", tool.name))?;
    }
    Ok(mapped)
}

fn validate(schema: &Value, arguments: Value) -> Result<Map<String, Value>, String> {
    let args = match arguments {
        Value::Null => Map::new(),
        Value::Object(map) => map,
        _ => return Err("arguments must be an object".to_string()),
    };
    let properties = schema
        .get("properties")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();

    for required in schema
        .get("required")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .unwrap_or_default()
    {
        let Some(name) = required.as_str() else {
            continue;
        };
        if args.get(name).is_none_or(|value| value.is_null()) {
            return Err(format!("missing required argument `{name}`"));
        }
    }

    for (name, value) in &args {
        let Some(property) = properties.get(name) else {
            return Err(format!("unknown argument `{name}`"));
        };
        if value.is_null() {
            continue;
        }
        let Some(kind) = property.get("type").and_then(Value::as_str) else {
            continue;
        };
        if !type_matches(kind, value) {
            return Err(format!("argument `{name}` must be a {kind}"));
        }
    }

    Ok(args)
}

fn type_matches(kind: &str, value: &Value) -> bool {
    match kind {
        "string" => value.is_string(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        "boolean" => value.is_boolean(),
        "object" => value.is_object(),
        "array" => value.is_array(),
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(profile: &Profile) -> Vec<&'static str> {
        profile.permitted().iter().map(|tool| tool.name).collect()
    }

    fn list(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn mcp_tool_names_are_unique_and_schemas_are_objects() {
        let mut seen = BTreeSet::new();
        for tool in tools() {
            assert!(seen.insert(tool.name), "duplicate tool name {}", tool.name);
            assert_eq!(
                tool.input_schema.get("type").and_then(Value::as_str),
                Some("object"),
                "{} input_schema is not an object schema",
                tool.name
            );
            assert!(
                tool.input_schema
                    .get("properties")
                    .is_some_and(Value::is_object),
                "{} input_schema has no properties map",
                tool.name
            );
            assert!(!tool.groups.is_empty(), "{} belongs to no group", tool.name);
            assert!(
                !tool.description.is_empty(),
                "{} has no description",
                tool.name
            );
        }
    }

    #[test]
    fn every_tool_backs_onto_a_known_api_or_local_method() {
        for tool in tools() {
            match tool.backing {
                Backing::Api(method) => {
                    let request = json!({"id": "t", "method": method, "params": {}});
                    if let Err(err) = serde_json::from_value::<crate::api::schema::Request>(request)
                    {
                        assert!(
                            !err.to_string().contains("unknown variant"),
                            "{} backs onto unknown api method {method}: {err}",
                            tool.name
                        );
                    }
                }
                Backing::Local(method) => assert!(
                    crate::cli::bridge::BRIDGE_LOCAL_METHODS.contains(&method),
                    "{} backs onto {method}, which the bridge does not answer",
                    tool.name
                ),
                Backing::InProcess(_) => {}
            }
        }
    }

    #[test]
    fn mcp_overseer_profile_reads_captures_and_pages_and_nothing_else() {
        let profile = Profile::named("overseer").unwrap();
        let listed = names(&profile);
        assert!(listed.contains(&"session_overview"));
        assert!(listed.contains(&"docket_add"));
        assert!(listed.contains(&"push_page"));
        assert!(listed.contains(&"doctor"));
        assert!(!listed.contains(&"docket_promote"));
        assert!(!listed.contains(&"agent_send"));
        assert!(!listed.contains(&"agent_set_state"));
        assert!(!listed.contains(&"overseer_chat"));
        assert!(!listed.contains(&"overseer_tick"));
    }

    #[test]
    fn mcp_read_profile_refuses_every_writing_tool() {
        let profile = Profile::named("read").unwrap();
        for tool in tools() {
            let permitted = profile.permits(tool);
            assert_eq!(
                permitted,
                tool.groups.contains(&Group::Read),
                "{} permitted={permitted} under the read profile",
                tool.name
            );
        }
        assert!(Profile::named("nope").is_none());
    }

    #[test]
    fn mcp_all_profile_serves_the_whole_table() {
        assert_eq!(
            Profile::named("all").unwrap().permitted().len(),
            tools().len()
        );
    }

    #[test]
    fn mcp_docket_add_without_the_docket_group_captures_and_keeps_its_source() {
        let profile = Profile::named("overseer").unwrap();
        let tool = tool("docket_add").unwrap();
        let params = prepare(
            tool,
            &profile,
            json!({
                "title": "rotate the xai key",
                "due": "2030-01-01",
                "repeat": "1w",
                "kind": "slated",
                "status": "open",
                "notes": "billing wall clock",
                "source": {"file": "CHARTER.md", "line": 12},
            }),
        )
        .unwrap();
        assert_eq!(params["kind"], json!("captured"));
        assert_eq!(params["status"], json!("inbox"));
        assert!(params.get("due").is_none());
        assert!(params.get("repeat").is_none());
        assert_eq!(params["source"], json!({"file": "CHARTER.md", "line": 12}));
        assert_eq!(params["notes"], json!("billing wall clock"));
    }

    #[test]
    fn mcp_docket_add_with_the_docket_group_keeps_the_date() {
        let profile = Profile::named("all").unwrap();
        let params = prepare(
            tool("docket_add").unwrap(),
            &profile,
            json!({"title": "file the annual report", "kind": "slated", "due": "2030-01-01"}),
        )
        .unwrap();
        assert_eq!(params["kind"], json!("slated"));
        assert_eq!(params["due"], json!("2030-01-01"));
    }

    #[test]
    fn mcp_agent_send_queues_without_the_send_group() {
        let mut nudge = Profile::from_groups(&[Group::Nudge]);
        let tool = tool("agent_send").unwrap();
        let params = prepare(
            tool,
            &nudge,
            json!({"target": "pi", "text": "look at the docket", "queue": false}),
        )
        .unwrap();
        assert_eq!(params["queue"], json!(true));

        nudge.allow(&list(&["send"])).unwrap();
        let params = prepare(
            tool,
            &nudge,
            json!({"target": "pi", "text": "look at the docket", "queue": false}),
        )
        .unwrap();
        assert_eq!(params["queue"], json!(false));
    }

    #[test]
    fn mcp_pane_mark_seen_renames_target_to_the_api_field() {
        let params = prepare(
            tool("pane_mark_seen").unwrap(),
            &Profile::from_groups(&[Group::Seen]),
            json!({"target": "pane-3"}),
        )
        .unwrap();
        assert_eq!(params, json!({"pane_id": "pane-3"}));
    }

    #[test]
    fn mcp_agent_read_defaults_to_the_visible_screen() {
        let params = prepare(
            tool("agent_read").unwrap(),
            &Profile::named("read").unwrap(),
            json!({"target": "pi"}),
        )
        .unwrap();
        assert_eq!(params["source"], json!("visible"));
    }

    #[test]
    fn mcp_arguments_are_checked_against_the_tools_own_schema() {
        let profile = Profile::named("all").unwrap();
        let tool = tool("docket_add").unwrap();
        let err = prepare(tool, &profile, json!({})).unwrap_err();
        assert!(err.contains("missing required argument `title`"), "{err}");

        let err = prepare(tool, &profile, json!({"title": 7})).unwrap_err();
        assert!(err.contains("`title` must be a string"), "{err}");

        let err = prepare(tool, &profile, json!({"title": "x", "urgent": true})).unwrap_err();
        assert!(err.contains("unknown argument `urgent`"), "{err}");

        // Past the hand-written check, the typed request still refuses.
        let err = prepare(tool, &profile, json!({"title": "x", "kind": "urgent"})).unwrap_err();
        assert!(err.contains("docket_add"), "{err}");
    }

    #[test]
    fn mcp_config_tools_replace_the_default_set_and_flags_apply_after() {
        let table: toml::Table = "runtime = \"claude\"\ntools = [\"read\", \"docket_add\"]\n"
            .parse()
            .unwrap();
        let mut profile = Profile::from_config(&table).unwrap().unwrap();
        let listed = names(&profile);
        assert!(listed.contains(&"session_overview"));
        assert!(listed.contains(&"docket_add"));
        assert!(
            !listed.contains(&"push_page"),
            "config replaced the default set"
        );

        profile.allow(&list(&["push"])).unwrap();
        assert!(names(&profile).contains(&"push_page"));

        profile.deny(&list(&["memory_search"])).unwrap();
        let listed = names(&profile);
        assert!(!listed.contains(&"memory_search"));
        assert!(
            listed.contains(&"session_overview"),
            "denying one tool kept the rest"
        );

        let empty: toml::Table = "runtime = \"claude\"\n".parse().unwrap();
        assert!(Profile::from_config(&empty).is_none());

        let wrong: toml::Table = "tools = 3\n".parse().unwrap();
        assert!(Profile::from_config(&wrong).unwrap().is_err());
    }

    #[test]
    fn mcp_denying_a_tool_demotes_the_group_that_granted_it() {
        let mut profile = Profile::named("all").unwrap();
        assert!(profile.grants(Group::Docket));
        profile.deny(&list(&["docket_promote"])).unwrap();
        assert!(!profile.grants(Group::Docket));
        let listed = names(&profile);
        assert!(!listed.contains(&"docket_promote"));
        assert!(listed.contains(&"docket_complete"));
        assert!(listed.contains(&"docket_add"));

        let mut profile = Profile::named("all").unwrap();
        profile.deny(&list(&["docket"])).unwrap();
        let listed = names(&profile);
        assert!(!listed.contains(&"docket_promote"));
        assert!(
            listed.contains(&"docket_add"),
            "docket-inbox still grants capture"
        );
    }

    #[test]
    fn mcp_unknown_allow_and_deny_names_are_refused() {
        let mut profile = Profile::empty();
        assert!(profile.allow(&list(&["docket_nope"])).is_err());
        assert!(profile.deny(&list(&["not-a-group"])).is_err());
    }
}
