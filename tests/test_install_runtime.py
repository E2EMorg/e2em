import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("install_runtime",ROOT / "scripts/install_runtime.py")
module = importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
class Install(unittest.TestCase):
    def test_install_enrol_rotate_revoke_and_uninstall_preserve_unmanaged_files(self):
        with tempfile.TemporaryDirectory() as directory:
            home=Path(directory); binary=home/"source";binary.write_bytes(b"executable fixture")
            prefix=["--home",str(home)]
            module.main([*prefix,"install","--binary",str(binary)])
            config=home/".config/e2em"
            self.assertEqual((config/"grants.json").stat().st_mode & 0o777,0o600)
            with self.assertRaises(ValueError):module.main([*prefix,"install","--binary",str(binary)])
            module.main([*prefix,"enrol","first"])
            original=json.loads((config/"apps/first.json").read_text())
            module.main([*prefix,"enrol","first"])
            replacement=json.loads((config/"apps/first.json").read_text())
            self.assertNotEqual(original["secret"],replacement["secret"])
            self.assertEqual(len(json.loads((config/"grants.json").read_text())["grants"]),1)
            with self.assertRaises(ValueError):module.main([*prefix,"enrol","../escape"])
            module.main([*prefix,"revoke","first"])
            self.assertFalse((config/"apps/first.json").exists())
            module.main([*prefix,"uninstall"])
            self.assertFalse(config.exists());self.assertTrue(binary.exists())
