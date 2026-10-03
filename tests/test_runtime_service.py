"""Live service/SDK conformance, invoked by the Rust black-box test."""
import asyncio
import copy
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT / "sdk/python"))
from e2em import Client, E2EMError, utf16_span, _assessment
CASES = json.loads((ROOT / "tests/conformance/assessments.json").read_text(encoding="utf-8"))
POLICY_FAILURES = json.loads((ROOT / "tests/conformance/policy-failures.json").read_text(encoding="utf-8"))
POLICY_REPORTS = json.loads((ROOT / "tests/conformance/policy-reports.json").read_text(encoding="utf-8"))
POLICY = json.loads((ROOT / "tests/conformance/email-policy.json").read_text(encoding="utf-8"))

class Values(unittest.TestCase):
    def test_utf16_offsets_and_codepoint_boundaries(self):
        self.assertEqual(utf16_span("🙂 alex@example.test",5,22),(3,20))
        self.assertEqual(utf16_span("\ufeff🙂 alex@example.test",8,25),(4,21))
        for start,end in ((1,5),(-1,3),(0,99)):
            with self.assertRaises(E2EMError): utf16_span("🙂 x",start,end)
    def test_nullable_limits_do_not_apply_numeric_constraints_to_null(self):
        from e2em.schema import valid
        self.assertTrue(valid(None,{"type":["integer","null"],"minimum":0}))
        self.assertFalse(valid(-1,{"type":["integer","null"],"minimum":0}))
    def test_unserializable_request_has_typed_error(self):
        for value in (float("nan"), object(), "\ud800"):
            with self.assertRaises(E2EMError) as error: asyncio.run(Client()._send({"value":value}))
            self.assertEqual(error.exception.code,"INVALID_REQUEST")
    def test_generated_value_types_import_without_model_dependencies(self):
        import e2em.types
        self.assertNotIn("torch",sys.modules)
        self.assertNotIn("transformers",sys.modules)

