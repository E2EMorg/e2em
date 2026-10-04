"""Build unsigned runtime DEB/RPM/PKG/MSI artifacts with native packaging tools.

Normal installers contain the native backend; offline installers also contain
Gandalf. Package managers own executables; user setup provisions private models.
"""
import argparse
import hashlib
import json
import plistlib
from pathlib import Path
import re
import shutil
import struct
import subprocess
import tempfile
import tomllib
import uuid
import xml.etree.ElementTree as ET

ROOT = Path(__file__).resolve().parents[1]
TARGETS = {
    "x86_64-unknown-linux-musl": ("linux", "amd64", "x86_64"),
    "aarch64-unknown-linux-musl": ("linux", "arm64", "aarch64"),
    "x86_64-apple-darwin": ("macos", "x86_64", "x86_64"),
    "aarch64-apple-darwin": ("macos", "arm64", "aarch64"),
    "x86_64-pc-windows-msvc": ("windows", "x64", "x86_64"),
}
UPGRADE_CODE = "9AC5C583-6E53-4C30-9F66-B435D8C9A40E"
WIX_NS = "http://wixtoolset.org/schemas/v4/wxs"
WIX_UI_NS = "http://wixtoolset.org/schemas/v4/wxs/ui"

LINUX_DESKTOP = """[Desktop Entry]
Type=Application
Name=E2EM Setup
Comment=Set up private message assessment and check runtime status
Exec=/usr/bin/e2emd --setup
Icon=e2em
Terminal=false
Categories=Settings;Utility;
StartupNotify=false
"""
SETUP_ICON = '<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128" viewBox="0 0 128 128"><rect width="128" height="128" rx="28" fill="#191b29"/><path d="M35 30h58v14H51v13h35v14H51v13h42v14H35z" fill="#ba9dff"/></svg>'


def version_value(value):
    if not re.fullmatch(r"(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)", value):
        raise ValueError("package version must be MAJOR.MINOR.PATCH")
    major, minor, patch = map(int, value.split('.'))
    if major > 255 or minor > 255 or patch > 65535:
        raise ValueError("package version exceeds Windows Installer limits")
    return value


def digest(path):
    with path.open('rb') as source:
        return hashlib.file_digest(source, 'sha256').hexdigest()


def verify_binary(binary, target):
    platform, _, arch = TARGETS[target]
    with binary.open('rb') as source:
        header = source.read(64)
        if platform == 'linux':
            machine = 62 if arch == 'x86_64' else 183
            if len(header) < 64 or header[:6] != b'\x7fELF\x02\x01' or struct.unpack_from('<H', header, 18)[0] != machine:
                raise ValueError('runtime must be a matching 64-bit ELF executable')
            offset = struct.unpack_from('<Q', header, 32)[0]
            size, count = struct.unpack_from('<HH', header, 54)
            if not 56 <= size <= 256 or not 0 < count <= 128:
                raise ValueError('invalid ELF program headers')
            source.seek(offset)
            for _ in range(count):
                program = source.read(size)
                if len(program) != size or struct.unpack_from('<I', program)[0] == 3:
                    raise ValueError('Linux package requires a static musl executable')
        elif platform == 'macos':
            cpu = 0x01000007 if arch == 'x86_64' else 0x0100000c
            if len(header) < 8 or header[:4] != b'\xcf\xfa\xed\xfe' or struct.unpack_from('<I', header, 4)[0] != cpu:
                raise ValueError('runtime must be a matching 64-bit Mach-O executable')
        else:
            if len(header) < 64 or header[:2] != b'MZ':
                raise ValueError('runtime must be a Windows PE executable')
            source.seek(struct.unpack_from('<I', header, 60)[0])
            if source.read(6) != b'PE\x00\x00\x64\x86':
                raise ValueError('runtime must be a Windows x64 executable')


def copy(source, destination, mode=0o644):
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(source, destination)
    destination.chmod(mode)


