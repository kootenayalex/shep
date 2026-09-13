import tempfile
import unittest
from pathlib import Path

from scripts import agent_detection_manifest_check as check


def manifest(agent_id: str, version: str, contains: str = "ready") -> str:
    return f'''id = "{agent_id}"
version = "{version}"
min_engine_version = 1
updated_at = "2026-06-10T00:00:00Z"

[[rules]]
id = "idle"
state = "idle"
contains = ["{contains}"]
'''


def catalog(agent_id: str = "codex", path: str = "codex.toml") -> str:
    return f'''schema_version = 1

[[agents]]
id = "{agent_id}"
path = "{path}"
'''


class AgentDetectionManifestCheckTests(unittest.TestCase):
    def test_accepts_extractors_with_a_capture_group(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[[extractors]]
id = "context_remaining"
region = "whole_recent"
regex = '(?i)context[^\\n%]*?(\\d{1,3})\\s*%'
capture = 1
"""
            )
            manifests = check.load_manifest_dir(bundled, engine_version=1)
            self.assertEqual(len(manifests["codex"][1]["extractors"]), 1)

    def test_rejects_extractor_capture_outside_its_groups(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[[extractors]]
id = "context_remaining"
regex = '(\\d+)%'
capture = 2
"""
            )
            with self.assertRaises(check.CheckError):
                check.load_manifest_dir(bundled, engine_version=1)

    def test_accepts_launch_and_headless_sections(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[launch]
bin = "codex"
fallback_bins = ["codex-cli"]
version_args = ["--version"]
argv = ["codex"]
env = { CODEX_QUIET = "1" }

[headless]
argv = ["codex", "exec"]
prompt = "stdin"
output = "text"
"""
            )
            manifests = check.load_manifest_dir(bundled, engine_version=1)
            self.assertEqual(manifests["codex"][1]["launch"]["bin"], "codex")
            self.assertEqual(manifests["codex"][1]["headless"]["argv"], ["codex", "exec"])

    def test_accepts_session_and_mcp_recipe_args_on_both_sections(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[launch]
bin = "codex"
session_new_args = ["--session-id", "{session_id}"]
session_resume_args = ["--resume", "{session_id}"]
mcp_config_args = ["--mcp-config", "{mcp_config}"]

[headless]
argv = ["codex", "exec"]
session_new_args = ["--session-id", "{session_id}"]
session_resume_args = ["--resume", "{session_id}"]
mcp_config_args = ["--mcp-config", "{mcp_config}", "--allowedTools", "mcp__{mcp_server}"]
"""
            )
            manifests = check.load_manifest_dir(bundled, engine_version=1)
            launch = manifests["codex"][1]["launch"]
            headless = manifests["codex"][1]["headless"]
            self.assertEqual(launch["mcp_config_args"], ["--mcp-config", "{mcp_config}"])
            self.assertEqual(launch["session_new_args"], ["--session-id", "{session_id}"])
            self.assertEqual(headless["mcp_config_args"][-1], "mcp__{mcp_server}")

    def test_rejects_recipe_args_that_are_empty_or_lose_their_placeholder(self):
        bad = [
            '[launch]\nbin = "codex"\nmcp_config_args = []\n',
            '[launch]\nbin = "codex"\nmcp_config_args = ["--mcp-config", "/etc/shep.json"]\n',
            '[launch]\nbin = "codex"\nsession_new_args = ["--new"]\n',
            '[launch]\nbin = "codex"\nmcp_config_args = "--mcp-config"\n',
            '[headless]\nargv = ["codex"]\nmcp_config_args = []\n',
            '[headless]\nargv = ["codex"]\nsession_resume_args = ["--resume"]\n',
        ]
        for section in bad:
            with tempfile.TemporaryDirectory() as tmp:
                bundled = Path(tmp)
                (bundled / "codex.toml").write_text(
                    manifest("codex", "2026.06.10.1") + "\n" + section
                )
                with self.assertRaises(check.CheckError, msg=section):
                    check.load_manifest_dir(bundled, engine_version=1)

    def test_rejects_an_unknown_mcp_field_name(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[headless]
argv = ["codex"]
mcp_config_env = { SHEP_MCP = "{mcp_config}" }
"""
            )
            with self.assertRaisesRegex(check.CheckError, "unknown field"):
                check.load_manifest_dir(bundled, engine_version=1)

    def test_the_repo_publishes_the_same_bytes_it_bundles(self):
        """The website copy of a manifest at the bundled version is that file.

        The published manifest is what an installed shep hot-reloads, so a
        bundled change that never reached `website/agent-detection/` would ship
        one recipe in the binary and another over the wire.
        """
        bundled = check.load_manifest_dir(
            check.DEFAULT_BUNDLED_DIR, engine_version=check.read_engine_version(None)
        )
        for agent_id, (bundled_path, bundled_manifest) in bundled.items():
            website_path = check.DEFAULT_WEBSITE_DIR / bundled_path.name
            self.assertTrue(website_path.is_file(), f"{agent_id} is not published")
            website_manifest = check.load_toml(website_path)
            if website_manifest["version"] != bundled_manifest["version"]:
                continue
            self.assertEqual(
                website_path.read_text(encoding="utf-8"),
                bundled_path.read_text(encoding="utf-8"),
                f"{website_path} drifted from {bundled_path} at the same version",
            )

    def test_rejects_unknown_launch_field_and_bad_headless_prompt(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[launch]
bin = "codex"
binary = "codex"
"""
            )
            with self.assertRaises(check.CheckError):
                check.load_manifest_dir(bundled, engine_version=1)
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1")
                + """
[headless]
argv = ["codex", "exec"]
prompt = "file"
"""
            )
            with self.assertRaises(check.CheckError):
                check.load_manifest_dir(bundled, engine_version=1)

    def test_validates_bundled_and_matching_website_catalog(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundled = root / "bundled"
            website = root / "website"
            bundled.mkdir()
            website.mkdir()
            content = manifest("codex", "2026.06.10.1")
            (bundled / "codex.toml").write_text(content)
            (website / "codex.toml").write_text(content)
            (website / "index.toml").write_text(catalog())

            bundled_manifests = check.load_manifest_dir(bundled, engine_version=1)
            check.validate_catalog(website, bundled_manifests, engine_version=1)

    def test_rejects_website_version_lower_than_bundled(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundled = root / "bundled"
            website = root / "website"
            bundled.mkdir()
            website.mkdir()
            (bundled / "codex.toml").write_text(manifest("codex", "2026.06.10.2"))
            (website / "codex.toml").write_text(manifest("codex", "2026.06.10.1"))
            (website / "index.toml").write_text(catalog())

            bundled_manifests = check.load_manifest_dir(bundled, engine_version=1)
            with self.assertRaisesRegex(check.CheckError, "lower than bundled"):
                check.validate_catalog(website, bundled_manifests, engine_version=1)

    def test_rejects_same_version_content_drift(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundled = root / "bundled"
            website = root / "website"
            bundled.mkdir()
            website.mkdir()
            (bundled / "codex.toml").write_text(manifest("codex", "2026.06.10.1", "ready"))
            (website / "codex.toml").write_text(manifest("codex", "2026.06.10.1", "changed"))
            (website / "index.toml").write_text(catalog())

            bundled_manifests = check.load_manifest_dir(bundled, engine_version=1)
            with self.assertRaisesRegex(check.CheckError, "same version"):
                check.validate_catalog(website, bundled_manifests, engine_version=1)

    def test_rejects_unknown_catalog_agent(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            bundled = root / "bundled"
            website = root / "website"
            bundled.mkdir()
            website.mkdir()
            (bundled / "codex.toml").write_text(manifest("codex", "2026.06.10.1"))
            (website / "newagent.toml").write_text(manifest("newagent", "2026.06.10.1"))
            (website / "index.toml").write_text(catalog("newagent", "newagent.toml"))

            bundled_manifests = check.load_manifest_dir(bundled, engine_version=1)
            with self.assertRaisesRegex(check.CheckError, "unknown agent"):
                check.validate_catalog(website, bundled_manifests, engine_version=1)

    def test_rejects_manifest_requiring_newer_engine(self):
        with tempfile.TemporaryDirectory() as tmp:
            bundled = Path(tmp) / "bundled"
            bundled.mkdir()
            (bundled / "codex.toml").write_text(
                manifest("codex", "2026.06.10.1").replace(
                    "min_engine_version = 1", "min_engine_version = 2"
                )
            )

            with self.assertRaisesRegex(check.CheckError, "exceeds engine"):
                check.load_manifest_dir(bundled, engine_version=1)


if __name__ == "__main__":
    unittest.main()
