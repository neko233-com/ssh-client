use russh::{
    Channel, ChannelId,
    keys::{Algorithm, PrivateKey, PublicKey},
    server::{self, Server as _, Session, VerifiedPublicKey},
};
use std::{
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
#[cfg(all(unix, target_vendor = "apple"))]
const TIOCSCTTY_REQUEST: libc::c_ulong = libc::TIOCSCTTY as libc::c_ulong;
#[cfg(all(unix, not(target_vendor = "apple")))]
const TIOCSCTTY_REQUEST: libc::c_ulong = libc::TIOCSCTTY;
#[derive(Clone)]
struct Fixture {
    key: PublicKey,
    pty: Arc<AtomicBool>,
    echo: bool,
}
impl server::Server for Fixture {
    type Handler = Self;
    fn new_client(&mut self, _: Option<std::net::SocketAddr>) -> Self {
        self.clone()
    }
}
impl server::Handler for Fixture {
    type Error = russh::Error;
    async fn auth_publickey(
        &mut self,
        user: &str,
        key: VerifiedPublicKey<'_>,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if user == "test" && key.public_key == &self.key {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn auth_password(
        &mut self,
        user: &str,
        password: &str,
    ) -> Result<server::Auth, Self::Error> {
        Ok(if user == "test" && password == "fixture-password" {
            server::Auth::Accept
        } else {
            server::Auth::reject()
        })
    }
    async fn channel_open_session(
        &mut self,
        _: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut Session,
    ) -> Result<(), Self::Error> {
        reply.accept().await;
        Ok(())
    }
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        _: &str,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &[(russh::Pty, u32)],
        s: &mut Session,
    ) -> Result<(), Self::Error> {
        self.pty.store(true, Ordering::Relaxed);
        s.channel_success(channel)?;
        Ok(())
    }
    async fn exec_request(
        &mut self,
        ch: ChannelId,
        command: &[u8],
        s: &mut Session,
    ) -> Result<(), Self::Error> {
        if command == b"delayed" || command == b"reject-delayed" || command == b"slow-failure" {
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
        if command == b"reject" || command == b"reject-delayed" {
            s.channel_failure(ch)?;
            return Ok(());
        }
        s.channel_success(ch)?;
        if command == b"echo" {
            self.echo = true;
            return Ok(());
        }
        if command == b"no-status" {
            s.close(ch)?;
            return Ok(());
        }
        s.data(ch, b"out\n".to_vec())?;
        s.extended_data(ch, 1, b"err\n".to_vec())?;
        s.exit_status_request(
            ch,
            if command == b"failure" || command == b"slow-failure" {
                37
            } else {
                0
            },
        )?;
        // Output after exit-status must still be drained.
        s.data(ch, b"tail\n".to_vec())?;
        s.eof(ch)?;
        s.close(ch)?;
        Ok(())
    }
    async fn shell_request(&mut self, ch: ChannelId, s: &mut Session) -> Result<(), Self::Error> {
        s.channel_success(ch)?;
        self.echo = true;
        Ok(())
    }
    async fn data(
        &mut self,
        ch: ChannelId,
        data: &[u8],
        s: &mut Session,
    ) -> Result<(), Self::Error> {
        if self.echo {
            s.data(ch, data.to_vec())?;
        }
        Ok(())
    }
    async fn channel_eof(&mut self, ch: ChannelId, s: &mut Session) -> Result<(), Self::Error> {
        if self.echo {
            s.exit_status_request(ch, 0)?;
            s.eof(ch)?;
            s.close(ch)?;
        }
        Ok(())
    }
}
struct Harness {
    _dir: tempfile::TempDir,
    port: u16,
    key: PathBuf,
    hosts: PathBuf,
    server: tokio::task::JoinHandle<()>,
    pty: Arc<AtomicBool>,
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Harness {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let private = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let host = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
        let key = dir.path().join("identity");
        std::fs::write(
            &key,
            private
                .to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
        let hosts = dir.path().join("hosts");
        let socket = tokio::net::TcpListener::bind(("127.0.0.1", 0))
            .await
            .unwrap();
        let port = socket.local_addr().unwrap().port();
        ssh_client::verify_host(&hosts, "127.0.0.1", port, host.public_key(), true).unwrap();
        let pty = Arc::new(AtomicBool::new(false));
        let mut fixture = Fixture {
            key: private.public_key().clone(),
            pty: pty.clone(),
            echo: false,
        };
        let config = Arc::new(server::Config {
            keys: vec![host],
            window_size: 32768,
            auth_rejection_time: Duration::from_millis(1),
            auth_rejection_time_initial: Some(Duration::ZERO),
            ..Default::default()
        });
        let server = tokio::spawn(async move {
            fixture.run_on_socket(config, &socket).await.unwrap();
        });
        Self {
            _dir: dir,
            port,
            key,
            hosts,
            server,
            pty,
        }
    }
    async fn invoke(&self, args: &[&str], command: Option<&str>, input: &[u8]) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_ssh-client"));
        cmd.arg("-B")
            .arg("-p")
            .arg(self.port.to_string())
            .arg("-i")
            .arg(&self.key)
            .arg("--known-hosts")
            .arg(&self.hosts)
            .args(args)
            .arg("test@127.0.0.1");
        if let Some(c) = command {
            cmd.arg(c);
        }
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        let input = input.to_vec();
        let mut cmd = tokio::process::Command::from(cmd);
        cmd.kill_on_drop(true);
        let mut child = cmd.spawn().unwrap();
        let mut stdin = child.stdin.take().unwrap();
        let input_task = tokio::spawn(async move {
            use tokio::io::AsyncWriteExt;
            if let Err(error) = stdin.write_all(&input).await {
                assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
            }
        });
        // Read stdout/stderr concurrently with stdin; timeout drops and kills
        // the child instead of leaving a stuck blocking task in the test runtime.
        let output = tokio::time::timeout(Duration::from_secs(60), child.wait_with_output())
            .await
            .expect("client hung")
            .unwrap();
        input_task.await.unwrap();
        output
    }
}
#[tokio::test]
async fn command_stdout_stderr_and_trailing_data() {
    let h = Harness::new().await;
    let o = h.invoke(&[], Some("success"), b"").await;
    assert_eq!(
        o.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert_eq!(o.stdout, b"out\ntail\n");
    assert_eq!(o.stderr, b"err\n");
}
#[tokio::test]
async fn remote_failure_exit_preserved() {
    let h = Harness::new().await;
    assert_eq!(
        h.invoke(&[], Some("failure"), b"").await.status.code(),
        Some(37)
    );
}
#[tokio::test]
async fn binary_stdin_round_trip() {
    let h = Harness::new().await;
    let payload: Vec<u8> = (0..4 * 1024 * 1024).map(|n| (n % 256) as u8).collect();
    let o = h.invoke(&[], Some("echo"), &payload).await;
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, payload);
}
#[tokio::test]
async fn nonpty_shell_round_trip() {
    let h = Harness::new().await;
    let o = h.invoke(&["-T"], None, b"hello shell\n").await;
    assert_eq!(o.status.code(), Some(0));
    assert_eq!(o.stdout, b"hello shell\n");
    assert!(!h.pty.load(Ordering::Relaxed));
}
#[tokio::test]
async fn forced_pty_requested() {
    let h = Harness::new().await;
    let o = h.invoke(&["-t"], Some("success"), b"").await;
    assert_eq!(o.status.code(), Some(0));
    assert!(h.pty.load(Ordering::Relaxed));
}
#[tokio::test]
async fn rejected_request_exits_255() {
    let h = Harness::new().await;
    let o = h.invoke(&[], Some("reject"), b"").await;
    assert_eq!(o.status.code(), Some(255));
}
#[tokio::test]
async fn missing_status_is_error() {
    let h = Harness::new().await;
    let o = h.invoke(&[], Some("no-status"), b"").await;
    assert_eq!(o.status.code(), Some(255));
}
#[tokio::test]
async fn wrong_user_fails_auth() {
    let h = Harness::new().await;
    let o = h.invoke(&["-l", "other"], Some("success"), b"").await;
    assert_eq!(o.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Authentication failed"));
}
#[tokio::test]
async fn wrong_key_fails_auth() {
    let h = Harness::new().await;
    let key = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    std::fs::write(
        &h.key,
        key.to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    assert_eq!(
        h.invoke(&[], Some("success"), b"").await.status.code(),
        Some(255)
    );
}
#[tokio::test]
async fn unknown_host_fails_before_auth() {
    let h = Harness::new().await;
    std::fs::remove_file(&h.hosts).unwrap();
    let o = h.invoke(&[], Some("success"), b"").await;
    assert_eq!(o.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Unknown host"));
    assert!(!h.hosts.exists());
}
#[tokio::test]
async fn accept_new_persists_key_then_connects() {
    let h = Harness::new().await;
    std::fs::remove_file(&h.hosts).unwrap();
    assert_eq!(
        h.invoke(&["--accept-new"], Some("success"), b"")
            .await
            .status
            .code(),
        Some(0)
    );
    assert!(h.hosts.is_file());
}
#[tokio::test]
async fn changed_host_rejected_even_accept_new() {
    let h = Harness::new().await;
    std::fs::remove_file(&h.hosts).unwrap();
    let wrong = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap();
    ssh_client::verify_host(&h.hosts, "127.0.0.1", h.port, wrong.public_key(), true).unwrap();
    let o = h.invoke(&["--accept-new"], Some("success"), b"").await;
    assert_eq!(o.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&o.stderr).contains("CHANGED"));
}
#[tokio::test]
async fn no_stdin_sends_eof() {
    let h = Harness::new().await;
    let o = h.invoke(&["-n"], Some("echo"), b"").await;
    assert_eq!(o.status.code(), Some(0));
    assert!(o.stdout.is_empty());
}
#[tokio::test]
async fn encrypted_key_batch_rejects_without_prompt() {
    let h = Harness::new().await;
    let k = PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519)
        .unwrap()
        .encrypt(&mut rand::rng(), "secret")
        .unwrap();
    std::fs::write(
        &h.key,
        k.to_openssh(russh::keys::ssh_key::LineEnding::LF)
            .unwrap()
            .as_bytes(),
    )
    .unwrap();
    let o = h.invoke(&[], Some("success"), b"").await;
    assert_eq!(o.status.code(), Some(255));
    assert!(String::from_utf8_lossy(&o.stderr).contains("interactive passphrase"));
}
#[tokio::test]
async fn handshake_timeout_bounded() {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let t = tempfile::tempdir().unwrap();
    let path = t.path().join("hosts");
    let pending = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        tokio::time::sleep(Duration::from_secs(5)).await;
    });
    let start = std::time::Instant::now();
    let output = tokio::task::spawn_blocking(move || {
        Command::new(env!("CARGO_BIN_EXE_ssh-client"))
            .args([
                "-B",
                "--connect-timeout",
                "1",
                "-p",
                &port.to_string(),
                "--known-hosts",
            ])
            .arg(path)
            .arg("test@127.0.0.1")
            .output()
            .unwrap()
    })
    .await
    .unwrap();
    assert_eq!(output.status.code(), Some(255));
    assert!(start.elapsed() < Duration::from_secs(4));
    pending.abort();
}