def stage_payload(binary, platform, stage, inference_dir=None, model_package=None, setup_binary=None):
    if platform == 'windows':
        copy(binary, stage / 'e2emd.exe', 0o755)
        copy(ROOT / 'scripts/install_windows_runtime.ps1', stage / 'install_windows_runtime.ps1')
        if not setup_binary: raise ValueError('Windows packages require the native e2em-setup GUI launcher')
        copy(setup_binary, stage / 'e2em-setup.exe', 0o755)
        docs = stage
    else:
        prefix = 'usr' if platform == 'linux' else 'usr/local'
        binary_path = 'bin/e2emd' if platform == 'linux' else 'libexec/e2em/e2emd'
        copy(binary, stage / prefix / binary_path, 0o755)
        docs = stage / prefix / 'share/e2em'
        copy(ROOT / 'scripts/install_runtime.py', docs / 'install_runtime.py')
        if platform == 'macos':
            copy(ROOT / 'scripts/install_macos_runtime.py', docs / 'install_macos_runtime.py')
            bundle = stage / 'Applications/E2EM Setup.app/Contents'
            (bundle / 'MacOS').mkdir(parents=True)
            (bundle / 'Info.plist').write_bytes(plistlib.dumps({
                'CFBundleIdentifier': 'org.e2em.setup', 'CFBundleName': 'E2EM Setup',
                'CFBundleDisplayName': 'E2EM Setup', 'CFBundleExecutable': 'E2EM Setup',
                'CFBundlePackageType': 'APPL', 'CFBundleVersion': '1',
                'LSUIElement': True,
            }))
            launcher = bundle / 'MacOS/E2EM Setup'
            launcher.write_text('#!/bin/sh\nexec /usr/local/libexec/e2em/e2emd --setup\n', encoding='utf-8')
            launcher.chmod(0o755)
        else:
            desktop = stage / 'usr/share/applications/org.e2em.Setup.desktop'
            desktop.parent.mkdir(parents=True)
            desktop.write_text(LINUX_DESKTOP, encoding='utf-8')
            icon = stage / 'usr/share/icons/hicolor/scalable/apps/e2em.svg'
            icon.parent.mkdir(parents=True)
            icon.write_text(SETUP_ICON, encoding='utf-8')
    copy(ROOT / 'LICENSE', docs / 'LICENSE')
    copy(ROOT / 'docs/runtime/PACKAGING.md', docs / 'PACKAGING.md')
    copy(ROOT / 'docs/runtime/DESKTOP.md', docs / 'DESKTOP.md')
    if inference_dir:
        backend = stage if platform == 'windows' else stage / ('usr' if platform == 'linux' else 'usr/local') / 'libexec/e2em'
        worker = 'e2em-inference.exe' if platform == 'windows' else 'e2em-inference'
        library = 'onnxruntime.dll' if platform == 'windows' else 'libonnxruntime.dylib' if platform == 'macos' else 'libonnxruntime.so'
        for name in (worker, library, 'ONNXRUNTIME-LICENSE', 'ONNXRUNTIME-ThirdPartyNotices.txt'):
            if not (inference_dir / name).is_file(): raise ValueError('incomplete native inference payload')
        for path in inference_dir.iterdir():
            if not path.is_file() or path.is_symlink(): raise ValueError('invalid native inference asset')
            copy(path, backend / path.name, 0o755 if path.name == worker else 0o644)
        copy(ROOT / 'docs/runtime/MODELS.md', docs / 'MODELS.md')
        if platform != 'windows': copy(ROOT / 'scripts/setup_models.py', docs / 'setup_models.py')
    if model_package:
        descriptor = json.loads((model_package / 'model.json').read_text(encoding="utf-8"))
        if descriptor['manifest']['id'] != 'gandalf' or descriptor['manifest']['license'] != 'MIT':
            raise ValueError('offline payload must contain the approved Gandalf package')
        for name, asset in descriptor['manifest']['files'].items():
            if Path(name).name != name: raise ValueError('model filename escapes package')
            path = model_package / name
            if path.stat().st_size != asset['bytes'] or digest(path) != asset['sha256']:
                raise ValueError('offline model asset failed verification')
            copy(path, docs / 'models/gandalf' / name)
        copy(model_package / 'model.json', docs / 'models/gandalf/model.json')


def deb_control(version, architecture, stage):
    size = sum(p.stat().st_size for p in stage.rglob('*') if p.is_file())
    return f"""Package: e2em-runtime
Version: {version}
Section: utils
Priority: optional
Architecture: {architecture}
Installed-Size: {(size + 1023) // 1024}
Depends: libc6 (>= 2.28), libstdc++6
Recommends: systemd, xdg-utils
Suggests: python3 (>= 3.11)
Maintainer: E2EM Project <noreply@e2em.org>
Homepage: https://e2em.org
Description: Local E2EM runtime with native Gandalf inference
 Per-user authenticated message assessment with custom model support.
 Open E2EM Setup from your applications menu to install the model and start the runtime.
"""


