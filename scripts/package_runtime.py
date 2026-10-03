"""Build unsigned runtime DEB/RPM/PKG/MSI artifacts with native packaging tools.

Payloads contain no model, grants, secrets, startup hooks or diagnostic workers.
Package managers own executables; user setup references them without copying.
"""
import argparse
import hashlib
import json
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


def stage_payload(binary, platform, stage):
    if platform == 'windows':
        copy(binary, stage / 'e2emd.exe', 0o755)
        copy(ROOT / 'scripts/install_windows_runtime.ps1', stage / 'install_windows_runtime.ps1')
        docs = stage
    else:
        prefix = 'usr' if platform == 'linux' else 'usr/local'
        binary_path = 'bin/e2emd' if platform == 'linux' else 'libexec/e2em/e2emd'
        copy(binary, stage / prefix / binary_path, 0o755)
        docs = stage / prefix / 'share/e2em'
        copy(ROOT / 'scripts/install_runtime.py', docs / 'install_runtime.py')
        if platform == 'macos':
            copy(ROOT / 'scripts/install_macos_runtime.py', docs / 'install_macos_runtime.py')
    copy(ROOT / 'LICENSE', docs / 'LICENSE')
    copy(ROOT / 'docs/runtime/PACKAGING.md', docs / 'PACKAGING.md')
    copy(ROOT / 'docs/runtime/DESKTOP.md', docs / 'DESKTOP.md')


def deb_control(version, architecture, stage):
    size = sum(p.stat().st_size for p in stage.rglob('*') if p.is_file())
    return f"""Package: e2em-runtime
Version: {version}
Section: utils
Priority: optional
Architecture: {architecture}
Installed-Size: {(size + 1023) // 1024}
Depends: python3 (>= 3.11)
Recommends: systemd
Maintainer: E2EM Project <noreply@e2em.org>
Homepage: https://e2em.org
Description: Local rules-only E2EM developer runtime
 Per-user authenticated message assessment with personal email warnings.
 Enrolment and service startup are explicit. No contextual model is included.
"""


def rpm_spec(version, architecture):
    return f"""Name: e2em-runtime
Version: {version}
Release: 1
Summary: Local rules-only E2EM developer runtime
License: MIT
URL: https://e2em.org
BuildArch: {architecture}
Requires: python3 >= 3.11
Recommends: systemd
# Rust musl executable is static. Keep automatic dependency scanning enabled.
%global debug_package %{{nil}}
%global __brp_strip %{{nil}}
%description
Per-user authenticated message assessment with personal email warnings.
Enrolment and service startup are explicit. No contextual model is included.
%prep
%build
%install
mkdir -p %{{buildroot}}/usr
cp -a %{{_sourcedir}}/payload/usr/. %{{buildroot}}/usr/
%files
%attr(0755,root,root) /usr/bin/e2emd
%dir /usr/share/e2em
%attr(0644,root,root) /usr/share/e2em/*
"""


def wix_source(version, stage):
    ET.register_namespace('', WIX_NS)
    def element(parent, name, **attrs):
        return ET.SubElement(parent, f'{{{WIX_NS}}}{name}', attrs)
    wix = ET.Element(f'{{{WIX_NS}}}Wix')
    package = element(wix, 'Package', Name='E2EM Runtime', Manufacturer='E2EM Project',
                      Version=version, UpgradeCode=UPGRADE_CODE, Scope='perUser',
                      InstallerVersion='500', Language='1033')
    element(package, 'MajorUpgrade', DowngradeErrorMessage='A newer E2EM Runtime is installed.',
            Schedule='afterInstallInitialize')
    element(package, 'MediaTemplate', EmbedCab='yes')
    directory = element(package, 'StandardDirectory', Id='LocalAppDataFolder')
    folder = element(directory, 'Directory', Id='INSTALLFOLDER', Name='E2EM Runtime')
    feature = element(package, 'Feature', Id='Runtime', Title='E2EM Runtime', Level='1')
    for index, source in enumerate(sorted(stage.iterdir())):
        # Per-user files need an HKCU registry keypath (ICE38). WiX cannot
        # auto-generate GUIDs for components combining files and registry keys.
        guid = str(uuid.uuid5(uuid.UUID(UPGRADE_CODE), 'x64:' + source.name)).upper()
        component = element(folder, 'Component', Id=f'Payload{index}', Guid=guid, Bitness='always64')
        element(component, 'File', Id=f'File{index}', Source=str(source), KeyPath='no')
        element(component, 'RegistryValue', Root='HKCU', Key=r'Software\E2EM\Runtime',
                Name=source.name, Type='integer', Value='1', KeyPath='yes')
        if index == 0:
            element(component, 'RegistryValue', Root='HKCU', Key=r'Software\E2EM\Runtime',
                    Name='Version', Type='string', Value=version)
            element(component, 'RemoveFolder', Id='RemoveRuntimeFolder', Directory='INSTALLFOLDER', On='uninstall')
        element(feature, 'ComponentRef', Id=f'Payload{index}')
    return ET.tostring(wix, encoding='unicode', xml_declaration=True)


