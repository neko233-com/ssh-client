use clap::Parser;
use russh::keys::{Algorithm, PrivateKey};
use ssh_client::{Cli, parse_destination, verify_host};
fn key() -> PrivateKey {
    PrivateKey::random(&mut rand::rng(), Algorithm::Ed25519).unwrap()
}
#[test]
fn user_and_host() {
    assert_eq!(
        parse_destination("alice@example.org", None).unwrap(),
        ("alice".into(), "example.org".into())
    );
}
#[test]
fn explicit_user_wins() {
    assert_eq!(parse_destination("a@host", Some("b")).unwrap().0, "b");
}
#[test]
fn ipv6() {
    assert_eq!(parse_destination("a@[::1]", None).unwrap().1, "::1");
}
#[test]
fn bare_ipv6() {
    assert_eq!(parse_destination("::1", Some("a")).unwrap().1, "::1");
}
#[test]
fn rejects_invalid_destinations() {
    for value in [
        "",
        "@host",
        "a@",
        "a@@host",
        "a@bad host",
        "a@-host",
        "a@host:2222",
        "a@host\nother",
        "a@../host",
    ] {
        assert!(parse_destination(value, Some("a")).is_err(), "{value:?}");
    }
}
#[test]
fn default_secure_options() {
    let cli = Cli::try_parse_from(["ssh-client", "u@host"]).unwrap();
    assert!(!cli.accept_new);
    assert_eq!(cli.port, 22);
    assert!(!cli.password);
}
#[test]
fn rejects_zero_port() {
    assert!(Cli::try_parse_from(["ssh", "-p", "0", "host"]).is_err());
}
#[test]
fn rejects_zero_timeout() {
    assert!(Cli::try_parse_from(["ssh", "--connect-timeout", "0", "host"]).is_err());
}
#[test]
fn rejects_conflicting_pty() {
    assert!(Cli::try_parse_from(["ssh", "-t", "-T", "host"]).is_err());
}
#[test]
fn rejects_batch_password() {
    assert!(Cli::try_parse_from(["ssh", "-B", "--password", "host"]).is_err());
}
#[test]
fn command_keeps_flags() {
    let c = Cli::try_parse_from(["ssh", "-p", "2222", "u@host", "ls", "-la"]).unwrap();
    assert_eq!(c.command, ["ls", "-la"]);
}
#[test]
fn unknown_host_rejected_without_write() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("absent/hosts");
    assert!(verify_host(&p, "host", 22, key().public_key(), false).is_err());
    assert!(!p.exists());
}
#[test]
fn accept_new_persists_then_strict_accepts() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    let k = key();
    verify_host(&p, "host", 22, k.public_key(), true).unwrap();
    verify_host(&p, "host", 22, k.public_key(), false).unwrap();
}
#[test]
fn changed_key_always_rejected() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    verify_host(&p, "host", 22, key().public_key(), true).unwrap();
    let before = std::fs::read(&p).unwrap();
    for accept in [false, true] {
        assert!(
            verify_host(&p, "host", 22, key().public_key(), accept)
                .unwrap_err()
                .to_string()
                .contains("CHANGED")
        );
    }
    assert_eq!(before, std::fs::read(p).unwrap());
}
#[test]
fn different_ports_separate_identity() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    let a = key();
    let b = key();
    verify_host(&p, "host", 22, a.public_key(), true).unwrap();
    verify_host(&p, "host", 2222, b.public_key(), true).unwrap();
    verify_host(&p, "host", 2222, b.public_key(), false).unwrap();
    assert!(verify_host(&p, "host", 22, b.public_key(), false).is_err());
}
#[test]
fn no_duplicate_records() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    let k = key();
    for _ in 0..3 {
        verify_host(&p, "host", 22, k.public_key(), true).unwrap();
    }
    assert_eq!(std::fs::read_to_string(p).unwrap().lines().count(), 1);
}
#[test]
fn malformed_database_fails_closed() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    for content in [
        "bad",
        "host ssh-ed25519 rubbish",
        "@revoked host ssh-ed25519 rubbish",
        "|1|hashed ssh-ed25519 rubbish",
    ] {
        std::fs::write(&p, content).unwrap();
        assert!(verify_host(&p, "host", 22, key().public_key(), true).is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), content);
    }
}
#[test]
fn comments_and_missing_newline_preserved() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    std::fs::write(&p, "# note").unwrap();
    verify_host(&p, "host", 22, key().public_key(), true).unwrap();
    assert!(
        std::fs::read_to_string(p)
            .unwrap()
            .starts_with("# note\nhost ")
    );
}
#[test]
fn concurrent_tofu_rejects_second_key() {
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    let mut handles = Vec::new();
    for _ in 0..5 {
        let p = p.clone();
        handles.push(std::thread::spawn(move || {
            verify_host(&p, "host", 22, key().public_key(), true)
        }));
    }
    assert_eq!(
        handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(Result::is_ok)
            .count(),
        1
    );
}
#[cfg(unix)]
#[test]
fn created_database_private() {
    use std::os::unix::fs::PermissionsExt;
    let t = tempfile::tempdir().unwrap();
    let p = t.path().join("hosts");
    verify_host(&p, "host", 22, key().public_key(), true).unwrap();
    assert_eq!(
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777,
        0o600
    );
}
