"""Explicit per-user macOS agent prototype; installation never starts the agent."""
import argparse
import json
import os
from pathlib import Path
import plistlib
import re
import secrets
import shutil
import uuid

from install_runtime import atomic_json, private_dir, remove_updates, remove_onboarding
from setup_models import arguments as model_arguments, provision, remove_models

LABEL = "org.e2em.runtime"


def launch_agent(binary, socket, grants, label=LABEL, idle_seconds=300, auto_update=True):
    return {
        "Label": label,
        "ProgramArguments": [str(binary), "--socket", str(socket), "--grants",
                             str(grants), *(["--auto-update"] if auto_update else []),
                             "--idle-seconds", str(idle_seconds)],
        "RunAtLoad": True,
        "KeepAlive": {"SuccessfulExit": False},
        "ProcessType": "Interactive",
        "Umask": 0o077,
        "ExitTimeOut": 10,
    }


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=Path, default=Path.home())
    sub = parser.add_subparsers(dest="command", required=True)
    install = sub.add_parser("install")
    install.add_argument("--binary", type=Path, required=True)
    install.add_argument("--use-packaged-binary", action="store_true")
    install.add_argument("--no-auto-update", action="store_true")
    model_arguments(install)
    for command in ("enrol", "revoke"):
        action = sub.add_parser(command); action.add_argument("principal")
        if command == "enrol": action.add_argument("--model-management", action="store_true")
    sub.add_parser("uninstall")
    args = parser.parse_args(argv)
    home = args.home.absolute()
    config = home / ".config/e2em"
    binary = home / ".local/bin/e2emd"
    agent = home / "Library/LaunchAgents" / (LABEL + ".plist")
    runtime = home / "Library/Caches/e2em"
    socket = runtime / "runtime.sock"
    if len(os.fsencode(socket)) >= 104:
        raise ValueError("macOS Unix socket path must be shorter than 104 bytes")
    if args.command == "install":
        if any(p.exists() or p.is_symlink() for p in (binary, agent, config, runtime)):
            raise ValueError("refusing to overwrite an existing installation")
        if not args.binary.is_file():
            raise ValueError("missing runtime binary")
        private_dir(config)
        private_dir(runtime)
        binary.parent.mkdir(parents=True, exist_ok=True)
        packaged = args.binary.resolve() if args.use_packaged_binary else None
        if not packaged:
            shutil.copyfile(args.binary, binary)
            binary.chmod(0o700)
        agent.parent.mkdir(parents=True, exist_ok=True)
        with agent.open("xb") as output:
            os.chmod(agent, 0o600)
            configuration = launch_agent(packaged or binary, socket, config / "grants.json", auto_update=not (args.no_auto_update or args.offline))
            if args.rules_only: configuration['ProgramArguments'].append('--rules-only')
            if args.offline: configuration['ProgramArguments'].append('--offline')
            plistlib.dump(configuration, output)
        atomic_json(config / "grants.json", {"provider": "project-" + uuid.uuid4().hex,
                                            "grants": []})
        atomic_json(config / "installation.json", {"version": 1, "platform": "macos", **({"packaged_binary": str(packaged)} if packaged else {})})
        provision(args, packaged or binary, config)
        print(f"Installed. Start with: launchctl bootstrap gui/{os.getuid()} {agent}")
        return
    private_dir(config)
    private_dir(runtime)
    marker = json.loads((config / "installation.json").read_text(encoding="utf-8"))
    base = {"version": 1, "platform": "macos"}
    packaged_marker = (
        isinstance(marker, dict) and set(marker) == {*base, "packaged_binary"}
        and all(marker[k] == v for k,v in base.items())
        and isinstance(marker["packaged_binary"], str)
        and Path(marker["packaged_binary"]).is_absolute()
    )
    if marker != base and not packaged_marker:
        raise ValueError("unmanaged installation")
    if args.command == "uninstall":
        # bootout must complete first. Refuse even stale sockets here: shutdown
        # normally removes them, and a live agent must never lose its grants.
        if socket.exists() or socket.is_symlink():
            raise ValueError("stop the agent and remove its stale socket before uninstalling")
        remove_onboarding(config)
        apps = config / "apps"
        if apps.exists():
            private_dir(apps)
            for path in apps.glob("*.json"):
                path.unlink()
            if not any(apps.iterdir()):
                apps.rmdir()
        managed = [config / "grants.json", config / "installation.json"]
        if agent.exists(): managed.append(agent)
        if (config / 'start-runtime.plist').exists(): managed.append(config / 'start-runtime.plist')
        if (config / 'onboarding.json').exists(): managed.append(config / 'onboarding.json')
        if "packaged_binary" not in marker:
            managed.append(binary)
        for path in managed:
            path.unlink()
        remove_updates(config)
        remove_models(config)
        for path in (config, runtime):
            if not any(path.iterdir()):
                path.rmdir()
        print("Removed managed prototype files.")
        return
    if not re.fullmatch(r"[a-zA-Z0-9_-]{1,64}", args.principal):
        raise ValueError("principal must be a simple application slug")
    grants_path = config / "grants.json"
    grants = json.loads(grants_path.read_text(encoding="utf-8"))
    grants["grants"] = [g for g in grants["grants"] if g["principal"] != args.principal]
    credential = config / "apps" / (args.principal + ".json")
    if args.command == "revoke":
        atomic_json(grants_path, grants)
        credential.unlink(missing_ok=True)
        return
    if len(grants["grants"]) >= 64:
        raise ValueError("grant limit reached")
    grant = {"principal": args.principal, "uid": os.getuid(), "secret": secrets.token_hex(32)}
    if args.model_management: grant['model_management'] = True
    grants["grants"].append(grant)
    atomic_json(grants_path, grants)
    private_dir(config / "apps")
    atomic_json(credential, {"kind": "project", "socket_path": str(socket),
                            "principal": grant["principal"], "secret": grant["secret"],
                            "provider": grants["provider"]})
    print(f"Credentials: {credential}. Keep this file private.")


if __name__ == "__main__":
    main()
