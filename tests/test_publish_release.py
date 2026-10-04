"""Release replacement rejects stale backups and restores interrupted publication."""
import copy
import hashlib
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / 'scripts'))
import publish_release as publish


class PublishTest(unittest.TestCase):
    def test_download_count_changes_do_not_invalidate_backup(self):
        original = {'id': 1, 'assets': [{'id': 2, 'name': 'runtime.deb', 'size': 4,
            'digest': 'sha256:abcd', 'download_count': 1}]}
        downloaded = copy.deepcopy(original)
        downloaded['assets'][0]['download_count'] = 20
        self.assertEqual(publish.release_identity(original), publish.release_identity(downloaded))
        downloaded['assets'][0]['digest'] = 'sha256:changed'
        self.assertNotEqual(publish.release_identity(original), publish.release_identity(downloaded))

    def test_published_bytes_and_extra_assets_are_verified(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'runtime.deb').write_bytes(b'bytes')
            asset = {'name': 'runtime.deb', 'size': 5, 'digest': 'sha256:' + hashlib.sha256(b'bytes').hexdigest()}
            with patch.object(publish, 'api', return_value=[asset]):
                publish.verify_remote('owner/repo', 1, root)
            with patch.object(publish, 'api', return_value=[asset, dict(asset, name='obsolete.deb')]):
                with self.assertRaisesRegex(ValueError, 'asset names'):
                    publish.verify_remote('owner/repo', 1, root)
            with patch.object(publish, 'api', return_value=[dict(asset, digest='sha256:wrong')]):
                with self.assertRaisesRegex(ValueError, 'checksum mismatch'):
                    publish.verify_remote('owner/repo', 1, root)

    def test_interrupted_upload_restores_assets_tag_and_notes(self):
        commit = 'a' * 40
        old = {'id': 1, 'tag_name': 'v0.1.2', 'target_commitish': 'main', 'body': 'Previous notes',
            'assets': [{'id': 2, 'name': 'runtime.deb', 'size': 5, 'digest': 'sha256:old'}]}
        ref = {'object': {'sha': 'b' * 40}}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            assets, backup = root / 'new', root / 'backup'
            assets.mkdir()
            backup.mkdir()
            (backup / 'assets').mkdir()
            (backup / 'release.json').write_text(json.dumps(old))
            (backup / 'tag.json').write_text(json.dumps(ref))
            (assets / 'release-provenance.json').write_text(json.dumps({'source_commit': commit, 'build_url': 'build'}))
            def api(path, *args):
                if '/git/ref/tags/' in path: return ref
                if '/git/refs/tags/' in path: return {}
                if '/assets?' in path: return old['assets'] + [{'name': 'new-report.json'}]
                return old
            with patch.dict(os.environ, {'GITHUB_SHA': commit}), \
                    patch.object(publish, 'api', side_effect=api) as remote, \
                    patch.object(publish, 'gh') as gh, \
                    patch.object(publish, 'verify_assets'), \
                    patch.object(publish, 'verify_remote') as verified, \
                    patch.object(publish, 'upload', side_effect=[RuntimeError('interrupted'), None]) as upload:
                with self.assertRaisesRegex(RuntimeError, 'interrupted'):
                    publish.replace_release('owner/repo', 'v0.1.2', assets, backup)
                self.assertEqual(upload.call_args_list[1].args, ('v0.1.2', backup / 'assets'))
                self.assertIn('sha=' + ref['object']['sha'], remote.call_args_list[-1].args)
                self.assertEqual((backup / 'replacement-notes.md').read_text(), 'Previous notes')
                gh.assert_any_call('release', 'delete-asset', 'v0.1.2', 'new-report.json', '--yes')
                verified.assert_called_once_with('owner/repo', 1, backup / 'assets')

