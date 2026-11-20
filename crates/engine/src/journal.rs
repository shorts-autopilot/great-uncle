//! The journal and the autosave snapshot: what survives a crash.
//!
//! A desk is programmed over hours, and the only copy of that work between
//! saves is memory. Two files close the gap:
//!
//! * the **journal** is an append-only list of the statements that changed
//!   persisted state, flushed after each one, so replaying it reproduces the
//!   session using the same code path as normal editing;
//! * the **autosave** is a full snapshot of the show and the desk, written
//!   periodically and after every `Save`, so replay never has to begin from an
//!   empty document.
//!
//! Both live beside the show: `look.olshow.journal` and `look.olshow.autosave`.
//! Replay is deliberately dull — it is `Engine::execute` on each journaled
//! line — because a recovery path that reimplements command semantics will
//! eventually disagree with them.

use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::desk::{Desk, DeskFile};
use crate::show::Show;
use crate::store;

/// Rotate at 8 MiB, roughly sixty thousand statements. The autosave means an
/// old journal is never needed, so one generation is all that is kept.
pub const JOURNAL_LIMIT: u64 = 8 * 1024 * 1024;

/// Which layer an entry changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EntryKind {
    Show,
    Programmer,
    Desk,
}

/// One journaled statement.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    /// Monotonic across sessions; replay requires an unbroken chain.
    pub rev: u64,
    /// RFC 3339 in UTC. Local time would need a timezone database.
    pub at: String,
    pub line: String,
    pub kind: EntryKind,
}

/// What was read back from a journal file.
#[derive(Debug, Default, PartialEq)]
pub struct Tail {
    pub entries: Vec<Entry>,
    /// A final line that did not parse, usually a write cut short by a crash.
    pub incomplete: usize,
}

/// The path of a companion file: `show.olshow` + `.journal`.
pub fn path_for(show: &Path, suffix: &str) -> PathBuf {
    let mut name = show.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

/// Read a journal, tolerating a truncated final line.
pub fn read(path: &Path) -> io::Result<Tail> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Tail::default()),
        Err(error) => return Err(error),
    };
    let mut tail = Tail::default();
    let mut lines = text.lines().enumerate().peekable();
    while let Some((index, line)) = lines.next() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Entry>(line) {
            Ok(entry) => {
                if tail.incomplete > 0 {
                    // A bad line that is not the last one means the file was
                    // damaged, not merely cut short: refuse rather than replay
                    // a document with a hole in the middle.
                    return Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("{}: bad entry on line {}", path.display(), index + 1),
                    ));
                }
                tail.entries.push(entry);
            }
            Err(_) if lines.peek().is_none() => tail.incomplete += 1,
            Err(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    format!("{}: bad entry on line {}", path.display(), index + 1),
                ))
            }
        }
    }
    Ok(tail)
}

/// The append-only journal of one session.
pub struct Journal {
    file: BufWriter<File>,
    path: PathBuf,
    rev: u64,
    entries: u64,
    bytes: u64,
    failure: Option<String>,
}