def build(format_name, binary, target, output, version, stage_only=False):
    version = version_value(version)
    platform, deb_arch, rpm_arch = TARGETS[target]
    if format_name not in {'linux': {'deb', 'rpm'}, 'macos': {'pkg'}, 'windows': {'msi'}}[platform]:
        raise ValueError('package format does not match the target')
    binary = binary.resolve(strict=True)
    if not binary.is_file() or binary.stat().st_size == 0:
        raise ValueError('provide a nonempty built runtime executable')
    verify_binary(binary, target)
    output = output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    name = f'e2em-runtime-{version}-{target}.{format_name}'
    artifact = output / name
    if artifact.exists() or (output / (name + '.json')).exists():
        raise ValueError('refusing to overwrite existing package artifacts')
    with tempfile.TemporaryDirectory(prefix='e2em-package-') as temporary:
        work = Path(temporary)
        stage = work / 'payload'
        stage.mkdir()
        stage_payload(binary, platform, stage)
        if format_name == 'deb':
            (stage / 'DEBIAN').mkdir()
            (stage / 'DEBIAN/control').write_text(deb_control(version, deb_arch, stage))
            command = ['dpkg-deb', '--build', '--root-owner-group', str(stage), str(artifact)]
        elif format_name == 'rpm':
            for folder in ['BUILD', 'BUILDROOT', 'RPMS', 'SOURCES', 'SPECS', 'SRPMS']:
                (work / folder).mkdir()
            shutil.copytree(stage, work / 'SOURCES/payload')
            spec = work / 'SPECS/runtime.spec'
            spec.write_text(rpm_spec(version, rpm_arch))
            command = ['rpmbuild', '-bb', '--define', f'_topdir {work}', str(spec)]
        elif format_name == 'pkg':
            command = ['pkgbuild', '--root', str(stage), '--identifier', 'org.e2em.runtime',
                       '--version', version, '--install-location', '/', '--ownership', 'recommended', str(artifact)]
        else:
            spec = work / 'runtime.wxs'
            spec.write_text(wix_source(version, stage))
            command = ['wix', 'build', str(spec), '-arch', 'x64', '-out', str(artifact)]
        if stage_only:
            destination = output / (name + '.staging')
            if destination.exists():
                raise ValueError('refusing to overwrite an existing staging directory')
            shutil.copytree(work, destination)
            if format_name == 'msi':
                (destination / 'runtime.wxs').write_text(wix_source(version, destination / 'payload'))
            return destination
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
                'scope': 'package payload only; per-user enrolment/startup is explicit',
                'model_included': False}
    (output / (name + '.json')).write_text(json.dumps(metadata, indent=2) + '\n')
    (output / (name + '.sha256')).write_text(f'{metadata["sha256"]}  {name}\n')
    return artifact


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--format', choices=['deb', 'rpm', 'pkg', 'msi'], required=True)
    parser.add_argument('--target', choices=TARGETS, required=True)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--version', default=tomllib.loads((ROOT / 'Cargo.toml').read_text(encoding="utf-8"))['package']['version'])
    parser.add_argument('--stage-only', action='store_true', help='render payload/manifests without claiming a package build')
    args = parser.parse_args()
    print(build(args.format, args.binary, args.target, args.output, args.version, args.stage_only))


if __name__ == '__main__':
    main()
