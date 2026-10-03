"""Build Python wheel/sdist, npm tarball and a tracked Rust SDK source archive."""
import argparse
from pathlib import Path
import subprocess
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
    print(f"Built SDK packages {version} in {output}")


if __name__ == "__main__":
    main()
