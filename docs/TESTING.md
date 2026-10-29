# Verification contract

A test definition is not evidence that the test passed. Final execution results
must be recorded separately in `docs/VALIDATION.md` for the exact tested source.

## Native matrix

The CI workflow uses native runners, not a successful cross-compile as a substitute
for runtime tests:

| OS | Architectures | Runner labels |
|---|---|---|
| Linux | x86_64, aarch64 | ubuntu-24.04, ubuntu-24.04-arm |
| macOS | Intel, Apple Silicon | macos-15-intel, macos-15 |
| Windows | x86_64, ARM64 | windows-2025, windows-11-arm |

Runner labels were checked against the [official GitHub runner documentation](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
Availability depends on repository Actions policy and GitHub capacity.

Each job must pass formatting, Clippy, Rust tests including actual SSH transport,
installer tests appropriate to its OS, release build, and both command aliases'
version smoke tests before contributing a release archive. Publication waits for
all six jobs, rejects missing/extra/empty archives, and generates SHA256SUMS.

## Local commands

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
python -m unittest discover -s tests -p 'test_*.py' -v
```

On Windows also run the PowerShell installer test script specified in CI.
Installer tests use temporary HOME/prefix/fixtures; never run tests against your
normal installation or rely on production SSH credentials.

## Scope and limits

Cover success and failure paths: destination/port/CLI validation, trust enrollment,
unknown/changed host keys, authentication failure, command stdout/stderr/exit status,
stdin/EOF, timeouts, installer integrity failure, unexpected archive entries,
repeat installation, PATH deduplication, ownership conflict, uninstall preservation,
and all six release asset names. Check the actual test inventory rather than
assuming every item here has an implemented test.

Interactive raw terminal/PTY, resize and terminal restoration must additionally be
verified in real native terminals. In-process server tests do not establish every
OpenSSH server/version, keyboard-interactive, smartcard, proxy, forwarding or SFTP
compatibility. Unsupported features must be documented, never silently accepted.
