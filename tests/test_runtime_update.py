"""Offline daemon supervision: activation, rollback, controls and shutdown."""
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = os.environ.get('E2EM_SERVICE_BIN')


@unittest.skipUnless(BINARY and os.name == 'posix', 'requires built Unix daemon')
class RuntimeUpdate(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='e2em-update-')
        self.root = Path(self.temporary.name)
        self.grants = self.root / 'grants.json'
        self.grants.write_text(json.dumps({'provider': 'update-test', 'grants': []}))
        self.grants.chmod(0o600)
        self.socket = self.root / 'runtime.sock'
        self.updates = self.root / 'updates'
        self.updates.mkdir(mode=0o700)
        self.write_json('status.json', {'next_check': int(time.time()) + 3600, 'channel': 'preview'})
        self.process = None
        self.log = (self.root / 'stderr').open('w+')
        self.original = self.grants.read_bytes()

    def tearDown(self):
        if self.process:
            if self.process.poll() is None:
                self.process.terminate()
            try:
                self.process.wait(timeout=15)
            except subprocess.TimeoutExpired:
                self.process.kill()
                self.process.wait(timeout=5)
        self.log.close()
        self.temporary.cleanup()

    def write_json(self, name, value):
        path = self.updates / name
        path.write_text(json.dumps(value))
        path.chmod(0o600)

    def wait_for(self, predicate, timeout=20):
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if predicate():
                return
            if self.process and self.process.poll() is not None:
                self.log.seek(0)
                self.fail('supervisor exited: ' + self.log.read())
            time.sleep(0.05)
        self.log.seek(0)
        self.fail('timed out: ' + self.log.read())

    def start(self, *extra):
        self.process = subprocess.Popen([BINARY, '--rules-only', '--socket', str(self.socket), '--grants', str(self.grants),
                                         '--auto-update', '--update-idle-seconds', '3600', *extra],
                                        stdout=subprocess.DEVNULL, stderr=self.log)

    def state(self):
        try:
            return json.loads((self.updates / 'state.json').read_text(encoding="utf-8"))
        except FileNotFoundError:
            return {}

    def candidate(self, version, data):
        path = self.updates / ('e2emd-' + version)
        path.write_bytes(data)
        path.chmod(0o700)
        return {'version': version, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()}

    def test_verified_candidate_activates_and_shutdown_removes_endpoint(self):
        version = subprocess.check_output([BINARY, '--version'], text=True).strip().split()[-1]
        candidate = self.candidate(version, Path(BINARY).read_bytes())
        self.write_json('state.json', {'pending': candidate})
        self.start()
        self.wait_for(lambda: self.state().get('active') == candidate)
        self.assertFalse(self.state()['trial'])
        self.assertIsNone(self.state()['pending'])
        self.assertEqual(self.grants.read_bytes(), self.original)
        self.assertTrue(self.socket.exists())
        self.process.terminate()
        self.assertEqual(self.process.wait(timeout=15), 0)
        self.assertFalse(self.socket.exists())

    def test_failed_startup_rolls_back_and_does_not_retry_bad_release(self):
        script = f'#!{sys.executable}\nimport sys\nif "--version" in sys.argv:\n print("e2emd 0.2.0")\nelse:\n sys.exit(3)\n'
        candidate = self.candidate('0.2.0', script.encode())
        self.write_json('state.json', {'pending': candidate})
        self.start()
        self.wait_for(lambda: self.state().get('rejected') == '0.2.0' and self.socket.exists())
        self.assertIsNone(self.state()['pending'])
        self.assertIsNone(self.state()['active'])
        self.assertEqual(self.grants.read_bytes(), self.original)

    def test_interrupted_trial_is_rejected_before_relaunch(self):
        candidate = self.candidate('0.2.0', b'not executable')
        self.write_json('state.json', {'pending': candidate, 'trial': True})
        self.start()
        self.wait_for(lambda: self.state().get('rejected') == '0.2.0' and self.socket.exists())
        self.assertFalse(self.state()['trial'])

    def test_corrupt_cached_payload_is_rejected_before_execution(self):
        candidate = self.candidate('0.2.0', b'corrupt binary')
        candidate['sha256'] = 'a' * 64
        self.write_json('state.json', {'pending': candidate})
        self.start()
        self.wait_for(lambda: self.state().get('rejected') == '0.2.0' and self.socket.exists())
        self.assertIsNone(self.state()['active'])

    def test_corrupt_update_metadata_keeps_installed_runtime_available(self):
        (self.updates / 'state.json').write_text('{broken')
        self.start()
        self.wait_for(self.socket.exists)
        self.assertEqual(self.grants.read_bytes(), self.original)
        self.assertIsNone(self.process.poll())

    def test_disabled_updates_leave_no_supervisor_and_ignore_staged_payload(self):
        candidate = self.candidate('0.2.0', b'invalid')
        self.write_json('state.json', {'pending': candidate})
        self.start('--no-auto-update')
        self.wait_for(self.socket.exists)
        self.assertFalse((self.updates / 'supervisor.lock').exists())
        self.assertEqual(self.state()['pending'], candidate)

    def test_status_check_request_and_orphan_cleanup(self):
        self.start()
        self.wait_for(self.socket.exists)
        command = [BINARY, '--grants', str(self.grants)]
        status = json.loads(subprocess.check_output([*command, '--update-status'], text=True))
        self.assertEqual(status['stage'], 'waiting_for_idle')
        self.process.send_signal(signal.SIGUSR1)
        subprocess.run([*command, '--update-check-now'], check=True, capture_output=True)
        self.wait_for(lambda: not (self.updates / 'check-request.json').exists())
        # SIGKILL skips normal supervisor cleanup; the child must notice its loss.
        self.process.kill()
        self.process.wait(timeout=5)
        deadline = time.monotonic() + 5
        while self.socket.exists() and time.monotonic() < deadline:
            time.sleep(0.05)
        self.assertFalse(self.socket.exists(), 'orphan daemon retained its endpoint')

    def test_interrupt_shutdown_reaches_supervisor_and_daemon(self):
        # Interrupt at the earliest externally visible point, rather than wait
        # for readiness. Repeated starts exercise the endpoint/handler race.
        for attempt in range(8):
            with self.subTest(attempt=attempt):
                self.start()
                self.wait_for(self.socket.exists)
                self.process.send_signal(signal.SIGINT)
                self.assertEqual(self.process.wait(timeout=15), 0)
                self.assertFalse(self.socket.exists())


if __name__ == '__main__':
    unittest.main()
