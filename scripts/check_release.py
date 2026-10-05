"""Validate synchronized release versions and the complete publication payload."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import shutil
import tomllib
from package_runtime import verify_binary
from package_update import UPDATE_TARGETS

ROOT = Path(__file__).resolve().parents[1]
ONBOARDING_REPORTS = (
    'onboarding-linux.json', 'onboarding-windows.json',
    'onboarding-x86_64-apple-darwin.json', 'onboarding-aarch64-apple-darwin.json',
)
DEVICE_REPORTS = ('devices-linux.json', 'devices-windows.json',
                  'devices-x86_64-apple-darwin.json', 'devices-aarch64-apple-darwin.json')


def verify_devices(directory):
    for name in DEVICE_REPORTS:
        report = json.loads((directory / name).read_text(encoding='utf-8'))
        error = report.get('max_probability_error')
        if (not isinstance(error, (int, float)) or not 0 <= error < 2e-5
                or report['cpu']['selected_provider'] != 'CPU'
                or not report['cpu']['executed_nodes'].get('CPUExecutionProvider')
                or not report['auto']['executed_nodes']):
            raise ValueError(f'native device qualification did not pass: {name}')


def verify_onboarding(directory):
    for name in ONBOARDING_REPORTS:
        report = json.loads((directory / name).read_text(encoding='utf-8'))
        if not all(report.get(check) is True for check in (
            'guided_setup', 'automatic_app_connection', 'model_assessment', 'autostart', 'preferences_restart', 'reopen', 'retry',
            'credentials_preserved', 'foreign_origin_rejected',
        )):
            raise ValueError(f'guided onboarding qualification did not pass: {name}')


def release_version(root=ROOT):
    versions = {}
    for name in ("Cargo.toml", "crates/e2em-ffi/Cargo.toml", "crates/e2em-platform/Cargo.toml", "crates/e2em-inference/Cargo.toml"):
        versions[name] = tomllib.loads((root / name).read_text(encoding="utf-8"))["package"]["version"]
    versions["sdk/python/pyproject.toml"] = tomllib.loads((root / "sdk/python/pyproject.toml").read_text(encoding="utf-8"))["project"]["version"]
    versions["sdk/node/package.json"] = json.loads((root / "sdk/node/package.json").read_text(encoding="utf-8"))["version"]
    dart_version = re.findall(r'^version:\s*([^\s]+)\s*$', (root / 'sdk/dart/pubspec.yaml').read_text(encoding='utf-8'), re.MULTILINE)
    if len(dart_version) != 1:
        raise ValueError('Dart SDK must declare one release version')
    versions['sdk/dart/pubspec.yaml'] = dart_version[0]
    if len(set(versions.values())) != 1:
        raise ValueError(f"runtime and SDK versions differ: {versions}")
    version = versions["Cargo.toml"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("release version must be MAJOR.MINOR.PATCH")
    return version


def stage_assets(source, destination):
    # Artifact downloads retain their upload paths. Flatten only after checking
    # all basenames, so two native reports can never overwrite each other.
    files = {}
    for path in sorted(source.rglob("*")):
        if path.is_symlink():
            raise ValueError("release artifacts must not contain symlinks")
        if not path.is_file():
            continue
        if path.name in files:
            raise ValueError(f"duplicate release asset basename: {path.name}")
        files[path.name] = path
    if destination.exists() and any(destination.iterdir()):
        raise ValueError("release staging directory must be empty")
    destination.mkdir(parents=True, exist_ok=True)
    for name, path in files.items():
        shutil.copyfile(path, destination / name)


def verify_assets(directory, version):
    names = [
        f"e2em-runtime-{version}-x86_64-unknown-linux-musl.deb",
        f"e2em-runtime-{version}-x86_64-unknown-linux-musl.rpm",
        f"e2em-runtime-{version}-x86_64-pc-windows-msvc.msi",
        f"e2em-runtime-{version}-x86_64-apple-darwin.pkg",
        f"e2em-runtime-{version}-aarch64-apple-darwin.pkg",
        f"e2em_local-{version}-py3-none-any.whl",
        f"e2em_local-{version}.tar.gz",
        f"e2em-local-{version}.tgz",
        f"e2em-rust-sdk-{version}.tar.gz",
        f"e2em-dart-sdk-{version}.tar.gz",
        *[f"e2em-c-sdk-{version}-{target}.zip" for target in (
            "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc",
            "x86_64-apple-darwin", "aarch64-apple-darwin")],
        "deb.json", "rpm.json", "msi.json", "pkg-x86_64-apple-darwin.json",
        "pkg-aarch64-apple-darwin.json", "e2em-windows.json",
        "deb-normal.json", "rpm-normal.json", "msi-normal.json", "pkg-normal-x86_64-apple-darwin.json", "pkg-normal-aarch64-apple-darwin.json",
        "e2em-macos-macos-15-intel.json", "e2em-macos-macos-15.json",
    ]
    updates = {
        f'e2em-update-{version}-{target}{".exe" if "windows" in target else ".bin"}': target
        for target in UPDATE_TARGETS
    }
    names.extend(updates)
    names.extend(ONBOARDING_REPORTS)
    names.extend(DEVICE_REPORTS)
    installers = names[:5] + [name.replace(f'-{version}-', f'-{version}-offline-') for name in names[:5]]
    names.extend(installers[5:])
    names.extend(['gandalf-model.json', 'gandalf-0.0.1-package.zip', 'gandalf-export-parity.json',
                  'gandalf-linux.json', 'gandalf-windows.json', 'gandalf-x86_64-apple-darwin.json', 'gandalf-aarch64-apple-darwin.json'])
    for name in names:
        path = directory / name
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing or empty release asset: {name}")
    for name in installers:
        path = directory / name
        metadata = json.loads(Path(str(path) + ".json").read_text(encoding="utf-8"))
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if metadata["version"] != version or metadata["sha256"] != digest:
            raise ValueError(f"installer metadata mismatch: {name}")
        if metadata["signed"] is not False:
            raise ValueError("installer metadata must truthfully report unsigned artifacts")
        if metadata.get('native_inference') is not True or metadata.get('model_included') != ('-offline-' in name):
            raise ValueError('installer is missing its required native backend or offline model')
    verify_onboarding(directory)
    verify_devices(directory)
    descriptor = json.loads((directory / 'gandalf-model.json').read_text(encoding="utf-8"))
    manifest = descriptor['manifest']
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PublicKey
    key = bytes.fromhex((ROOT / 'src/runtime/gandalf-public-key.hex').read_text(encoding="utf-8").strip())
    Ed25519PublicKey.from_public_bytes(key).verify(bytes.fromhex(descriptor['signature']), json.dumps(manifest, sort_keys=True, separators=(',', ':'), ensure_ascii=False).encode())
    if manifest['id'] != 'gandalf' or manifest['license'] != 'MIT': raise ValueError('incorrect default model metadata')
    for name, asset in manifest['files'].items():
        path = directory / asset['url'].rsplit('/', 1)[-1]
        if not path.is_file() or path.stat().st_size != asset['bytes'] or hashlib.sha256(path.read_bytes()).hexdigest() != asset['sha256']:
            raise ValueError('published model assets differ from signed manifest')
    parity = json.loads((directory / 'gandalf-export-parity.json').read_text(encoding="utf-8"))
    if parity['onnx']['passed'] is not True or parity['onnx']['sha256'] != manifest['files']['model.onnx']['sha256']:
        raise ValueError('model export parity gate did not pass for these weights')
    for name in ['gandalf-linux.json', 'gandalf-windows.json', 'gandalf-x86_64-apple-darwin.json', 'gandalf-aarch64-apple-darwin.json']:
        report = json.loads((directory / name).read_text(encoding="utf-8"))
        if not all(report.get(check) is True for check in ('signed_offline_provisioning','default_policy_scoring','custom_model_selection','unknown_model_recovery','token_limit_reported','tampered_descriptor_rejected','native_torch_parity')):
            raise ValueError('native model qualification did not pass')
    for name, target in updates.items():
        path = directory / name
        if path.stat().st_size > 128 * 1024 * 1024:
            raise ValueError(f'update payload too large: {name}')
        verify_binary(path, target)
    # Reject unexpected directories and compute an integrity manifest of every asset.
    entries = []
    for path in sorted(directory.iterdir()):
        if path.name == "SHA256SUMS":
            continue
        if not path.is_file() or path.is_symlink():
            raise ValueError(f"invalid release asset: {path.name}")
        entries.append(f"{hashlib.sha256(path.read_bytes()).hexdigest()}  {path.name}\n")
    (directory / "SHA256SUMS").write_text("".join(entries))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag")
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--downloads", type=Path)
    args = parser.parse_args()
    version = release_version()
    if args.tag and args.tag != "v" + version:
        parser.error(f"tag must match runtime/SDK version: v{version}")
    if args.downloads:
        if not args.assets:
            parser.error("--downloads requires --assets")
        stage_assets(args.downloads, args.assets)
    if args.assets:
        verify_assets(args.assets, version)
    print(f"Runtime and SDK release {version} validated")


if __name__ == "__main__":
    main()