impl Journal {
    /// Open (or create) a journal, continuing the revision chain it holds.
    pub fn open(path: PathBuf) -> io::Result<Self> {
        let tail = read(&path)?;
        let rev = tail.entries.last().map(|entry| entry.rev).unwrap_or(0);
        let bytes = fs::metadata(&path).map(|meta| meta.len()).unwrap_or(0);
        let file = OpenOptions::new().create(true).append(true).open(&path)?;
        Ok(Self {
            file: BufWriter::new(file),
            path,
            rev,
            entries: tail.entries.len() as u64,
            bytes,
            failure: None,
        })
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn entries(&self) -> u64 {
        self.entries
    }

    /// Why the journal stopped recording, if it did.
    pub fn failure(&self) -> Option<&str> {
        self.failure.as_deref()
    }

    /// Record one statement. A write failure disables the journal for the rest
    /// of the session instead of failing the command: programming must not stop
    /// because a disk filled up.
    pub fn note(&mut self, kind: EntryKind, line: &str, rev: u64) {
        if self.failure.is_some() {
            return;
        }
        let rev = rev.max(self.rev + 1);
        let entry = Entry {
            rev,
            at: now_rfc3339(),
            line: line.to_string(),
            kind,
        };
        let written = match serde_json::to_string(&entry) {
            Ok(json) => (|| -> io::Result<u64> {
                self.file.write_all(json.as_bytes())?;
                self.file.write_all(b"\n")?;
                self.file.flush()?;
                Ok(json.len() as u64 + 1)
            })(),
            Err(error) => Err(io::Error::other(error)),
        };
        match written {
            Ok(bytes) => {
                self.rev = rev;
                self.entries += 1;
                self.bytes += bytes;
                if self.bytes >= JOURNAL_LIMIT {
                    if let Err(error) = self.rotate() {
                        self.failure = Some(error.to_string());
                    }
                }
            }
            Err(error) => self.failure = Some(error.to_string()),
        }
    }

    /// Start a fresh file, keeping one generation.
    fn rotate(&mut self) -> io::Result<()> {
        self.file.flush()?;
        let mut previous = self.path.clone().into_os_string();
        previous.push(".1");
        let previous = PathBuf::from(previous);
        let _ = fs::remove_file(&previous);
        fs::rename(&self.path, &previous)?;
        self.file = BufWriter::new(
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(&self.path)?,
        );
        self.bytes = 0;
        Ok(())
    }
}

/// A full snapshot of the document and the desk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub rev: u64,
    pub at: String,
    pub show: Show,
    pub desk: DeskFile,
}

impl Snapshot {
    pub fn capture(rev: u64, show: &Show, desk: &Desk) -> Self {
        Self {
            rev,
            at: now_rfc3339(),
            show: show.clone(),
            desk: desk.to_file(show),
        }
    }

    pub fn save(&self, path: &Path) -> io::Result<()> {
        let json = serde_json::to_string_pretty(self)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        store::write_atomic(path, json.as_bytes())
    }

    /// Load a snapshot, treating a missing file as "none".
    pub fn load(path: &Path) -> io::Result<Option<Self>> {
        let text = match fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        let snapshot: Snapshot = serde_json::from_str(&text)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
        Ok(Some(snapshot))
    }

    /// The document and the desk this snapshot describes.
    pub fn restore(&self) -> (Show, Desk, Vec<String>) {
        let (desk, warnings) = Desk::from_file(self.desk.clone(), &self.show);
        (self.show.clone(), desk, warnings)
    }
}

/// RFC 3339 in UTC, without a date library.
pub fn now_rfc3339() -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let seconds = now.as_secs() as i64;
    let days = seconds.div_euclid(86_400);
    let second_of_day = seconds.rem_euclid(86_400);
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        second_of_day / 3600,
        (second_of_day / 60) % 60,
        second_of_day % 60
    )
}