#[tokio::test]
async fn remote_exit_does_not_wait_for_open_local_stdin() {
    let h = Harness::new().await;
    let mut command = Command::new(env!("CARGO_BIN_EXE_ssh-client"));
    command
        .args(["-B", "-p", &h.port.to_string(), "-i"])
        .arg(&h.key)
        .arg("--known-hosts")
        .arg(&h.hosts)
        .args(["test@127.0.0.1", "success"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = tokio::time::timeout(
        Duration::from_secs(5),
        tokio::task::spawn_blocking(move || {
            let mut child = command.spawn().unwrap();
            let held_open_stdin = child.stdin.take().unwrap();
            let output = child.wait_with_output().unwrap();
            drop(held_open_stdin);
            output
        }),
    )
    .await
    .expect("exit blocked on local stdin")
    .unwrap();
    assert_eq!(output.status.code(), Some(0));
}

#[test]
fn alias_binary_has_same_version() {
    let primary = Command::new(env!("CARGO_BIN_EXE_ssh-client"))
        .arg("--version")
        .output()
        .unwrap();
    let alias = Command::new(env!("CARGO_BIN_EXE_ssh"))
        .arg("--version")
        .output()
        .unwrap();
    assert!(primary.status.success());
    assert!(alias.status.success());
    assert_eq!(primary.stdout, alias.stdout);
}

#[cfg(unix)]
async fn terminal_restoration(command_name: &str, expected: i32) {
    use std::os::{
        fd::{AsRawFd, FromRawFd},
        unix::process::CommandExt,
    };
    let h = Harness::new().await;
    let mut command = Command::new(env!("CARGO_BIN_EXE_ssh-client"));
    command
        .args(["-B", "-t", "-p", &h.port.to_string(), "-i"])
        .arg(&h.key)
        .arg("--known-hosts")
        .arg(&h.hosts)
        .args(["test@127.0.0.1", command_name]);
    let result = tokio::task::spawn_blocking(move || {
        let mut master = -1;
        let mut slave = -1;
        // SAFETY: valid output pointers; openpty initializes distinct owned file descriptors.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: ownership of descriptors returned by successful openpty is transferred exactly once.
        let _master = unsafe { std::fs::File::from_raw_fd(master) };
        let slave_file = unsafe { std::fs::File::from_raw_fd(slave) };
        let mut before = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: descriptor is open and output points to writable termios storage.
        assert_eq!(
            unsafe { libc::tcgetattr(slave_file.as_raw_fd(), before.as_mut_ptr()) },
            0
        );
        let before = unsafe { before.assume_init() };
        command
            .stdin(Stdio::from(slave_file.try_clone().unwrap()))
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // SAFETY: only async-signal-safe libc calls in the fork child; stdin is already the PTY slave.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, TIOCSCTTY_REQUEST, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let start = std::time::Instant::now();
        let mut saw_raw = false;
        let status = loop {
            let mut attr = std::mem::MaybeUninit::<libc::termios>::uninit();
            if unsafe { libc::tcgetattr(slave, attr.as_mut_ptr()) } == 0 {
                let attr = unsafe { attr.assume_init() };
                saw_raw |= attr.c_lflag & libc::ICANON == 0;
            }
            if let Some(status) = child.try_wait().unwrap() {
                break status;
            }
            if start.elapsed() > Duration::from_secs(5) {
                child.kill().unwrap();
                let _ = child.wait();
                panic!("PTY client hung");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let mut after = std::mem::MaybeUninit::<libc::termios>::uninit();
        assert_eq!(unsafe { libc::tcgetattr(slave, after.as_mut_ptr()) }, 0);
        let after = unsafe { after.assume_init() };
        assert!(saw_raw, "client never entered raw terminal mode");
        assert_eq!(after.c_lflag, before.c_lflag, "terminal flags not restored");
        assert_eq!(after.c_iflag, before.c_iflag);
        assert_eq!(after.c_oflag, before.c_oflag);
        assert_eq!(after.c_cflag, before.c_cflag);
        assert_eq!(after.c_cc, before.c_cc);
        status.code()
    })
    .await
    .unwrap();
    assert_eq!(result, Some(expected));
}
#[cfg(unix)]
#[tokio::test]
async fn raw_terminal_restored_after_success() {
    terminal_restoration("delayed", 0).await;
}
#[cfg(unix)]
#[tokio::test]
async fn raw_terminal_restored_after_rejection() {
    terminal_restoration("reject-delayed", 255).await;
}

#[cfg(unix)]
async fn prompt_authentication(encrypted_key: bool, credential: &str, expected: i32) {
    use std::{
        io::{Read, Write},
        os::{fd::FromRawFd, unix::process::CommandExt},
    };
    let h = Harness::new().await;
    if encrypted_key {
        let key = russh::keys::load_secret_key(&h.key, None)
            .unwrap()
            .encrypt(&mut rand::rng(), "fixture-passphrase")
            .unwrap();
        std::fs::write(
            &h.key,
            key.to_openssh(russh::keys::ssh_key::LineEnding::LF)
                .unwrap()
                .as_bytes(),
        )
        .unwrap();
    }
    let mut command = Command::new(env!("CARGO_BIN_EXE_ssh-client"));
    command
        .args(["-n", "-T", "-p", &h.port.to_string(), "--known-hosts"])
        .arg(&h.hosts);
    if encrypted_key {
        command.arg("-i").arg(&h.key);
    } else {
        command.arg("--password");
    }
    command
        .args(["test@127.0.0.1", "success"])
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let credential = credential.to_owned();
    let result = tokio::task::spawn_blocking(move || {
        let mut master = -1;
        let mut slave = -1;
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        let mut master_file = unsafe { std::fs::File::from_raw_fd(master) };
        let slave_file = unsafe { std::fs::File::from_raw_fd(slave) };
        assert_ne!(
            unsafe { libc::fcntl(master, libc::F_SETFL, libc::O_NONBLOCK) },
            -1
        );
        command.stdin(Stdio::from(slave_file.try_clone().unwrap()));
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if libc::ioctl(0, TIOCSCTTY_REQUEST, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let mut child = command.spawn().unwrap();
        let start = std::time::Instant::now();
        let mut captured = Vec::new();
        let mut sent = false;
        loop {
            let mut buf = [0; 1024];
            if let Ok(n) = master_file.read(&mut buf) {
                captured.extend_from_slice(&buf[..n]);
            }
            let mut term = std::mem::MaybeUninit::<libc::termios>::uninit();
            if !sent
                && captured.ends_with(b": ")
                && unsafe { libc::tcgetattr(slave, term.as_mut_ptr()) } == 0
                && unsafe { term.assume_init() }.c_lflag & libc::ECHO == 0
            {
                master_file
                    .write_all(format!("{credential}\n").as_bytes())
                    .unwrap();
                sent = true;
            }
            if let Some(status) = child.try_wait().unwrap() {
                assert!(sent);
                assert!(
                    !String::from_utf8_lossy(&captured).contains(&credential),
                    "credential was echoed"
                );
                return status.code();
            }
            if start.elapsed() > Duration::from_secs(8) {
                child.kill().unwrap();
                let _ = child.wait();
                panic!(
                    "authentication prompt hung: {}",
                    String::from_utf8_lossy(&captured)
                );
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    })
    .await
    .unwrap();
    assert_eq!(result, Some(expected));
}
#[cfg(unix)]
#[tokio::test]
async fn password_prompt_authenticates_without_echo() {
    prompt_authentication(false, "fixture-password", 0).await;
}
#[cfg(unix)]
#[tokio::test]
async fn incorrect_password_is_rejected() {
    prompt_authentication(false, "wrong-password", 255).await;
}
#[cfg(unix)]
#[tokio::test]
async fn encrypted_key_passphrase_prompt_authenticates() {
    prompt_authentication(true, "fixture-passphrase", 0).await;
}

#[tokio::test]
async fn saturated_stdin_does_not_hide_early_remote_exit() {
    let h = Harness::new().await;
    let start = std::time::Instant::now();
    let output = h
        .invoke(&[], Some("slow-failure"), &vec![b'x'; 4 * 1024 * 1024])
        .await;
    assert_eq!(
        output.status.code(),
        Some(37),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(output.stdout, b"out\ntail\n");
    assert!(start.elapsed() < Duration::from_secs(5));
}
