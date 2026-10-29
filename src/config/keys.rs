//! The key store: the Ed25519 private key of an agent, OpenSSH format, mode 600.

use super::io;
use crate::error::{CbError, CbResult};
#[cfg(unix)]
use crate::fsutil::mode_is_private;
use crate::protocol::names::AgentName;
use crate::protocol::signature::generate_key;
use ssh_key::{LineEnding, PrivateKey};
use std::path::{Path, PathBuf};

/// Load the private key. On Unix the file must be mode 600 (or stricter).
pub fn load_key(path: &Path) -> CbResult<PrivateKey> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let meta = std::fs::metadata(path)
            .map_err(|e| CbError::config(format!("key {}: {e}", path.display())))?;
        if !mode_is_private(meta.permissions().mode()) {
            return Err(CbError::config(format!(
                "key {} has mode {:o}; it must be 600",
                path.display(),
                meta.permissions().mode() & 0o777
            )));
        }
    }
    let text = std::fs::read_to_string(path)
        .map_err(|e| CbError::config(format!("key {}: {e}", path.display())))?;
    let key = PrivateKey::from_openssh(&text)
        .map_err(|e| CbError::config(format!("key {}: {e}", path.display())))?;
    if key.algorithm() != ssh_key::Algorithm::Ed25519 {
        return Err(CbError::config(format!(
            "key {} is not an Ed25519 key",
            path.display()
        )));
    }
    Ok(key)
}

/// Create the key if the file does not exist; otherwise load it. Returns `(key, created)`.
pub fn load_or_create_key(path: &Path) -> CbResult<(PrivateKey, bool)> {
    if path.exists() {
        return Ok((load_key(path)?, false));
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(io("create key dir"))?;
    }
    let key = generate_key()?;
    let pem = key
        .to_openssh(LineEnding::LF)
        .map_err(|e| CbError::config(format!("encode key: {e}")))?;
    write_private(path, pem.as_bytes())?;
    let pubkey = key
        .public_key()
        .to_openssh()
        .map_err(|e| CbError::config(format!("encode public key: {e}")))?;
    let mut pub_path = path.as_os_str().to_owned();
    pub_path.push(".pub");
    std::fs::write(PathBuf::from(pub_path), format!("{pubkey}\n"))
        .map_err(io("write public key"))?;
    Ok((key, true))
}

fn write_private(path: &Path, data: &[u8]) -> CbResult<()> {
    use std::io::Write;
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts
        .open(path)
        .map_err(|e| CbError::config(format!("create key {}: {e}", path.display())))?;
    f.write_all(data).map_err(io("write key"))
}

/// The public key line (`ssh-ed25519 AAAA... <comment>`), with the agent name as comment.
pub fn public_line(key: &PrivateKey, agent: &AgentName) -> CbResult<String> {
    let mut p = key.public_key().clone();
    p.set_comment(agent.as_str());
    p.to_openssh()
        .map_err(|e| CbError::config(format!("encode public key: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ConfigDir;

    #[test]
    fn key_is_created_with_mode_600_and_reloads_to_the_same_key() {
        let dir = tempfile::tempdir().unwrap();
        let name = AgentName::parse("claude@t1").unwrap();
        let cd = ConfigDir(dir.path().to_path_buf());
        let p = cd.key_path(&name);
        let (k1, created) = load_or_create_key(&p).unwrap();
        assert!(created);
        let (k2, created2) = load_or_create_key(&p).unwrap();
        assert!(!created2);
        assert_eq!(k1.public_key(), k2.public_key());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = std::fs::metadata(&p).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
        }
        let line = public_line(&k1, &name).unwrap();
        assert!(line.starts_with("ssh-ed25519 ") && line.ends_with("claude@t1"));
    }

    #[cfg(unix)]
    #[test]
    fn a_key_with_open_permissions_is_refused() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("k");
        load_or_create_key(&p).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = load_key(&p).unwrap_err();
        assert!(e.to_string().contains("must be 600"), "{e}");
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert!(load_key(&p).is_ok());
    }
}
