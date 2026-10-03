"""Release gates reject SDK version drift and incomplete distribution payloads."""
import json
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import unittest
import zipfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
from check_release import release_version, verify_assets, stage_assets
from package_c_sdk import package


class ReleaseTest(unittest.TestCase):
    def test_sdk_version_drift_prevents_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("Cargo.toml", "crates/e2em-ffi/Cargo.toml", "crates/e2em-platform/Cargo.toml",
                         "sdk/python/pyproject.toml", "sdk/node/package.json"):
                target = root / name
                target.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(ROOT / name, target)
            self.assertEqual(release_version(root), release_version(ROOT))
            target = root / "sdk/node/package.json"
            manifest = json.loads(target.read_text(encoding="utf-8"))
            manifest["version"] = "0.2.0"
            target.write_text(json.dumps(manifest))
            with self.assertRaisesRegex(ValueError, "versions differ"):
                release_version(root)

    def test_wrong_tag_and_missing_assets_prevent_publication(self):
        command = [sys.executable, str(ROOT / "scripts/check_release.py"), "--tag", "v999.0.0"]
        result = subprocess.run(command, capture_output=True, text=True)
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("tag must match", result.stderr)
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaisesRegex(ValueError, "missing or empty"):
                verify_assets(Path(directory), release_version())
            self.assertFalse((Path(directory) / "SHA256SUMS").exists())

    def test_native_upload_paths_flatten_without_overwriting_reports(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / "downloads"
            (source / "linux/runtime").mkdir(parents=True)
            (source / "linux/reports").mkdir()
            (source / "linux/runtime/installer.deb").write_bytes(b"installer")
            (source / "linux/reports/deb.json").write_text("{}")
            stage_assets(source, root / "release")
            self.assertEqual({p.name for p in (root / "release").iterdir()}, {"installer.deb", "deb.json"})
            (source / "other").mkdir()
            (source / "other/deb.json").write_text("different report")
            with self.assertRaisesRegex(ValueError, "duplicate"):
                stage_assets(source, root / "rejected")
            self.assertFalse((root / "rejected").exists())

    def test_c_sdk_contains_libraries_headers_and_license_and_refuses_overwrite(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for name in ("libe2em_ffi.so", "libe2em_ffi.a"):
                (root / name).write_bytes(b"synthetic native library")
            archive = package("x86_64-unknown-linux-gnu", root, root / "out")
            with zipfile.ZipFile(archive) as bundle:
                self.assertIn("lib/libe2em_ffi.so", bundle.namelist())
                self.assertIn("include/e2em.h", bundle.namelist())
                self.assertIn(b"MIT License", bundle.read("LICENSE"))
            with self.assertRaises(FileExistsError):
                package("x86_64-unknown-linux-gnu", root, root / "out")
