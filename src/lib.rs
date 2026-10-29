use anyhow::{Context, Result, bail};
use clap::Parser;
use fs2::FileExt;
use russh::{
    ChannelMsg, Disconnect, client,
    keys::{PrivateKeyWithHashAlg, PublicKeyOrCertificate},
};
use std::{
    io::{IsTerminal, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::io::AsyncWriteExt;
use zeroize::Zeroizing;

/// Native SSH terminal and remote command client. Not a drop-in implementation of every OpenSSH option.
#[derive(Debug, Parser)]
#[command(version, about, trailing_var_arg = true)]
pub struct Cli {
    /// Destination, hostname or user@hostname; use -p for port
    pub destination: String,
    /// Remote shell command (arguments joined like OpenSSH; quote shell syntax locally)
    pub command: Vec<String>,
    #[arg(short = 'p', long, default_value_t = 22, value_parser = clap::value_parser!(u16).range(1..))]
    pub port: u16,
    #[arg(short = 'l', long)]
    pub user: Option<String>,
    #[arg(short = 'i', long)]
    pub identity: Option<PathBuf>,
    /// Dedicated known-host database (default ~/.ssh-client/known_hosts)
    #[arg(long)]
    pub known_hosts: Option<PathBuf>,
    /// Explicit trust-on-first-use. Changed keys are NEVER accepted.
    #[arg(long)]
    pub accept_new: bool,
    /// Prompt for password, never accept passwords as command-line arguments
    #[arg(long, conflicts_with = "batch")]
    pub password: bool,
    /// Disable password/passphrase prompts
    #[arg(short = 'B', long)]
    pub batch: bool,
    /// Force PTY, including with a remote command
    #[arg(short = 't', long, conflicts_with = "no_pty")]
    pub pty: bool,
    /// Disable PTY
    #[arg(short = 'T', long)]
    pub no_pty: bool,
    /// Do not read stdin (sends EOF)
    #[arg(short = 'n', long)]
    pub no_stdin: bool,
    /// Connection/handshake/authentication timeout seconds
    #[arg(long, default_value_t = 15, value_parser = clap::value_parser!(u64).range(1..))]
    pub connect_timeout: u64,
}

pub fn parse_destination(value: &str, explicit_user: Option<&str>) -> Result<(String, String)> {
    let (name, host) = value
        .rsplit_once('@')
        .map_or((None, value), |(u, h)| (Some(u), h));
    if name.is_some_and(|n| n.is_empty() || n.contains('@')) {
        bail!("Invalid destination username");
    }
    let user = explicit_user
        .map(str::to_owned)
        .or_else(|| name.map(str::to_owned))
        .or_else(|| std::env::var("USER").ok())
        .or_else(|| std::env::var("USERNAME").ok())
        .context("Specify a username using user@host or -l")?;
    let host = host
        .strip_prefix('[')
        .and_then(|x| x.strip_suffix(']'))
        .unwrap_or(host);
    if host.is_empty()
        || user.is_empty()
        || host.starts_with('-')
        || host.chars().any(|c| {
            c.is_whitespace() || c.is_control() || matches!(c, '@' | ',' | '/' | '[' | ']')
        })
        || user
            .chars()
            .any(|c| c.is_control() || c.is_whitespace() || c == '@')
    {
        bail!("Invalid username or host");
    }
    if host.contains(':') && host.parse::<std::net::Ipv6Addr>().is_err() {
        bail!("Use -p PORT, not host:port");
    }
    Ok((user, host.to_owned()))
}

fn home() -> Result<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
        .context("Home directory unavailable; use --known-hosts and -i")
}
fn host_label(host: &str, port: u16) -> String {
    if port == 22 {
        host.into()
    } else {
        format!("[{host}]:{port}")
    }
}

/// Dedicated strict known-host database: exact hosts, no certificates, wildcard, hashed or marker syntax.
/// Rejects malformed records rather than silently bypassing security policy.
pub fn verify_host(
    path: &Path,
    host: &str,
    port: u16,
    key: &russh::keys::PublicKey,
    accept_new: bool,
) -> Result<()> {
    let label = host_label(host, port);
    if !path.exists() && !accept_new {
        bail!(
            "Unknown host {label}, fingerprint {}; verify out of band, then use --accept-new (TOFU)",
            key.fingerprint(russh::keys::HashAlg::Sha256)
        );
    }
    if accept_new && let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(accept_new).create(accept_new);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(path)
        .context("Cannot open known-host database")?;
    if accept_new {
        file.lock_exclusive()?;
    } else {
        FileExt::lock_shared(&file)?;
    }
    let mut text = String::new();
    file.read_to_string(&mut text)?;
    let mut recorded = false;
    let mut matched = false;
    for (index, line) in text.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let fields: Vec<_> = line.split_whitespace().collect();
        if fields.len() < 3
            || fields[0].starts_with('@')
            || fields[0].starts_with('|')
            || fields[0].contains('*')
            || fields[0].contains('?')
            || fields[0].contains(',')
        {
            bail!(
                "Unsupported or malformed known-host record at line {}. Use ssh-client's dedicated database",
                index + 1
            );
        }
        let entry = russh::keys::PublicKey::from_openssh(&format!("{} {}", fields[1], fields[2]))
            .context("Malformed known-host public key")?;
        if fields[0] == label {
            recorded = true;
            if entry == *key {
                matched = true;
            }
        }
    }
    if matched {
        return Ok(());
    }
    if recorded {
        bail!(
            "HOST KEY CHANGED for {label}; connection refused. Verify with the administrator before editing the database"
        );
    }
    if !accept_new {
        bail!(
            "Unknown host {label}, fingerprint {}. Verify out of band then use --accept-new",
            key.fingerprint(russh::keys::HashAlg::Sha256)
        );
    }
    file.seek(SeekFrom::End(0))?;
    if !text.is_empty() && !text.ends_with('\n') {
        writeln!(file)?;
    }
    writeln!(file, "{} {}", label, key.to_openssh()?)?;
    file.sync_all()?;
    eprintln!(
        "Trusted new host {label}: {} (TOFU)",
        key.fingerprint(russh::keys::HashAlg::Sha256)
    );
    Ok(())
}

