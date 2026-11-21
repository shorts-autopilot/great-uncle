//! File management beside the show/desk pair.
//!
//! This module owns the filesystem operations that are not a document write:
//! timestamped backups, folder listings, and copying, renaming or deleting a
//! show together with its companion desk. The document formats stay in
//! [`crate::show`] and [`crate::desk`]; nothing here changes a byte of either.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use thiserror::Error;

use crate::desk::Desk;
use crate::limits::MAX_SHOW_BYTES;
use crate::store;

/// Number of timestamped backups retained for one show.
pub const BACKUP_LIMIT: usize = 10;
/// Directory created beside a show for its timestamped backups.
pub const BACKUP_DIRECTORY: &str = "backups";

/// One show file found by a folder listing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShowFileInfo {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

/// A file operation that could not be completed.
#[derive(Debug, Error)]
pub enum FileError {
    #[error("{path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: io::Error,
    },
    #[error("{0}")]
    Invalid(String),
}

impl FileError {
    fn io(path: &Path, source: io::Error) -> Self {
        Self::Io {
            path: path.to_path_buf(),
            source,
        }
    }
}

/// True for the two spellings used by show files: `look.olshow` and
/// `look.olshow.json`. The check is case-insensitive so a listing behaves the
/// same on Windows and macOS, where file names are commonly case-folded.
pub fn is_show_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    lower.ends_with(".olshow") || lower.ends_with(".olshow.json")
}

/// List the show files in `directory`, sorted by file name.
///
/// Only directory metadata is read. The listing deliberately does not parse
/// each document: a folder with a hundred shows should open instantly, and a
/// damaged file must still be visible so the operator can find and repair it.
pub fn list_show_files(directory: &Path) -> Result<Vec<ShowFileInfo>, FileError> {
    let entries = fs::read_dir(directory).map_err(|error| FileError::io(directory, error))?;
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|error| FileError::io(directory, error))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !is_show_file_name(&name) {
            continue;
        }
        let metadata = entry
            .metadata()
            .map_err(|error| FileError::io(&entry.path(), error))?;
        if !metadata.is_file() {
            continue;
        }
        files.push(ShowFileInfo {
            path: entry.path(),
            name,
            bytes: metadata.len(),
            modified: metadata.modified().ok(),
        });
    }
    files.sort_by(|left, right| left.name.cmp(&right.name));
    Ok(files)
}

/// The UTC time used in a backup filename: `YYYYMMDDTHHMMSSZ`.
pub fn utc_stamp(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(time);
    format!("{year:04}{month:02}{day:02}T{hour:02}{minute:02}{second:02}Z")
}

/// The UTC time used in a listing: `YYYY-MM-DD HH:MM:SS UTC`.
pub fn format_utc(time: SystemTime) -> String {
    let (year, month, day, hour, minute, second) = utc_parts(time);
    format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02} UTC")
}

fn utc_parts(time: SystemTime) -> (i64, u32, u32, u32, u32, u32) {
    let seconds = time
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let days = seconds / 86_400;
    let remainder = seconds % 86_400;
    let (year, month, day) = civil_from_days(days as i64);
    let hour = (remainder / 3_600) as u32;
    let minute = ((remainder % 3_600) / 60) as u32;
    let second = (remainder % 60) as u32;
    (year, month, day, hour, minute, second)
}

/// Howard Hinnant's civil-from-days algorithm. `days` is an offset from
/// 1970-01-01; the result is the proleptic Gregorian date.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let day_of_era = z - era * 146_097;
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let mut year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_prime = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * month_prime + 2) / 5 + 1;
    let month = month_prime + if month_prime < 10 { 3 } else { -9 };
    if month <= 2 {
        year += 1;
    }
    (year, month as u32, day as u32)
}

/// The directory that holds a show's timestamped backups.
pub fn backup_directory(show_path: &Path) -> PathBuf {
    show_path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."))
        .join(BACKUP_DIRECTORY)
}

/// A safe filename stem for a show. Only letters, digits, `.`, `_` and `-` are
/// kept; everything else becomes `-`. This prevents a show name from creating
/// a subdirectory or a path traversal when it is used for a backup.
fn safe_stem(base: &str) -> String {
    let mut stem: String = base
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '.' | '_' | '-') {
                character
            } else {
                '-'
            }
        })
        .collect();
    stem = stem.trim_matches('-').to_string();
    if stem.is_empty() || stem == "." || stem == ".." {
        "show".into()
    } else {
        stem
    }
}

