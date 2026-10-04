"""Native accelerators must be usable without Python on the installed machine."""
import importlib.metadata
from pathlib import Path
import sys
import tempfile
from types import SimpleNamespace
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import stage_inference


class NativeLibraries(unittest.TestCase):
    def test_provider_dsos_and_directml_are_staged_without_python_extensions(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); capi = root / 'capi'; capi.mkdir()
            for name in ('libonnxruntime_providers_shared.so', 'libonnxruntime_providers_cuda.so',
                         'libonnxruntime_providers_tensorrt.so', 'onnxruntime_pybind11_state.so',
                         'onnxruntime.dll', 'DirectML.dll', 'onnxruntime_pybind11_state.pyd'):
                (capi / name).write_bytes(b'native fixture')
            linux = root / 'linux'; linux.mkdir()
            stage_inference.stage_libraries(capi, linux, False)
            self.assertEqual({p.name for p in linux.iterdir()}, {
                'libonnxruntime_providers_shared.so', 'libonnxruntime_providers_cuda.so'})
            windows = root / 'windows'; windows.mkdir()
            stage_inference.stage_libraries(capi, windows, True)
            self.assertEqual({p.name for p in windows.iterdir()}, {'onnxruntime.dll', 'DirectML.dll'})

    def test_cuda_libraries_and_each_components_licence_are_required(self):
        with tempfile.TemporaryDirectory() as temporary:
            root = Path(temporary); output = root / 'output'; output.mkdir()
            packages = {}
            for name in stage_inference.NVIDIA_COMPONENTS:
                library = Path('nvidia') / name / 'lib' / f'lib{name}.so.12'
                notice = Path(name + '.dist-info') / 'licenses/License.txt'
                for entry in (library, notice):
                    (root / entry).parent.mkdir(parents=True, exist_ok=True)
                    (root / entry).write_bytes(b'fixture')
                packages[name] = SimpleNamespace(files=[library, notice], version='1.2.3',
                                                locate_file=lambda entry: root / entry)
            versions = stage_inference.stage_cuda_dependencies(output, packages.__getitem__)
            self.assertEqual(set(versions), set(stage_inference.NVIDIA_COMPONENTS))
            self.assertEqual(len(list(output.iterdir())), 2 * len(packages))
            packages[stage_inference.NVIDIA_COMPONENTS[0]].files.pop()
            with self.assertRaisesRegex(ValueError, 'licence notices'):
                stage_inference.stage_cuda_dependencies(output, packages.__getitem__)


if __name__ == '__main__':
    unittest.main()
