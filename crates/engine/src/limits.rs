//! Limits applied to untrusted input before it is parsed.
//!
//! A show, a `.gdtf` profile and an `.mvr` rig all come from outside the
//! project: a fixture library, a vendor, a colleague's email. Every one of
//! them is a zip archive or a JSON document that can claim a huge size, nest
//! entries or use `..` paths. The checks here run first, name the limit that
//! was exceeded, and keep the parsers away from input that is simply too big.
//!
//! The numbers are deliberately generous for real files — the largest vendor
//! GDTF libraries are a few hundred entries and a few megabytes — and small
//! enough that a hostile archive cannot exhaust memory before the reader sees
//! it.

use std::io::{Read, Seek};

use zip::ZipArchive;

/// Entries in one `.gdtf` or `.mvr` archive.
pub const MAX_ARCHIVE_ENTRIES: usize = 64;
/// Uncompressed bytes in one archive entry (4 MiB).
pub const MAX_ENTRY_BYTES: u64 = 4 * 1024 * 1024;
/// Uncompressed bytes across every entry of an archive (16 MiB).
pub const MAX_ARCHIVE_BYTES: u64 = 16 * 1024 * 1024;
/// DMX channels in one mode of a fixture type.
pub const MAX_MODE_CHANNELS: usize = 512;
/// Channel functions (named ranges) on one channel.
pub const MAX_CHANNEL_FUNCTIONS: usize = 4096;
/// Fixture types embedded in one show.
pub const MAX_SHOW_TYPES: usize = 1024;
/// Bytes in a show file on disk (64 MiB).
pub const MAX_SHOW_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum LimitError {
    #[error("archive has {found} entries; the limit is {MAX_ARCHIVE_ENTRIES}")]
    ArchiveEntries { found: usize },
    #[error(
        "archive entry `{name}` is {size} bytes uncompressed; the limit is {MAX_ENTRY_BYTES} bytes"
    )]
    EntryTooLarge { name: String, size: u64 },
    #[error("archive expands to {size} bytes; the limit is {MAX_ARCHIVE_BYTES} bytes")]
    ArchiveTooLarge { size: u64 },
    #[error(
        "archive entry `{name}` has an unsafe path (absolute, drive-qualified or containing `..`)"
    )]
    UnsafePath { name: String },
    #[error("mode `{mode}` has {found} channels; the limit is {MAX_MODE_CHANNELS}")]
    ModeChannels { mode: String, found: usize },
    #[error("channel `{attribute}` has {found} functions; the limit is {MAX_CHANNEL_FUNCTIONS}")]
    ChannelFunctions { attribute: String, found: usize },
    #[error("show has {found} fixture types; the limit is {MAX_SHOW_TYPES}")]
    FixtureTypes { found: usize },
    #[error("show file is {size} bytes; the limit is {MAX_SHOW_BYTES} bytes")]
    ShowTooLarge { size: u64 },
    #[error("unreadable archive: {0}")]
    Archive(String),
}

/// Check an archive's entry count, names and uncompressed sizes before any
/// entry is read. Directories count towards the entry limit but have no bytes.
pub fn check_archive<R: Read + Seek>(archive: &mut ZipArchive<R>) -> Result<(), LimitError> {
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(LimitError::ArchiveEntries {
            found: archive.len(),
        });
    }
    let mut total = 0u64;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|error| LimitError::Archive(error.to_string()))?;
        let name = entry
            .name()
            .map_err(|error| LimitError::Archive(error.to_string()))?
            .to_string();
        if !safe_path(&name) {
            return Err(LimitError::UnsafePath { name });
        }
        if entry.is_dir() {
            continue;
        }
        let size = entry.size();
        if size > MAX_ENTRY_BYTES {
            return Err(LimitError::EntryTooLarge { name, size });
        }
        total = total.saturating_add(size);
        if total > MAX_ARCHIVE_BYTES {
            return Err(LimitError::ArchiveTooLarge { size: total });
        }
    }
    Ok(())
}

