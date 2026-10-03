"""Run an isolated transient per-user service, then remove it."""
import asyncio
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
ROOT=Path(__file__).resolve().parents[1]
sys.path.insert(0,str(ROOT / "sdk/python"))
from e2em import Client
async def main():
    name=f"e2em-conformance-{os.getpid()}"
    runtime=Path(os.environ["XDG_RUNTIME_DIR"]) / name / "runtime.sock"
    with tempfile.TemporaryDirectory(prefix=name,dir=Path.home()/".cache") as directory:
        grants=Path(directory)/"grants.json"
        grants.write_text(json.dumps({"provider":"systemd-fixture","grants":[{"principal":"reference","uid":os.getuid(),"secret":"c"*64}]}));grants.chmod(0o600)
        binary=str(Path(sys.argv[1]).resolve())
        command=["systemd-run","--user","--collect","--unit",name,"--property",f"RuntimeDirectory={name}","--property","RuntimeDirectoryMode=0700","--property","UMask=0077",binary,"--socket",str(runtime),"--grants",str(grants),"--idle-seconds","1"]
        try:
            subprocess.run(command,check=True,capture_output=True)
            for _ in range(200):
                if runtime.exists():break
                await asyncio.sleep(.01)
            async with await Client.open(str(runtime),"reference","c"*64,"systemd-fixture") as client:
                request=json.loads((ROOT/"tests/conformance/assessments.json").read_text(encoding="utf-8"))[0]["request"]
                assert (await client.assess(request)).action == "warn"
                await asyncio.sleep(1.1)
                assert (await client.capabilities())["runtime_state"] == "unloaded"
        finally:
            subprocess.run(["systemctl","--user","stop",name],check=True,capture_output=True)
        assert not runtime.exists()
    print("Per-user systemd start, shared contract, idle unload and stop/removal passed.")
if __name__=="__main__":asyncio.run(main())
