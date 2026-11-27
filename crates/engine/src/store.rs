//! Crash-safe file writes.
//!
//! Every document the desk owns — the show, the desk file, later the autosave
//! snapshot — is written through [`write_atomic`]: the bytes go to a temporary
//! file in the same directory, are flushed and synced, and only then replace the
//! target by rename. A failure at any point leaves the previous file intact, so
//! a power cut can never cost more than the unsaved edit.
//!
//! One generation of backup is kept as `<path>.bak`, taken immediately before
//! the rename, so even a successful rename of bad content costs one save, not
//! the document.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Write `bytes` to `path` atomically, keeping the previous generation.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)?;
        }
    }
    let temp = sibling(path, "tmp");
    let result = write_then_rename(path, &temp, bytes);
    if result.is_err() {
        // The original is untouched; a stray temp file is only clutter.
        let _ = fs::remove_file(&temp);
    }
    result
}

fn write_then_rename(path: &Path, temp: &Path, bytes: &[u8]) -> std::io::Result<()> {
    {
        let mut file = fs::File::create(temp)?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    if path.is_file() {
        // A copy, not a rename: the target must stay in place until the new
        // content is ready to replace it.
        let _ = fs::copy(path, sibling(path, "bak"));
    }
    replace_file(temp, path)
}

/// Rename `temp` over `path`, replacing an existing document.
///
/// POSIX `rename` replaces the target; Windows refuses to rename onto an
/// existing file, so the old document is removed first. The `.bak` copy above
/// already holds the previous generation, so a crash between the two steps
/// still leaves a recoverable show.
#[cfg(windows)]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    if path.is_file() {
        fs::remove_file(path)?;
    }
    fs::rename(temp, path)
}

#[cfg(not(windows))]
fn replace_file(temp: &Path, path: &Path) -> std::io::Result<()> {
    fs::rename(temp, path)
}

fn sibling(path: &Path, extension: &str) -> PathBuf {
    let mut name: OsString = path
        .file_name()
        .map(|name| name.to_os_string())
        .unwrap_or_else(|| OsString::from("document"));
    name.push(format!(".{extension}"));
    path.with_file_name(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("openlight-store-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn writes_the_file_and_leaves_no_temp() {
        let dir = temp_dir("write");
        let path = dir.join("show.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        assert!(!sibling(&path, "tmp").exists());
        assert!(
            !sibling(&path, "bak").exists(),
            "no previous generation yet"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn replaces_atomically_and_keeps_one_generation() {
        let dir = temp_dir("replace");
        let path = dir.join("show.json");
        write_atomic(&path, b"first").unwrap();
        write_atomic(&path, b"second").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"second");
        assert_eq!(fs::read(sibling(&path, "bak")).unwrap(), b"first");
        assert!(!sibling(&path, "tmp").exists());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_write_leaves_the_previous_file() {
        let dir = temp_dir("failure");
        let path = dir.join("show.json");
        write_atomic(&path, b"first").unwrap();
        // A directory where the temp file would go makes the create fail.
        fs::create_dir_all(sibling(&path, "tmp")).unwrap();
        assert!(write_atomic(&path, b"second").is_err());
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_parent_directories_are_created() {
        let dir = temp_dir("parents");
        let path = dir.join("shows/gig/show.json");
        write_atomic(&path, b"first").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"first");
        let _ = fs::remove_dir_all(&dir);
    }
}