fn show_stem(show_path: &Path) -> String {
    let Some(name) = show_path.file_name().and_then(|name| name.to_str()) else {
        return "show".into();
    };
    let lower = name.to_ascii_lowercase();
    let base = if lower.ends_with(".olshow.json") {
        &name[..name.len() - ".olshow.json".len()]
    } else if lower.ends_with(".olshow") {
        &name[..name.len() - ".olshow".len()]
    } else {
        show_path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("show")
    };
    safe_stem(base)
}

fn unique_backup(directory: &Path, stem: &str, stamp: &str) -> PathBuf {
    let first = directory.join(format!("{stem}-{stamp}.olshow"));
    if !first.exists() {
        return first;
    }
    for counter in 2u32.. {
        let candidate = directory.join(format!("{stem}-{stamp}-{counter}.olshow"));
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!("the counter is unbounded")
}

/// The next available backup path for `show_path` at `now`.
///
/// The path is deterministic for a given time. A second save in the same
/// second gets `-2`, `-3` and so on; the suffix is inserted before the
/// extension so the file remains a normal `.olshow` document.
pub fn backup_path(show_path: &Path, now: SystemTime) -> PathBuf {
    unique_backup(
        &backup_directory(show_path),
        &show_stem(show_path),
        &utc_stamp(now),
    )
}

/// The default explicit-`Backup` path: `backups/<show>-<UTC>.olshow` in the
/// current directory. The show name is sanitised the same way as a file stem.
pub fn default_backup_path(show_name: &str, now: SystemTime) -> PathBuf {
    unique_backup(
        Path::new(BACKUP_DIRECTORY),
        &safe_stem(show_name),
        &utc_stamp(now),
    )
}

/// Copy an existing show into its backup folder. Returns the new path, or
/// `None` when the source does not exist yet.
///
/// Retention is best-effort: a backup that was written successfully is not
/// reported as failed because an old file could not be removed.
pub fn backup(show_path: &Path, now: SystemTime) -> Result<Option<PathBuf>, FileError> {
    if !show_path.is_file() {
        return Ok(None);
    }
    let bytes = read_show_bytes(show_path)?;
    let destination = backup_path(show_path, now);
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent).map_err(|error| FileError::io(parent, error))?;
    }
    store::write_atomic(&destination, &bytes)
        .map_err(|error| FileError::io(&destination, error))?;
    prune_backups(show_path);
    Ok(Some(destination))
}

/// Remove the oldest matching backups until at most [`BACKUP_LIMIT`] remain.
///
/// Returns the paths that were removed. A missing backup directory is not an
/// error: it simply has nothing to prune.
pub fn prune_backups(show_path: &Path) -> Vec<PathBuf> {
    let directory = backup_directory(show_path);
    let Ok(entries) = fs::read_dir(&directory) else {
        return Vec::new();
    };
    let stem = show_stem(show_path);
    let prefix = format!("{stem}-");
    let mut backups: Vec<(PathBuf, SystemTime)> = entries
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            let lower = name.to_ascii_lowercase();
            if !lower.ends_with(".olshow") || !name.starts_with(&prefix) {
                return None;
            }
            let metadata = entry.metadata().ok()?;
            if !metadata.is_file() {
                return None;
            }
            let modified = metadata.modified().unwrap_or(UNIX_EPOCH);
            Some((entry.path(), modified))
        })
        .collect();
    // Modification time is the real age; the name is a stable tie-breaker for
    // files written within the same clock tick.
    backups.sort_by(|left, right| left.1.cmp(&right.1).then_with(|| left.0.cmp(&right.0)));
    let mut removed = Vec::new();
    while backups.len() > BACKUP_LIMIT {
        let (path, _) = backups.remove(0);
        if fs::remove_file(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
}

/// Read a show-sized file, refusing a runaway document before allocating for
/// it. The same limit is used for the companion desk: it is a small file, and
/// a corrupt/grown file must not make a copy operation allocate without bound.
fn read_show_bytes(path: &Path) -> Result<Vec<u8>, FileError> {
    let metadata = fs::metadata(path).map_err(|error| FileError::io(path, error))?;
    if metadata.len() > MAX_SHOW_BYTES {
        return Err(FileError::Invalid(format!(
            "{}: file is larger than the {} byte limit",
            path.display(),
            MAX_SHOW_BYTES
        )));
    }
    fs::read(path).map_err(|error| FileError::io(path, error))
}

/// Copy a show and its companion desk. The destination must not exist, and
/// the pair is treated as a unit: a failed desk copy removes the new show.
pub fn copy_show(source: &Path, destination: &Path) -> Result<(), FileError> {
    if source == destination {
        return Err(FileError::Invalid(
            "source and destination are the same file".into(),
        ));
    }
    if !source.is_file() {
        return Err(FileError::Invalid(format!(
            "{}: not a show file",
            source.display()
        )));
    }
    if destination.exists() {
        return Err(FileError::Invalid(format!(
            "{}: destination already exists",
            destination.display()
        )));
    }
    let source_desk = Desk::path_for(source);
    let destination_desk = Desk::path_for(destination);
    // Even a source without a desk must not inherit a stale desk at the
    // destination: the two files would then describe different documents.
    if destination_desk.exists() {
        return Err(FileError::Invalid(format!(
            "{}: destination desk already exists",
            destination_desk.display()
        )));
    }
    let bytes = read_show_bytes(source)?;
    store::write_atomic(destination, &bytes).map_err(|error| FileError::io(destination, error))?;
    if source_desk.is_file() {
        let desk_bytes = match read_show_bytes(&source_desk) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = fs::remove_file(destination);
                return Err(error);
            }
        };
        if let Err(error) = store::write_atomic(&destination_desk, &desk_bytes) {
            let _ = fs::remove_file(destination);
            return Err(FileError::io(&destination_desk, error));
        }
    }
    Ok(())
}

