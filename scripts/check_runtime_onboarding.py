"""Exercise the installed guided setup on a disposable native desktop CI host."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import platform
import shutil
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from e2em import Client


def run(*args):
    result = subprocess.run([str(a) for a in args], capture_output=True, text=True, timeout=180)
    if result.returncode:
        raise RuntimeError(f'{args[0]} failed: {result.stderr}')
    return result.stdout


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--disposable-host', action='store_true', required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    system = platform.system()
    home = Path.home()
    config = Path(os.environ['LOCALAPPDATA']) / 'E2EM' if system == 'Windows' else home / '.config/e2em'
    unit = home / '.config/systemd/user/e2emd.service'
    agent = home / 'Library/LaunchAgents/org.e2em.runtime.plist'
    if config.exists() or (unit.exists() if system == 'Linux' else agent.exists() if system == 'Darwin' else False):
        parser.error('refusing to overwrite an existing user installation')
    os.environ['E2EM_NATIVE_DIAGNOSTICS'] = '1'
    processes = []
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))

    def screen():
        # Exercise Windows' installed GUI entry point, not only the daemon CLI.
        entry = binary.with_name('e2em-setup.exe') if system == 'Windows' else binary
        command = [str(entry), *([] if system == 'Windows' else ['--setup']), '--setup-no-browser']
        process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        processes.append(process)
        line = process.stdout.readline().strip()
        if not line.startswith('Open E2EM Setup: '):
            raise RuntimeError(f'Setup did not expose its screen: {line}')
        url = line.removeprefix('Open E2EM Setup: ')
        parsed = urllib.parse.urlsplit(url)
        assert parsed.hostname == '127.0.0.1'
        origin = f'{parsed.scheme}://{parsed.netloc}'
        def request(path, body=None, request_origin=None):
            headers = {'Origin': request_origin or origin, 'Content-Type': 'application/json'} if body is not None else {}
            request = urllib.request.Request(url + path, data=json.dumps(body).encode() if body is not None else None, headers=headers)
            with opener.open(request, timeout=10) as response:
                raw = response.read()
                return json.loads(raw) if response.headers.get_content_type() == 'application/json' else raw.decode()
        return process, request

    async def sdk_check(credential):
        async with await Client.connect(config_path=credential) as client:
            capabilities = await client.capabilities()
            assert capabilities['model'].startswith('gandalf@')
            assessment = await client.assess('Hello there!', policies=['abuse.threat'], deadline_ms=30000)
            assert assessment.status == 'assessed', assessment

    try:
        process, request = screen()
        assert request('status')['stage'] == 'welcome'
        assert 'E2EM Setup' in request('')
        request('close', {})
        assert process.wait(timeout=10) == 0
        # A provisioning failure must leave a resumable installation rather
        # than requiring deletion of private grants or completed downloads.
        failed = subprocess.run([str(binary), '--setup-headless', '--offline', '--setup-model-source', str(config / 'missing-model-package')], capture_output=True, text=True, timeout=30)
        assert failed.returncode != 0, 'invalid offline model unexpectedly installed'
        initial_grants = (config / 'grants.json').read_bytes()
        process, request = screen()
        assert request('status')['stage'] == 'stopped'
        try:
            request('start', {'offline': True, 'auto_update': False, 'start_at_login': True}, 'http://foreign.invalid')
            raise AssertionError('setup accepted a foreign origin')
        except urllib.error.HTTPError as error:
            assert error.code == 403
        request('start', {'offline': True, 'auto_update': False, 'start_at_login': True})
        deadline = time.monotonic() + 300
        stages = set()
        while time.monotonic() < deadline:
            state = request('status')
            stages.add(state['stage'])
            if state['stage'] == 'ready': break
            if state['stage'] == 'error': raise AssertionError(state['message'])
            time.sleep(.5)
        else: raise AssertionError('guided setup did not become ready')
        connected = request('enrol', {'principal': 'my-app'})
        # The internal setup grant and provider must survive the failed model
        # step and the successful retry unchanged.
        initial_registry = json.loads(initial_grants)
        current_registry = json.loads((config / 'grants.json').read_bytes())
        assert current_registry['provider'] == initial_registry['provider']
        assert initial_registry['grants'][0] in current_registry['grants']
        credential = Path(connected['credential_path'])
        before = credential.read_bytes()
        grants = (config / 'grants.json').read_bytes()
        request('enrol', {'principal': 'my-app'})
        assert credential.read_bytes() == before, 'reconnecting rotated credentials'
        asyncio.run(sdk_check(credential))
        request('close', {})
        assert process.wait(timeout=10) == 0
        assert json.loads(run(binary, '--setup-status'))['stage'] == 'ready', 'closing setup stopped the runtime'
        model = json.loads(run(binary, '--grants', config / 'grants.json', '--model-status'))
        assert all(not installed['auto_update'] for installed in model['models'])
        # Opening setup twice reuses the original screen rather than creating a
        # competing download, configuration writer or runtime instance.
        second, request = screen()
        assert request('status')['stage'] == 'ready'
        duplicate = run(binary, '--setup', '--setup-no-browser')
        assert 'already open' in duplicate
        # Preference changes must restart the existing runtime with its new
        # flags, including Windows processes whose argument names are quoted.
        for login in (False, True):
            preferences = {'offline': True, 'auto_update': False, 'start_at_login': login}
            request('start', preferences)
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                state = request('status')
                if state['stage'] == 'error': raise AssertionError(state['message'])
                if state['stage'] == 'ready' and state.get('preferences') == preferences: break
                time.sleep(.5)
            else: raise AssertionError('runtime did not restart with changed preferences')
            if system == 'Linux':
                enabled = subprocess.run(['systemctl', '--user', 'is-enabled', 'e2emd'], capture_output=True, text=True)
                assert (enabled.stdout.strip() == 'enabled') == login
            elif system == 'Darwin':
                assert agent.exists() == login, 'login preference did not update the LaunchAgent'
            else:
                value = run('powershell.exe', '-NoProfile', '-Command', r"$v = Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -ErrorAction SilentlyContinue; if ($v) { $v.'E2EM Runtime' }")
                assert ('start-runtime.ps1' in value) == login
        if system == 'Windows':
            count = run('powershell.exe', '-NoProfile', '-Command', r'''@(Get-CimInstance Win32_Process -Filter "Name='e2emd.exe'" | Where-Object { $_.CommandLine -match '(^|\s)"?--grants"?(\s|$)' }).Count''')
            assert count.strip() == '1', 'setup created a duplicate background runtime'
        request('close', {})
        assert second.wait(timeout=10) == 0
        run(binary, '--setup-headless', '--offline')
        assert credential.read_bytes() == before
        assert (config / 'grants.json').read_bytes() == grants
        if system == 'Linux':
            assert run('systemctl', '--user', 'is-enabled', 'e2emd').strip() == 'enabled'
        elif system == 'Darwin':
            run('launchctl', 'print', f'gui/{os.getuid()}/org.e2em.runtime')
        else:
            value = run('powershell.exe', '-NoProfile', '-Command', r"(Get-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run').'E2EM Runtime'")
            assert 'start-runtime.ps1' in value
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps({'platform': system, 'guided_setup': True, 'model_assessment': True,
            'autostart': True, 'preferences_restart': True, 'reopen': True, 'retry': True, 'credentials_preserved': True, 'foreign_origin_rejected': True,
            'setup_stages_observed': sorted(stages)}, indent=2) + '\n')
        print('Guided setup, verified model assessment, background startup, app enrolment and reopen passed')
    except Exception:
        if system == 'Darwin':
            diagnostic = subprocess.run(['launchctl', 'print', f'gui/{os.getuid()}/org.e2em.runtime'], capture_output=True, text=True)
            print(diagnostic.stdout, diagnostic.stderr, flush=True)
            processes_status = subprocess.run(['ps', '-axo', 'pid,ppid,command'], capture_output=True, text=True).stdout
            print('\n'.join(line for line in processes_status.splitlines() if 'e2em' in line), flush=True)
            log = config / 'startup-diagnostics.log'
            if log.exists(): print(log.read_text(encoding='utf-8', errors='replace')[-16384:], flush=True)
        raise
    finally:
        for process in processes:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
        if system == 'Linux':
            run('systemctl', '--user', 'disable', '--now', 'e2emd')
            unit.unlink(missing_ok=True)
            run('systemctl', '--user', 'daemon-reload')
        elif system == 'Darwin':
            subprocess.run(['launchctl', 'bootout', f'gui/{os.getuid()}/org.e2em.runtime'], capture_output=True)
            agent.unlink(missing_ok=True)
            runtime = home / 'Library/Caches/e2em'
            shutil.rmtree(runtime, ignore_errors=True)
        else:
            run('powershell.exe', '-NoProfile', '-Command', r"Remove-ItemProperty -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run' -Name 'E2EM Runtime' -ErrorAction SilentlyContinue; Get-Process -Name e2emd -ErrorAction SilentlyContinue | Stop-Process -Force")
        shutil.rmtree(config, ignore_errors=True)


if __name__ == '__main__':
    main()
