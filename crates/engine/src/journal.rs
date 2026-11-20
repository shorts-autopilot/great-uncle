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
//!
//! Replay has to know where to start. A successful `Save` writes a
//! [`EntryKind::Checkpoint`] entry naming the show file, and recovery begins
//! after the newest checkpoint for the document it loaded. Without that, a
//! save followed by a crash would replay the whole journal onto a file that
//! already contains those statements — and statements that are not idempotent
//! (an `Undo`, a store that captures a programmer) would produce a different
//! document than the one on disk. The autosave says the same thing for a
//! snapshot: its revision is the newest one already captured, or nothing at all
//! when it was taken with the journal switched off, in which case every entry
//! in the journal predates the snapshot and replaying any of it would apply
//! stale statements to a newer document.

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
    /// The show file named by `line` contains everything up to this revision.
    /// Written by a successful save; replay starts after the last one.
    Checkpoint,
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

/// Where replay must begin for the document now in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Baseline {
    /// The newest revision the document already reflects; replay starts after
    /// it.
    pub rev: u64,
    /// The oldest revision still in the journal when its head was lost to
    /// rotation or damage.
    pub head_lost: Option<u64>,
}

/// Where the document that was loaded came from, which decides how much of the
/// journal it already contains.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReplayFrom {
    /// No snapshot was restored: the show file is the starting point.
    File,
    /// A snapshot captured at this journal revision was restored.
    Snapshot(u64),
    /// A snapshot captured while no journal was open was restored. Every entry
    /// in the journal predates it, so replaying the tail onto it would apply
    /// statements the snapshot already replaced.
    SnapshotWithoutJournal,
}

impl Tail {
    /// The revision the document already reflects.
    ///
    /// A save after a restored snapshot is newer still, so the greater of the
    /// two wins. A snapshot taken with the journal off reflects the whole
    /// journal: the file was a fork that never consumed it, so starting after
    /// the head is the only safe answer. A journal that begins after revision 1
    /// lost its head; the oldest entry available is then the best starting
    /// point, and the caller is told so it can say as much.
    pub fn baseline(&self, show: &Path, from: ReplayFrom) -> Baseline {
        let key = path_key(show);
        let checkpoint = self
            .entries
            .iter()
            .rev()
            .find(|entry| entry.kind == EntryKind::Checkpoint && entry.line == key)
            .map(|entry| entry.rev)
            .unwrap_or(0);
        let snapshot = match from {
            ReplayFrom::File => 0,
            ReplayFrom::Snapshot(rev) => rev,
            ReplayFrom::SnapshotWithoutJournal => {
                self.entries.last().map(|entry| entry.rev).unwrap_or(0)
            }
        };
        let mut baseline = Baseline {
            rev: snapshot.max(checkpoint),
            head_lost: None,
        };
        if let Some(first) = self.entries.first() {
            if first.rev > baseline.rev.saturating_add(1) {
                baseline.rev = first.rev - 1;
                baseline.head_lost = Some(first.rev);
            }
        }
        baseline
    }
}

