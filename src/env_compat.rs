//! Transition compatibility for the `herdr` -> `shep` environment-variable rename.
//!
//! Shep reads the modern `SHEP_*` variables but falls back to the legacy
//! `HERDR_*` names, so hook scripts and panes set up by an older `herdr` build
//! keep working during the transition. On the write side, shep sets BOTH the
//! `SHEP_*` variable and its legacy `HERDR_*` alias on child panes and hook
//! processes, so already-installed hook scripts that still read `HERDR_*`
//! continue to report state.
//!
//! Centralizing the rule here keeps the fallback/dual-write logic out of
//! individual call sites.

use std::ffi::{OsStr, OsString};

use portable_pty::CommandBuilder;

const CURRENT_PREFIX: &str = "SHEP_";
const LEGACY_PREFIX: &str = "HERDR_";

/// Returns the legacy `HERDR_*` alias for a `SHEP_*` variable name, if the name
/// carries the current prefix.
fn legacy_alias(name: &str) -> Option<String> {
    name.strip_prefix(CURRENT_PREFIX)
        .map(|rest| format!("{LEGACY_PREFIX}{rest}"))
}

/// Reads an environment variable as an [`OsString`], preferring the modern
/// `SHEP_*` name and falling back to the legacy `HERDR_*` alias. Empty values
/// are treated as unset.
pub(crate) fn var_os(name: &str) -> Option<OsString> {
    if let Some(value) = std::env::var_os(name).filter(|value| !value.is_empty()) {
        return Some(value);
    }
    legacy_alias(name).and_then(|alias| std::env::var_os(alias).filter(|value| !value.is_empty()))
}

/// String form of [`var_os`]: prefers `SHEP_*`, falls back to legacy `HERDR_*`.
pub(crate) fn var(name: &str) -> Option<String> {
    var_os(name).and_then(|value| value.into_string().ok())
}

/// Sets `name` and its legacy `HERDR_*` alias on a child command with the same
/// value, so hook scripts still reading the legacy names keep working during the
/// transition.
pub(crate) fn set_child_env(cmd: &mut CommandBuilder, name: &str, value: impl AsRef<OsStr>) {
    let value = value.as_ref();
    cmd.env(name, value);
    if let Some(alias) = legacy_alias(name) {
        cmd.env(alias, value);
    }
}

/// Removes `name` and its legacy `HERDR_*` alias from a child [`std::process::Command`],
/// so readers that fall back to the legacy names don't pick up a stale inherited value.
pub(crate) fn remove_std_child_env(cmd: &mut std::process::Command, name: &str) {
    cmd.env_remove(name);
    if let Some(alias) = legacy_alias(name) {
        cmd.env_remove(alias);
    }
}

/// Test support: removes `name` AND its legacy `HERDR_*` alias from the process
/// environment. Unit tests must scrub both prefixes because the dev machine runs
/// inside a herdr/shep session, so legacy variables leak into the test process
/// and [`var_os`]'s fallback would read them.
#[cfg(test)]
pub(crate) fn remove_process_env_for_test(name: &str) {
    std::env::remove_var(name);
    if let Some(alias) = legacy_alias(name) {
        std::env::remove_var(alias);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_alias_maps_shep_prefix_to_herdr() {
        assert_eq!(
            legacy_alias("SHEP_SOCKET_PATH").as_deref(),
            Some("HERDR_SOCKET_PATH")
        );
        assert_eq!(legacy_alias("SHEP_ENV").as_deref(), Some("HERDR_ENV"));
        assert_eq!(legacy_alias("PATH"), None);
    }

    #[test]
    fn var_prefers_current_then_falls_back_to_legacy() {
        let current = "SHEP_ENV_COMPAT_TEST_CURRENT";
        let legacy = "HERDR_ENV_COMPAT_TEST_CURRENT";
        std::env::remove_var(current);
        std::env::remove_var(legacy);

        // Neither set.
        assert_eq!(var(current), None);

        // Only legacy set -> fallback.
        std::env::set_var(legacy, "legacy");
        assert_eq!(var(current).as_deref(), Some("legacy"));

        // Current set -> preferred over legacy.
        std::env::set_var(current, "current");
        assert_eq!(var(current).as_deref(), Some("current"));

        std::env::remove_var(current);
        std::env::remove_var(legacy);
    }
}
