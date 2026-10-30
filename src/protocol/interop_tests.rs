//! Interop proof: crewboard and `ssh-keygen` accept each other's signatures
//! (spec/identity#author-and-signature).
//!
//! These tests need `ssh-keygen` with `-Y` support. If it is missing they print a clear
//! SKIPPED line and pass, unless `CREWBOARD_REQUIRE_SSH_KEYGEN=1` is set (CI sets it on
//! Linux and macOS), in which case a missing tool is a failure. Each direction has a
//! planted negative case, so the check can fail.

use super::SIG_NAMESPACE;
use super::canonical::{IssueRef, canonical};
use super::names::{AgentName, Kind, Re, Timestamp, To};
use super::signature::tests::armor_to_sig;
use super::signature::{Outgoing, generate_key, sig_to_armor, sign_message, verify_message};
use ssh_key::PublicKey;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

/// `true` if the tests can run. Prints a SKIPPED line otherwise, or panics when required.
fn have_ssh_keygen(test: &str) -> bool {
    let ok = Command::new("ssh-keygen")
        .arg("-?")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok();
    if !ok {
        if std::env::var("CREWBOARD_REQUIRE_SSH_KEYGEN").as_deref() == Ok("1") {
            panic!("{test}: ssh-keygen not found but CREWBOARD_REQUIRE_SSH_KEYGEN=1");
        }
        eprintln!("SKIPPED {test}: ssh-keygen not found in PATH (interop proof not run)");
    }
    ok
}

fn issue() -> IssueRef {
    IssueRef::new("example-org", "app", 12)
}

fn canonical_bytes(key: &ssh_key::PrivateKey) -> (Vec<u8>, String) {
    let out = Outgoing {
        from: AgentName::parse("hermes@host-b").unwrap(),
        to: To::parse("claude@host-a").unwrap(),
        kind: Kind::Report,
        re: Re::None,
        ts: Timestamp::parse("2026-10-15T14:05:00Z").unwrap(),
    };
    let msg = sign_message(key, &issue(), out, "interop body").unwrap();
    verify_message(&msg, &issue(), key.public_key()).unwrap();
    (
        canonical(&issue(), &msg.header, &msg.body).into_bytes(),
        msg.header.sig,
    )
}

/// Run `ssh-keygen -Y verify`. Returns true if it accepted the signature.
fn keygen_verify(
    dir: &Path,
    pubkey_line: &str,
    namespace: &str,
    sig_armor: &str,
    data: &[u8],
) -> bool {
    let allowed = dir.join("allowed_signers");
    std::fs::write(
        &allowed,
        format!("agent namespaces=\"{SIG_NAMESPACE}\" {pubkey_line}\n"),
    )
    .unwrap();
    let sig_file = dir.join("msg.sig");
    std::fs::write(&sig_file, sig_armor).unwrap();
    let mut child = Command::new("ssh-keygen")
        .args(["-Y", "verify", "-I", "agent", "-n", namespace])
        .arg("-f")
        .arg(&allowed)
        .arg("-s")
        .arg(&sig_file)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(data).unwrap();
    child.wait().unwrap().success()
}

#[test]
fn crewboard_signs_and_ssh_keygen_verifies() {
    if !have_ssh_keygen("crewboard_signs_and_ssh_keygen_verifies") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let key = generate_key().unwrap();
    let pubkey = key.public_key().to_openssh().unwrap();
    let (data, sig) = canonical_bytes(&key);
    let armor = sig_to_armor(&sig);

    // Must pass.
    assert!(
        keygen_verify(dir.path(), &pubkey, SIG_NAMESPACE, &armor, &data),
        "ssh-keygen refused a valid crewboard signature"
    );
    // Must be refused: tampered data, wrong namespace, wrong key.
    let mut tampered = data.clone();
    tampered[0] ^= 1;
    assert!(
        !keygen_verify(dir.path(), &pubkey, SIG_NAMESPACE, &armor, &tampered),
        "tampered data accepted"
    );
    assert!(
        !keygen_verify(dir.path(), &pubkey, "other", &armor, &data),
        "wrong namespace accepted"
    );
    let other = generate_key().unwrap().public_key().to_openssh().unwrap();
    assert!(
        !keygen_verify(dir.path(), &other, SIG_NAMESPACE, &armor, &data),
        "wrong key accepted"
    );
}

#[test]
fn ssh_keygen_signs_and_crewboard_verifies() {
    if !have_ssh_keygen("ssh_keygen_signs_and_crewboard_verifies") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let keyfile = dir.path().join("id_ed25519");
    let gen_ok = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "interop", "-f"])
        .arg(&keyfile)
        .status()
        .unwrap()
        .success();
    assert!(gen_ok, "ssh-keygen could not create a key");
    let public = PublicKey::from_openssh(
        &std::fs::read_to_string(dir.path().join("id_ed25519.pub")).unwrap(),
    )
    .unwrap();

    // The data is a real crewboard canonical form.
    let throwaway = generate_key().unwrap();
    let (data, _) = canonical_bytes(&throwaway);
    let datafile = dir.path().join("msg");
    std::fs::write(&datafile, &data).unwrap();
    let signed = Command::new("ssh-keygen")
        .args(["-Y", "sign", "-n", SIG_NAMESPACE, "-f"])
        .arg(&keyfile)
        .arg(&datafile)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(signed, "ssh-keygen -Y sign failed");
    let sig = armor_to_sig(&std::fs::read_to_string(dir.path().join("msg.sig")).unwrap()).unwrap();

    // Must pass.
    super::signature::verify(&public, &data, &sig)
        .expect("crewboard refused a valid ssh-keygen signature");
    // Must be refused: tampered data, wrong key.
    let mut tampered = data.clone();
    tampered[0] ^= 1;
    assert!(
        super::signature::verify(&public, &tampered, &sig).is_err(),
        "tampered data accepted"
    );
    assert!(
        super::signature::verify(throwaway.public_key(), &data, &sig).is_err(),
        "wrong key accepted"
    );
}