/// The identity a checkpoint records: the canonical path, so saving from another
/// directory still names the same file. A path that cannot be canonicalised is
/// used as written.
fn path_key(path: &Path) -> String {
    fs::canonicalize(path)
        .unwrap_or_else(|_| path.to_path_buf())
        .to_string_lossy()
        .into_owned()
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

    /// The revision of the last entry written, which is the newest state this
    /// journal describes.
    pub fn rev(&self) -> u64 {
        self.rev
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

    /// Record that the show file at `show` now holds everything up to `rev`.
    ///
    /// The entry names the path, so a journal shared by several shows does not
    /// let one show's save vouch for another's.
    pub fn checkpoint(&mut self, show: &Path, rev: u64) {
        self.note(EntryKind::Checkpoint, &path_key(show), rev);
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
    /// The journal revision the snapshot already contains, or `None` when it
    /// was captured with no journal open. An old snapshot that says zero is
    /// read as revision zero, exactly as it was written.
    #[serde(default)]
    pub rev: Option<u64>,
    pub at: String,
    pub show: Show,
    pub desk: DeskFile,
}

impl Snapshot {
    pub fn capture(rev: Option<u64>, show: &Show, desk: &Desk) -> Self {
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

    /// Where replay must begin for this snapshot.
    pub fn replay_from(&self) -> ReplayFrom {
        match self.rev {
            Some(rev) => ReplayFrom::Snapshot(rev),
            None => ReplayFrom::SnapshotWithoutJournal,
        }
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
    fn a_checkpoint_says_where_replay_starts() {
        let journal_path = temp("checkpoint");
        let show = temp("checkpoint.olshow");
        fs::write(&show, "{}").unwrap();
        {
            let mut journal = Journal::open(journal_path.clone()).unwrap();
            journal.note(EntryKind::Show, "Fixture 1 At 50", 1);
            journal.note(EntryKind::Show, "Fixture 1 At 60", 2);
            // The caller's revision is a floor: the checkpoint lands after
            // every entry already written.
            journal.checkpoint(&show, 0);
            journal.note(EntryKind::Show, "Fixture 1 At 70", 0);
        }
        let tail = read(&journal_path).unwrap();
        let baseline = tail.baseline(&show, ReplayFrom::File);
        assert_eq!(baseline.rev, 3, "the checkpoint is the newest entry");
        assert_eq!(baseline.head_lost, None);
        assert_eq!(
            tail.baseline(&show, ReplayFrom::Snapshot(9)).rev,
            9,
            "a newer autosave wins over an older save"
        );

        // A journal shared by two shows must not replay one from the other's
        // save.
        let other = temp("checkpoint-other.olshow");
        fs::write(&other, "{}").unwrap();
        assert_eq!(tail.baseline(&other, ReplayFrom::File).rev, 0);

        let _ = fs::remove_file(&journal_path);
        let _ = fs::remove_file(&show);
        let _ = fs::remove_file(&other);
    }

    #[test]
    fn a_journal_that_lost_its_head_starts_where_the_file_does() {
        let path = temp("head");
        fs::write(
            &path,
            concat!(
                "{\"rev\":41,\"at\":\"now\",\"line\":\"Fixture 1 At 50\",\"kind\":\"programmer\"}\n",
                "{\"rev\":42,\"at\":\"now\",\"line\":\"Fixture 1 At 60\",\"kind\":\"programmer\"}\n"
            ),
        )
        .unwrap();
        let tail = read(&path).unwrap();
        let baseline = tail.baseline(Path::new("/no/such/show.olshow"), ReplayFrom::File);
        assert_eq!(baseline.rev, 40, "the oldest entry's predecessor");
        assert_eq!(baseline.head_lost, Some(41));

        // A snapshot from before the rotation is no better than the file head.
        let stale = tail.baseline(Path::new("/no/such/show.olshow"), ReplayFrom::Snapshot(20));
        assert_eq!(stale.rev, 40);
        assert_eq!(stale.head_lost, Some(41));
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn a_snapshot_taken_without_a_journal_skips_the_whole_tail() {
        let path = temp("no-journal-snapshot");
        {
            let mut journal = Journal::open(path.clone()).unwrap();
            journal.note(EntryKind::Show, "Fixture 1 At 50", 1);
            journal.note(EntryKind::Show, "Fixture 1 At 60", 2);
        }
        let tail = read(&path).unwrap();
        // The snapshot was captured in a session with the journal off, so none
        // of these entries can be trusted to be missing from it.
        let baseline = tail.baseline(
            Path::new("/no/such/show.olshow"),
            ReplayFrom::SnapshotWithoutJournal,
        );
        assert_eq!(baseline.rev, 2);
        assert_eq!(baseline.head_lost, None);

        let snapshot = Snapshot::capture(None, &crate::test_support::demo_show(), &Desk::new());
        assert_eq!(snapshot.rev, None);
        assert_eq!(snapshot.replay_from(), ReplayFrom::SnapshotWithoutJournal);
        let with_rev = Snapshot::capture(Some(4), &crate::test_support::demo_show(), &Desk::new());
        assert_eq!(with_rev.replay_from(), ReplayFrom::Snapshot(4));
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
        let snapshot = Snapshot::capture(Some(7), &show, &desk);
        snapshot.save(&path).unwrap();

        let loaded = Snapshot::load(&path).unwrap().unwrap();
        assert_eq!(loaded.rev, Some(7));
        let (restored, restored_desk, warnings) = loaded.restore();
        assert_eq!(restored, show);
        assert_eq!(restored_desk.programmer.get(1, "Dimmer"), Some(0.5));
        assert!(warnings.is_empty(), "{warnings:?}");
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path_for(&path, ".bak"));
    }
}