/// Rename a show and its companion desk. The destination must not exist, and
/// the show is moved back if the desk cannot be moved.
pub fn rename_show(source: &Path, destination: &Path) -> Result<(), FileError> {
    if source == destination {
        return Err(FileError::Invalid(
            "source and destination are the same file".into(),
        ));
    }
    if !source.is_file() {
        return Err(FileError::Invalid(format!(
            "{}: not a show file",
            source.display()
        )));
    }
    if destination.exists() {
        return Err(FileError::Invalid(format!(
            "{}: destination already exists",
            destination.display()
        )));
    }
    let source_desk = Desk::path_for(source);
    let destination_desk = Desk::path_for(destination);
    // See `copy_show`: a stale destination desk is never attached to a moved
    // show, even when the source has no desk of its own.
    if destination_desk.exists() {
        return Err(FileError::Invalid(format!(
            "{}: destination desk already exists",
            destination_desk.display()
        )));
    }
    fs::rename(source, destination).map_err(|error| FileError::io(source, error))?;
    if source_desk.is_file() {
        if let Err(error) = fs::rename(&source_desk, &destination_desk) {
            let rollback = fs::rename(destination, source);
            let suffix = match rollback {
                Ok(()) => String::new(),
                Err(rollback_error) => {
                    format!("; moving the show back also failed: {rollback_error}")
                }
            };
            return Err(FileError::Invalid(format!(
                "{}: {error}{suffix}",
                source_desk.display()
            )));
        }
    }
    Ok(())
}

