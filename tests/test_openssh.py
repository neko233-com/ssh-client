"""Independent OpenSSH server interoperability; no system daemon/config changes."""
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import tempfile
import time
import unittest

ROOT = Path(__file__).resolve().parents[1]
BINARY = ROOT / 'target' / 'debug' / 'ssh-client'
SSHD = shutil.which('sshd') or ('/usr/sbin/sshd' if Path('/usr/sbin/sshd').exists() else None)
if os.environ.get('SSH_CLIENT_REQUIRE_OPENSSH') == '1' and not (os.name == 'posix' and SSHD and BINARY.exists()):
    raise RuntimeError('Required OpenSSH interoperability prerequisites unavailable: build binaries and install openssh-server')


@unittest.skipUnless(os.name == 'posix' and SSHD and BINARY.exists(), 'Requires POSIX, local OpenSSH server and cargo test binaries')
class OpenSshTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        import pwd
        cls.user = pwd.getpwuid(os.getuid()).pw_name
        cls.directory = tempfile.TemporaryDirectory()
        cls.root = Path(cls.directory.name)
        for name in ('host', 'identity'):
            subprocess.run(['ssh-keygen', '-q', '-t', 'ed25519', '-N', '', '-f', str(cls.root / name)], check=True)
        shutil.copyfile(cls.root / 'identity.pub', cls.root / 'authorized')
        with socket.socket() as listener:
            listener.bind(('127.0.0.1', 0))
            cls.port = listener.getsockname()[1]
        config = '\n'.join([
            f'Port {cls.port}', 'ListenAddress 127.0.0.1', f'HostKey {cls.root}/host',
            f'PidFile {cls.root}/pid', f'AuthorizedKeysFile {cls.root}/authorized',
            'StrictModes no', 'UsePAM no', 'PasswordAuthentication no',
            'KbdInteractiveAuthentication no', 'PubkeyAuthentication yes',
            'AllowTcpForwarding no', 'X11Forwarding no', 'PermitTunnel no',
            'LogLevel VERBOSE', '',
        ])
        (cls.root / 'config').write_text(config)
        cls.log = (cls.root / 'log').open('w+')
        cls.server = subprocess.Popen([SSHD, '-D', '-e', '-f', str(cls.root / 'config')], stderr=cls.log, stdout=cls.log)
        for _ in range(100):
            if cls.server.poll() is not None:
                cls.log.seek(0)
                raise RuntimeError('OpenSSH fixture failed: ' + cls.log.read())
            try:
                with socket.create_connection(('127.0.0.1', cls.port), timeout=.1):
                    break
            except OSError:
                time.sleep(.05)
        else:
            raise RuntimeError('OpenSSH fixture startup timeout')
        host_key = (cls.root / 'host.pub').read_text().split()
        (cls.root / 'hosts').write_text(f'[127.0.0.1]:{cls.port} {host_key[0]} {host_key[1]}\n')

    @classmethod
    def tearDownClass(cls):
        cls.server.terminate()
        cls.server.wait(timeout=10)
        cls.log.close()
        cls.directory.cleanup()

    def invoke(self, command, data=b'', extra=()):
        return subprocess.run([str(BINARY), '-B', '-p', str(self.port), '-i', str(self.root / 'identity'), '--known-hosts', str(self.root / 'hosts'), *extra, f'{self.user}@127.0.0.1', command], input=data, capture_output=True, timeout=15)

    def test_command_streams_and_exit(self):
        result = self.invoke('printf hello; printf error >&2; exit 37')
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertEqual(result.stdout, b'hello')
        self.assertEqual(result.stderr, b'error')

    def test_early_remote_exit_with_saturated_stdin(self):
        started = time.monotonic()
        result = self.invoke('exit 37', b'x' * (4 * 1024 * 1024))
        self.assertEqual(result.returncode, 37, result.stderr)
        self.assertLess(time.monotonic() - started, 10)

    def test_large_stdin_round_trip(self):
        data = bytes(range(256)) * 2048
        result = self.invoke('cat', data)
        self.log.flush()
        self.assertEqual(result.returncode, 0, result.stderr + (self.root / "log").read_bytes())
        self.assertEqual(result.stdout, data)

    def test_forced_pty(self):
        result = self.invoke('test -t 0 && printf pty-ok', extra=('-t',))
        self.log.flush()
        log = (self.root / 'log').read_bytes()
        if result.returncode != 0 and b'login records by non-root user' in log:
            self.skipTest('OpenSSH fixture cannot allocate login PTY as unprivileged container user')
        self.assertEqual(result.returncode, 0, result.stderr + log)
        self.assertIn(b'pty-ok', result.stdout)

    def test_alias_version(self):
        alias = ROOT / 'target' / 'debug' / 'ssh'
        a = subprocess.run([str(alias), '--version'], capture_output=True, timeout=10)
        b = subprocess.run([str(BINARY), '--version'], capture_output=True, timeout=10)
        self.assertEqual(a.returncode, 0)
        self.assertEqual(a.stdout, b.stdout)

    def test_raw_terminal_restored_after_command(self):
        import pty
        import termios
        master, slave = pty.openpty()
        def terminal_state():
            if sys.platform == 'darwin':
                import fcntl
                import struct
                # Darwin marks pending input when canonical mode is restored.
                # FIONREAD lets the kernel process that transient state without
                # consuming input, so all terminal attributes can be compared.
                fcntl.ioctl(slave, termios.FIONREAD, struct.pack('i', 0))
            return termios.tcgetattr(slave)
        original = terminal_state()
        try:
            process = subprocess.Popen([str(BINARY), '-B', '-n', '-t', '-p', str(self.port), '-i', str(self.root / 'identity'), '--known-hosts', str(self.root / 'hosts'), f'{self.user}@127.0.0.1', 'printf terminal-ok'], stdin=slave, stdout=slave, stderr=subprocess.PIPE)
            _, error = process.communicate(timeout=15)
            # Terminal attributes must restore even if the unprivileged OpenSSH
            # fixture aborts its PTY session (e.g. cannot write system login records).
            self.assertEqual(terminal_state(), original)
            if process.returncode != 0:
                self.assertIn(b'login records by non-root user', (self.root / 'log').read_bytes(), error)
        finally:
            os.close(master)
            os.close(slave)


if __name__ == '__main__':
    unittest.main()
