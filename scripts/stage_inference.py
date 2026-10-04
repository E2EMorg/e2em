"""Stage native accelerated inference; Python is used only while building installers."""
import argparse
import importlib.metadata
import importlib.util
import json
import os
from pathlib import Path
import shutil
import sys


def stage_libraries(capi, output, windows):
    # Windows includes DirectML.dll. Linux provider DSOs must travel with the
    # main library; copying providers_shared alone leaves CUDA unusable.
    for path in capi.iterdir():
        native = path.suffix.lower() == '.dll' if windows else (
            'providers_' in path.name and '.so' in path.name and 'tensorrt' not in path.name)
        if native and path.is_file():
            shutil.copyfile(path, output / path.name)


NVIDIA_COMPONENTS = (
    'nvidia-cublas-cu12', 'nvidia-cuda-runtime-cu12', 'nvidia-cuda-nvrtc-cu12',
    'nvidia-cudnn-cu12', 'nvidia-cufft-cu12', 'nvidia-curand-cu12', 'nvidia-nvjitlink-cu12',
)


def stage_cuda_dependencies(output, distribution=importlib.metadata.distribution):
    versions = {}
    for name in NVIDIA_COMPONENTS:
        package = distribution(name)
        libraries = []; notices = []
        for entry in package.files or ():
            path = Path(package.locate_file(entry))
            if '/lib/' in str(entry) and '.so' in path.name and path.is_file():
                libraries.append(path)
            elif path.name.lower() in ('license.txt', 'license') and path.is_file():
                notices.append(path)
        if not libraries or not notices:
            raise ValueError(f'{name} must include native libraries and licence notices')
        for path in libraries:
            shutil.copyfile(path, output / path.name)
        for index, path in enumerate(notices):
            shutil.copyfile(path, output / f'NVIDIA-{name}-LICENSE-{index}.txt')
        versions[name] = package.version
    return versions


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--require-provider', action='append', default=[])
    parser.add_argument('--bundle-cuda', action='store_true')
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    spec = importlib.util.find_spec('onnxruntime')
    if spec is None:
        parser.error('install the pinned platform ONNX Runtime wheel in the build environment')
    import onnxruntime
    providers = onnxruntime.get_available_providers()
    missing = set(args.require_provider) - set(providers)
    if missing:
        parser.error(f'ONNX Runtime is missing required providers: {sorted(missing)}')
    root = Path(spec.origin).parent
    capi = root / 'capi'
    windows = sys.platform == 'win32'
    library = 'onnxruntime.dll' if windows else 'libonnxruntime.dylib' if sys.platform == 'darwin' else 'libonnxruntime.so'
    matches = [p for p in capi.glob('onnxruntime.dll' if windows else 'libonnxruntime*.dylib' if sys.platform == 'darwin' else 'libonnxruntime.so*') if p.is_file()]
    if len(matches) != 1:
        parser.error('expected exactly one matching ONNX Runtime library')
    shutil.copyfile(args.worker, args.output / ('e2em-inference.exe' if windows else 'e2em-inference'))
    shutil.copyfile(matches[0], args.output / library)
    stage_libraries(capi, args.output, windows)
    components = {}
    if args.bundle_cuda:
        if sys.platform != 'linux' or 'CUDAExecutionProvider' not in providers:
            parser.error('CUDA dependency bundling requires the Linux CUDA ONNX Runtime wheel')
        components = stage_cuda_dependencies(args.output)
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
    (args.output / 'inference-backends.json').write_text(json.dumps({
        'onnxruntime': onnxruntime.__version__, 'available_providers': providers,
        'native_components': components,
    }, indent=2) + '\n', encoding='utf-8')
    print(f'Staged native worker and ONNX Runtime providers {providers} with licence notices')


if __name__ == '__main__': main()
