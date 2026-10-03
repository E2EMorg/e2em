"""Native Gandalf provisioning, SDK selection, coverage and recovery qualification."""
import argparse
import asyncio
import hashlib
import json
import os
from pathlib import Path
import platform
import secrets
import subprocess
import sys
import tempfile
import time
import uuid

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / 'sdk/python'))
from e2em import Client, E2EMError


def run(*command):
    return subprocess.run([str(a) for a in command], check=True, capture_output=True, text=True)


async def assess(socket, registry, report):
    grant = registry['grants'][0]
    for _ in range(100):
        try:
            client = await Client.open(socket, grant['principal'], grant['secret'], registry['provider'])
            break
        except E2EMError:
            await asyncio.sleep(.1)
    else: raise RuntimeError('native daemon did not become ready')
    try:
        models = await client.models()
        assert models['default_model'] == 'gandalf'
        assert client.capability_manifest['model'].startswith('gandalf@')
        started = time.perf_counter()
        all_rules = await client.assess('Thank you for your help.', context=['Can you help me with this?'], custom_policies=['Keep launch dates private.'], deadline_ms=30000)
        assert all_rules.status == 'assessed', all_rules.value
        assert not all_rules.unevaluated, all_rules.unevaluated
        assert len(all_rules.scores) >= 38, all_rules.scores
        reference = report.pop('_reference', None)
        if reference:
            catalogue = {p['id']: p['wording'] for p in json.loads((ROOT / 'src/runtime/presets.json').read_text(encoding='utf-8'))['presets']}
            catalogue['custom-1'] = 'Keep launch dates private.'
            expected = json.loads(Path(reference).read_text(encoding='utf-8'))['reference_probabilities']
            errors = [abs(score - expected[catalogue[name]]) for name, score in all_rules.scores.items()]
            assert max(errors) < 2e-5, errors
            report['max_native_probability_error'] = max(errors)
            report['native_torch_parity'] = True
        report['all_default_policies_seconds'] = time.perf_counter() - started
        threat = await client.assess('I will kill you tonight.', policies=['abuse.threat'], deadline_ms=30000)
        assert threat.status == 'assessed' and threat.scores['abuse.threat'] > .95, threat.value
        # Owner registration via the SDK can reference a local package or URL.
        await client.install_model(report.pop('_package'), name='custom')
        custom = await client.assess('I will kill you tonight.', policies=['abuse.threat'], model='custom', deadline_ms=30000)
        assert custom.status == 'assessed' and abs(custom.scores['abuse.threat'] - threat.scores['abuse.threat']) < 1e-6, custom.value
        unknown = await client.assess('Thank you.', policies=['abuse.threat'], model='missing')
        assert unknown.status == 'error' and unknown.value['error_code'] == 'MODEL_UNAVAILABLE'
        recovered = await client.assess('Thank you.', policies=['abuse.threat'], deadline_ms=30000)
        assert recovered.status == 'assessed', recovered.value
        long = await client.assess('Earlier evidence. ' * 600 + 'Final evidence.', policies=['abuse.threat'], deadline_ms=30000)
        assert long.status == 'indeterminate' and not long.value['coverage']['target_complete']
        assert 'MODEL_TOKEN_LIMIT' in long.value['reason_codes']
        report.update(default_policy_scoring=True, custom_model_selection=True, unknown_model_recovery=True, token_limit_reported=True, model_identity=threat.value['versions']['model'])
    finally: await client.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--library', type=Path, required=True)
    parser.add_argument('--package', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--reference', type=Path)
    parser.add_argument('--bootstrap', action='store_true', help='exercise first start of an upgraded package with existing grants')
    args = parser.parse_args()
    package = args.package.resolve()
    report = {'platform': platform.platform(), 'architecture': platform.machine(), 'quality_evaluation': False, '_package': str(package)}
    if args.reference: report['_reference'] = str(args.reference.resolve())
    # A short path is needed for macOS's 104-byte Unix socket limit.
    with tempfile.TemporaryDirectory(prefix='e2em-model-', dir='/tmp' if os.name == 'posix' else None) as temporary:
        root = Path(temporary); root.chmod(0o700)
        if os.name == 'nt':
            sid = run('powershell.exe', '-NoProfile', '-Command', '[System.Security.Principal.WindowsIdentity]::GetCurrent().User.Value').stdout.strip()
            # Apply the same private DACL as the installer before writing grants.
            run('powershell.exe', '-NoProfile', '-Command', f"$p='{str(root).replace(chr(39), chr(39)*2)}'; $a=Get-Acl -LiteralPath $p; $a.SetAccessRuleProtection($true,$false); $a.SetOwner([System.Security.Principal.WindowsIdentity]::GetCurrent().User); foreach($r in @($a.Access)){{$a.RemoveAccessRuleAll($r)}}; foreach($s in @('{sid}','S-1-5-18')){{$a.AddAccessRule((New-Object System.Security.AccessControl.FileSystemAccessRule($s,'FullControl','ContainerInherit,ObjectInherit','None','Allow')))}}; Set-Acl -LiteralPath $p -AclObject $a")
            socket = r'\\.\pipe\e2em-' + uuid.uuid4().hex
            owner = {'sid': sid}
            option = '--pipe'
        else:
            socket = str(root / 'runtime.sock'); owner = {'uid': os.getuid()}; option = '--socket'
        registry = {'provider': 'model-check', 'grants': [{'principal': 'probe', 'secret': secrets.token_hex(32), 'model_management': True, **owner}]}
        grants = root / 'grants.json'; grants.write_text(json.dumps(registry)); grants.chmod(0o600)
        command = [args.binary.resolve(), '--grants', grants]
        if args.bootstrap:
            (root / 'installation.json').write_text(json.dumps({'version': 1, 'packaged_binary': str(args.binary.resolve())}), encoding='utf-8')
            report['upgrade_first_start_provisioned'] = True
        else:
            run(*command, '--model-init', '--model-worker', args.worker.resolve(), '--model-library', args.library.resolve())
            run(*command, '--offline', '--model-install', package, '--model-no-update')
        report['signed_offline_provisioning'] = True
        process = subprocess.Popen([str(a) for a in [*command, '--offline', option, socket, '--idle-seconds', '1']], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
        try:
            asyncio.run(assess(socket, registry, report))
        finally:
            process.terminate()
            try: process.wait(timeout=10)
            except subprocess.TimeoutExpired: process.kill(); process.wait()
        assert json.loads(grants.read_text(encoding='utf-8')) == registry, 'provisioning changed existing grants'
        # A descriptor tamper must never replace the active verified package.
        bad = root / 'bad'; bad.mkdir(); descriptor = json.loads((package / 'model.json').read_text(encoding="utf-8")); descriptor['manifest']['license'] = 'tampered'; (bad / 'model.json').write_text(json.dumps(descriptor))
        before = (root / 'models.json').read_bytes()
        failed = subprocess.run([str(a) for a in [*command, '--model-install', bad]], capture_output=True)
        assert failed.returncode != 0 and (root / 'models.json').read_bytes() == before
        report['tampered_descriptor_rejected'] = True
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    print('Native Gandalf, all defaults, custom model, recovery and token coverage passed')


if __name__ == '__main__': main()
