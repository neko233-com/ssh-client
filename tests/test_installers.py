"""Offline installer acceptance: never touches the actual user's profiles or PATH."""
import hashlib
import io
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import unittest

ROOT = Path(__file__).resolve().parents[1]

@unittest.skipIf(os.name == 'nt', 'POSIX installer tests run on Linux and macOS')
class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.base = Path(self.tmp.name).resolve()
        self.home = self.base / 'home'
        self.home.mkdir()
        self.prefix = self.home / 'custom dir'
        self.tools = self.base / 'tools'
        self.tools.mkdir()
        self.fixture = self.base / 'fixture'
        self.fixture.mkdir()
        self.env = dict(os.environ, HOME=str(self.home), PATH=str(self.tools) + os.pathsep + os.environ['PATH'], FIXTURE=str(self.fixture))
        (self.tools / 'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo x86_64;; esac\n')
        (self.tools / 'uname').chmod(0o755)
        # curl is a fixture transport; production script has no insecure URL override.
        (self.tools / 'curl').write_text('''#!/bin/sh
while [ "$#" -gt 0 ]; do
 case "$1" in https://*) url=$1;; -o) shift; out=$1;; esac
 shift
done
case "$url" in */releases/latest) printf '{"tag_name":"v0.1.0"}' > "$out";; */SHA256SUMS) cp "$FIXTURE/SHA256SUMS" "$out";; *.tar.gz) printf '%s\n' "$url" > "$FIXTURE/requested-url"; cp "$FIXTURE/package.tar.gz" "$out";; *) exit 9;; esac
''')
        (self.tools / 'curl').chmod(0o755)
        self.make_archive()

    def make_archive(self, names=('ssh', 'ssh-client'), symlink=False):
        archive = self.fixture / 'package.tar.gz'
        with tarfile.open(archive, 'w:gz') as tar:
            for name in names:
                entry = tarfile.TarInfo(name)
                body = getattr(self, 'payload', b'#!/bin/sh\necho fixture\n')
                if symlink and name == 'ssh':
                    entry.type = tarfile.SYMTYPE
                    entry.linkname = '/etc/passwd'
                    tar.addfile(entry)
                else:
                    entry.size = len(body)
                    entry.mode = 0o755
                    tar.addfile(entry, io.BytesIO(body))
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.fixture / 'SHA256SUMS').write_text(digest + '  ssh-client-v0.1.0-' + getattr(self, 'target', 'x86_64-unknown-linux-gnu') + '.tar.gz\n')

    def run_script(self, *args, ok=True):
        result = subprocess.run(['sh', str(ROOT / 'scripts/install.sh'), '--prefix', str(self.prefix), *args], env=self.env, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
        if ok:
            self.assertEqual(result.returncode, 0, result.stderr)
        else:
            self.assertNotEqual(result.returncode, 0, result.stdout)
        return result

    def test_install_and_repeat_are_idempotent(self):
        self.run_script('--version', 'v0.1.0')
        self.assertEqual(subprocess.check_output([str(self.prefix / 'bin/ssh-client')], text=True).strip(), 'fixture')
        self.assertTrue((self.prefix / 'bin/ssh').is_file())
        self.run_script()
        for name in ('.profile', '.bashrc', '.zshrc'):
            self.assertEqual((self.home / name).read_text().count('# >>> ssh-client PATH'), 1)

    def test_uninstall_preserves_foreign_files_and_profiles(self):
        (self.home / '.profile').write_text('export KEEP=yes\n')
        self.run_script()
        (self.prefix / 'keep.txt').write_text('keep')
        self.run_script('--uninstall')
        self.assertFalse((self.prefix / 'bin/ssh').exists())
        self.assertEqual((self.prefix / 'keep.txt').read_text(), 'keep')
        self.assertIn('export KEEP=yes', (self.home / '.profile').read_text())
        self.assertNotIn('ssh-client PATH', (self.home / '.profile').read_text())
        self.run_script('--uninstall')

    def test_profile_path_resolves_both_names_once(self):
        self.run_script()
        command = '. "$HOME/.profile"; . "$HOME/.profile"; command -v ssh; command -v ssh-client; printf "%s\\n" "$PATH"'
        result = subprocess.check_output(['sh', '-c', command], env=self.env, text=True).splitlines()
        self.assertEqual(result[0], str(self.prefix / 'bin/ssh'))
        self.assertEqual(result[1], str(self.prefix / 'bin/ssh-client'))
        self.assertEqual(result[2].split(':').count(str(self.prefix / 'bin')), 1)

    def test_no_path_does_not_create_profiles(self):
        self.run_script('--no-path')
        self.assertFalse((self.home / '.profile').exists())

    def test_foreign_binary_not_overwritten(self):
        (self.prefix / 'bin').mkdir(parents=True)
        (self.prefix / 'bin/ssh').write_text('foreign')
        self.run_script(ok=False)
        self.assertEqual((self.prefix / 'bin/ssh').read_text(), 'foreign')

    def test_modified_owned_binary_preserved(self):
        self.run_script()
        (self.prefix / 'bin/ssh').write_text('changed')
        self.run_script('--uninstall', ok=False)
        self.assertEqual((self.prefix / 'bin/ssh').read_text(), 'changed')
        self.run_script(ok=False)

    def test_bad_checksum_does_not_install(self):
        (self.fixture / 'SHA256SUMS').write_text('0' * 64 + '  ssh-client-v0.1.0-x86_64-unknown-linux-gnu.tar.gz\n')
        self.run_script(ok=False)
        self.assertFalse((self.prefix / 'bin/ssh').exists())

    def test_extra_archive_path_rejected(self):
        self.make_archive(('ssh', '../escape'))
        self.run_script(ok=False)
        self.assertFalse((self.base / 'escape').exists())

    def test_symlink_archive_rejected(self):
        self.make_archive(symlink=True)
        self.run_script(ok=False)

    def test_symlink_prefix_rejected(self):
        target = self.base / 'target'
        target.mkdir()
        self.prefix.symlink_to(target, target_is_directory=True)
        self.run_script(ok=False)
        self.assertEqual(list(target.iterdir()), [])

    def test_existing_profile_symlink_preserved(self):
        target = self.home / 'real-profile'
        target.write_text('untouched\n')
        (self.home / '.profile').symlink_to(target)
        self.run_script()
        self.assertEqual(target.read_text(), 'untouched\n')

    def test_failed_second_move_rolls_back(self):
        self.run_script()
        receipt = (self.prefix / '.ssh-client-owned').read_bytes()
        old = (self.prefix / 'bin/ssh-client').read_bytes()
        self.payload = b'#!/bin/sh\necho changed\n'
        self.make_archive()
        real_mv = shutil.which('mv')
        (self.tools / 'mv').write_text('#!/bin/sh\ncase "$2" in */.stage.*/ssh) exit 73;; esac\nexec ' + real_mv + ' "$@"\n')
        (self.tools / 'mv').chmod(0o755)
        self.run_script(ok=False)
        self.assertEqual((self.prefix / '.ssh-client-owned').read_bytes(), receipt)
        self.assertEqual((self.prefix / 'bin/ssh-client').read_bytes(), old)
        self.assertFalse((self.prefix / '.install-lock').exists())
        (self.tools / 'mv').unlink()
        self.run_script()

    def test_malformed_profile_preserved(self):
        profile = self.home / '.profile'
        contents = '# >>> ssh-client PATH ' + str(self.prefix) + ' >>>\nIMPORTANT=keep\n'
        profile.write_text(contents)
        self.run_script()
        self.assertEqual(profile.read_text(), contents)
        self.run_script('--uninstall')
        self.assertEqual(profile.read_text(), contents)

    def test_existing_lock_rejects_install_and_uninstall(self):
        self.run_script()
        (self.prefix / '.install-lock').mkdir()
        self.run_script(ok=False)
        self.run_script('--uninstall', ok=False)
        self.assertTrue((self.prefix / 'bin/ssh').exists())

    def test_supported_architecture_mapping(self):
        for os_name, cpu, target in (
            ('Linux', 'aarch64', 'aarch64-unknown-linux-gnu'),
            ('Darwin', 'arm64', 'aarch64-apple-darwin'),
            ('Darwin', 'x86_64', 'x86_64-apple-darwin'),
        ):
            with self.subTest(target=target):
                self.target = target
                self.make_archive()
                (self.tools / 'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo ' + os_name + ';; -m) echo ' + cpu + ';; esac\n')
                self.run_script('--no-path')
                self.assertTrue((self.fixture / 'requested-url').read_text().strip().endswith('-' + target + '.tar.gz'))
                self.run_script('--uninstall', '--no-path')

    @unittest.skipIf(hasattr(os, 'geteuid') and os.geteuid() == 0, 'permission denial requires an unprivileged process')
    def test_uninstall_permission_failure_retains_receipt_and_path(self):
        self.run_script()
        receipt = (self.prefix / '.ssh-client-owned').read_bytes()
        profile = (self.home / '.profile').read_bytes()
        (self.prefix / 'bin').chmod(0o555)
        try:
            self.run_script('--uninstall', ok=False)
            self.assertEqual((self.prefix / '.ssh-client-owned').read_bytes(), receipt)
            self.assertEqual((self.home / '.profile').read_bytes(), profile)
            self.assertTrue((self.prefix / 'bin/ssh').exists())
            self.assertTrue((self.prefix / 'bin/ssh-client').exists())
        finally:
            (self.prefix / 'bin').chmod(0o755)
        self.run_script('--uninstall')
        self.assertFalse((self.prefix / '.ssh-client-owned').exists())

    def test_uninstall_partial_failure_retains_receipt_and_path(self):
        self.run_script()
        receipt = (self.prefix / '.ssh-client-owned').read_bytes()
        profile = (self.home / '.profile').read_bytes()
        real_rm = shutil.which('rm')
        (self.tools / 'rm').write_text('#!/bin/sh\ncase "$2" in */bin/ssh) ' + real_rm + ' -f "$2"; exit 73;; esac\nexec ' + real_rm + ' "$@"\n')
        (self.tools / 'rm').chmod(0o755)
        self.run_script('--uninstall', ok=False)
        self.assertEqual((self.prefix / '.ssh-client-owned').read_bytes(), receipt)
        self.assertEqual((self.home / '.profile').read_bytes(), profile)
        self.assertFalse((self.prefix / 'bin/ssh').exists())
        self.assertTrue((self.prefix / 'bin/ssh-client').exists())
        (self.tools / 'rm').unlink()
        self.run_script('--uninstall')
        self.assertFalse((self.prefix / '.ssh-client-owned').exists())

    def test_invalid_version_rejected(self):
        self.run_script('--version', '../../attack', ok=False)

    def test_download_failure_no_install(self):
        (self.tools / 'curl').write_text('#!/bin/sh\nexit 22\n')
        self.run_script(ok=False)
        self.assertFalse(self.prefix.exists())

    def test_unsupported_cpu_rejected(self):
        (self.tools / 'uname').write_text('#!/bin/sh\ncase "$1" in -s) echo Linux;; -m) echo mips;; esac\n')
        self.run_script(ok=False)

if __name__ == '__main__':
    unittest.main()
