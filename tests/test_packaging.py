import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location('package', Path(__file__).resolve().parents[1] / 'scripts/package.py')
package = importlib.util.module_from_spec(spec)
spec.loader.exec_module(package)


class PackagingTests(unittest.TestCase):
    def test_supported_names_unique(self):
        names = [package.archive_name('0.1.0', t) for t in package.TARGETS]
        self.assertEqual(len(set(names)), 6)
        self.assertEqual(sum(n.endswith('.zip') for n in names), 2)

    def test_invalid_target_rejected(self):
        with self.assertRaises(ValueError):
            package.archive_name('0.1.0', '../escape')

    def test_missing_platform_blocks_release(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(ValueError):
                package.manifest('0.1.0', Path(directory))

    def test_manifest_matches_all_six(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in package.TARGETS:
                (root / package.archive_name('0.1.0', target)).write_bytes(b'fixture')
            package.manifest('0.1.0', root)
            result = (root / 'SHA256SUMS').read_text()
            self.assertEqual(len(result.splitlines()), 6)
            self.assertTrue(all(line.startswith('f16d05ec6b29248d2c61adb1e9263f78e4f7bace1b955014a2d17872cfe4064d') for line in result.splitlines()))

    def test_unexpected_file_blocks_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / 'secret.txt').write_text('not a release asset')
            with self.assertRaises(ValueError):
                package.manifest('0.1.0', root)

    def test_empty_platform_blocks_release(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            for target in package.TARGETS:
                (root / package.archive_name('0.1.0', target)).touch()
            with self.assertRaises(ValueError):
                package.manifest('0.1.0', root)


if __name__ == '__main__':
    unittest.main()
