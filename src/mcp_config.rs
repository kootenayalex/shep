//! The MCP client config shep hands to a runtime.
//!
//! One shape, written in one place, because two things need it: `shep mcp
//! config`, which prints or writes it for a person wiring an editor up by
//! hand, and (later) the runtime recipes, which point a headless or launched
//! agent at a profile-bounded server. `command` is this very executable, so a
//! config written by a dev build points at the dev build.

use std::io;
use std::path::Path;

use serde_json::{Map, Value};

/// The name the tools appear under in a client (`mcp__shep__session_overview`).
pub(crate) const MCP_SERVER_NAME: &str = "shep";

/// `{"mcpServers": {"shep": {"command": …, "args": ["mcp", "--profile", P]}}}`.
///
/// `--socket` is spliced in only when this process was itself pointed at an
/// explicit socket: a client started from a test rig or a named session must
/// reach the same server, and one started normally must keep resolving the
/// socket the way every other command does.
pub(crate) fn client_config(profile: &str) -> Value {
    let command = std::env::current_exe()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| "shep".to_string());
    let mut args = vec![
        Value::from("mcp"),
        Value::from("--profile"),
        Value::from(profile),
    ];
    if let Some(socket) = crate::env_compat::var(crate::api::SOCKET_PATH_ENV_VAR) {
        args.push(Value::from("--socket"));
        args.push(Value::from(socket));
    }

    let mut server = Map::new();
    server.insert("command".to_string(), Value::from(command));
    server.insert("args".to_string(), Value::Array(args));
    let mut servers = Map::new();
    servers.insert(MCP_SERVER_NAME.to_string(), Value::Object(server));
    let mut root = Map::new();
    root.insert("mcpServers".to_string(), Value::Object(servers));
    Value::Object(root)
}

/// Write [`client_config`] to `path`, creating its parent directory.
pub(crate) fn write_client_config(path: &Path, profile: &str) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)?;
        }
    }
    let text = serde_json::to_string_pretty(&client_config(profile))?;
    std::fs::write(path, format!("{text}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_client_config_names_the_profile_and_this_executable() {
        let value = client_config("overseer");
        let server = &value["mcpServers"]["shep"];
        assert!(server["command"].as_str().is_some_and(|c| !c.is_empty()));
        let args: Vec<&str> = server["args"]
            .as_array()
            .unwrap()
            .iter()
            .map(|arg| arg.as_str().unwrap())
            .collect();
        assert_eq!(&args[..3], &["mcp", "--profile", "overseer"]);
    }

    #[test]
    fn mcp_write_client_config_creates_the_parent_directory() {
        let dir = std::env::temp_dir().join(format!("shep-mcp-config-{}", std::process::id()));
        let path = dir.join("nested/shep.json");
        write_client_config(&path, "read").unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("\"--profile\""));
        assert!(text.contains("\"read\""));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
