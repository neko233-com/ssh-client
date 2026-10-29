# Validation record

Local verification includes Linux x86_64 on 2026-10-28 UTC and Windows x86_64
on 2026-10-29 UTC. The Linux run preceded later cross-platform test portability fixes;
check the hosted CI for the final source across all six platforms.

## Linux x86_64 — original source archive

Host: Linux x86_64, unprivileged cloud workspace. Rust 1.99.1, pinned by
`rust-toolchain.toml`; dependencies pinned in `Cargo.lock`.

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | PASS |
| `cargo clippy --locked --all-targets -- -D warnings` | PASS |
| `cargo test --locked --all-targets` | 43 PASS: 20 core, 23 transport |
| Python `unittest` discovery | 43 cases: 42 PASS, 1 environmental SKIP |
| PowerShell 7 installer fixture suite on Linux | 21 assertions PASS |
| Linux x86_64 optimized release build | PASS |
| Packaged `ssh` and `ssh-client` `--version` smoke tests | PASS |
| Six-package manifest gate tests | PASS; incomplete/extra/empty input is rejected |

The first combined PowerShell invocation could not create its default cache in
this cloud host's read-only home. Retried successfully using temporary
`XDG_CACHE_HOME`, `XDG_CONFIG_HOME`, `XDG_RUNTIME_DIR`, and `XDG_DATA_HOME`.
This changes only test runtime directories, not persistent machine settings.

## Windows x86_64 — current source

Host: Windows x86_64, Rust 1.99.1 with an isolated MSVC Build Tools environment.
The Rust toolchain, build output and test fixtures were confined to task-local
directories; the desktop user's persistent PATH was not changed.

| Check | Result |
|---|---|
| Rust formatting and Clippy (`--locked --all-targets -D warnings`) | PASS |
| Rust unit and transport tests | 37 PASS: 19 core, 18 transport |
| Python `unittest` discovery | 17 PASS, 26 platform SKIP |
| Windows PowerShell installer fixtures | 31 assertions PASS |
| Windows x86_64 release build and both alias smoke tests | PASS |
| GitHub Actions workflow lint | PASS |

The persistent user PATH cases were deliberately not enabled on this desktop;
they are restricted to disposable Windows CI runners.

## What the tests establish

- CLI validation, default strict host trust, explicit TOFU, port separation,
  changed-key refusal, malformed database refusal, concurrent trust enrollment,
  private file creation and record preservation.
- A real encrypted native russh test server, client subprocesses, key and password
  authentication, encrypted-key passphrase input, password non-echo, wrong
  credentials, unknown/changed hosts, SSH session success/failure, stdout/stderr,
  exit code 37, data after exit-status, stdin/EOF, remote exit while local stdin
  stays open, handshake timeout and both binary aliases. The large-input regression explicitly sends 4 MiB
  to a remote process that immediately exits 37; it must preserve 37 promptly,
  not deadlock on the SSH send window. Bidirectional 4 MiB data is also verified.
- Unix pseudo-terminal tests observe raw mode and verify all terminal attributes
  are restored after both success and server rejection. PTY request handling is
  covered; not every emulator or resize situation is exercised.
- Independent OpenSSH 10.0p2 server: real command output/exit status and a 512 KiB
  binary stdin round trip. The fixture uses temporary keys/config and loopback
  ports, not system SSH configuration or production credentials.
- POSIX installer: 20 tests, including all four OS/architecture mappings,
  checksum and archive rejection, repeat installation, managed PATH resolution,
  concurrency exclusion, injected second-move rollback, foreign/modified file
  preservation, symlink refusal, malformed profile preservation, and uninstall.
- PowerShell transport fixtures cover checksum/archive validation, installation,
  repetition, ownership and uninstall, including injected partial-deletion failure
  rollback. They do **not** establish native Windows
  executable locking, real registry PATH updates, or actual downloaded EXE use.

## Explicit skip and remaining platform checks

The independent OpenSSH forced-PTY case is skipped only when the local server
reports it cannot write login records as this unprivileged container user. Raw
terminal restoration on that error is verified separately; successful raw-mode
restoration is verified against the native test server.

| Platform | Native client/tests | Native installer | Public install/download |
|---|---|---|---|
| Linux x86_64 | Prior source PASS, with skip above | Local fixture PASS | See release status |
| Linux ARM64 | Not in local record; see Actions | Not in local record; see Actions | See release status |
| macOS Intel | Not in local record; see Actions | Not in local record; see Actions | See release status |
| macOS Apple Silicon | Not in local record; see Actions | Not in local record; see Actions | See release status |
| Windows x86_64 | Local PASS, 37 tests | Local fixture PASS, 31 assertions | See release status |
| Windows ARM64 | Not in local record; see Actions | Not in local record; see Actions | See release status |

Windows native installer CI includes additional executable-lock update and uninstall
rollback assertions, plus opt-in persistent registry PATH installation/repetition/
uninstallation and fresh-process resolution using real client binaries. The
persistent PATH cases were not run on the Windows desktop. There are 31 native Windows assertions before the
PATH opt-in and 48 with the CI PATH opt-in. The tests restore the disposable CI account's original
PATH in a finally block; they are not enabled on a user desktop.
Windows/macOS real-terminal password input, terminal resize, signal interruption,
and restoration must be verified natively before claiming complete acceptance.

## Publication state

This file records local results before the first publication. The current
six-platform result and release availability can be checked in [GitHub Actions](https://github.com/neko233-com/ssh-client/actions)
and [GitHub Releases](https://github.com/neko233-com/ssh-client/releases).
The release workflow publishes only after all six native jobs pass. A local
result here is not evidence of a hosted CI result or public installation.

No production SSH configuration, system SSH executable, firewall setting,
code signing or notarization was changed by these local checks.

## Review fixes incorporated

Independent review found and reproduced an early-remote-exit/large-stdin deadlock
and receipt loss after failed uninstall. Both have dedicated regression tests.
The transport now drains server output/exit independently of a cancellable sender.
Uninstall retains ownership/PATH on binary deletion failure; Windows restores
already removed binaries.

Release helper unit tests cover source/tag binding, missing/duplicate/changed
assets, and resuming an existing draft by release ID rather than the published-only
tag endpoint. These are mocked/local tests, not a live GitHub publication.
