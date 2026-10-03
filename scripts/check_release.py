"""Validate synchronized release versions and the complete publication payload."""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def release_version(root=ROOT):
    versions = {}
    for name in ("Cargo.toml", "crates/e2em-ffi/Cargo.toml", "crates/e2em-platform/Cargo.toml"):
        versions[name] = tomllib.loads((root / name).read_text())["package"]["version"]
    versions["sdk/python/pyproject.toml"] = tomllib.loads((root / "sdk/python/pyproject.toml").read_text())["project"]["version"]
    versions["sdk/node/package.json"] = json.loads((root / "sdk/node/package.json").read_text())["version"]
    if len(set(versions.values())) != 1:
        raise ValueError(f"runtime and SDK versions differ: {versions}")
    version = versions["Cargo.toml"]
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", version):
        raise ValueError("release version must be MAJOR.MINOR.PATCH")
    return version


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
        *[f"e2em-c-sdk-{version}-{target}.zip" for target in (
            "x86_64-unknown-linux-gnu", "x86_64-pc-windows-msvc",
            "x86_64-apple-darwin", "aarch64-apple-darwin")],
        "deb.json", "rpm.json", "msi.json", "pkg-x86_64-apple-darwin.json",
        "pkg-aarch64-apple-darwin.json", "e2em-windows.json",
        "e2em-macos-macos-15-intel.json", "e2em-macos-macos-15.json",
    ]
    for name in names:
        path = directory / name
        if not path.is_file() or not path.stat().st_size:
            raise ValueError(f"missing or empty release asset: {name}")
    for name in names[:5]:
        path = directory / name
        metadata = json.loads(Path(str(path) + ".json").read_text())
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        if metadata["version"] != version or metadata["sha256"] != digest:
            raise ValueError(f"installer metadata mismatch: {name}")
        if metadata["signed"] is not False:
            raise ValueError("preview metadata must report unsigned artifacts")
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
    args = parser.parse_args()
    version = release_version()
    if args.tag and args.tag != "v" + version:
        parser.error(f"tag must match runtime/SDK version: v{version}")
    if args.assets:
        verify_assets(args.assets, version)
    print(f"Runtime and SDK release {version} validated")


if __name__ == "__main__":
    main()
