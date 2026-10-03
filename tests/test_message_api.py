"""Message-first defaults, optional context, and installer-managed app settings."""
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python"))
from e2em import E2EMError, _connection
from e2em.message import PRESETS, request


class MessageDefaults(unittest.TestCase):
    def test_default_request_includes_the_entire_catalogue_without_context(self):
        draft = request("Hello")
        self.assertEqual([rule["category"] for rule in draft["policy"]["rules"]], list(PRESETS))
        self.assertTrue(all(rule["context_requirement"] == ("supplied_window" if rule["category"] == "spam.repeat" else "target_only") for rule in draft["policy"]["rules"]))
        self.assertEqual(draft["context"], [])
        self.assertNotEqual(draft["request_id"], request("Hello")["request_id"])

    def test_optional_context_is_preserved_and_custom_text_adds_to_defaults(self):
        draft = request("🙂 Original draft", context=["Prior message", "Reply"], custom_policies="Keep project details private.")
        self.assertEqual(draft["message"]["text"], "🙂 Original draft")
        self.assertEqual([turn["text"] for turn in draft["context"]], ["Prior message", "Reply"])
        self.assertEqual(len(draft["policy"]["rules"]), len(PRESETS) + 1)
        self.assertEqual(draft["policy"]["rules"][-1]["policy_text"], "Keep project details private.")
        self.assertTrue(all(rule["context_requirement"] == "supplied_window" for rule in draft["policy"]["rules"] if rule["match"] != "detected"))

    def test_selection_and_custom_only_mode_do_not_require_files(self):
        self.assertEqual(request("Hello", policies="identity.hate")["policy"]["rules"][0]["category"], "identity.hate")
        draft = request("Hello", policies=[], context="Earlier message", custom_policies=["Keep project details private."])
        self.assertEqual(len(draft["policy"]["rules"]), 1)
        self.assertEqual(draft["context"][0]["text"], "Earlier message")
        self.assertEqual(draft["policy"]["rules"][0]["match"], "policy_text")

    def test_sdk_catalogues_match_the_runtime_source(self):
        source = json.loads((ROOT / "src/runtime/presets.json").read_text(encoding="utf-8"))
        for path in [ROOT / "sdk/python/e2em/presets.json", ROOT / "sdk/node/presets.json"]:
            self.assertEqual(json.loads(path.read_text(encoding="utf-8")), source)


@unittest.skipUnless(os.name == "posix", "Unix ownership and mode checks")
class LocalConnectionSettings(unittest.TestCase):
    def test_private_settings_are_loaded_and_broadened_or_symlinked_files_are_rejected(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "app.json"
            config = dict(socket_path="/local/runtime.sock", principal="my-app", secret="a" * 64, provider="local")
            path.write_text(json.dumps(config)); path.chmod(0o600)
            self.assertEqual(_connection(config_path=path), config)
            path.chmod(0o644)
            with self.assertRaises(E2EMError): _connection(config_path=path)
            path.chmod(0o600)
            link = path.with_name("link.json"); link.symlink_to(path)
            with self.assertRaises(E2EMError): _connection(config_path=link)
            with self.assertRaises(E2EMError): _connection(app="../other-app", config_path=path)
            with self.assertRaises(E2EMError): _connection(app="different-app", config_path=path)
            path.write_text("[]")
            with self.assertRaises(E2EMError) as error: _connection(config_path=path)
            self.assertEqual(error.exception.code, "MODEL_UNAVAILABLE")


if __name__ == "__main__": unittest.main()
