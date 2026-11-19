# 04 · Autosave, journal and replay

**Rank** 4 · **Order** 3 · **Size** M · **No format change** · **Depends on** 03

## Problem

A crash loses the session. `Save` is manual, `Show::save` writes straight over
the target, and there is no record of what was typed. On a desk that is not a
convenience gap: the operator has been programming for an hour and the only copy
of that work is RAM.

There is also a correctness gap under the crash story: a save interrupted
half-way can leave a truncated JSON file, which `Load` refuses, which means the
*previous* good file is gone too.

## Behaviour when this is done

```
$ openlight --show look.olshow --autosave 30
Auto-saving to look.olshow.autosave every 30 s; journal at look.olshow.journal
```

After a power cut:

```
$ openlight --show look.olshow
Recovered 412 edits from the journal (autosave 2026-11-19 21:04:12).
Save? [Y/n]
```

- A save is all-or-nothing: the old file survives any failure.
- Every statement that changed the show or the programmer is on disk within
  one flush, so at most the last autosave interval plus the journal tail is at
  risk, and the journal tail is replayed, not lost.
- Nothing about recovery is silent: `Status` shows the journal path, entry
  count and last write.

## Design

### Atomic writer

One helper, used by the show, the desk and the autosave snapshot:

```rust
// crates/engine/src/store.rs
pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()>;
```

1. Write `<path>.tmp-<pid>` in the same directory (same filesystem, so the
   rename is atomic).
2. `flush`, then `sync_all` on the file.
3. Rename over the target, then fsync the directory on Unix.
4. On any error, remove the temp file and return the error; the original is
   untouched.
5. Keep one generation: before the rename, hard-link or copy the current file
   to `<path>.bak` (skip when it does not exist). A bad save therefore costs one
   generation, never the file.

`Show::save` and `DeskFile::save` become thin wrappers so every caller gets the
guarantee, including `Save Show`.

### Journal

```rust
// crates/engine/src/journal.rs
#[derive(Serialize, Deserialize)]
pub struct Entry {
    pub rev: u64,          // engine revision after the statement
    pub at: String,        // RFC 3339, local time
    pub line: String,      // the exact statement text
    pub kind: EntryKind,   // Show | Programmer | Desk
}

pub struct Journal { file: BufWriter<File>, path: PathBuf, rev: u64, entries: u64 }
```

- **What is recorded.** Only statements that changed persisted state: the ones
  that already bump `revision` for a show edit, plus programmer writes. Not
  `Status`, `Sheet`, listings, `Go`, fader moves, `Undo`/`Redo`.
- **Where the hook lives.** `Engine::mutate` (show edits) and the two programmer
  writers call `self.journal.note(kind, &self.label, self.revision)`. One call
  site per kind, so a new command cannot forget to journal: if it changes state,
  it went through one of those three functions.
- **Write policy.** Append one line and `flush`; no `sync_all` per entry. A
  crash can lose the OS buffer (bounded by the page cache flush interval, tens
  of milliseconds) but can never corrupt earlier entries.
- **Failure policy.** A journal write failure is reported once as a warning in
  `Status` and disables the journal for the session. Programming must not fail
  because a disk filled up.
- **Rotation.** At 8 MiB the file is renamed to `.journal.1` (one generation) and
  a fresh one starts. The autosave means an old journal is never needed.
- **Off switch.** `--journal off`, and the journal is off by default when no
  autosave path is known (a bare REPL with no show file), so demos and tests are
  deterministic.

### Autosave and recovery

- A full snapshot (`Show` + `Desk`) is written to `<show>.autosave` every
  `--autosave <secs>` (default off, `30` is the suggested setting) and always
  after every `Save`, so the snapshot revision is never behind the file.
- The snapshot header records `rev` and the UTC time.
- Recovery on start: if `<show>.journal` exists and its last entries are newer
  than `<show>.autosave` (or the show has no autosave and the journal is
  non-empty), `Load` replays the tail. Replay is `Engine::execute` on each line
  with the journal *off* and history recording disabled; a line that fails is
  skipped with a warning (a cue deleted later is not an error).
- Replay stops at the first `rev` that is not exactly `expected + 1`, so a
  spliced or partially written file cannot silently reorder edits. A truncated
  final line is dropped by the JSONL reader and reported as `1 incomplete entry`.
- After a successful recovery the show is **dirty** and the prompt says so; the
  operator chooses `Save` or `Discard`. `--no-recover` skips it for scripted
  runs.

### Why a journal and not just autosave

Autosave gives you a document; the journal gives you the *last minute* of the
document. Replaying statements (rather than storing diffs) means the recovery
path uses the same code as normal editing, so it cannot diverge from the
command semantics — the same reason undo uses whole snapshots instead of
inverses.

## Implementation plan

1. `store.rs` with `write_atomic` and its failure tests; convert `Show::save`
   and `DeskFile::save`; keep `save`'s signature.
2. `journal.rs`: `Journal::open`, `note`, `tail_since(rev)`, rotation, JSONL
   parsing that tolerates a trailing partial line. Tests use a temp dir.
3. Give `Engine` an `Option<Journal>` plus `set_journal`; call it from `mutate`
   and the two programmer writers.
4. `Engine::load` returns the tail alongside the show; add
   `Engine::recover(&mut self, entries) -> RecoveryReport`.
5. Wire `--autosave`, `--journal`, `--no-recover` and the recovery prompt in
   `crates/console`; add a `Journal` line to `Status`.
6. Add an integration test that runs a script, kills the engine without saving,
   and recovers to an equal show.

## Tests

- **store** — a save over an existing file leaves the old bytes readable if the
  temp write fails (simulate with a read-only directory); `.bak` holds the
  previous generation; no temp file remains on success or failure.
- **journal** — round-trip of entries; a truncated last line is ignored; a gap
  in `rev` stops replay; rotation at the size cap keeps one generation; a
  failure disables the journal once and does not fail the command.
- **engine** — `At 50; Store Cue 1; Clear Values` journals three entries; a
  listing and a `Go` journal nothing; recovery from a journal tail reproduces
  the show and the programmer exactly; a line referencing a deleted fixture is
  skipped, not fatal.
- **console** — `--autosave 0` disables; recovery prompt honours `--no-recover`;
  `Status` names the journal path.

## Boundaries

- No continuous WAL, no crash-consistent mmap, no replication.
- No journal of DMX output or transport events; this recovers programming, not
  a show's cue state (playback position is desk state saved by autosave).
- No compression, no encryption, no signing.

## Risks

- **Replay is not idempotent for cumulative commands.** `Fixture 1 At 50` is
  fine; `Store Cue` capturing the programmer depends on the programmer at that
  moment, which replay reconstructs exactly because every programmer write is
  journaled. The rule to enforce in review: *any* new command that changes
  persisted state must go through `mutate` or a programmer writer, or replay
  drifts. The test with a mixed script is the guard.
- **Journal grows in a long session.** 8 MiB is roughly 60k statements; rotation
  plus autosave keeps recovery bounded.
- **Recovering into a changed show.** Replay runs against the autosave snapshot,
  never against a hand-edited file; if the snapshot is missing, replay runs
  against the loaded show and is reported as *partial*.
