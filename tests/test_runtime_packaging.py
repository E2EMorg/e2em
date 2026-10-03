"""Offline payload validation and package-owned/user-owned lifecycle regressions."""
import json
import os
from pathlib import Path
import plistlib
import struct
import sys
import tempfile
import unittest
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'scripts'))
import package_runtime as package
import install_runtime
import install_macos_runtime


def executable(path, target):
    if 'linux' in target:
        header = bytearray(120)
        header[:6] = b'\x7fELF\x02\x01'
        struct.pack_into('<H', header, 18, 62 if target.startswith('x86') else 183)
        struct.pack_into('<Q', header, 32, 64)
        struct.pack_into('<HH', header, 54, 56, 1)
        struct.pack_into('<I', header, 64, 1)
    elif 'darwin' in target:
        header = bytearray(32)
        header[:4] = b'\xcf\xfa\xed\xfe'
        struct.pack_into('<I', header, 4, 0x01000007 if target.startswith('x86') else 0x0100000c)
    else:
        header = bytearray(70)
        header[:2] = b'MZ'
        struct.pack_into('<I', header, 60, 64)
        header[64:] = b'PE\x00\x00\x64\x86'
    path.write_bytes(header)
    path.chmod(0o755)


class PackageTest(unittest.TestCase):
    def test_versions_reject_path_injection_and_msi_overflow(self):
        for value in ['../1.0.0', '1.0.0\nRequires: injected', '01.2.3', '1.0.0-rc1', '256.0.0', '1.256.0', '1.0.65536']:
            with self.subTest(value=value), self.assertRaises(ValueError):
                package.version_value(value)
        self.assertEqual(package.version_value('255.255.65535'), '255.255.65535')

    def test_binary_architecture_and_static_linux_are_enforced(self):
        with tempfile.TemporaryDirectory() as temporary:
            path = Path(temporary) / 'binary'
            for target in package.TARGETS:
                executable(path, target)
                package.verify_binary(path, target)
            executable(path, 'x86_64-unknown-linux-musl')
            with self.assertRaises(ValueError):
                package.verify_binary(path, 'aarch64-unknown-linux-musl')
            value = bytearray(path.read_bytes())
            struct.pack_into('<I', value, 64, 3)  # PT_INTERP: dynamic host dependency
            path.write_bytes(value)
            with self.assertRaises(ValueError):
                package.verify_binary(path, 'x86_64-unknown-linux-musl')

    def test_payloads_contain_only_runtime_setup_and_docs(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary)
            for platform in ['windows', 'macos', 'linux']:
                stage = root / platform
                binary = root / 'binary'; binary.write_bytes(b'built executable')
                package.stage_payload(binary, platform, stage)
                names = {p.name for p in stage.rglob('*') if p.is_file()}
                expected = {'PACKAGING.md', 'DESKTOP.md', 'LICENSE'}
                expected |= {'e2emd.exe', 'install_windows_runtime.ps1'} if platform == 'windows' else {'e2emd', 'install_runtime.py'}
                if platform == 'macos': expected.add('install_macos_runtime.py')
                self.assertEqual(names, expected)

    def test_wix_per_user_upgrade_and_escaped_sources(self):
        with tempfile.TemporaryDirectory(prefix='e2em & review ') as temporary:
            stage = Path(temporary)
            (stage / 'e2emd.exe').write_bytes(b'binary')
            document = ET.fromstring(package.wix_source('1.2.3', stage))
            ns = {'w': package.WIX_NS}
            product = document.find('w:Package', ns)
            self.assertEqual(product.get('Scope'), 'perUser')
            self.assertEqual(product.get('UpgradeCode'), package.UPGRADE_CODE)
            self.assertIsNotNone(product.find('w:MajorUpgrade', ns))
            self.assertEqual(document.find('.//w:File', ns).get('Source'), str(stage / 'e2emd.exe'))
            self.assertEqual(document.find('.//w:RegistryValue', ns).get('Root'), 'HKCU')
            guid = document.find('.//w:Component', ns).get('Guid')
            self.assertNotEqual(guid, '*')
            changed_version = ET.fromstring(package.wix_source('1.2.4', stage))
            self.assertEqual(changed_version.find('.//w:Component', ns).get('Guid'), guid)
            self.assertIsNone(document.find('.//w:CustomAction', ns))

    def test_stage_only_never_claims_package_or_checksum(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); binary = root / 'binary'
            executable(binary, 'x86_64-pc-windows-msvc')
            staged = package.build('msi', binary, 'x86_64-pc-windows-msvc', root / 'out', '1.2.3', True)
            self.assertTrue(staged.is_dir())
            self.assertFalse(list((root / 'out').glob('*.msi')))
            self.assertFalse(list((root / 'out').glob('*.sha256')))
            document = ET.parse(staged / 'runtime.wxs')
            for source in document.findall('.//{'+package.WIX_NS+'}File'):
                self.assertTrue(Path(source.get('Source')).is_file())
            with self.assertRaises(ValueError):
                package.build('msi', binary, 'x86_64-pc-windows-msvc', root / 'out', '1.2.3', True)

    def test_format_must_match_target(self):
        with self.assertRaises(ValueError):
            package.build('msi', Path('missing'), 'x86_64-unknown-linux-musl', Path('out'), '1.0.0')


@unittest.skipUnless(os.name == 'posix', 'Unix setup tools require POSIX ownership')
class PackageSetupTest(unittest.TestCase):
    def test_package_owned_binary_survives_user_setup_removal(self):
        # Mac paths are deliberately short enough for Darwin's socket limit.
        with tempfile.TemporaryDirectory(prefix='e2em-pkg-', dir='/tmp') as temporary:
            root = Path(temporary); binary = root / 'e2emd'; binary.write_bytes(b'package-owned'); binary.chmod(0o755)
            for name, installer in [('linux', install_runtime), ('macos', install_macos_runtime)]:
                home = root / name; home.mkdir(mode=0o700)
                installer.main(['--home', str(home), 'install', '--binary', str(binary), '--use-packaged-binary'])
                installer.main(['--home', str(home), 'enrol', 'app'])
                config = home / '.config/e2em'
                marker = json.loads((config / 'installation.json').read_text(encoding="utf-8"))
                self.assertEqual(marker['packaged_binary'], str(binary.resolve()))
                self.assertFalse((home / '.local/bin/e2emd').exists())
                if name == 'linux':
                    self.assertIn('"'+str(binary.resolve())+'"', (home / '.config/systemd/user/e2emd.service').read_text(encoding="utf-8"))
                else:
                    agent = plistlib.loads((home / 'Library/LaunchAgents/org.e2em.runtime.plist').read_bytes())
                    self.assertEqual(agent['ProgramArguments'][0], str(binary.resolve()))
                grants = (config / 'grants.json').read_bytes()
                # Package upgrade overwrites only its binary, not user state.
                binary.write_bytes(b'upgraded package')
                self.assertEqual((config / 'grants.json').read_bytes(), grants)
                installer.main(['--home', str(home), 'uninstall'])
                self.assertEqual(binary.read_bytes(), b'upgraded package')

    def test_invalid_systemd_package_path_leaves_no_partial_setup(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); home = root / 'user'; home.mkdir()
            binary = root / 'e2em%unsafe'; binary.write_bytes(b'binary')
            with self.assertRaises(ValueError):
                install_runtime.main(['--home', str(home), 'install', '--binary', str(binary), '--use-packaged-binary'])
            self.assertFalse((home / '.config/e2em').exists())


if __name__ == '__main__':
    unittest.main()
