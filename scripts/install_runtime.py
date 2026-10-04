"""Explicit per-user rules preview install/enrol/revoke/remove; never starts service."""
import argparse
import json
import os
from pathlib import Path
import re
import secrets
import shutil
import socket
import stat
import uuid
from setup_models import arguments as model_arguments, provision, remove_models

UNIT = """[Unit]
Description=Local E2EM runtime
[Service]
ExecStart=%h/.local/bin/e2emd --socket %t/e2em/runtime.sock --grants %h/.config/e2em/grants.json --auto-update
RuntimeDirectory=e2em
RuntimeDirectoryMode=0700
UMask=0077
NoNewPrivileges=yes
PrivateTmp=yes
Restart=on-failure
[Install]
WantedBy=default.target
"""

def private_dir(path):
    path.mkdir(parents=True, exist_ok=True, mode=0o700)
    metadata = path.lstat()
    if not stat.S_ISDIR(metadata.st_mode) or metadata.st_uid != os.getuid() or metadata.st_mode & 0o077:
        raise ValueError("configuration directory must be owned and private")

def atomic_json(path,value):
    temporary = path.with_name(path.name + "." + secrets.token_hex(8))
    try:
        with temporary.open("x",encoding="utf-8") as output:
            os.chmod(temporary,0o600)
            json.dump(value,output,indent=2); output.write("\n")
            output.flush(); os.fsync(output.fileno())
        temporary.replace(path)
    finally:
        temporary.unlink(missing_ok=True)

def remove_updates(config):
    updates = config / 'updates'
    if not updates.exists() and not updates.is_symlink():
        return
    private_dir(updates)
    names = {'state.json', 'status.json', 'supervisor.lock', 'check-request.json'}
    for path in updates.iterdir():
        if path.name in names or path.name.startswith('.e2em-update-') or re.fullmatch(r'e2emd-\d+\.\d+\.\d+(?:-[a-zA-Z0-9.-]+)?(?:\.exe)?', path.name):
            if path.is_file() or path.is_symlink():
                path.unlink()
    if not any(updates.iterdir()):
        updates.rmdir()

def remove_onboarding(config):
    setup = config / 'setup'
    if not setup.exists() and not setup.is_symlink(): return
    private_dir(setup)
    import fcntl
    lock = setup / 'session.lock'
    if lock.is_symlink(): raise ValueError('invalid setup lock')
    if lock.exists():
        with lock.open('r') as handle:
            try: fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            except BlockingIOError: raise ValueError('close E2EM Setup before uninstalling') from None
            for name in ('session.json', 'session.lock'):
                (setup / name).unlink(missing_ok=True)
    if not any(setup.iterdir()): setup.rmdir()

def main(argv=None):
    """Legacy command-line setup; packaged users normally use e2emd --setup."""
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home",type=Path,default=Path.home())
    sub = parser.add_subparsers(dest="command",required=True)
    install = sub.add_parser("install"); install.add_argument("--binary",type=Path,required=True)
    install.add_argument("--use-packaged-binary", action="store_true",
                         help="reference a package-managed executable instead of copying it")
    install.add_argument("--no-auto-update", action="store_true", help="disable idle background runtime updates")
    model_arguments(install)
    for command in ("enrol","revoke"):
        action = sub.add_parser(command); action.add_argument("principal")
        if command == "enrol": action.add_argument("--model-management", action="store_true")
    sub.add_parser("uninstall")
    args = parser.parse_args(argv)
    config = args.home / ".config/e2em"
    binary = args.home / ".local/bin/e2emd"
    unit = args.home / ".config/systemd/user/e2emd.service"
    if args.command == "install":
        packaged = args.binary.resolve() if args.use_packaged_binary else None
        # systemd quoted argument syntax: reject substitutions and line breaks
        # before creating any installation files.
        if packaged and any(c in str(packaged) for c in '\n\r%"\\'):
            raise ValueError("unsupported package-managed executable path")
        if binary.exists() or binary.is_symlink() or unit.exists() or unit.is_symlink() or config.exists():
            raise ValueError("refusing to overwrite an existing installation")
        if not args.binary.is_file(): raise ValueError("missing runtime binary")
        private_dir(config)
        binary.parent.mkdir(parents=True,exist_ok=True)
        if not packaged:
            shutil.copyfile(args.binary,binary); binary.chmod(0o700)
        unit.parent.mkdir(parents=True,exist_ok=True)
        unit_text = UNIT.replace('%h/.local/bin/e2emd', '"' + str(packaged) + '"') if packaged else UNIT
        if args.no_auto_update:
            unit_text = unit_text.replace(' --auto-update', '')
        if args.offline: unit_text = unit_text.replace(' --auto-update', '') .replace(' --grants', ' --offline --grants')
        if args.rules_only: unit_text = unit_text.replace(' --grants', ' --rules-only --grants')
        unit.write_text(unit_text); unit.chmod(0o600)
        atomic_json(config / "grants.json",{"provider":"project-"+uuid.uuid4().hex,"grants":[]})
        atomic_json(config / "installation.json",{"version":1, **({"packaged_binary": str(packaged)} if packaged else {})})
        provision(args, packaged or binary, config)
        print("Installed. Start with: systemctl --user daemon-reload; systemctl --user enable --now e2emd")
        return
    private_dir(config)
    marker = json.loads((config / "installation.json").read_text(encoding="utf-8"))
    packaged_marker = (
        isinstance(marker, dict) and set(marker) == {"version", "packaged_binary"}
        and marker["version"] == 1 and isinstance(marker["packaged_binary"], str)
        and Path(marker["packaged_binary"]).is_absolute()
    )
    if marker != {"version":1} and not packaged_marker:
        raise ValueError("unmanaged installation")
    if args.command == "uninstall":
        remove_onboarding(config)
        # The owner stops the unit first. Delete only managed files, not arbitrary app data.
        if (config / "apps").exists():
            for path in (config / "apps").glob("*.json"): path.unlink()
            (config / "apps").rmdir()
        managed = [unit,config / "grants.json",config / "installation.json"]
        if (config / 'onboarding.json').exists(): managed.append(config / 'onboarding.json')
        if "packaged_binary" not in marker: managed.append(binary)
        for path in managed: path.unlink()
        remove_updates(config)
        remove_models(config)
        if not any(config.iterdir()): config.rmdir()
        print("Removed. Reload the user service manager with systemctl --user daemon-reload.")
        return
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}",args.principal): raise ValueError("principal must be a simple application slug")
    grants_path = config / "grants.json"
    grants = json.loads(grants_path.read_text(encoding="utf-8"))
    grants["grants"] = [g for g in grants["grants"] if g["principal"] != args.principal]
    credential = config / "apps" / (args.principal+".json")
    if args.command == "revoke":
        atomic_json(grants_path,grants); credential.unlink(missing_ok=True); return
    if len(grants["grants"]) >= 64: raise ValueError("grant limit reached")
    grant = {"principal":args.principal,"uid":os.getuid(),"secret":secrets.token_hex(32)}
    if args.model_management: grant['model_management'] = True
    grants["grants"].append(grant)
    atomic_json(grants_path,grants)
    private_dir(config / "apps")
    runtime = Path(os.environ.get("XDG_RUNTIME_DIR",f"/run/user/{os.getuid()}")) / "e2em/runtime.sock"
    atomic_json(credential,{"kind":"project","socket_path":str(runtime),"principal":grant["principal"],"secret":grant["secret"],"provider":grants["provider"]})
    print(f"Credentials: {credential}. Keep this file private; enrolment is an explicit grant.")

if __name__ == "__main__": main()