pub struct HostVerifier {
    pub host: String,
    pub port: u16,
    pub path: PathBuf,
    pub accept_new: bool,
}
impl client::Handler for HostVerifier {
    type Error = anyhow::Error;
    async fn check_server_key(&mut self, key: &PublicKeyOrCertificate) -> Result<bool> {
        if key.certificate().is_some() {
            bail!(
                "Host certificates are unsupported; refusing certificate rather than skipping CA validation"
            );
        }
        verify_host(
            &self.path,
            &self.host,
            self.port,
            &key.public_key(),
            self.accept_new,
        )?;
        Ok(true)
    }
}

pub fn main_entry() -> i32 {
    let cli = Cli::parse();
    match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("Cannot initialize runtime")
        .and_then(|rt| rt.block_on(run(cli)))
    {
        Ok(code) => {
            if code <= 255 {
                code as i32
            } else {
                255
            }
        }
        Err(error) => {
            eprintln!("ssh-client: {error:#}");
            255
        }
    }
}

pub async fn run(cli: Cli) -> Result<u32> {
    let (user, host) = parse_destination(&cli.destination, cli.user.as_deref())?;
    let path = match cli.known_hosts {
        Some(ref p) => p.clone(),
        None => home()?.join(".ssh-client/known_hosts"),
    };
    let verifier = HostVerifier {
        host: host.clone(),
        port: cli.port,
        path,
        accept_new: cli.accept_new,
    };
    let config = Arc::new(client::Config {
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 3,
        ..Default::default()
    });
    let mut session = tokio::time::timeout(
        Duration::from_secs(cli.connect_timeout),
        client::connect(config, (host.as_str(), cli.port), verifier),
    )
    .await
    .context("Connection timed out")??;
    let mut authenticated = false;
    if !cli.password {
        let identity = cli.identity.clone().or_else(|| {
            home()
                .ok()
                .map(|h| h.join(".ssh/id_ed25519"))
                .filter(|p| p.is_file())
        });
        if let Some(path) = identity {
            let key=match russh::keys::load_secret_key(&path,None) {
                Ok(key)=>key,
                Err(russh::keys::Error::KeyIsEncrypted) if !cli.batch => {
                    let phrase=Zeroizing::new(rpassword::prompt_password("Key passphrase: ")?);
                    russh::keys::load_secret_key(&path,Some(&phrase)).context("Cannot decrypt private key")?
                },
                Err(e)=>return Err(e).with_context(||format!("Cannot read identity {} (encrypted keys require an interactive passphrase)",path.display()))
            };
            let hash = tokio::time::timeout(
                Duration::from_secs(cli.connect_timeout),
                session.best_supported_rsa_hash(),
            )
            .await
            .context("RSA negotiation timed out")??
            .flatten();
            authenticated = tokio::time::timeout(
                Duration::from_secs(cli.connect_timeout),
                session
                    .authenticate_publickey(&user, PrivateKeyWithHashAlg::new(Arc::new(key), hash)),
            )
            .await
            .context("Authentication timed out")??
            .success();
        }
    }
    if !authenticated && !cli.batch && (cli.password || std::io::stdin().is_terminal()) {
        let password = Zeroizing::new(rpassword::prompt_password(format!(
            "{user}@{host} password: "
        ))?);
        authenticated = tokio::time::timeout(
            Duration::from_secs(cli.connect_timeout),
            session.authenticate_password(&user, &*password),
        )
        .await
        .context("Authentication timed out")??
        .success();
    }
    if !authenticated {
        bail!("Authentication failed; specify -i KEY or --password (interactive)");
    }
    let channel = tokio::time::timeout(
        Duration::from_secs(cli.connect_timeout),
        session.channel_open_session(),
    )
    .await
    .context("Session open timed out")??;
    let use_pty =
        !cli.no_pty && (cli.pty || (cli.command.is_empty() && std::io::stdin().is_terminal()));
    let mut dimensions = crossterm::terminal::size().unwrap_or((80, 24));
    if use_pty {
        channel
            .request_pty(
                true,
                &std::env::var("TERM").unwrap_or_else(|_| "xterm-256color".into()),
                dimensions.0 as u32,
                dimensions.1 as u32,
                0,
                0,
                &[],
            )
            .await?;
    }
    if cli.command.is_empty() {
        channel.request_shell(true).await?;
    } else {
        channel.exec(true, cli.command.join(" ")).await?;
    }
    let _raw = RawGuard::new(use_pty && std::io::stdin().is_terminal())?;
    // Dedicated reader thread avoids Tokio stdin's blocking shutdown after a remote command exits.
    let (tx, mut rx) = tokio::sync::mpsc::channel::<std::io::Result<Vec<u8>>>(8);
    if !cli.no_stdin {
        std::thread::spawn(move || {
            let mut stdin = std::io::stdin().lock();
            loop {
                let mut buffer = vec![0; 8192];
                match stdin.read(&mut buffer) {
                    Ok(n) => {
                        buffer.truncate(n);
                        if tx.blocking_send(Ok(buffer)).is_err() || n == 0 {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.blocking_send(Err(e));
                        break;
                    }
                }
            }
        });
    } else {
        drop(tx);
    }
    // The remote receive window may reach zero while the remote process exits.
    // Keep sending (including resize/EOF) separate from receiving so a blocked
    // data() future can never prevent draining exit-status, stderr or close.
    let (mut reader, writer) = channel.split();
    let mut send_task = AbortSendTask(tokio::spawn(async move {
        let mut stdin_done = false;
        let mut resize = tokio::time::interval(Duration::from_millis(250));
        loop {
            tokio::select! {
                input = rx.recv(), if !stdin_done => match input {
                    Some(Ok(data)) if !data.is_empty() => {
                        if writer.data(&data[..]).await.is_err() { return None; }
                    },
                    Some(Err(error)) => {
                        let _ = writer.eof().await;
                        return Some(error);
                    },
                    _ => {
                        stdin_done = true;
                        if writer.eof().await.is_err() || !use_pty { return None; }
                    }
                },
                _ = resize.tick(), if use_pty => {
                    let next = crossterm::terminal::size().unwrap_or(dimensions);
                    if next != dimensions {
                        if writer.window_change(next.0 as u32, next.1 as u32, 0, 0).await.is_err() {
                            return None;
                        }
                        dimensions = next;
                    }
                }
            }
        }
    }));
    let mut status = None;
    let mut stdout = tokio::io::stdout();
    let mut stderr = tokio::io::stderr();
    while let Some(message) = reader.wait().await {
        match message {
            ChannelMsg::Data { data } => {
                stdout.write_all(&data).await?;
                stdout.flush().await?;
            }
            ChannelMsg::ExtendedData { data, ext: 1 } => {
                stderr.write_all(&data).await?;
                stderr.flush().await?;
            }
            ChannelMsg::ExitStatus { exit_status } => {
                status = Some(exit_status);
                send_task.0.abort();
            }
            ChannelMsg::ExitSignal { .. } => {
                status = Some(255);
                send_task.0.abort();
            }
            ChannelMsg::Failure => bail!("Server rejected session request"),
            ChannelMsg::Close => break,
            _ => {}
        }
    }
    // Cancelling a blocked writer also drops rx, releasing the bounded stdin
    // producer. The dedicated OS reader thread never blocks runtime shutdown.
    if send_task.0.is_finished()
        && let Ok(Some(error)) = (&mut send_task.0).await
    {
        return Err(error).context("Cannot read local stdin");
    }
    send_task.0.abort();
    let _ = session
        .disconnect(Disconnect::ByApplication, "", "en")
        .await;
    status.context("Server closed without an exit status")
}
struct AbortSendTask(tokio::task::JoinHandle<Option<std::io::Error>>);
impl Drop for AbortSendTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}
struct RawGuard(bool);
impl RawGuard {
    fn new(enable: bool) -> Result<Self> {
        if enable {
            crossterm::terminal::enable_raw_mode()?;
        }
        Ok(Self(enable))
    }
}
impl Drop for RawGuard {
    fn drop(&mut self) {
        if self.0 {
            let _ = crossterm::terminal::disable_raw_mode();
        }
    }
}
