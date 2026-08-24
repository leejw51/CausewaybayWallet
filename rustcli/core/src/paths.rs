//! Resolution and creation of the wallet home directory.

use std::path::{Path, PathBuf};

use crate::error::{self, Result};

pub const HOME_ENV: &str = "CAUSEWAYBAY_HOME";
pub const DEFAULT_DIR: &str = ".causewaybaywallet";

/// Resolve the wallet home: explicit flag, then `CAUSEWAYBAY_HOME`, then `~/.causewaybaywallet`.
pub fn resolve_home(explicit: Option<&Path>) -> Result<PathBuf> {
    if let Some(p) = explicit {
        return Ok(expand_tilde(p));
    }
    if let Ok(v) = std::env::var(HOME_ENV) {
        if !v.trim().is_empty() {
            return Ok(expand_tilde(Path::new(v.trim())));
        }
    }
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .map_err(|_| {
            error::internal("cannot determine the user home directory; set CAUSEWAYBAY_HOME")
        })?;
    Ok(PathBuf::from(home).join(DEFAULT_DIR))
}

/// Expand a leading `~`, which a shell would have done for an unquoted path.
///
/// Without this, `--home '~/wallets'` silently creates a directory literally
/// named `~` in the working directory.
fn expand_tilde(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    let rest = match text.strip_prefix("~/") {
        Some(rest) => rest,
        None if text == "~" => "",
        None => return path.to_path_buf(),
    };
    match std::env::var("HOME").or_else(|_| std::env::var("USERPROFILE")) {
        Ok(home) if !home.is_empty() => PathBuf::from(home).join(rest),
        _ => path.to_path_buf(),
    }
}

/// Create the home directory if missing, restricting it to the owner.
pub fn ensure_dir(dir: &Path) -> Result<()> {
    if !dir.exists() {
        std::fs::create_dir_all(dir)?;
    }
    set_private(dir, 0o700)
}

/// Write a file that is owner-only from the moment it exists.
///
/// Used for exports that carry key material: `fs::write` + a later chmod would
/// leave a window in which the file sits behind the umask, and an export lands
/// in an unprotected working directory rather than the 0700 wallet home.
pub fn write_private(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(path)?;
    // The mode above applies only on creation; a pre-existing file keeps its
    // permissions, so tighten those as well.
    set_private(path, 0o600)?;
    file.write_all(contents.as_ref())?;
    file.flush()?;
    Ok(())
}

/// Tighten permissions on a path. A no-op on platforms without Unix modes.
pub fn set_private(path: &Path, mode: u32) -> Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(mode);
        std::fs::set_permissions(path, perms)?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_leading_tilde_is_expanded() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".into());
        assert_eq!(
            expand_tilde(Path::new("~/wallets")),
            PathBuf::from(&home).join("wallets")
        );
        assert_eq!(expand_tilde(Path::new("~")), PathBuf::from(&home));
        // Only a leading `~/` counts; these are ordinary names.
        assert_eq!(expand_tilde(Path::new("/tmp/x")), PathBuf::from("/tmp/x"));
        assert_eq!(expand_tilde(Path::new("./~/x")), PathBuf::from("./~/x"));
        assert_eq!(expand_tilde(Path::new("~user/x")), PathBuf::from("~user/x"));
    }

    #[test]
    fn an_explicit_home_wins_over_the_environment() {
        let explicit = PathBuf::from("/tmp/explicit-home");
        assert_eq!(resolve_home(Some(&explicit)).unwrap(), explicit);
    }
}
