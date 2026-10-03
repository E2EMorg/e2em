"""Update assets match host targets and managed setup enables idle updates."""
import os
from pathlib import Path
import plistlib
import sys
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
from package_update import package, UPDATE_TARGETS
from test_runtime_packaging import executable
import install_runtime
import install_macos_runtime


class UpdatePackaging(unittest.TestCase):
    def test_raw_payloads_are_target_checked_and_cannot_be_overwritten(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            source = root / 'source'
            for target in UPDATE_TARGETS:
                executable(source, target)
                output = package(source, target, root / 'out', '1.2.3')
                self.assertEqual(output.read_bytes(), source.read_bytes())
                self.assertEqual(output.suffix, '.exe' if 'windows' in target else '.bin')
                with self.assertRaises(FileExistsError):
                    package(source, target, root / 'out', '1.2.3')
            with self.assertRaises(ValueError):
                package(source, 'x86_64-unknown-linux-musl', root / 'out', '1.2.4')

    @unittest.skipUnless(os.name == 'posix', 'Unix setup')
    def test_setup_defaults_opt_out_and_removal_preserve_owner_files(self):
        with tempfile.TemporaryDirectory(prefix='e2em-up-', dir='/tmp') as directory:
            root = Path(directory)
            source = root / 'source'
            source.write_bytes(b'packaged binary')
            for platform, installer in [('linux', install_runtime), ('macos', install_macos_runtime)]:
                for enabled in [True, False]:
                    home = root / f'{platform}-{enabled}'
                    home.mkdir(mode=0o700)
                    prefix = ['--home', str(home)]
                    installer.main([*prefix, 'install', '--rules-only', '--binary', str(source), '--use-packaged-binary',
                                    *([] if enabled else ['--no-auto-update'])])
                    if platform == 'linux':
                        startup = (home / '.config/systemd/user/e2emd.service').read_text(encoding="utf-8")
                    else:
                        startup = plistlib.loads((home / 'Library/LaunchAgents/org.e2em.runtime.plist').read_bytes())['ProgramArguments']
                    self.assertEqual('--auto-update' in startup, enabled)
                    updates = home / '.config/e2em/updates'
                    updates.mkdir(mode=0o700)
                    for name in ['state.json', 'status.json', 'supervisor.lock', 'e2emd-0.2.0']:
                        (updates / name).write_text('managed')
                    (updates / 'owner-note').write_text('keep')
                    installer.main([*prefix, 'uninstall'])
                    self.assertEqual([p.name for p in updates.iterdir()], ['owner-note'])
                    self.assertEqual(source.read_bytes(), b'packaged binary')


if __name__ == '__main__':
    unittest.main()
