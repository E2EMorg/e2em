import importlib.util
import json
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
spec = importlib.util.spec_from_file_location("install_macos_runtime", ROOT / "scripts/install_macos_runtime.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class MacInstall(unittest.TestCase):
    def test_agent_paths_permissions_and_grant_lifecycle(self):
        with tempfile.TemporaryDirectory() as directory, patch("builtins.print"):
            home = Path(directory) / "home with spaces"
            home.mkdir()
            source = home / "source"
            source.write_bytes(b"binary fixture")
            prefix = ["--home", str(home)]
            module.main([*prefix, "install", "--binary", str(source)])
            agent = home / "Library/LaunchAgents/org.e2em.runtime.plist"
            value = plistlib.loads(agent.read_bytes())
            self.assertEqual(value["ProgramArguments"][0], str(home / ".local/bin/e2emd"))
            self.assertEqual(value["ProgramArguments"][2], str(home / "Library/Caches/e2em/runtime.sock"))
            self.assertEqual(value["ProgramArguments"][-1], "300")
            self.assertNotIn("StartInterval", value)
            self.assertEqual(agent.stat().st_mode & 0o777, 0o600)
            config = home / ".config/e2em"
            runtime = home / "Library/Caches/e2em"
            self.assertEqual(runtime.stat().st_mode & 0o777, 0o700)
            with self.assertRaises(ValueError):
                module.main([*prefix, "install", "--binary", str(source)])
            module.main([*prefix, "enrol", "reference"])
            credential = config / "apps/reference.json"
            first = json.loads(credential.read_text(encoding="utf-8"))
            module.main([*prefix, "enrol", "reference"])
            second = json.loads(credential.read_text(encoding="utf-8"))
            self.assertNotEqual(first["secret"], second["secret"])
            self.assertEqual(first["socket_path"], value["ProgramArguments"][2])
            self.assertEqual(credential.stat().st_mode & 0o777, 0o600)
            with self.assertRaises(ValueError):
                module.main([*prefix, "enrol", "../escape"])
            module.main([*prefix, "revoke", "reference"])
            self.assertFalse(credential.exists())
            self.assertEqual(json.loads((config / "grants.json").read_text(encoding="utf-8"))["grants"], [])
            (runtime / "owner-note").write_text("keep")
            (runtime / "runtime.sock").write_text("still present")
            with self.assertRaises(ValueError):
                module.main([*prefix, "uninstall"])
            self.assertTrue(agent.exists())
            (runtime / "runtime.sock").unlink()
            module.main([*prefix, "uninstall"])
            self.assertFalse(agent.exists())
            self.assertFalse(config.exists())
            self.assertTrue(source.exists())
            self.assertEqual((runtime / "owner-note").read_text(encoding="utf-8"), "keep")

    def test_long_socket_path_is_rejected_before_installing_files(self):
        with tempfile.TemporaryDirectory() as directory:
            home = Path(directory) / ("x" * 100)
            with self.assertRaises(ValueError):
                module.main(["--home", str(home), "install", "--binary", __file__])
            self.assertFalse(home.exists())


if __name__ == "__main__":
    unittest.main()