@unittest.skipUnless(os.environ.get("E2EM_SERVICE_BIN"),"live service built by Rust integration test")
class Service(unittest.IsolatedAsyncioTestCase):
    async def asyncSetUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.directory = Path(self.tmp.name); self.directory.chmod(0o700)
        self.socket = self.directory / "runtime.sock"
        self.grants_path = self.directory / "grants.json"
        self.grants = {"provider":"test-provider","grants":[{"principal":p,"uid":os.getuid(),"secret":s*64} for p,s in (("python","a"),("node","b"))]}
        self.write_grants(); self.start(); self.clients = []; self.native_process = None
        await self.wait_ready(str(self.socket), "test-provider")
    def write_grants(self):
        self.grants_path.write_text(json.dumps(self.grants)); self.grants_path.chmod(0o600)
    def start(self):
        self.process = subprocess.Popen([os.environ["E2EM_SERVICE_BIN"],"--rules-only","--socket",str(self.socket),"--grants",str(self.grants_path),"--idle-seconds","1"],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
    async def connect(self,principal="python",secret=None,provider="test-provider"):
        client = await Client.open(str(self.socket),principal,secret or ("a" if principal == "python" else "b")*64,provider)
        self.clients.append(client); return client
    async def start_native(self):
        grants=self.directory / "native-grants.json"
        grants.write_text(json.dumps({"provider":"native-provider","grants":self.grants["grants"]}));grants.chmod(0o600)
        path=self.directory / "native.sock"
        self.native_process=subprocess.Popen([os.environ["E2EM_SERVICE_BIN"],"--rules-only","--socket",str(path),"--grants",str(grants)],stdout=subprocess.PIPE,stderr=subprocess.PIPE)
        await self.wait_ready(str(path), "native-provider")
        return str(path)
    async def wait_ready(self, path, provider):
        # Socket creation precedes chmod and accept. Authenticate before marking
        # a fixture ready, otherwise discovery can silently fall back during
        # that startup window. Capability calls do not warm the scorer.
        for _ in range(200):
            try:
                async with await Client.open(path, "python", "a"*64, provider):
                    return
            except (OSError, E2EMError, asyncio.TimeoutError):
                await asyncio.sleep(.01)
        self.fail("provider did not become authenticated and ready")
    async def asyncTearDown(self):
        for client in self.clients:
            await client.close()
        if self.process.poll() is None: self.process.send_signal(signal.SIGINT)
        stdout,stderr = await asyncio.to_thread(self.process.communicate,timeout=10)
        self.assertNotIn(b"alex@example", stdout+stderr)
        if self.native_process:
            self.native_process.send_signal(signal.SIGINT)
            await asyncio.to_thread(self.native_process.communicate,timeout=10)
        self.tmp.cleanup()
    async def test_common_fixtures_against_live_service(self):
        client = await self.connect()
        for case in CASES:
            result = await client.assess(case["request"])
            self.assertEqual(result.status,case["expected"]["status"])
            self.assertEqual(result.action,case["expected"]["action"])
            self.assertEqual([[s["start"],s["end"]] for f in result.value["findings"] for s in f["spans"]],case["expected"]["spans"])
            self.assertTrue(result.applies_to(case["request"]))
            edited = copy.deepcopy(case["request"]); edited["message"]["text"] += "edit"
            self.assertFalse(result.applies_to(edited))
        files = set(p.name for p in self.directory.iterdir())
        self.assertEqual(files,{"grants.json","runtime.sock"})
    async def test_shared_policy_failures(self):
        client = await self.connect()
        for case in POLICY_FAILURES:
            with self.assertRaises(E2EMError) as error: await client.validate_policy(case["policy"])
            self.assertEqual(error.exception.code,case["error_code"])
    async def test_all_named_categories_produce_reports(self):
        client = await self.connect()
        for case in POLICY_REPORTS:
            policy = copy.deepcopy(case["policy"]); policy["id"] = case["name"]
            reference = await client.validate_policy(policy)
            request = copy.deepcopy(CASES[0]["request"])
            request.pop("policy"); request["policy_ref"] = reference
            result = await client.assess(request)
            self.assertEqual(result.status, case["expected"]["status"])
            self.assertEqual(result.action, case["expected"]["action"])
            self.assertEqual(result.value["coverage"]["unevaluated_rules"], case["expected"]["unevaluated_rules"])
            self.assertEqual(result.value["reason_codes"], case["expected"]["reason_codes"])
            self.assertEqual(result.value["findings"], [])
    async def test_message_only_context_custom_text_and_automatic_connection(self):
        from e2em import assess
        from e2em.message import PRESETS
        settings = self.directory / "python-app.json"
        settings.write_text(json.dumps(dict(socket_path=str(self.socket), principal="python", secret="a"*64, provider="test-provider")))
        settings.chmod(0o600)
        client = await Client.connect("python", config_path=settings)
        self.clients.append(client)
        report = await client.assess("Hello")
        self.assertEqual(len(report.request["policy"]["rules"]), len(PRESETS))
        self.assertEqual(report.status, "indeterminate")
        self.assertEqual(set(report.unevaluated), {preset["id"] for preset in PRESETS.values() if "outgoing" in preset["directions"] and preset["id"] != "pii.email"})
        report = await client.assess("Contact alex@example.test", policies="pii.email", context="Earlier message")
        self.assertEqual(report.status, "assessed")
        self.assertEqual(report.action, "warn")
        current = report.request
        self.assertTrue(report.applies_to(current))
        current["context"][0]["text"] = "Edited context"
        self.assertFalse(report.applies_to(current))
        custom = await client.assess("Hello", policies=[], custom_policies="Keep project details private.", context=["Earlier message"])
        self.assertEqual(custom.status, "indeterminate")
        self.assertEqual(custom.unevaluated, ["custom-1"])
        self.assertEqual(custom.scores, {})
        self.assertNotIn("Keep project details private.", repr(custom))
        plain = await asyncio.to_thread(assess, "Hello", app="python", config_path=settings, policies=["pii.email"])
        self.assertEqual(plain.action, "allow")
        self.grants["grants"].append(dict(principal="my-app", uid=os.getuid(), secret="c"*64))
        self.write_grants()
        default_settings = self.directory / ".config/e2em/apps/my-app.json"
        default_settings.parent.mkdir(parents=True)
        default_settings.write_text(json.dumps(dict(socket_path=str(self.socket), principal="my-app", secret="c"*64, provider="test-provider")))
        default_settings.chmod(0o600)
        from unittest.mock import patch
        with patch("e2em.Path.home", return_value=self.directory):
            defaults = await asyncio.to_thread(assess, "Hello")
        self.assertEqual(len(defaults.request["policy"]["rules"]), len(PRESETS))
        self.assertEqual(defaults.status, "indeterminate")
    async def test_windows_pipe_adapter_preserves_provider_authentication(self):
        # Exercise the Windows client branch on the Unix fixture transport; this
        # validates adapter framing/authentication, not Windows ACLs or IOCP.
        from unittest.mock import patch
        from types import SimpleNamespace
        import e2em
        loop = asyncio.get_running_loop()
        calls = []
        async def connect_pipe(factory, path):
            calls.append(path)
            return await loop.create_unix_connection(factory, str(self.socket))
        with patch.object(e2em, "os", SimpleNamespace(name="nt")), patch.object(loop, "create_pipe_connection", connect_pipe, create=True):
            for path in (r"\\remote\pipe\e2em-app", r"\\.\pipe\other", r"\\.\pipe\e2em-", r"\\.\pipe\e2em-app/remote"):
                with self.assertRaises(E2EMError): await Client.open(path, "python", "a"*64, "test-provider")
            self.assertEqual(calls, [])
            async with await Client.open(r"\\.\pipe\e2em-app", "python", "a"*64, "test-provider") as client:
                self.assertEqual((await client.assess(CASES[0]["request"])).action, "warn")
            with self.assertRaises(E2EMError): await Client.open(r"\\.\pipe\e2em-app", "python", "a"*64, "imposter")

    async def test_wrong_peer_uid_cannot_enrol(self):
        self.grants["grants"][0]["uid"] += 1; self.write_grants()
        with self.assertRaises((OSError,asyncio.IncompleteReadError,E2EMError)): await self.connect()
    async def test_isolation_named_category_reporting_and_revocation(self):
        a = await self.connect(); b = await self.connect("node")
        ref = await a.validate_policy(POLICY)
        request = copy.deepcopy(CASES[0]["request"]); request.pop("policy"); request["policy_ref"] = ref
        self.assertEqual((await a.assess(request)).action,"warn")
        with self.assertRaises(E2EMError) as error: await b.assess(request)
        self.assertEqual(error.exception.code,"POLICY_NOT_FOUND")
        experimental = copy.deepcopy(POLICY); experimental["id"] = "experimental"
        experimental["rules"][0]["category"] = "abuse.threat"
        request["policy_ref"] = await a.validate_policy(experimental)
        report = await a.assess(request)
        self.assertEqual(report.status, "indeterminate")
        self.assertEqual(report.value["coverage"]["unevaluated_rules"], ["email-warning"])
        self.assertFalse(await b.cancel("another-principal-request"))
        self.grants["grants"] = [self.grants["grants"][1]]; self.write_grants()
        with self.assertRaises(E2EMError): await a.capabilities()
        self.assertTrue((await b.capabilities())["backend_ready"])
    async def test_provider_authentication_and_bad_secrets(self):
        with self.assertRaises(E2EMError): await self.connect(secret="0"*64)
        with self.assertRaises(E2EMError): await self.connect(provider="imposter")
        with self.assertRaises((OSError,asyncio.IncompleteReadError,E2EMError)): await self.connect(principal="unregistered")
    async def test_wrong_socket_and_directory_permissions(self):
        self.socket.chmod(0o666)
        with self.assertRaises(E2EMError): await self.connect()
        self.socket.chmod(0o600); self.directory.chmod(0o755)
        with self.assertRaises(E2EMError): await self.connect()
        self.directory.chmod(0o700)
    async def test_bad_frames_disconnect_without_crashing_service(self):
        for size,body in ((131073,b""),(0,b""),(1,b"\xff"),(2,b"{}")):
            client = await self.connect()
            client._writer.write(struct.pack(">I",size)+body)
            await client._writer.drain()
            await asyncio.sleep(0.02)
            self.assertTrue(client._closed)
        client = await self.connect(); self.assertTrue((await client.capabilities())["backend_ready"])
    async def test_restart_invalidates_provider_scoped_policy_references(self):
        a = await self.connect(); reference = await a.validate_policy(POLICY)
        self.process.send_signal(signal.SIGINT); await asyncio.to_thread(self.process.communicate,timeout=10)
        for _ in range(200):
            if not self.socket.exists(): break
            await asyncio.sleep(.01)
        self.start()
        await self.wait_ready(str(self.socket), "test-provider")
        b = await self.connect(); request = copy.deepcopy(CASES[0]["request"]); request.pop("policy"); request["policy_ref"] = reference
        with self.assertRaises(E2EMError) as error: await b.assess(request)
        self.assertEqual(error.exception.code,"POLICY_NOT_FOUND")
        request["policy_ref"] = await b.validate_policy(POLICY)
        self.assertEqual((await b.assess(request)).action,"warn")
    async def test_discovery_migration_pinning_and_incompatible_coverage(self):
        candidate = dict(kind="project",socket_path=str(self.socket),principal="python",secret="a"*64,provider="test-provider")
        client = await Client.discover([candidate],["pii.email"]); self.clients.append(client)
        self.assertEqual((await client.assess(CASES[0]["request"])).action,"warn")
        native_path = await self.start_native()
        native_candidate={**candidate,"kind":"native","provider":"native-provider","socket_path":native_path}
        native = await Client.discover([candidate,native_candidate],["pii.email"]); self.clients.append(native)
        self.assertEqual(native.provider,"native-provider")
        self.assertNotEqual(native.capability_manifest["provider"],client.capability_manifest["provider"])
        old=copy.deepcopy(CASES[0]["request"]);old.pop("policy");old["policy_ref"]=await client.validate_policy(POLICY)
        with self.assertRaises(E2EMError) as error: await native.assess(old)
        self.assertEqual(error.exception.code,"POLICY_NOT_FOUND")
        old["policy_ref"] = await native.validate_policy(POLICY)
        self.assertEqual((await native.assess(old)).action,"warn")
        with self.assertRaises(E2EMError): await Client.discover([candidate,native_candidate],["abuse.threat"])
        with self.assertRaises(E2EMError): await Client.discover([candidate,native_candidate],["pii.email"],pinned="unknown")
    async def test_node_reference_app_shares_runtime_and_fixtures(self):
        python = await self.connect()
        native = await self.start_native()
        process = await asyncio.create_subprocess_exec("node",str(ROOT / "sdk/node/test.mjs"),"--live",str(self.socket),"--native-socket",native,cwd=ROOT,stdout=asyncio.subprocess.PIPE,stderr=asyncio.subprocess.PIPE)
        result = await python.assess(CASES[0]["request"])
        out,err = await process.communicate()
        self.assertEqual(result.action,"warn")
        self.assertEqual(process.returncode,0,(out+err).decode())
    async def test_unclean_restart_recovers_only_private_stale_socket(self):
        self.process.kill(); await asyncio.to_thread(self.process.communicate,timeout=10)
        self.assertTrue(self.socket.exists())
        self.start()
        for _ in range(200):
            try:
                client = await self.connect(); break
            except (OSError,E2EMError,asyncio.IncompleteReadError): await asyncio.sleep(.01)
        else: self.fail("unclean restart did not recover")
        self.assertEqual((await client.assess(CASES[0]["request"])).action,"warn")
    async def test_blocking_facade_disposes_its_event_loop(self):
        from e2em import BlockingClient
        def run():
            with BlockingClient(socket_path=str(self.socket),principal="python",secret="a"*64,provider="test-provider") as client:
                self.assertEqual(client.assess(CASES[0]["request"]).action,"warn")
            self.assertFalse(client._thread.is_alive())
        await asyncio.to_thread(run)
    async def test_missing_or_malformed_result_never_allows(self):
        client = await self.connect(); request = CASES[0]["request"]
        result = (await client.assess(request)).value
        for change in ({"status":"error","action":"allow"},{"message_revision":"old"},{"duration_ms":-1},{"versions":{}},{"versions":{**result["versions"],"policy_version":"old"}},{"findings":[{}]}):
            with self.assertRaises(E2EMError): _assessment({**result,**change},request)
if __name__ == "__main__": unittest.main()
