"""C/C++ callers use the same fixtures as Rust and service clients."""
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
ROOT = Path(__file__).resolve().parents[1]
@unittest.skipUnless(os.environ.get("E2EM_FFI_LIBRARY"),"requires cargo build --workspace")
class Embedded(unittest.TestCase):
    def test_c_and_cpp_share_contract_without_ownership_errors(self):
        library = Path(os.environ["E2EM_FFI_LIBRARY"]).resolve()
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            flags = ["-I",str(ROOT/"crates/e2em-ffi/include"),"-L",str(library.parent),"-le2em_ffi","-Wl,-rpath,"+str(library.parent)]
            for compiler,source in (("cc","check.c"),("c++","check.cpp")):
                binary = directory/source
                command = [compiler,str(ROOT/"crates/e2em-ffi/examples"/source),"-o",str(binary),"-Wall","-Wextra","-Werror",*flags]
                if os.environ.get("E2EM_ASAN"):
                    command += ["-fsanitize=address,undefined","-fno-omit-frame-pointer"]
                    if sys.platform.startswith("linux"):
                        # GCC ASan's shadow mapping can collide with randomized
                        # PIE startup on Linux. Keep ASan/UBSan fully enabled,
                        # but fix the foreign test executable's load address.
                        command += ["-fno-pie","-no-pie"]
                subprocess.run(command,check=True,capture_output=True)
                if source.endswith("cpp"):
                    result = json.loads(subprocess.check_output([str(binary)],timeout=10))
                    self.assertEqual(result["kind"],"capabilities")
                cases = json.loads((ROOT/"tests/conformance/assessments.json").read_text(encoding="utf-8"))
                for case in cases:
                    call = directory / "call.json"; call.write_text(json.dumps(dict(call_id="c",api_version="0.1",operation=dict(op="assess",request=case["request"]))))
                    result = json.loads(subprocess.check_output([str(binary),str(call)],timeout=10))["assessment"]
                    self.assertEqual(result["status"],case["expected"]["status"])
                    self.assertEqual(result["action"],case["expected"]["action"])
                    self.assertEqual([[s["start"],s["end"]] for f in result["findings"] for s in f["spans"]],case["expected"]["spans"])
                for case in json.loads((ROOT/"tests/conformance/policy-failures.json").read_text(encoding="utf-8")):
                    call = directory / "call.json"; call.write_text(json.dumps(dict(call_id="c",api_version="0.1",operation=dict(op="validate_policy",policy=case["policy"]))))
                    result = json.loads(subprocess.check_output([str(binary),str(call)],timeout=10))
                    self.assertEqual(result["kind"],"error")
                    self.assertEqual(result["error_code"],case["error_code"])
                for case in json.loads((ROOT/"tests/conformance/policy-reports.json").read_text(encoding="utf-8")):
                    call = directory / "call.json"
                    call.write_text(json.dumps(dict(call_id="c",api_version="0.1",operation=dict(op="validate_policy",policy=case["policy"]))))
                    result = json.loads(subprocess.check_output([str(binary),str(call)],timeout=10))
                    self.assertEqual(result["kind"], "policy")
                    request = json.loads(json.dumps(cases[0]["request"]))
                    request["policy"] = case["policy"]
                    call.write_text(json.dumps(dict(call_id="c",api_version="0.1",operation=dict(op="assess",request=request))))
                    result = json.loads(subprocess.check_output([str(binary),str(call)],timeout=10))["assessment"]
                    self.assertEqual(result["status"], case["expected"]["status"])
                    self.assertEqual(result["action"], case["expected"]["action"])
                    self.assertEqual(result["coverage"]["unevaluated_rules"], case["expected"]["unevaluated_rules"])
                    self.assertEqual(result["reason_codes"], case["expected"]["reason_codes"])
if __name__ == "__main__": unittest.main()