/// Read one entry with a hard cap, regardless of what the central directory
/// claims its size is. A zip can lie about `size()`, so the limit is enforced
/// on the bytes actually produced rather than only in [`check_archive`].
///
/// The caller adds the returned length to its own running total, because one
/// archive's entries must also fit inside [`MAX_ARCHIVE_BYTES`].
pub fn read_entry(reader: &mut impl Read, name: &str) -> Result<Vec<u8>, LimitError> {
    let mut bytes = Vec::new();
    reader
        .take(MAX_ENTRY_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| LimitError::Archive(error.to_string()))?;
    let size = bytes.len() as u64;
    if size > MAX_ENTRY_BYTES {
        return Err(LimitError::EntryTooLarge {
            name: name.to_string(),
            size,
        });
    }
    Ok(bytes)
}

/// Whether an archive entry name is a plain relative path.
///
/// Nothing in the project extracts an archive to disk, but a reader that later
/// grows an `extract` must not inherit a zip-slip bug from its input, so the
/// check lives with the other limits.
pub fn safe_path(name: &str) -> bool {
    if name.is_empty() || name.starts_with('/') || name.starts_with('\\') {
        return false;
    }
    let normalized = name.replace('\\', "/");
    let bytes = normalized.as_bytes();
    if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
        return false;
    }
    !normalized.split('/').any(|part| part == "..")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn archive(entries: &[(&str, &[u8])]) -> ZipArchive<Cursor<Vec<u8>>> {
        let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let options = zip::write::SimpleFileOptions::default();
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        ZipArchive::new(writer.finish().unwrap()).unwrap()
    }

    #[test]
    fn a_small_archive_passes() {
        let mut archive = archive(&[("description.xml", b"<GDTF/>"), ("models/glb/a.glb", b"x")]);
        assert_eq!(check_archive(&mut archive), Ok(()));
    }

    #[test]
    fn sixty_five_entries_are_refused() {
        let names: Vec<String> = (0..=MAX_ARCHIVE_ENTRIES)
            .map(|index| format!("entry-{index}"))
            .collect();
        let entries: Vec<(&str, &[u8])> = names
            .iter()
            .map(|name| (name.as_str(), b"x".as_slice()))
            .collect();
        let mut archive = archive(&entries);
        assert_eq!(
            check_archive(&mut archive),
            Err(LimitError::ArchiveEntries {
                found: MAX_ARCHIVE_ENTRIES + 1
            })
        );
    }

    #[test]
    fn an_entry_over_four_mib_is_refused() {
        let big = vec![0u8; MAX_ENTRY_BYTES as usize + 1];
        let mut archive = archive(&[("big.bin", &big)]);
        assert_eq!(
            check_archive(&mut archive),
            Err(LimitError::EntryTooLarge {
                name: "big.bin".into(),
                size: MAX_ENTRY_BYTES + 1
            })
        );
    }

    #[test]
    fn an_archive_over_sixteen_mib_is_refused() {
        // Each entry is under the per-entry limit; together they are over the
        // archive limit, which is the one this test is about.
        let chunk = vec![0u8; 3 * 1024 * 1024 + 512 * 1024];
        let mut archive = archive(&[
            ("a.bin", &chunk),
            ("b.bin", &chunk),
            ("c.bin", &chunk),
            ("d.bin", &chunk),
            ("e.bin", &chunk),
            ("f.bin", &chunk),
        ]);
        assert!(matches!(
            check_archive(&mut archive),
            Err(LimitError::ArchiveTooLarge { .. })
        ));
    }

    #[test]
    fn an_entry_that_lies_about_its_size_is_cut_off() {
        let big = vec![0u8; MAX_ENTRY_BYTES as usize + 1];
        let mut reader: &[u8] = &big;
        assert_eq!(
            read_entry(&mut reader, "liar.bin"),
            Err(LimitError::EntryTooLarge {
                name: "liar.bin".into(),
                size: MAX_ENTRY_BYTES + 1
            })
        );
    }

    #[test]
    fn zip_slip_paths_are_refused() {
        for name in ["../escape", "a/../../escape", "/etc/passwd", "C:\\Windows"] {
            assert!(!safe_path(name), "{name} must be unsafe");
        }
        assert!(safe_path("description.xml"));
        assert!(safe_path("models/glb/a.glb"));

        let mut archive = archive(&[("../escape", b"x")]);
        assert_eq!(
            check_archive(&mut archive),
            Err(LimitError::UnsafePath {
                name: "../escape".into()
            })
        );
    }
}
