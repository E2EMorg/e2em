"""Native-only launchd/SDK/idle-RSS qualification; removes its temporary agent."""
import argparse
import asyncio
import json
import os
from pathlib import Path
import plistlib
import re
import subprocess
import sys
import tempfile

from install_macos_runtime import launch_agent
from check_macos_context import check_contexts

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "sdk/python"))
from e2em import Client


def run(*args):
    return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT)


async def check(args):
    label = f"org.e2em.conformance.{os.getpid()}"
    domain = f"gui/{os.getuid()}"
    service = f"{domain}/{label}"
    # Keep the socket below Darwin's 104-byte sun_path limit.
    with tempfile.TemporaryDirectory(prefix="e2-", dir="/tmp") as temporary:
        directory = Path(temporary)
        directory.chmod(0o700)
        socket = directory / "s"
        grants = directory / "grants.json"
        grants.write_text(json.dumps({"provider": label, "grants": [
            {"principal": p, "uid": os.getuid(), "secret": s * 64}
            for p, s in (("python", "a"), ("node", "b"))]}))
        grants.chmod(0o600)
        credential = directory / "node.json"
        credential.write_text(json.dumps({"socket_path": str(socket), "principal": "node",
                                          "provider": label, "secret": "b" * 64}))
        credential.chmod(0o600)
        agent = directory / "agent.plist"
        configuration = launch_agent(args.binary.resolve(), socket, grants, label, 1, auto_update=False)
        configuration["ProgramArguments"].append("--rules-only")
        agent.write_bytes(plistlib.dumps(configuration))
        agent.chmod(0o600)
        started = False
        node = None
        try:
            run("launchctl", "bootstrap", domain, str(agent))
            started = True
            for _ in range(500):
                if socket.exists():
                    break
                await asyncio.sleep(.01)
            state = run("launchctl", "print", service)
            match = re.search(r"\bpid = (\d+)", state)
            if not match:
                raise RuntimeError("launchd did not report a running agent")
            pid = match[1]
            rss = lambda: int(run("ps", "-o", "rss=", "-p", pid).strip())
            unloaded = rss()
            async with await Client.open(str(socket), "python", "a" * 64, label) as client:
                cases = json.loads((ROOT / "tests/conformance/assessments.json").read_text(encoding="utf-8"))
                node = await asyncio.create_subprocess_exec(
                    "node", str(ROOT / "sdk/node/check-agent.mjs"), str(credential),
                    stdout=asyncio.subprocess.PIPE, stderr=asyncio.subprocess.PIPE)
                for fixture in cases:
                    result = await client.assess(fixture["request"])
                    assert result.status == fixture["expected"]["status"]
                    assert result.action == fixture["expected"]["action"]
                    assert [[s["start"], s["end"]] for f in result.value["findings"] for s in f["spans"]] == fixture["expected"]["spans"]
                stdout, stderr = await node.communicate()
                if node.returncode:
                    raise RuntimeError(f"Node fixture client failed: {stderr.decode()}")
                node_report = json.loads(stdout)
                assert node_report["provider"] == client.capability_manifest["provider"]
                cycles = []
                for _ in range(3):
                    await client.assess(cases[0]["request"])
                    ready = rss()
                    await asyncio.sleep(1.2)
                    assert (await client.capabilities())["runtime_state"] == "unloaded"
                    cycles.append({"ready_rss_kib": ready, "unloaded_rss_kib": rss()})
                report = {"platform": "macOS", "version": run("sw_vers", "-productVersion").strip(),
                          "context": "unsandboxed per-user launchd GUI agent",
                          "backend": "rules-only", "python_fixtures": len(cases),
                          "node_fixtures": node_report["fixtures"], "initial_rss_kib": unloaded,
                          "cycles": cycles, "sandbox_qualification": False,
                          "app_contexts": await asyncio.to_thread(check_contexts, socket, directory, args.ffi_library or args.binary.resolve().parent / "libe2em_ffi.a")}
        finally:
            if node is not None and node.returncode is None:
                node.kill()
                await node.communicate()
            if started:
                run("launchctl", "bootout", service)
                for _ in range(500):
                    if not socket.exists():
                        break
                    await asyncio.sleep(.01)
                if socket.exists():
                    raise RuntimeError("agent shutdown did not remove its socket")
        return report


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--ffi-library", type=Path, help="explicit static ABI path, defaults alongside the daemon")
    args = parser.parse_args()
    if sys.platform != "darwin":
        parser.error("requires a native macOS GUI login session; cross-builds cannot qualify launchd")
    report = asyncio.run(check(args))
    args.output.write_text(json.dumps(report, indent=2) + "\n")


if __name__ == "__main__":
    main()
