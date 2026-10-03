"""Package a native C SDK with headers, shared/static libraries and MIT license."""
import argparse
from pathlib import Path
import zipfile
from check_release import release_version

ROOT = Path(__file__).resolve().parents[1]
LIBRARIES = {
    "x86_64-unknown-linux-gnu": ["libe2em_ffi.so", "libe2em_ffi.a"],
    "x86_64-apple-darwin": ["libe2em_ffi.dylib", "libe2em_ffi.a"],
    "aarch64-apple-darwin": ["libe2em_ffi.dylib", "libe2em_ffi.a"],
    "x86_64-pc-windows-msvc": ["e2em_ffi.dll", "e2em_ffi.dll.lib", "e2em_ffi.lib"],
}


def package(target, library_dir, output):
    version = release_version()
    sources = [(library_dir / name, "lib/" + name) for name in LIBRARIES[target]]
    sources += [(ROOT / "crates/e2em-ffi/include/e2em.h", "include/e2em.h"),
                (ROOT / "LICENSE", "LICENSE"),
                (ROOT / "crates/e2em-ffi/README.md", "README.md")]
    sources += [(path, "examples/" + path.name) for path in sorted((ROOT / "crates/e2em-ffi/examples").glob("*"))]
    for source, _ in sources:
        if not source.is_file():
            raise ValueError(f"missing native SDK input: {source}")
    output.mkdir(parents=True, exist_ok=True)
    archive = output / f"e2em-c-sdk-{version}-{target}.zip"
    with zipfile.ZipFile(archive, "x", compression=zipfile.ZIP_DEFLATED) as bundle:
        for source, name in sources:
            bundle.write(source, name)
    return archive


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=LIBRARIES, required=True)
    parser.add_argument("--library-dir", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    print(package(args.target, args.library_dir, args.output))


if __name__ == "__main__":
    main()
