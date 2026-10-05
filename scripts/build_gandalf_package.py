"""Build a signed, verified Gandalf deployment package for runtime installers.

Python/PyTorch are build-time tools only. Download the immutable research data,
export with parity checks, retain upstream notices and apply the owner's MIT
licence grant. No code is downloaded from the model repository.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
REVISION = "7477acb03721e7c59cce1e12f2163c0ad33cfbfa"
REPO = "https://huggingface.co/krazyjakee/gandalf"
VERSION = "0.0.1"
WEIGHTS = "916370da013a428a8ed3e00791049d12b3b3fd5836969d8b0492bc5de372d5c8"
FILES = ["model.safetensors", "config.json", "tokenizer.json", "tokenizer_config.json", "preset.json", "distillation.json", "ATTRIBUTION.md", "BASE_MODEL_LICENSE.txt"]


def digest(path):
    with path.open("rb") as source:
        return hashlib.file_digest(source, "sha256").hexdigest()


def fetch(url, path, limit):
    if not url.startswith("https://"):
        raise ValueError("model sources require HTTPS")
    with urllib.request.urlopen(url, timeout=120) as response, path.open("xb") as output:
        if not response.url.startswith("https://"):
            raise ValueError("model redirect requires HTTPS")
        size = 0
        while chunk := response.read(65536):
            size += len(chunk)
            if size > limit:
                raise ValueError("model asset exceeds advertised size")
            output.write(chunk)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--runtime-version", default="0.1.3")
    parser.add_argument("--snapshot", type=Path)
    parser.add_argument("--onnx", type=Path, help="reuse an export already checked with probe_gandalf.py")
    parser.add_argument("--signing-key", type=Path)
    args = parser.parse_args()
    output = args.output.resolve()
    if output.exists() and any(output.iterdir()):
        parser.error("model output directory must be empty")
    output.mkdir(parents=True, exist_ok=True)
    from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
    from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat

    raw = args.signing_key.read_bytes() if args.signing_key else bytes.fromhex(os.environ["E2EM_MODEL_SIGNING_KEY"])
    private = Ed25519PrivateKey.from_private_bytes(raw)
    expected = (ROOT / "src/runtime/gandalf-public-key.hex").read_text(encoding="utf-8").strip()
    if private.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw).hex() != expected:
        parser.error("release signing key does not match runtime trust root")
    with tempfile.TemporaryDirectory(prefix="e2em-model-build-") as temporary:
        work = Path(temporary)
        snapshot = args.snapshot.resolve() if args.snapshot else work / "source"
        if not args.snapshot:
            snapshot.mkdir()
            fetch(f"{REPO}/resolve/{REVISION}/release.json", snapshot / "release.json", 65536)
            release = json.loads((snapshot / "release.json").read_text(encoding="utf-8"))
            if release["model_weights_sha256"] != WEIGHTS:
                raise ValueError("research release differs from approved Gandalf checkpoint")
            for name in FILES:
                asset = release["files"][name]
                fetch(f"{REPO}/resolve/{REVISION}/{name}", snapshot / name, asset["bytes"])
                if (snapshot / name).stat().st_size != asset["bytes"] or digest(snapshot / name) != asset["sha256"]:
                    raise ValueError("source asset verification failed")
        if digest(snapshot / "model.safetensors") != WEIGHTS:
            raise ValueError("incorrect Gandalf weights")
        package = output / "package"
        package.mkdir()
        if args.onnx:
            shutil.copyfile(args.onnx, package / "model.onnx")
        else:
            subprocess.run([sys.executable, str(ROOT / "scripts/probe_gandalf.py"),
                            "--model-path", str(snapshot), "--onnx-output", str(package / "model.onnx"),
                            "--report", str(output / "gandalf-export-parity.json")], check=True)
        for name in ["tokenizer.json", "preset.json", "ATTRIBUTION.md", "BASE_MODEL_LICENSE.txt"]:
            shutil.copyfile(snapshot / name, package / name)
        shutil.copyfile(ROOT / "LICENSE", package / "LICENSE")
        # Preserve all provenance separately from the explicit owner licence grant.
        (package / "PROVENANCE.json").write_text(json.dumps({
            "source": REPO, "revision": REVISION, "version": VERSION,
            "weights_sha256": WEIGHTS, "owner_license": "MIT",
            "runtime_format": "FP32 ONNX", "max_tokens": 512, "tail_tokens": 64,
        }, indent=2) + "\n")
        assets = {}
        for path in sorted(package.iterdir()):
            name = f"gandalf-{VERSION}-{path.name}"
            assets[path.name] = {"bytes": path.stat().st_size, "sha256": digest(path),
                                 "url": f"https://github.com/E2EMorg/e2em/releases/download/v{args.runtime_version}/{name}"}
            shutil.copyfile(path, output / name)
        manifest = {"format": "e2em-onnx-policy-cross-encoder-v1", "id": "gandalf", "version": VERSION,
                    "source": REPO, "source_revision": REVISION, "license": "MIT", "max_tokens": 512,
                    "tail_tokens": 64, "evidence_format": "target-first", "files": assets}
        canonical = json.dumps(manifest, sort_keys=True, separators=(",", ":"), ensure_ascii=False).encode()
        descriptor = {"manifest": manifest, "signature": private.sign(canonical).hex()}
        encoded = json.dumps(descriptor, indent=2) + "\n"
        (package / "model.json").write_text(encoded)
        (output / "gandalf-model.json").write_text(encoded)
        with zipfile.ZipFile(output / f"gandalf-{VERSION}-package.zip", "w", compression=zipfile.ZIP_DEFLATED) as archive:
            for path in sorted(package.iterdir()):
                archive.write(path, path.name)
    print(f"Built signed Gandalf {VERSION} model package")


if __name__ == "__main__":
    main()