/// Days since the Unix epoch to a civil date (Howard Hinnant's algorithm).
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let month_index = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * month_index + 2) / 5 + 1) as u32;
    let month = if month_index < 10 {
        month_index + 3
    } else {
        month_index - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!("openlight-journal-{}-{name}", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path_for(&path, ".1"));
        path
    }

    #[test]
    fn entries_round_trip_and_continue_the_chain() {
        let path = temp("round-trip");
        {
            let mut journal = Journal::open(path.clone()).unwrap();
            journal.note(EntryKind::Show, "Fixture 1 At 50", 3);
            journal.note(EntryKind::Programmer, "Clear Values", 4);
        }
        let tail = read(&path).unwrap();
        assert_eq!(tail.entries.len(), 2);
        assert_eq!(tail.incomplete, 0);
        assert_eq!(tail.entries[0].rev, 3);
        assert_eq!(tail.entries[1].kind, EntryKind::Programmer);

        // Reopening continues from the last revision, even if the caller's
        // engine revision restarted at zero.
        let mut journal = Journal::open(path.clone()).unwrap();
        journal.note(EntryKind::Show, "Store Cue 1", 0);
        assert_eq!(read(&path).unwrap().entries[2].rev, 5);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_truncated_final_line_is_reported_not_replayed() {
        let path = temp("truncated");
        fs::write(
            &path,
            concat!(
                "{\"rev\":1,\"at\":\"now\",\"line\":\"Fixture 1 At 50\",\"kind\":\"show\"}\n",
                "{\"rev\":2,\"at\":\"now\",\"line\":\"Store Cu"
            ),
        )
        .unwrap();
        let tail = read(&path).unwrap();
        assert_eq!(tail.entries.len(), 1);
        assert_eq!(tail.incomplete, 1);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn damage_in_the_middle_is_an_error() {
        let path = temp("damaged");
        fs::write(
            &path,
            concat!(
                "{\"rev\":1,\"at\":\"now\",\"line\":\"Fixture 1 At 50\",\"kind\":\"show\"}\n",
                "not json\n",
                "{\"rev\":3,\"at\":\"now\",\"line\":\"Store Cue 1\",\"kind\":\"show\"}\n"
            ),
        )
        .unwrap();
        let error = read(&path).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn rotation_keeps_one_generation() {
        let path = temp("rotate");
        let mut journal = Journal::open(path.clone()).unwrap();
        // Fill past the cap with one padded line, then rotate.
        let padding = "x".repeat(JOURNAL_LIMIT as usize + 8);
        journal.note(EntryKind::Show, &padding, 1);
        assert_eq!(journal.entries(), 1);
        assert!(path_for(&path, ".1").exists(), "the old file is kept");
        assert!(fs::metadata(&path).unwrap().len() < 1024);
        journal.note(EntryKind::Show, "Store Cue 1", 2);
        assert_eq!(read(&path).unwrap().entries[0].rev, 2);
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path_for(&path, ".1"));
    }

    #[test]
    fn a_write_failure_disables_the_journal_once() {
        let path = temp("broken");
        let mut journal = Journal::open(path.clone()).unwrap();
        // Removing the file makes the rotation rename fail; the open handle
        // keeps accepting bytes, which is exactly the disk-full shape.
        fs::remove_file(&path).unwrap();
        let padding = "x".repeat(JOURNAL_LIMIT as usize + 8);
        journal.note(EntryKind::Show, &padding, 1);
        assert!(journal.failure().is_some(), "the failure is reported");
        let first = journal.failure().unwrap().to_string();
        journal.note(EntryKind::Show, "Fixture 1 At 60", 2);
        assert_eq!(journal.failure().unwrap(), first, "reported once");
    }

    #[test]
    fn timestamps_are_rfc3339() {
        let now = now_rfc3339();
        assert_eq!(now.len(), 20, "{now}");
        assert!(now.ends_with('Z'));
        assert_eq!(&now[4..5], "-");
    }

    #[test]
    fn civil_dates_are_right() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(19_000), (2022, 1, 8));
        assert_eq!(civil_from_days(20_776), (2026, 11, 19));
    }

    #[test]
    fn a_snapshot_round_trips_show_and_desk() {
        let path = temp("snapshot");
        let show = crate::test_support::demo_show();
        let mut desk = Desk::new();
        desk.programmer.set(1, "Dimmer", 0.5);
        let snapshot = Snapshot::capture(7, &show, &desk);
        snapshot.save(&path).unwrap();

        let loaded = Snapshot::load(&path).unwrap().unwrap();
        assert_eq!(loaded.rev, 7);
        let (restored, restored_desk, warnings) = loaded.restore();
        assert_eq!(restored, show);
        assert_eq!(restored_desk.programmer.get(1, "Dimmer"), Some(0.5));
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path_for(&path, ".bak"));
    }
}
