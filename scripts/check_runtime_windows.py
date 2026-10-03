"""Native Windows installer/pipe/SDK check; uses a temporary user installation."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python"))
from e2em import Client, E2EMError


async def check(args):
    with tempfile.TemporaryDirectory(prefix="e2em-native-") as temporary:
        install = Path(temporary) / "user installation"
        command = ["powershell.exe", "-NoProfile", "-NonInteractive", "-File",
                   str(ROOT / "scripts/install_windows_runtime.ps1"),
                   "-InstallDirectory", str(install)]
        def manage(action, *options):
            return subprocess.run(command + ["-Action", action, *options],
                                  check=True, capture_output=True, text=True).stdout
        def expect_failure(action, *options):
            try:
                manage(action, *options)
            except subprocess.CalledProcessError:
                return
            raise AssertionError(f"installer unexpectedly accepted {action}")
        manage("install", "-Binary", str(args.binary.resolve()))
        expect_failure("install", "-Binary", str(args.binary.resolve()))
        process = node = None
        note_created = False
        try:
            manage("enrol", "-Principal", "python")
            previous = json.loads((install / "app-python.json").read_text())
            manage("enrol", "-Principal", "python")
            manage("enrol", "-Principal", "node")
            credential = json.loads((install / "app-python.json").read_text())
            assert previous["secret"] != credential["secret"]
            process = subprocess.Popen([str(install / "e2emd.exe"), "--pipe", credential["socket_path"],
                                        "--grants", str(install / "grants.json"), "--idle-seconds", "1"],
                                       creationflags=subprocess.CREATE_NEW_PROCESS_GROUP,
                                       stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            client = None
            for _ in range(500):
                if process.poll() is not None:
                    raise RuntimeError("installed service exited before accepting clients")
                try:
                    client = await Client.open(credential["socket_path"], "python",
                                               credential["secret"], credential["provider"])
                    break
                except Exception:
                    await asyncio.sleep(.01)
            if client is None:
                raise RuntimeError("installed pipe did not become ready")
            expect_failure("uninstall")
            (install / "owner-note.txt").write_text("preserve")
            note_created = True
            try:
                old = await Client.open(previous["socket_path"], "python", previous["secret"], previous["provider"])
            except E2EMError:
                pass
            else:
                await old.close()
                raise AssertionError("rotated secret remained authorized")
            async with client:
                cases = json.loads((ROOT / "tests/conformance/assessments.json").read_text())
                node = await asyncio.create_subprocess_exec("node", str(ROOT / "sdk/node/check-agent.mjs"),
                            str(install / "app-node.json"), stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                for fixture in cases:
                    result = await client.assess(fixture["request"])
                    assert result.status == fixture["expected"]["status"]
                    assert result.action == fixture["expected"]["action"]
                    assert [[s["start"], s["end"]] for f in result.value["findings"] for s in f["spans"]] == fixture["expected"]["spans"]
                stdout, stderr = await node.communicate()
                if node.returncode:
                    raise RuntimeError(f"Node fixture check failed: {stderr.decode()}")
                node_report = json.loads(stdout)
                assert node_report["provider"] == client.capability_manifest["provider"]
                # A broadened grant file must disable assessments and must not
                # receive new secrets through ReplaceFile's preserved DACL.
                node_credential = json.loads((install / "app-node.json").read_text())
                async with await Client.open(node_credential["socket_path"], "node", node_credential["secret"], node_credential["provider"]) as acl_client:
                    subprocess.run(["icacls.exe", str(install / "grants.json"), "/grant", "*S-1-1-0:(R)"], check=True, capture_output=True)
                    try:
                        try:
                            await acl_client.capabilities()
                        except E2EMError:
                            pass
                        else:
                            raise AssertionError("broad grant ACL remained authorized")
                        expect_failure("enrol", "-Principal", "should-refuse")
                    finally:
                        subprocess.run(["icacls.exe", str(install / "grants.json"), "/remove:g", "*S-1-1-0"], check=True, capture_output=True)
                await asyncio.sleep(1.2)
                assert (await client.capabilities())["runtime_state"] == "unloaded"
                manage("revoke", "-Principal", "python")
                try:
                    await client.capabilities()
                except Exception:
                    pass
                else:
                    raise AssertionError("revoked client remained authorized")
            report = {"platform": "Windows", "version": sys.getwindowsversion().build,
                      "context": "unpackaged per-user foreground named-pipe prototype",
                      "backend": "rules-only", "python_fixtures": len(cases),
                      "node_fixtures": node_report["fixtures"], "grant_revocation": True,
                      "grant_rotation": True, "broad_grant_acl_rejected": True,
                      "overwrite_and_live_uninstall_refused": True,
                      "packaged_app_qualification": False}
        finally:
            if node is not None and node.returncode is None:
                node.kill()
                await node.communicate()
            shutdown_error = None
            if process is not None:
                if process.poll() is None:
                    process.send_signal(signal.CTRL_BREAK_EVENT)
                try:
                    stdout, stderr = await asyncio.to_thread(process.communicate, timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    await asyncio.to_thread(process.communicate)
                    shutdown_error = RuntimeError("console shutdown did not finish")
                if process.returncode:
                    shutdown_error = RuntimeError(f"service exit {process.returncode}: {stderr.decode(errors='replace')}")
            manage("uninstall")
            if note_created:
                assert (install / "owner-note.txt").read_text() == "preserve"
                (install / "owner-note.txt").unlink()
                install.rmdir()
            if shutdown_error is not None:
                raise shutdown_error
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if os.name != "nt":
        parser.error("requires native Windows; Wine cannot qualify user installation or ACLs")
    args.output.write_text(json.dumps(asyncio.run(check(args)), indent=2) + "\n")


if __name__ == "__main__":
    main()
