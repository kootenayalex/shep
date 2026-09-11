use crate::api::schema::InstalledPluginInfo;

pub(super) fn plugin_config_dir(plugin_id: &str) -> std::path::PathBuf {
    crate::plugin_paths::plugin_config_dir(plugin_id)
}

pub(super) fn plugin_state_dir(plugin_id: &str) -> std::path::PathBuf {
    crate::plugin_paths::plugin_state_dir(plugin_id)
}

pub(super) fn ensure_plugin_user_dirs(plugin: &InstalledPluginInfo) -> std::io::Result<()> {
    crate::plugin_paths::ensure_plugin_user_dirs(&plugin.plugin_id)
}

/// The plugin's own `[plugins.<id>]` table from config.toml as one JSON
/// object (`{}` when unset), so a plugin reads its settings with a single
/// `json.loads` instead of locating and parsing shep's config itself.
pub(super) fn plugin_config_json(
    plugins_config: &crate::config::PluginsConfig,
    plugin_id: &str,
) -> String {
    plugins_config
        .get(plugin_id)
        .and_then(|table| serde_json::to_string(table).ok())
        .unwrap_or_else(|| "{}".to_string())
}

pub(super) fn plugin_path_env(plugin: &InstalledPluginInfo) -> Vec<(String, String)> {
    let config_dir = plugin_config_dir(&plugin.plugin_id);
    let state_dir = plugin_state_dir(&plugin.plugin_id);

    vec![
        ("SHEP_PLUGIN_ROOT".to_string(), plugin.plugin_root.clone()),
        (
            "SHEP_PLUGIN_CONFIG_DIR".to_string(),
            config_dir.display().to_string(),
        ),
        (
            "SHEP_PLUGIN_STATE_DIR".to_string(),
            state_dir.display().to_string(),
        ),
    ]
}

#[cfg(test)]
mod tests {
    use super::plugin_config_json;

    #[test]
    fn plugin_config_json_serialises_only_the_plugins_own_table() {
        let config: crate::config::PluginsConfig = toml::from_str(
            r#"
[overseer]
runtime = "claude"
quiet_hours = [22, 7]

[other]
preset = "wide"
"#,
        )
        .unwrap();
        assert_eq!(
            plugin_config_json(&config, "overseer"),
            r#"{"quiet_hours":[22,7],"runtime":"claude"}"#
        );
        assert_eq!(plugin_config_json(&config, "absent"), "{}");
    }
}