def rpm_spec(version, architecture):
    return f"""Name: e2em-runtime
Version: {version}
Release: 1
Summary: Local E2EM runtime with native Gandalf inference
License: MIT
URL: https://e2em.org
BuildArch: {architecture}
Recommends: systemd
Recommends: xdg-utils
Suggests: python3 >= 3.11
# Rust musl executable is static. Keep automatic dependency scanning enabled.
# The CUDA provider is loaded only when a usable GPU driver is present. Its
# optional driver dependency must not prevent installation on CPU-only hosts.
%global __requires_exclude ^libcuda[.]so[.]1($|[(])
%global debug_package %{{nil}}
%global __brp_strip %{{nil}}
%description
Per-user authenticated message assessment with custom model support.
Open E2EM Setup from your applications menu to install the model and start the runtime.
%prep
%build
%install
mkdir -p %{{buildroot}}/usr
cp -a %{{_sourcedir}}/payload/usr/. %{{buildroot}}/usr/
%files
%attr(0755,root,root) /usr/bin/e2emd
/usr/share/e2em
/usr/libexec/e2em
/usr/share/applications/org.e2em.Setup.desktop
/usr/share/icons/hicolor/scalable/apps/e2em.svg
"""


def wix_source(version, stage):
    ET.register_namespace('', WIX_NS)
    ET.register_namespace('ui', WIX_UI_NS)
    def element(parent, name, **attrs):
        return ET.SubElement(parent, f'{{{WIX_NS}}}{name}', attrs)
    wix = ET.Element(f'{{{WIX_NS}}}Wix')
    package = element(wix, 'Package', Name='E2EM Runtime', Manufacturer='E2EM Project',
                      Version=version, UpgradeCode=UPGRADE_CODE, Scope='perUser',
                      InstallerVersion='500', Language='1033')
    element(package, 'MajorUpgrade', DowngradeErrorMessage='A newer E2EM Runtime is installed.',
            Schedule='afterInstallInitialize', AllowSameVersionUpgrades='yes')
    element(package, 'MediaTemplate', EmbedCab='yes')
    ET.SubElement(package, f'{{{WIX_UI_NS}}}WixUI', {'Id': 'WixUI_Minimal'})
    license_file = stage / 'setup-license.rtf'
    license_text = (ROOT / 'LICENSE').read_text(encoding='utf-8').replace('\\', '\\\\').replace('{', '\\{').replace('}', '\\}').replace('\n', '\\par\n')
    license_file.write_text('{\\rtf1\\ansi\\deff0 ' + license_text + '}', encoding='utf-8')
    element(package, 'WixVariable', Id='WixUILicenseRtf', Value=str(license_file))
    element(package, 'Property', Id='WIXUI_EXITDIALOGOPTIONALCHECKBOXTEXT', Value='Open E2EM Setup to install the model and start E2EM')
    element(package, 'Property', Id='WIXUI_EXITDIALOGOPTIONALCHECKBOX', Value='1')
    element(package, 'CustomAction', Id='LaunchSetup', Directory='INSTALLFOLDER',
            ExeCommand='"[INSTALLFOLDER]e2em-setup.exe"', Return='asyncNoWait', Execute='immediate', Impersonate='yes')
    ui = element(package, 'UI')
    element(ui, 'Publish', Dialog='ExitDialog', Control='Finish', Event='DoAction', Value='LaunchSetup',
            Condition='WIXUI_EXITDIALOGOPTIONALCHECKBOX = 1 AND NOT Installed', Order='1')
    directory = element(package, 'StandardDirectory', Id='LocalAppDataFolder')
    folder = element(directory, 'Directory', Id='INSTALLFOLDER', Name='E2EM Runtime')
    feature = element(package, 'Feature', Id='Runtime', Title='E2EM Runtime', Level='1')
    programs = element(package, 'StandardDirectory', Id='ProgramMenuFolder')
    menu = element(programs, 'Directory', Id='E2EMMenu', Name='E2EM')
    shortcut = element(menu, 'Component', Id='SetupShortcut', Guid=str(uuid.uuid5(uuid.UUID(UPGRADE_CODE), 'setup-shortcut')).upper(), Bitness='always64')
    element(shortcut, 'Shortcut', Id='E2EMSetupShortcut', Name='E2EM Setup', Target='[INSTALLFOLDER]e2em-setup.exe',
            WorkingDirectory='INSTALLFOLDER')
    element(shortcut, 'RegistryValue', Root='HKCU', Key=r'Software\E2EM\Runtime', Name='SetupShortcut', Type='integer', Value='1', KeyPath='yes')
    element(shortcut, 'RemoveFolder', Id='RemoveE2EMMenu', On='uninstall')
    element(feature, 'ComponentRef', Id='SetupShortcut')
    folders = {'.': folder}
    remove_folders = set()
    for index, source in enumerate(sorted(p for p in stage.rglob('*') if p.is_file() and p != license_file)):
        relative = source.relative_to(stage)
        parent = folder
        prefix = Path()
        for part in relative.parent.parts:
            prefix /= part
            key = prefix.as_posix()
            if key not in folders:
                folders[key] = element(parent, 'Directory', Id='Dir' + hashlib.sha256(key.encode()).hexdigest()[:16], Name=part)
            parent = folders[key]
        # Per-user files need an HKCU registry keypath (ICE38). WiX cannot
        # auto-generate GUIDs for components combining files and registry keys.
        guid = str(uuid.uuid5(uuid.UUID(UPGRADE_CODE), 'x64:' + relative.as_posix())).upper()
        component = element(parent, 'Component', Id=f'Payload{index}', Guid=guid, Bitness='always64')
        for path in [relative.parent, *relative.parent.parents]:
            key = path.as_posix()
            if key != '.' and key not in remove_folders:
                element(component, 'RemoveFolder', Id='Remove' + hashlib.sha256(key.encode()).hexdigest()[:16], Directory=folders[key].get('Id'), On='uninstall')
                remove_folders.add(key)
        element(component, 'File', Id=f'File{index}', Source=str(source), KeyPath='no')
        element(component, 'RegistryValue', Root='HKCU', Key=r'Software\E2EM\Runtime',
                Name=relative.as_posix(), Type='integer', Value='1', KeyPath='yes')
        if index == 0:
            element(component, 'RegistryValue', Root='HKCU', Key=r'Software\E2EM\Runtime',
                    Name='Version', Type='string', Value=version)
            element(component, 'RemoveFolder', Id='RemoveRuntimeFolder', Directory='INSTALLFOLDER', On='uninstall')
        element(feature, 'ComponentRef', Id=f'Payload{index}')
    return ET.tostring(wix, encoding='unicode', xml_declaration=True)


