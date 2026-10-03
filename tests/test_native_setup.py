"""Regressions exposed by native platform release qualification."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from check_macos_context import native_link_flags


class NativeLinkFlagsTest(unittest.TestCase):
    def test_colored_cargo_diagnostics_do_not_corrupt_library_names(self):
        stderr = "\x1b[1m\x1b[92mnote\x1b[0m: native-static-libs: -lSystem -liconv -lc -lm\x1b[0m\n"
        self.assertEqual(native_link_flags(stderr), ["-lSystem", "-liconv", "-lc", "-lm"])
        with self.assertRaises(RuntimeError):
            native_link_flags("compiler did not emit static libraries")


@unittest.skipUnless(os.name == "nt", "Windows PowerShell and native ACLs required")
class WindowsSetupTest(unittest.TestCase):
    def test_reenrolment_atomically_replaces_private_grants_without_a_backup(self):
        with tempfile.TemporaryDirectory(prefix="e2em-replace-") as directory:
            root = Path(directory)
            binary = root / "fixture.exe"
            binary.write_bytes(b"synthetic fixture, never executed")
            install = root / "user installation"
            command = ["powershell.exe", "-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass",
                       "-File", str(ROOT / "scripts/install_windows_runtime.ps1"),
                       "-InstallDirectory", str(install)]
            def manage(*args):
                result = subprocess.run([*command, *args], capture_output=True, text=True)
                self.assertEqual(result.returncode, 0, result.stderr)
            manage("-Action", "install", "-Binary", str(binary))
            try:
                manage("-Action", "enrol", "-Principal", "fixture")
                first = json.loads((install / "app-fixture.json").read_text())
                manage("-Action", "enrol", "-Principal", "fixture")
                second = json.loads((install / "app-fixture.json").read_text())
                self.assertNotEqual(first["secret"], second["secret"])
                registry = json.loads((install / "grants.json").read_text())
                self.assertEqual(len(registry["grants"]), 1)
                self.assertEqual(registry["grants"][0]["secret"], second["secret"])
                self.assertEqual({p.name for p in install.iterdir()},
                                 {"e2emd.exe", "grants.json", "installation.json", "app-fixture.json"})
            finally:
                manage("-Action", "uninstall")
