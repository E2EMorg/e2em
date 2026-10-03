"""Native ad-hoc App Sandbox experiment; no blanket broker entitlement claim."""
import json
from pathlib import Path
import plistlib
import shlex
import shutil
import subprocess
import sys
import uuid

ROOT = Path(__file__).resolve().parents[1]


def request_header(directory):
    fixture = json.loads((ROOT / "tests/conformance/assessments.json").read_text())[0]
    call = {"call_id": "context-probe", "api_version": "0.1", "operation": {"op": "assess", "request": fixture["request"]}}
    literal = json.dumps(json.dumps(call, ensure_ascii=True))
    path = directory / "e2em_probe_request.h"
    path.write_text("static const unsigned char call_json[] = " + literal + ";\n")
    return path


def check_contexts(socket, directory, ffi_library):
    if sys.platform != "darwin":
        raise RuntimeError("App Sandbox qualification requires native macOS")
    ffi_library = ffi_library.resolve()
    if not ffi_library.is_file():
        raise RuntimeError("build the embedded static ABI alongside the daemon first")
    request_header(directory)
    # Ask rustc for the native libraries required by this target's static ABI.
    result = subprocess.run(["cargo", "rustc", "--locked", "-p", "e2em-ffi", "--lib", "--", "--print", "native-static-libs"],
                            cwd=ROOT, text=True, capture_output=True, check=True)
    link = next((line.split("native-static-libs:", 1)[1] for line in result.stderr.splitlines() if "native-static-libs:" in line), None)
    if link is None:
        raise RuntimeError("rustc did not report native static link dependencies")
    executable = directory / "context-probe"
    subprocess.run(["xcrun", "clang", str(ROOT / "scripts/probes/macos_context.c"), "-I", str(ROOT / "crates/e2em-ffi/include"),
                    "-I", str(directory), str(ffi_library), *shlex.split(link), "-lsandbox", "-Wall", "-Wextra", "-Werror", "-o", str(executable)], check=True)
    def invoke(path):
        return json.loads(subprocess.check_output([str(path), str(socket)], text=True, stderr=subprocess.PIPE, timeout=10))
    ordinary = invoke(executable)
    assert ordinary["connected"] and not ordinary["sandboxed"] and ordinary["embedded_warning"]
    label = "org.e2em.conformance.context." + uuid.uuid4().hex
    container = Path.home() / "Library/Containers" / label
    if container.exists():
        raise RuntimeError("refusing an existing sandbox container")
    bundle = directory / "ContextProbe.app"
    macos = bundle / "Contents/MacOS"
    macos.mkdir(parents=True)
    program = macos / "context-probe"
    shutil.copyfile(executable, program)
    program.chmod(0o700)
    (bundle / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": label,
        "CFBundleExecutable": "context-probe", "CFBundlePackageType": "APPL", "CFBundleVersion": "1"}))
    entitlements = directory / "sandbox.plist"
    entitlements.write_bytes(plistlib.dumps({"com.apple.security.app-sandbox": True}))
    try:
        subprocess.run(["codesign", "--force", "--sign", "-", "--timestamp=none", "--entitlements", str(entitlements), str(bundle)], check=True, capture_output=True)
        subprocess.run(["codesign", "--verify", "--strict", str(bundle)], check=True, capture_output=True)
        sandbox = invoke(program)
        assert sandbox["sandboxed"], "signature must actually activate App Sandbox"
        assert sandbox["embedded_warning"], "the embedded rules fallback must work within App Sandbox"
        return {"unsandboxed_user_executable": ordinary,
                "ad_hoc_app_sandbox_without_exceptions": sandbox,
                "shared_sandbox_service_supported": False,
                "shared_service_path_observation": "endpoint reachable; shared authentication/enrolment unqualified" if sandbox["connected"] else "endpoint denied by the sandbox",
                "embedded_rules_fallback": True,
                "limits": "One ad-hoc signed bundle without network/file exceptions; no App Group, entitlement broker, notarization, App Store or universal sandbox bypass qualification."}
    finally:
        if container.exists():
            shutil.rmtree(container)
