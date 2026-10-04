"""Run Dart desktop SDK conformance against a native rules-only runtime."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve()
    if os.name == 'nt' and binary.suffix != '.exe':
        binary = binary.with_suffix('.exe')
    with tempfile.TemporaryDirectory(prefix='e2em-dart-') as temporary:
        directory = Path(temporary)
        directory.chmod(0o700)
        if os.name == 'nt':
            sid = subprocess.check_output(['powershell.exe', '-NoProfile', '-NonInteractive',
                '-Command', '[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value'], text=True).strip()
            owner = {'sid': sid}
            endpoint = rf'\\.\pipe\e2em-dart-{os.getpid()}'
            transport = ['--pipe', endpoint]
        else:
            owner = {'uid': os.getuid()}
            endpoint = str(directory / 'runtime.sock')
            transport = ['--socket', endpoint]
        grants = directory / 'grants.json'
        settings = directory / 'daccord.json'
        grants.write_text(json.dumps({'provider': 'dart-test-provider', 'grants': [
            {'principal': 'daccord', 'secret': 'a' * 64, **owner}]}), encoding='utf-8')
        settings.write_text(json.dumps({'socket_path': endpoint, 'principal': 'daccord',
            'secret': 'a' * 64, 'provider': 'dart-test-provider'}), encoding='utf-8')
        for file in [grants, settings]:
            if os.name == 'nt':
                # Replace the DACL rather than adding one grant to a runner's
                # existing ACL, which may contain explicit administrator ACEs.
                subprocess.run(['powershell.exe', '-NoProfile', '-NonInteractive', '-Command', r'''
$ErrorActionPreference = 'Stop'
$env:PSModulePath = Join-Path $PSHOME 'Modules'
Import-Module (Join-Path $PSHOME 'Modules/Microsoft.PowerShell.Security/Microsoft.PowerShell.Security.psd1') -ErrorAction Stop
$sid = [Security.Principal.WindowsIdentity]::GetCurrent().User
$acl = New-Object Security.AccessControl.FileSecurity
$acl.SetOwner($sid)
$acl.SetAccessRuleProtection($true, $false)
foreach ($owner in @($sid, (New-Object Security.Principal.SecurityIdentifier('S-1-5-18')))) {
    $acl.AddAccessRule((New-Object Security.AccessControl.FileSystemAccessRule($owner, 'FullControl', 'Allow')))
}
Set-Acl -LiteralPath $env:E2EM_DART_PRIVATE_FILE -AclObject $acl
'''], check=True, capture_output=True, env={**os.environ, 'E2EM_DART_PRIVATE_FILE': str(file)})
            else:
                file.chmod(0o600)
        with (directory / 'runtime.log').open('wb') as log:
            process = subprocess.Popen([str(binary), '--rules-only', '--grants', str(grants), *transport],
                                       stdout=log, stderr=log)
            try:
                subprocess.run(['dart', 'test', 'test/live.dart'],
                               cwd=ROOT / 'sdk/dart', check=True, timeout=120,
                               env={**os.environ, 'E2EM_DART_CONFIG': str(settings)})
            except subprocess.CalledProcessError:
                log.flush()
                print((directory / 'runtime.log').read_text(encoding='utf-8', errors='replace'))
                raise
            finally:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait(timeout=10)


if __name__ == '__main__':
    main()
