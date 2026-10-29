# Security

This client is new software, not an audited drop-in OpenSSH replacement.

- Host-key checking is mandatory. Unknown hosts require explicit trust enrollment;
  a changed key must fail even in accept-new mode. Confirm fingerprints through
  an independent trusted channel before enrolling a host.
- Do not put passwords or private key contents in command-line arguments or issue
  reports. Avoid testing against production hosts or real credentials.
- The installer verifies release archive SHA-256 checksums downloaded over HTTPS.
  Checksums detect corruption, not compromise of the GitHub release account.
  Release binaries are not platform code-signed or independently audited.
- Installation must not replace a system SSH executable, change firewall rules,
  start an SSH server, or alter system SSH configuration.
- The dedicated install directory can precede system SSH on PATH. That intentionally
  changes which `ssh` executable resolves in a new shell; uninstall restores the
  prior PATH entry without deleting the system client.
- When reporting a vulnerability, do not publish credentials, private hostnames,
  private keys, or a working exploit against a third-party system. Use GitHub's
  private vulnerability reporting facility if enabled, or first ask the maintainer
  for a private reporting channel without including sensitive details.
