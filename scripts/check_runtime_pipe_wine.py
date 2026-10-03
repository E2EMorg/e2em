"""Reproducible pipe compatibility checks; Wine is never native qualification."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

COMPATIBILITY_TESTS = (
    "named_pipe_authentication_fixtures_revocation_and_restart",
    "windows_registry_rejects_foreign_sid_legacy_uid_duplicates_and_large_files",
    "pipe_names_reject_remote_and_unrelated_names",
)
SECURITY_TEST = "first_instance_rejects_namespace_takeover"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("test_binary", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = args.test_binary.resolve()
    if not binary.is_file():
        parser.error("build the Windows GNU runtime_windows test with --no-run first")
    with tempfile.TemporaryDirectory(prefix="e2em-pipe-wine-") as temporary:
        env = {**os.environ, "WINEPREFIX": str(Path(temporary) / "prefix"),
               "WINEDEBUG": "-all", "WINEDLLOVERRIDES": "mscoree,mshtml=d", "DISPLAY": ""}
        def invoke(test):
            return subprocess.run(["wine", str(binary), test, "--exact", "--nocapture", "--test-threads=1"],
                                  env=env, text=True, capture_output=True, timeout=90)
        try:
            for test in COMPATIBILITY_TESTS:
                result = invoke(test)
                if result.returncode or "1 passed" not in result.stdout:
                    raise RuntimeError(f"Wine compatibility failure: {test}\n{result.stdout}{result.stderr}")
            security = invoke(SECURITY_TEST)
            if security.returncode not in (0, 101):
                raise RuntimeError(f"Wine security test failed to execute: {security.stderr}")
            report = {"platform": "Windows GNU binary under Wine on Linux",
                      "wine_version": subprocess.check_output(["wine", "--version"], text=True).strip(),
                      "passed_compatibility_tests": list(COMPATIBILITY_TESTS),
                      "first_instance_security_test_passed_under_wine": security.returncode == 0,
                      "security_test_output": security.stdout + security.stderr,
                      "native_qualification": False,
                      "limits": "Compatibility only. Native Windows token/ACL, first-instance enforcement, Python IOCP, installer and packaged-app qualification remain open. No macOS, contextual-model memory or energy qualification."}
        finally:
            subprocess.run(["wineserver", "-k"], env=env, capture_output=True, check=False)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(f"{len(COMPATIBILITY_TESTS)} Wine compatibility tests passed; native qualification remains open.")


if __name__ == "__main__":
    main()
