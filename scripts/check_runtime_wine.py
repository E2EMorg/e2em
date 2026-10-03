"""Windows GNU embedded ABI compatibility test in an isolated Wine prefix.
This does not qualify the Windows named-pipe service, ACLs or packaged apps.
"""
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
ROOT=Path(__file__).resolve().parents[1]
def main():
    directory=Path(sys.argv[1]).resolve()
    with tempfile.TemporaryDirectory(prefix="e2em-wine-") as temporary:
        temporary=Path(temporary)
        shutil.copyfile(directory/"e2em_ffi.dll",temporary/"e2em_ffi.dll")
        binary=temporary/"check.exe"
        subprocess.run(["x86_64-w64-mingw32-gcc",str(ROOT/"crates/e2em-ffi/examples/check.c"),"-I",str(ROOT/"crates/e2em-ffi/include"),"-L",str(directory),"-le2em_ffi","-Wall","-Wextra","-Werror","-o",str(binary)],check=True,capture_output=True)
        env={**os.environ,"WINEPREFIX":str(temporary/"prefix"),"WINEDEBUG":"-all","WINEDLLOVERRIDES":"mscoree,mshtml=d","DISPLAY":""}
        def invoke(operation):
            path=temporary/"call.json";path.write_text(json.dumps(dict(call_id="c",api_version="0.1",operation=operation)))
            result=subprocess.run(["wine",str(binary),"Z:"+str(path).replace("/","\\")],cwd=temporary,env=env,capture_output=True,text=True,timeout=60)
            if result.returncode:raise RuntimeError(f"Wine foreign client failed ({result.returncode}): {result.stderr}")
            return json.loads(result.stdout)
        assessments=json.loads((ROOT/"tests/conformance/assessments.json").read_text(encoding="utf-8"))
        policies=json.loads((ROOT/"tests/conformance/policy-failures.json").read_text(encoding="utf-8"))
        for case in assessments:
            result=invoke(dict(op="assess",request=case["request"]))["assessment"]
            assert result["status"] == case["expected"]["status"]
            assert result["action"] == case["expected"]["action"]
            assert [[s["start"],s["end"]] for f in result["findings"] for s in f["spans"]] == case["expected"]["spans"]
        for case in policies:
            result=invoke(dict(op="validate_policy",policy=case["policy"]))
            assert result["error_code"] == case["error_code"]
        subprocess.run(["wineserver","-k"],env=env,capture_output=True,check=False)
    print(f"Windows GNU C ABI: {len(assessments)} assessment and {len(policies)} policy fixtures passed under Wine.")
if __name__ == "__main__":main()
