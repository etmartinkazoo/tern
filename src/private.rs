//! Files that hold mail, contacts, tokens or passwords are readable only by you: their
//! folders are 0700 and the files 0600, whatever the umask says.

use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::Path;

/// Create a folder (and its parents) and make it owner-only.
pub fn dir(p: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(p)?;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o700))
}

/// Write a file owner-only, atomically: a temporary file next to it, then a rename. Its
/// folder is created owner-only if missing.
pub fn write(p: &Path, bytes: impl AsRef<[u8]>) -> std::io::Result<()> {
    if let Some(parent) = p.parent() {
        if !parent.exists() {
            dir(parent)?;
        }
    }
    let mut name = p.file_name().unwrap_or_default().to_os_string();
    name.push(".tmp");
    let tmp = p.with_file_name(name);
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        // `mode` only applies when the file is new.
        f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        f.write_all(bytes.as_ref())?;
        f.sync_all()?;
    }
    std::fs::rename(tmp, p)
}

/// At startup: make tern's own config, data and cache folders owner-only, which also
/// covers anything older versions left in them with looser permissions.
pub fn harden() {
    for d in [crate::config::config_dir(), crate::config::cache_dir(), dirs::data_dir().unwrap_or_default().join("tern")] {
        if d.is_dir() {
            let _ = std::fs::set_permissions(&d, std::fs::Permissions::from_mode(0o700));
        }
    }
    let cfg = crate::config::default_path();
    if cfg.is_file() {
        let _ = std::fs::set_permissions(&cfg, std::fs::Permissions::from_mode(0o600));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_only() {
        let root = std::env::temp_dir().join(format!("tern-private-{}", uuid::Uuid::new_v4()));
        let f = root.join("a/b/secret.json");
        write(&f, b"x").unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&f), 0o600);
        assert_eq!(mode(&root.join("a/b")), 0o700);
        // Rewriting a file that was world-readable tightens it.
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o644)).unwrap();
        write(&f, b"y").unwrap();
        assert_eq!(mode(&f), 0o600);
        assert_eq!(std::fs::read(&f).unwrap(), b"y");
        std::fs::remove_dir_all(root).unwrap();
    }
}