/// Delete a show and its companion desk. The `.bak` generation and the
/// `backups/` folder are deliberately left alone.
pub fn delete_show(path: &Path) -> Result<Vec<PathBuf>, FileError> {
    if !path.is_file() {
        return Err(FileError::Invalid(format!(
            "{}: not a show file",
            path.display()
        )));
    }
    let mut removed = Vec::new();
    fs::remove_file(path).map_err(|error| FileError::io(path, error))?;
    removed.push(path.to_path_buf());
    let desk = Desk::path_for(path);
    if desk.is_file() {
        fs::remove_file(&desk).map_err(|error| FileError::io(&desk, error))?;
        removed.push(desk);
    }
    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        let directory =
            std::env::temp_dir().join(format!("openlight-files-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        directory
    }

    fn time(seconds: u64) -> SystemTime {
        UNIX_EPOCH + Duration::from_secs(seconds)
    }

    #[test]
    fn utc_epoch_and_leap_day_are_formatted() {
        assert_eq!(utc_stamp(time(0)), "19700101T000000Z");
        assert_eq!(format_utc(time(0)), "1970-01-01 00:00:00 UTC");
        // 2024-02-29 00:00:00 UTC.
        assert_eq!(utc_stamp(time(1_709_164_800)), "20240229T000000Z");
        assert_eq!(format_utc(time(1_709_164_800)), "2024-02-29 00:00:00 UTC");
    }

    #[test]
    fn lists_show_files_and_skips_other_files_and_directories() {
        let directory = temp_dir("list");
        fs::write(directory.join("b.olshow"), b"b").unwrap();
        fs::write(directory.join("a.olshow.json"), b"aa").unwrap();
        fs::write(directory.join("notes.txt"), b"note").unwrap();
        fs::create_dir_all(directory.join("folder.olshow")).unwrap();

        let files = list_show_files(&directory).unwrap();
        let names: Vec<&str> = files.iter().map(|file| file.name.as_str()).collect();
        assert_eq!(names, vec!["a.olshow.json", "b.olshow"]);
        assert_eq!(files[0].bytes, 2);
        assert!(files[0].modified.is_some());
        assert!(list_show_files(&directory.join("missing")).is_err());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn backup_names_are_safe_and_same_second_collisions_get_a_suffix() {
        let directory = temp_dir("backup-name");
        let show = directory.join("look.olshow");
        fs::write(&show, b"show").unwrap();
        let first = backup(&show, time(1_709_164_800)).unwrap().unwrap();
        assert_eq!(
            first,
            directory.join("backups/look-20240229T000000Z.olshow")
        );
        let second = backup(&show, time(1_709_164_800)).unwrap().unwrap();
        assert_eq!(
            second,
            directory.join("backups/look-20240229T000000Z-2.olshow")
        );

        let spaced = directory.join("My Show.olshow");
        fs::write(&spaced, b"show").unwrap();
        let path = backup_path(&spaced, time(0));
        assert_eq!(path.file_name().unwrap(), "My-Show-19700101T000000Z.olshow");
        assert_eq!(
            default_backup_path("My Show", time(0)),
            PathBuf::from("backups/My-Show-19700101T000000Z.olshow")
        );
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn keeps_ten_backups_and_leaves_other_files_alone() {
        let directory = temp_dir("backup-retain");
        let show = directory.join("look.olshow");
        fs::write(&show, b"show").unwrap();
        let unrelated = directory.join("backups/other.olshow");
        fs::create_dir_all(unrelated.parent().unwrap()).unwrap();
        fs::write(&unrelated, b"keep").unwrap();

        for second in 0..11u64 {
            backup(&show, time(second)).unwrap();
        }
        let backups = list_show_files(&directory.join("backups")).unwrap();
        assert_eq!(
            backups
                .iter()
                .filter(|file| file.name.starts_with("look-"))
                .count(),
            BACKUP_LIMIT
        );
        assert!(backups.iter().any(|file| file.name == "other.olshow"));
        assert!(!directory
            .join("backups/look-19700101T000000Z.olshow")
            .exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn backup_missing_show_is_a_no_op() {
        let directory = temp_dir("backup-missing");
        let show = directory.join("missing.olshow");
        assert_eq!(backup(&show, time(0)).unwrap(), None);
        assert!(!directory.join("backups").exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn copies_the_show_and_its_companion_desk() {
        let directory = temp_dir("copy");
        let source = directory.join("source.olshow");
        let destination = directory.join("destination.olshow");
        fs::write(&source, b"show").unwrap();
        fs::write(Desk::path_for(&source), b"desk").unwrap();

        copy_show(&source, &destination).unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"show");
        assert_eq!(fs::read(Desk::path_for(&destination)).unwrap(), b"desk");

        assert!(copy_show(&source, &destination).is_err());
        assert_eq!(fs::read(&source).unwrap(), b"show");
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn a_stale_destination_desk_is_not_attached_to_a_show_without_one() {
        let directory = temp_dir("copy-stale-desk");
        let source = directory.join("source.olshow");
        let destination = directory.join("destination.olshow");
        fs::write(&source, b"show").unwrap();
        fs::write(Desk::path_for(&destination), b"stale desk").unwrap();

        assert!(copy_show(&source, &destination).is_err());
        assert!(!destination.exists());
        assert!(source.exists());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn renames_the_show_and_its_companion_desk() {
        let directory = temp_dir("rename");
        let source = directory.join("source.olshow");
        let destination = directory.join("destination.olshow");
        fs::write(&source, b"show").unwrap();
        fs::write(Desk::path_for(&source), b"desk").unwrap();

        rename_show(&source, &destination).unwrap();
        assert!(!source.exists());
        assert_eq!(fs::read(&destination).unwrap(), b"show");
        assert_eq!(fs::read(Desk::path_for(&destination)).unwrap(), b"desk");
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn delete_removes_the_pair_but_keeps_recovery_files() {
        let directory = temp_dir("delete");
        let show = directory.join("look.olshow");
        fs::write(&show, b"show").unwrap();
        fs::write(Desk::path_for(&show), b"desk").unwrap();
        fs::write(directory.join("look.olshow.bak"), b"old").unwrap();
        let backup_path = backup(&show, time(0)).unwrap().unwrap();

        let removed = delete_show(&show).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!show.exists());
        assert!(!Desk::path_for(&show).exists());
        assert!(directory.join("look.olshow.bak").exists());
        assert!(backup_path.exists());
        assert!(delete_show(&directory).is_err());
        let _ = fs::remove_dir_all(&directory);
    }

    #[test]
    fn show_file_names_are_recognised_case_insensitively() {
        assert!(is_show_file_name("look.olshow"));
        assert!(is_show_file_name("look.OLSHOW.JSON"));
        assert!(!is_show_file_name("look.oldesk"));
        assert!(!is_show_file_name("look.json"));
    }
}
