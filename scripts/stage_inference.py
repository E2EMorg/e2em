"""Stage native CPU inference; Python is used only while building installers."""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import sys


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.find_spec('onnxruntime')
    if spec is None:
        parser.error('install the pinned CPU onnxruntime wheel in the build environment')
    root = Path(spec.origin).parent
    capi = root / 'capi'
    windows = sys.platform == 'win32'
    library = 'onnxruntime.dll' if windows else 'libonnxruntime.dylib' if sys.platform == 'darwin' else 'libonnxruntime.so'
    matches = [p for p in capi.glob('onnxruntime.dll' if windows else 'libonnxruntime*.dylib' if sys.platform == 'darwin' else 'libonnxruntime.so*') if p.is_file()]
    if len(matches) != 1:
        parser.error('expected exactly one matching ONNX Runtime library')
    shutil.copyfile(args.worker, args.output / ('e2em-inference.exe' if windows else 'e2em-inference'))
    shutil.copyfile(matches[0], args.output / library)
    for path in capi.glob('*providers_shared*'):
        if path.is_file(): shutil.copyfile(path, args.output / path.name)
    for name in ('LICENSE', 'ThirdPartyNotices.txt'):
        if not (root / name).is_file(): parser.error('ONNX Runtime wheel is missing licence notices')
        shutil.copyfile(root / name, args.output / ('ONNXRUNTIME-' + name))
    if windows:
        # ONNX Runtime's DLL needs the VC runtime even when the Rust worker is
        # statically linked. Ship the redistributable CRT beside both binaries.
        redists = []
        for variable in ('ProgramFiles', 'ProgramFiles(x86)'):
            if os.environ.get(variable):
                redists.extend((Path(os.environ[variable]) / 'Microsoft Visual Studio/2022').glob('*/VC/Redist/MSVC/*/x64/Microsoft.VC143.CRT'))
        if not redists: parser.error('Visual Studio x64 redistributable CRT is required to stage a self-contained Windows backend')
        crt = sorted(redists)[-1]
        for path in crt.glob('*.dll'): shutil.copyfile(path, args.output / path.name)
        (args.output / 'MSVC-NOTICE.txt').write_text('Microsoft Visual C++ Runtime redistributable files. Copyright Microsoft Corporation. Distributed under the Visual Studio Distributable Code terms: https://aka.ms/vs/17/redistribution\n')
    if not windows:
        (args.output / 'e2em-inference').chmod(0o755)
    print('Staged native worker and CPU ONNX Runtime with licence notices')


if __name__ == '__main__': main()
