//! The remote bearer token. It is a file, never a log line, and the page asks
//! the user to paste it once into session storage.

use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

const TOKEN_BYTES: usize = 32;

/// `~/.openlight/remote.token`, or a relative path when the environment has no
/// home directory.
pub fn default_path() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    home.join(".openlight").join("remote.token")
}

/// Read an existing token or create a new 32-byte random one with mode 0600.
pub fn load_or_create(path: &Path) -> io::Result<String> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    match read_private_token(path) {
        Ok(token) => return Ok(token),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }

    let token = random_token();
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            writeln!(file, "{token}")?;
            file.sync_all()?;
            Ok(token)
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => read_private_token(path),
        Err(error) => Err(error),
    }
}

fn read_private_token(path: &Path) -> io::Result<String> {
    let token = read_token(path)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600))?;
    }
    Ok(token)
}

fn read_token(path: &Path) -> io::Result<String> {
    let mut token = String::new();
    std::fs::File::open(path)?.read_to_string(&mut token)?;
    let token = token.trim().to_string();
    if token.len() < 32 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "remote token is shorter than 32 characters",
        ));
    }
    Ok(token)
}

fn random_token() -> String {
    let mut bytes = [0u8; TOKEN_BYTES];
    #[cfg(unix)]
    {
        if let Ok(mut source) = std::fs::File::open("/dev/urandom") {
            if source.read_exact(&mut bytes).is_ok() {
                return hex(&bytes);
            }
        }
    }
    // A machine without `/dev/urandom` is not a supported show machine; this
    // fallback still avoids a constant token and keeps the server usable.
    let seed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos() as u64)
        .unwrap_or(0)
        ^ (std::process::id() as u64).rotate_left(17);
    let mut state = seed | 1;
    for byte in &mut bytes {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        *byte = state as u8;
    }
    hex(&bytes)
}

fn hex(bytes: &[u8]) -> String {
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push_str(&format!("{byte:02x}"));
    }
    text
}

/// Compare without an early return, so a timing side channel cannot reveal the
/// matching prefix length.
pub fn constant_time_eq(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (a, b) in left.iter().zip(right) {
        difference |= a ^ b;
    }
    difference == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_token_once_and_reuses_it() {
        let directory = std::env::temp_dir().join(format!(
            "openlight-remote-token-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = directory.join("remote.token");
        let first = load_or_create(&path).unwrap();
        let second = load_or_create(&path).unwrap();
        assert_eq!(first, second);
        assert_eq!(first.len(), TOKEN_BYTES * 2);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
        let _ = fs::remove_dir_all(&directory);
    }

    #[cfg(unix)]
    #[test]
    fn an_existing_token_is_made_private() {
        use std::os::unix::fs::PermissionsExt;
        let directory = std::env::temp_dir().join(format!(
            "openlight-remote-token-private-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("remote.token");
        let token = "a".repeat(TOKEN_BYTES * 2);
        std::fs::write(&path, &token).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create(&path).unwrap(), token);
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn comparison_rejects_different_values_and_lengths() {
        assert!(constant_time_eq("abcdef", "abcdef"));
        assert!(!constant_time_eq("abcdef", "abcdeg"));
        assert!(!constant_time_eq("abcdef", "abcde"));
    }
}
