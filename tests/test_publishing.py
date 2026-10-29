import hashlib
import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('publish', Path(__file__).resolve().parents[1] / 'scripts/publish.py')
publish = importlib.util.module_from_spec(spec)
spec.loader.exec_module(publish)


class PublishTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.path = Path(self.tmp.name) / 'package.zip'
        self.path.write_bytes(b'verified')
        self.expected = {self.path.name: self.path}
        self.asset = {'name': self.path.name, 'size': 8, 'digest': 'sha256:' + hashlib.sha256(b'verified').hexdigest()}

    def check(self, assets):
        publish.check_assets({'assets': assets}, self.expected, 'test/repo', 'v0.1.0')

    def test_resumes_existing_draft_without_published_tag_route(self):
        commit = 'a' * 40
        draft = {'id': 123, 'tag_name': 'v0.1.0', 'target_commitish': commit,
                 'draft': True, 'assets': [], 'html_url': 'https://github.com/test/repo/releases/tag/v0.1.0'}
        calls = []
        def api(*args, **kwargs):
            calls.append(args)
            if '/tags/' in ' '.join(args):
                raise AssertionError('Published-only tag route must not be used for drafts')
            if '--paginate' in args:
                return [[dict(draft)]]
            if '--method' in args:
                self.assertIn('PATCH', args)  # Existing draft must not be recreated.
                self.assertIn('repos/test/repo/releases/123', args)
                draft['draft'] = False
                return dict(draft)
            self.assertEqual(args, ('api', 'repos/test/repo/releases/123'))
            return dict(draft)
        with patch.object(publish, 'gh', side_effect=api), \
             patch.object(publish, 'resolve_tag', side_effect=[None, None, None, commit]), \
             patch.object(publish, 'check_assets'), \
             patch.object(publish.subprocess, 'run') as upload:
            result = publish.publish_release('test/repo', commit, 'v0.1.0', self.expected)
        self.assertEqual(result, draft['html_url'])
        upload.assert_called_once()
        self.assertFalse(any('POST' in args for args in calls))

    def test_duplicate_drafts_rejected(self):
        draft = {'id': 1, 'tag_name': 'v0.1.0'}
        with patch.object(publish, 'gh', return_value=[[draft], [dict(draft, id=2)]]):
            with self.assertRaises(RuntimeError):
                publish.find_release('test/repo', 'v0.1.0')

    def test_exact_draft_source_accepted(self):
        publish.check_source({'target_commitish': 'a' * 40}, None, 'a' * 40)

    def test_moving_branch_draft_rejected(self):
        with self.assertRaises(RuntimeError):
            publish.check_source({'target_commitish': 'main'}, None, 'a' * 40)

    def test_mismatched_version_tag_rejected(self):
        with self.assertRaises(RuntimeError):
            publish.check_source({'target_commitish': 'a' * 40}, 'b' * 40, 'a' * 40)

    def test_exact_asset_accepted(self):
        self.check([self.asset])

    def test_missing_asset_rejected(self):
        with self.assertRaises(RuntimeError):
            self.check([])

    def test_duplicate_asset_rejected(self):
        with self.assertRaises(RuntimeError):
            self.check([self.asset, self.asset])

    def test_extra_asset_rejected(self):
        with self.assertRaises(RuntimeError):
            self.check([self.asset, dict(self.asset, name='other')])

    def test_changed_asset_rejected(self):
        with self.assertRaises(RuntimeError):
            self.check([dict(self.asset, digest='sha256:' + '0' * 64)])

    def test_wrong_size_rejected(self):
        with self.assertRaises(RuntimeError):
            self.check([dict(self.asset, size=7)])


if __name__ == '__main__':
    unittest.main()
