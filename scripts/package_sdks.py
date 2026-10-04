"""Build Python wheel/sdist, npm tarball and tracked Dart/Rust SDK source archives."""
import argparse
from pathlib import Path
import subprocess
import tarfile
from check_release import release_version

ROOT = Path(__file__).resolve().parents[1]


def main():
    import sys
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    version = release_version()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("SDK output directory must be empty")
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run([sys.executable, "-m", "build", "--outdir", str(output), str(ROOT / "sdk/python")], check=True)
    subprocess.run(["npm", "pack", "--pack-destination", str(output)], cwd=ROOT / "sdk/node", check=True)
    subprocess.run(["git", "archive", "--format=tar.gz", f"--prefix=e2em-rust-sdk-{version}/",
                    "-o", str(output / f"e2em-rust-sdk-{version}.tar.gz"), "HEAD"], cwd=ROOT, check=True)
    dart_files = subprocess.run(['git', 'ls-files', '-z', 'sdk/dart'], cwd=ROOT,
                                check=True, capture_output=True).stdout.decode().split('\0')
    if not any(name.startswith('sdk/dart/lib/src/') for name in dart_files):
        raise ValueError('Dart SDK implementation must be tracked before packaging')
    with tarfile.open(output / f'e2em-dart-sdk-{version}.tar.gz', 'w:gz') as archive:
        for name in sorted(filter(None, dart_files)):
            path = ROOT / name
            if path.is_symlink() or not path.is_file():
                raise ValueError(f'invalid Dart SDK source file: {name}')
            archive.add(path, arcname=f'e2em-dart-sdk-{version}/{path.relative_to(ROOT / "sdk/dart")}', recursive=False)
    print(f"Built SDK packages {version} in {output}")


if __name__ == "__main__":
    main()
