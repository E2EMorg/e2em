"""Bounded native CPU/GPU parity and actual ONNX node execution evidence."""
import argparse
from collections import Counter
import json
import os
from pathlib import Path
import platform
import subprocess
import tempfile


def probe(worker, library, model, device, directory, extra_env=None):
    env = os.environ.copy()
    if platform.system() == 'Linux':
        env['LD_LIBRARY_PATH'] = str(library.parent) + os.pathsep + env.get('LD_LIBRARY_PATH', '')
    env.update(extra_env or {})
    result = subprocess.run([str(worker), '--model', str(model), '--library', str(library),
                             '--device', device, '--device-status', '--profile', str(directory / device)],
                            env=env, capture_output=True, text=True, timeout=60, check=True)
    value = json.loads(result.stdout)
    events = json.loads(Path(value.pop('profile')).read_text())
    value['executed_nodes'] = dict(Counter(event.get('args', {}).get('provider') for event in events
                                         if event.get('cat') == 'Node' and event.get('args', {}).get('provider')))
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--worker', required=True, type=Path)
    parser.add_argument('--library', required=True, type=Path)
    parser.add_argument('--model', required=True, type=Path)
    parser.add_argument('--output', required=True, type=Path)
    parser.add_argument('--require-gpu', action='store_true')
    args = parser.parse_args()
    worker, library, model = args.worker.resolve(), args.library.resolve(), args.model.resolve()
    with tempfile.TemporaryDirectory(prefix='e2em-device-check-') as temporary:
        directory = Path(temporary)
        cpu = probe(worker, library, model, 'cpu', directory)
        auto = probe(worker, library, model, 'auto', directory)
        gpu_nodes = sum(count for provider, count in auto['executed_nodes'].items() if provider != 'CPUExecutionProvider')
        error = abs(cpu['probe_probability'] - auto['probe_probability'])
        assert error < 2e-5, f'CPU/accelerator probability parity error: {error}'
        if args.require_gpu:
            assert auto['selected_provider'] != 'CPU' and gpu_nodes > 0, auto
        report = {'platform': platform.platform(), 'cpu': cpu, 'auto': auto,
                  'accelerator_nodes_executed': gpu_nodes, 'max_probability_error': error,
                  'gpu_execution_verified': gpu_nodes > 0,
                  'limits': 'One fixed native scoring probe. No category-quality, throughput, full-length GPU memory or cross-platform GPU qualification.'}
        if platform.system() == 'Linux' and auto['selected_provider'] == 'CUDA':
            # Driver ordinals honour visibility, including an empty allocation.
            disabled = probe(worker, library, model, 'auto', directory, {'CUDA_VISIBLE_DEVICES': ''})
            assert disabled['selected_provider'] == 'CPU', disabled
            assert abs(cpu['probe_probability'] - disabled['probe_probability']) < 2e-5
            report['no_visible_gpu_fallback'] = disabled
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(report, indent=2) + '\n', encoding='utf-8')
        print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
