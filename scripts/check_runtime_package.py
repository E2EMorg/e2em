"""Install/upgrade/remove packages on disposable native CI hosts only."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import plistlib
import subprocess
import tempfile


def run(*args):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True)
    if result.returncode:
        print(result.stdout, result.stderr)
        result.check_returncode()
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--format', choices=['deb', 'rpm', 'pkg', 'msi'], required=True)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--upgrade-package', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--model-source', type=Path, help='local model fixture before release URLs become public')
    args = parser.parse_args()
    expected = {'deb': 'Linux', 'rpm': 'Linux', 'pkg': 'Darwin', 'msi': 'Windows'}[args.format]
    if platform.system() != expected:
        parser.error('native host required')
    package = args.package.resolve(strict=True)
    upgrade = args.upgrade_package.resolve(strict=True)
    initial_version = json.loads(Path(str(package) + '.json').read_text(encoding="utf-8"))['version']
    offline = json.loads(Path(str(package) + '.json').read_text(encoding="utf-8"))['model_included']
    upgrade_version = json.loads(Path(str(upgrade) + '.json').read_text(encoding="utf-8"))['version']
    if tuple(map(int, upgrade_version.split('.'))) <= tuple(map(int, initial_version.split('.'))):
        parser.error('upgrade fixture must have a newer package version')
    args.output.parent.mkdir(parents=True, exist_ok=True)
    if args.format == 'msi':
        payload = Path(os.environ['LOCALAPPDATA']) / 'E2EM Runtime'
        binary = payload / 'e2emd.exe'
    elif args.format == 'pkg':
        payload = Path('/usr/local/share/e2em')
        binary = Path('/usr/local/libexec/e2em/e2emd')
    else:
        payload = Path('/usr/share/e2em')
        binary = Path('/usr/bin/e2emd')
    if binary.exists() or payload.exists():
        parser.error('refusing to modify an existing runtime installation; use a disposable CI host')
    def install(path, logdir):
        if args.format == 'msi':
            result = subprocess.run(['msiexec.exe', '/i', str(path), '/qn', '/norestart', '/l*v', str(logdir / (path.name + '.log'))])
            if result.returncode not in (0, 3010):
                raise RuntimeError(f'MSI installation failed: {result.returncode}; inspect MSI log')
        elif args.format == 'pkg':
            run('sudo', 'installer', '-pkg', path, '-target', '/')
        elif args.format == 'deb':
            run('dpkg', '-i', path)
        else:
            run('rpm', '-Uvh', path)
    def remove(logdir):
        if args.format == 'msi':
            run('msiexec.exe', '/x', upgrade, '/qn', '/norestart', '/l*v', logdir / 'remove.log')
        elif args.format == 'pkg':
            # Exact managed payload only; no recursive deletion of shared roots.
            for path in [*payload.rglob('*'), *binary.parent.rglob('*')]:
                if path.is_dir(): continue
                run('sudo', 'rm', '-f', path)
            for path in sorted([p for p in payload.rglob('*') if p.is_dir()] + [p for p in binary.parent.rglob('*') if p.is_dir()], key=lambda p: len(p.parts), reverse=True):
                run('sudo', 'rmdir', path)
            run('sudo', 'rmdir', payload, binary.parent)
            # This app bundle is wholly owned by this package.
            bundle = Path('/Applications/E2EM Setup.app')
            for path in sorted(bundle.rglob('*'), key=lambda p: len(p.parts), reverse=True):
                run('sudo', 'rmdir' if path.is_dir() else 'rm', *([] if path.is_dir() else ['-f']), path)
            run('sudo', 'rmdir', bundle)
            run('sudo', 'pkgutil', '--forget', 'org.e2em.runtime')
        elif args.format == 'deb':
            run('dpkg', '-r', 'e2em-runtime')
        else:
            run('rpm', '-e', 'e2em-runtime')
    def installed_version():
        if args.format == 'msi':
            return run('powershell.exe', '-NoProfile', '-Command',
                       '(Get-ItemProperty -LiteralPath "HKCU:\\Software\\E2EM\\Runtime").Version').stdout.strip()
        if args.format == 'pkg':
            return plistlib.loads(run('pkgutil', '--pkg-info-plist', 'org.e2em.runtime').stdout.encode())['pkg-version']
        if args.format == 'deb':
            return run('dpkg-query', '-W', '-f=${Version}', 'e2em-runtime').stdout.strip()
        return run('rpm', '-q', '--qf', '%{VERSION}', 'e2em-runtime').stdout.strip()
    with tempfile.TemporaryDirectory(prefix='e2em-package-check-', dir='/tmp' if args.format == 'pkg' else None) as temporary:
        working = Path(temporary)
        home = working / 'user'
        home.mkdir(mode=0o700)
        install(package, args.output.parent)
        run(binary, '--help')
        assert installed_version() == initial_version, 'initial package version mismatch'
        if offline:
            backend = binary.parent if args.format in ('pkg', 'msi') else Path('/usr/libexec/e2em')
            run('python' if args.format == 'msi' else 'python3', Path(__file__).parent / 'check_gandalf_runtime.py',
                '--binary', binary, '--worker', backend / ('e2em-inference.exe' if args.format == 'msi' else 'e2em-inference'),
                '--library', backend / ('onnxruntime.dll' if args.format == 'msi' else 'libonnxruntime.dylib' if args.format == 'pkg' else 'libonnxruntime.so'),
                '--package', payload / 'models/gandalf', '--bootstrap', '--output', working / 'migration.json')
        if args.format == 'msi':
            setup = payload / 'install_windows_runtime.ps1'
            user_config = home / 'E2EM'
            run('powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', setup,
                '-Action', 'install', '-Binary', binary, '-UsePackagedBinary', '-InstallDirectory', user_config,
                *(['-ModelSource', args.model_source.resolve()] if args.model_source else []))
            run('powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', setup,
                '-Action', 'enrol', '-Principal', 'package-check', '-InstallDirectory', user_config)
            credential = user_config / 'app-package-check.json'
        else:
            setup = payload / ('install_macos_runtime.py' if args.format == 'pkg' else 'install_runtime.py')
            run('python3', setup, '--home', home, 'install', '--binary', binary, '--use-packaged-binary',
                *(['--model-source', args.model_source.resolve()] if args.model_source else []))
            run('python3', setup, '--home', home, 'enrol', 'package-check')
            user_config = home / '.config/e2em'
            credential = user_config / 'apps/package-check.json'
        before = credential.read_bytes()
        grants = (user_config / 'grants.json').read_bytes()
        assert json.loads((user_config / 'installation.json').read_text(encoding="utf-8"))['packaged_binary'] == str(binary)
        model_status = json.loads(run(binary, '--grants', user_config / 'grants.json', '--model-status').stdout)
        assert model_status['default'] == 'gandalf' and model_status['models'][0]['identity'].startswith('gandalf@')
        install(upgrade, args.output.parent)
        run(binary, '--help')
        assert installed_version() == upgrade_version, 'package version did not advance'
        assert credential.read_bytes() == before, 'upgrade changed credentials'
        assert (user_config / 'grants.json').read_bytes() == grants, 'upgrade changed grants'
        # Config is outside every payload/receipt. Package removal preserves it.
        remove(args.output.parent)
        assert not binary.exists(), 'package removal left the executable'
        assert credential.read_bytes() == before, 'package removal erased credentials'
        assert (user_config / 'grants.json').read_bytes() == grants, 'package removal erased grants'
        # User teardown still succeeds with the executable/package already gone.
        if args.format == 'msi':
            local_setup = working / 'setup.ps1'
            # Use repository copy after MSI removes its setup tool.
            local_setup.write_bytes((Path(__file__).parent / 'install_windows_runtime.ps1').read_bytes())
            run('powershell.exe', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', local_setup,
                '-Action', 'uninstall', '-InstallDirectory', user_config)
        else:
            run('python3', Path(__file__).parent / setup.name, '--home', home, 'uninstall')
        assert not (user_config / 'installation.json').exists()
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps({'platform': platform.platform(), 'format': args.format,
        'os_release': platform.freedesktop_os_release() if expected == 'Linux' else {},
        'python_version': platform.python_version(), 'architecture': platform.machine(),
        'package_sha256': hashlib.sha256(package.read_bytes()).hexdigest(),
        'upgrade_sha256': hashlib.sha256(upgrade.read_bytes()).hexdigest(),
        'initial_version': initial_version, 'upgraded_version': upgrade_version,
        'install': True, 'upgrade': True, 'remove': True, 'credentials_preserved': True,
        'package_managed_binary': True, 'user_teardown': True, 'model_provisioned': True,
        'limits': 'Package lifecycle and model provisioning; inference is qualified separately.'}, indent=2) + '\n')
    print('Native package install/upgrade/remove and credential preservation passed')


if __name__ == '__main__':
    main()
