"""Stage standalone daemon payloads for the idle background updater."""
import argparse
from pathlib import Path
import shutil
import tomllib

from package_runtime import ROOT, TARGETS, verify_binary, version_value

UPDATE_TARGETS = tuple(target for target in TARGETS if target != 'aarch64-unknown-linux-musl')


def package(binary, target, output, version):
    version = version_value(version)
    if target not in UPDATE_TARGETS:
        raise ValueError('unsupported update target')
    if binary.is_symlink() or not binary.is_file() or not 0 < binary.stat().st_size <= 128 * 1024 * 1024:
        raise ValueError('update payload must be a bounded regular executable')
    verify_binary(binary, target)
    output.mkdir(parents=True, exist_ok=True)
    suffix = '.exe' if 'windows' in target else '.bin'
    destination = output / f'e2em-update-{version}-{target}{suffix}'
    # Exclusive creation prevents accidental replacement of a published payload.
    with binary.open('rb') as source, destination.open('xb') as sink:
        shutil.copyfileobj(source, sink)
    destination.chmod(0o755)
    return destination


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--target', choices=UPDATE_TARGETS, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--version', default=tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding="utf-8"))['package']['version'])
    args = parser.parse_args()
    print(package(args.binary, args.target, args.output, args.version))


if __name__ == '__main__':
    main()