def build(format_name, binary, target, output, version, stage_only=False, inference_dir=None, model_package=None, offline=False, setup_binary=None):
    version = version_value(version)
    platform, deb_arch, rpm_arch = TARGETS[target]
    if format_name not in {'linux': {'deb', 'rpm'}, 'macos': {'pkg'}, 'windows': {'msi'}}[platform]:
        raise ValueError('package format does not match the target')
    binary = binary.resolve(strict=True)
    if not binary.is_file() or binary.stat().st_size == 0:
        raise ValueError('provide a nonempty built runtime executable')
    verify_binary(binary, target)
    if platform == 'windows':
        if not setup_binary: raise ValueError('build e2em-setup and pass --setup-binary for Windows packages')
        setup_binary = setup_binary.resolve(strict=True)
        verify_binary(setup_binary, target)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    if offline and not model_package: raise ValueError('offline installer needs the verified Gandalf package')
    name = f'e2em-runtime-{version}{"-offline" if offline else ""}-{target}.{format_name}'
    artifact = output / name
    if artifact.exists() or (output / (name + '.json')).exists():
        raise ValueError('refusing to overwrite existing package artifacts')
    with tempfile.TemporaryDirectory(prefix='e2em-package-') as temporary:
        work = Path(temporary)
        stage = work / 'payload'
        stage.mkdir()
        stage_payload(binary, platform, stage, inference_dir, model_package if offline else None, setup_binary)
        pre_command = None
        if format_name == 'deb':
            (stage / 'DEBIAN').mkdir()
            (stage / 'DEBIAN/control').write_text(deb_control(version, deb_arch, stage))
            # CUDA libraries make the payload several gigabytes. A moderate
            # xz level keeps the broadly supported format with bounded build
            # time and decompression memory.
            command = ['dpkg-deb', '-Zxz', '-z3', '--build', '--root-owner-group', str(stage), str(artifact)]
        elif format_name == 'rpm':
            for folder in ['BUILD', 'BUILDROOT', 'RPMS', 'SOURCES', 'SPECS', 'SRPMS']:
                (work / folder).mkdir()
            shutil.copytree(stage, work / 'SOURCES/payload')
            spec = work / 'SPECS/runtime.spec'
            spec.write_text(rpm_spec(version, rpm_arch))
            command = ['rpmbuild', '-bb', '--define', f'_topdir {work}', '--define', '_binary_payload w3.xzdio', str(spec)]
        elif format_name == 'pkg':
            component = work / 'e2em-component.pkg'
            pre_command = ['pkgbuild', '--root', str(stage), '--identifier', 'org.e2em.runtime',
                           '--version', version, '--install-location', '/', '--ownership', 'recommended', str(component)]
            resources = work / 'resources'
            resources.mkdir()
            (resources / 'welcome.html').write_text('<html><h1>E2EM Runtime</h1><p>Private message assessment on this computer.</p><p>After installation, open <b>E2EM Setup</b> in Applications. It installs Gandalf, starts E2EM, and checks that everything works.</p></html>', encoding='utf-8')
            (resources / 'conclusion.html').write_text('<html><h1>Finish with E2EM Setup</h1><p>Open <b>Applications → E2EM Setup</b> and choose <b>Set up E2EM</b>.</p><p>Keep the setup screen open while it installs the model. Once it says <b>Ready</b>, E2EM runs in the background and starts when you sign in.</p><p>No Terminal commands or Python installation are needed.</p></html>', encoding='utf-8')
            distribution = work / 'distribution.xml'
            distribution.write_text(f'''<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
  <title>E2EM Runtime</title><welcome file="welcome.html"/><conclusion file="conclusion.html"/>
  <options customize="never" require-scripts="false" rootVolumeOnly="true"/>
  <domains enable_anywhere="false" enable_currentUserHome="false" enable_localSystem="true"/>
  <choices-outline><line choice="runtime"/></choices-outline>
  <choice id="runtime" title="E2EM Runtime" visible="false"><pkg-ref id="org.e2em.runtime"/></choice>
  <pkg-ref id="org.e2em.runtime" version="{version}">e2em-component.pkg</pkg-ref>
</installer-gui-script>''', encoding='utf-8')
            command = ['productbuild', '--distribution', str(distribution), '--resources', str(resources),
                       '--package-path', str(work), str(artifact)]
        else:
            spec = work / 'runtime.wxs'
            spec.write_text(wix_source(version, stage))
            command = ['wix', 'build', str(spec), '-ext', 'WixToolset.UI.wixext', '-arch', 'x64', '-out', str(artifact)]
        if stage_only:
            destination = output / (name + '.staging')
            if destination.exists():
                raise ValueError('refusing to overwrite an existing staging directory')
            shutil.copytree(work, destination)
            if format_name == 'msi':
                (destination / 'runtime.wxs').write_text(wix_source(version, destination / 'payload'))
            return destination
        if pre_command: subprocess.run(pre_command, check=True)
        subprocess.run(command, check=True)
        if format_name == 'rpm':
            packages = list((work / 'RPMS').rglob('*.rpm'))
            if len(packages) != 1:
                raise ValueError('expected exactly one runtime RPM')
            shutil.copyfile(packages[0], artifact)
    if not artifact.is_file() or artifact.stat().st_size == 0:
        raise ValueError('native tool did not produce a nonempty package')
    metadata = {'name': name, 'version': version, 'target': target, 'format': format_name,
                'sha256': digest(artifact), 'binary_sha256': digest(binary), 'signed': False,
                'scope': 'package-owned runtime with guided per-user model setup and background startup',
                'model_included': offline, 'native_inference': bool(inference_dir)}
    (output / (name + '.json')).write_text(json.dumps(metadata, indent=2) + '\n')
    (output / (name + '.sha256')).write_text(f'{metadata["sha256"]}  {name}\n')
    return artifact


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--format', choices=['deb', 'rpm', 'pkg', 'msi'], required=True)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--setup-binary', type=Path, help='native e2em-setup.exe GUI launcher (required for Windows)')
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--version', default=tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding="utf-8"))['package']['version'])
    parser.add_argument('--stage-only', action='store_true', help='render payload/manifests without claiming a package build')
    parser.add_argument('--inference-dir', type=Path)
    parser.add_argument('--model-package', type=Path)
    parser.add_argument('--offline', action='store_true')
    args = parser.parse_args()
    print(build(args.format, args.binary, args.target, args.output, args.version, args.stage_only, args.inference_dir, args.model_package, args.offline, args.setup_binary))


if __name__ == '__main__':
    main()
