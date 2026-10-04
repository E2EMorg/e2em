"""Replace an explicitly selected GitHub release, retaining a restorable backup."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
from datetime import datetime, timezone

from check_release import verify_assets


def gh(*args):
    result = subprocess.run(['gh', *map(str, args)], capture_output=True, text=True)
    if result.returncode:
        raise RuntimeError(f'GitHub command failed: {result.stderr.strip()}')
    return result.stdout


def api(path, *args):
    return json.loads(gh('api', path, *args))


def release_identity(release):
    # Downloads change counters while the backup is being read. Compare only
    # fields that affect the publication and the identity of its actual bytes.
    return {key: release.get(key) for key in (
        'id', 'tag_name', 'target_commitish', 'name', 'body', 'draft', 'prerelease', 'immutable',
    )} | {'assets': sorted((a['name'], a['id'], a['size'], a.get('digest')) for a in release['assets'])}


def verify_remote(repository, release_id, assets):
    published = api(f'repos/{repository}/releases/{release_id}/assets?per_page=100')
    expected = {p.name: p for p in assets.iterdir() if p.is_file()}
    if {p['name'] for p in published} != set(expected):
        raise ValueError('published asset names differ from the verified bundle')
    for asset in published:
        path = expected[asset['name']]
        digest = 'sha256:' + hashlib.sha256(path.read_bytes()).hexdigest()
        if asset['size'] != path.stat().st_size or asset.get('digest') != digest:
            raise ValueError(f"published asset checksum mismatch: {path.name}")


def upload(tag, directory):
    # Publish the integrity manifest last, after every payload and metadata file.
    files = sorted(directory.iterdir(), key=lambda p: (p.name == 'SHA256SUMS', p.name))
    for path in files:
        if path.is_file():
            gh('release', 'upload', tag, path, '--clobber')


def provenance(directory, tag):
    commit = os.environ['GITHUB_SHA']
    if not re.fullmatch('[a-f0-9]{40}', commit):
        raise ValueError('invalid source commit')
    repository = os.environ['GITHUB_REPOSITORY']
    report = {'tag': tag, 'source_commit': commit, 'repository': repository,
              'build_url': f"https://github.com/{repository}/actions/runs/{os.environ['GITHUB_RUN_ID']}",
              'built_at': datetime.now(timezone.utc).isoformat()}
    (directory / 'release-provenance.json').write_text(json.dumps(report, indent=2) + '\n')
    verify_assets(directory, tag.removeprefix('v'))


def backup_release(repository, tag, destination):
    release = api(f'repos/{repository}/releases/tags/{tag}')
    if release.get('immutable') or release['draft'] or release['prerelease']:
        raise ValueError('replacement requires a mutable published stable release')
    latest = api(f'repos/{repository}/releases/latest')
    if latest['id'] != release['id']:
        raise ValueError('replacement target is no longer the latest stable release')
    ref = api(f'repos/{repository}/git/ref/tags/{tag}')
    destination.mkdir(parents=True, exist_ok=False)
    (destination / 'release.json').write_text(json.dumps(release, indent=2))
    (destination / 'tag.json').write_text(json.dumps(ref, indent=2))
    payload = destination / 'assets'
    gh('release', 'download', tag, '--dir', payload)
    verify_remote(repository, release['id'], payload)


def replace_release(repository, tag, assets, backup):
    old = json.loads((backup / 'release.json').read_text())
    ref = json.loads((backup / 'tag.json').read_text())
    current = api(f'repos/{repository}/releases/tags/{tag}')
    current_ref = api(f'repos/{repository}/git/ref/tags/{tag}')
    latest = api(f'repos/{repository}/releases/latest')
    if release_identity(current) != release_identity(old) or current_ref != ref or latest['id'] != old['id']:
        raise ValueError('release changed after backup; refusing replacement')
    provenance_report = json.loads((assets / 'release-provenance.json').read_text())
    if provenance_report['source_commit'] != os.environ['GITHUB_SHA']:
        raise ValueError('release provenance does not match the build commit')
    verify_assets(assets, tag.removeprefix('v'))
    names = {p.name for p in assets.iterdir()}
    notes = Path('.github/RELEASE_NOTES.md').read_text()
    notes += f"\nRebuilt from [{os.environ['GITHUB_SHA'][:12]}](https://github.com/{repository}/commit/{os.environ['GITHUB_SHA']}); [all platform qualification results]({provenance_report['build_url']}).\n"
    notes_file = backup / 'replacement-notes.md'
    notes_file.write_text(notes)
    try:
        upload(tag, assets)
        for asset in old['assets']:
            if asset['name'] not in names:
                gh('release', 'delete-asset', tag, asset['name'], '--yes')
        verify_remote(repository, old['id'], assets)
        api(f'repos/{repository}/git/refs/tags/{tag}', '--method', 'PATCH',
            '-f', f"sha={os.environ['GITHUB_SHA']}", '-F', 'force=true')
        gh('release', 'edit', tag, '--notes-file', notes_file,
           '--target', os.environ['GITHUB_SHA'])
    except Exception:
        print('Publication failed; restoring the previous release assets and tag.', flush=True)
        upload(tag, backup / 'assets')
        for asset in api(f'repos/{repository}/releases/{old["id"]}/assets?per_page=100'):
            if asset['name'] not in {p['name'] for p in old['assets']}:
                gh('release', 'delete-asset', tag, asset['name'], '--yes')
        api(f'repos/{repository}/git/refs/tags/{tag}', '--method', 'PATCH',
            '-f', f"sha={ref['object']['sha']}", '-F', 'force=true')
        notes_file.write_text(old.get('body') or '')
        gh('release', 'edit', tag, '--notes-file', notes_file, '--target', old['target_commitish'])
        verify_remote(repository, old['id'], backup / 'assets')
        raise
    print(f'Replaced {tag}: {len(names)} verified assets from {os.environ["GITHUB_SHA"]}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--tag', required=True)
    parser.add_argument('--assets', type=Path)
    parser.add_argument('--backup', type=Path)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--backup-only', action='store_true')
    mode.add_argument('--provenance-only', action='store_true')
    mode.add_argument('--replace-existing', action='store_true')
    args = parser.parse_args()
    if not re.fullmatch(r'v[0-9]+\.[0-9]+\.[0-9]+', args.tag):
        parser.error('tag must be vMAJOR.MINOR.PATCH')
    repository = os.environ['GITHUB_REPOSITORY']
    if args.backup_only:
        if not args.backup: parser.error('--backup-only requires --backup')
        backup_release(repository, args.tag, args.backup)
    elif args.provenance_only:
        if not args.assets: parser.error('--provenance-only requires --assets')
        provenance(args.assets, args.tag)
    else:
        if not args.assets or not args.backup: parser.error('replacement requires --assets and --backup')
        replace_release(repository, args.tag, args.assets, args.backup)


if __name__ == '__main__':
    main()
