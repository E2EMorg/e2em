"""Provision the private model store before user setup completes."""
from pathlib import Path
import os
import shutil
import subprocess


def arguments(parser):
    parser.add_argument('--rules-only', action='store_true', help='explicit diagnostic installation without models')
    parser.add_argument('--model-worker', type=Path)
    parser.add_argument('--model-library', type=Path)
    parser.add_argument('--model-source', help='local deployment package or HTTPS descriptor; defaults to Gandalf')
    parser.add_argument('--offline', action='store_true', help='disable all network access, including updates')


def provision(args, executable, config):
    if args.rules_only: return
    executable = Path(executable).resolve()
    platform = 'macos' if os.uname().sysname == 'Darwin' else 'linux'
    backend = executable.parent if platform == 'macos' else Path('/usr/libexec/e2em')
    worker = (args.model_worker or backend / 'e2em-inference').resolve()
    library = (args.model_library or backend / ('libonnxruntime.dylib' if platform == 'macos' else 'libonnxruntime.so')).resolve()
    bundled = Path('/usr/local/share/e2em' if platform == 'macos' else '/usr/share/e2em') / 'models/gandalf'
    source = args.model_source or (str(bundled) if bundled.is_dir() else 'gandalf')
    if args.offline and not Path(source).is_dir():
        raise ValueError('offline setup needs an offline installer or --model-source local-package')
    command = [str(executable), '--grants', str(config / 'grants.json')]
    if args.offline: command += ['--offline']
    model_config = config / 'models.json'
    if not model_config.exists():
        subprocess.run([*command, '--model-init', '--model-worker', str(worker), '--model-library', str(library)], check=True)
    subprocess.run([*command, '--model-install', source, *(['--model-no-update'] if args.offline or args.no_auto_update else [])], check=True)


def remove_models(config):
    root = config / 'models'
    if root.exists():
        # Never follow a link out of the owner's model store.
        from install_runtime import private_dir
        private_dir(root)
        for path in root.iterdir():
            if path.name == 'manager.lock' and path.is_file(): path.unlink()
            elif path.is_dir() and not path.is_symlink() and ((path / 'model.json').is_file() or path.name.endswith('.staging')):
                shutil.rmtree(path)
        if not any(root.iterdir()): root.rmdir()
    (config / 'models.json').unlink(missing_ok=True)
