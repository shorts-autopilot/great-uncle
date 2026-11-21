# 24 · File management, backups and templates

**Rank** 10 (follow-up) · **Order** 24 · **Size** S–M · **No format change** · **Depends on** 03 (show/desk split), 04 (atomic writes)

## Problem

The desk can write and read a show, but it cannot help an operator manage one.
`Save "look.olshow"` overwrites the target and leaves only the single automatic
`.bak` generation; there is no timestamped backup, no way to list the shows in
a folder, no save-as, and no way to copy, rename or delete a show without a
file manager. Templates are worse: a template is only "a show file you happen
to load", so nothing in the desk distinguishes a starting point from the show
the operator is about to overwrite.

Those are not cosmetic gaps. Backups and a known template are the two things
an operator reaches for before a show, and file organisation is the first step
when a show is moved from a programming desk to a playback desk. The existing
`Show::save` and `Desk::save` pair also already has an owner: the engine writes
the two files, the console/App layer decides which paths are current. File
management must not create a second owner by quietly rebinding a path, journal
or autosave file.

This item is deliberately small. It adds a file layer and commands around the
existing document format; it does not add a history database, show metadata, a
converter or a GUI file browser.

## Behaviour when this is done

```text
> Files "shows"
shows/look.olshow        22.4 KiB  2026-11-21 14:30:05 UTC
shows/playback.olshow    18.1 KiB  2026-11-20 21:04:12 UTC

> Save As "shows/look-festival.olshow"
Saved show to shows/look-festival.olshow
Saved desk to shows/look-festival.oldesk
Backup shows/backups/look-festival-20261121T143005Z.olshow

> Backup
Backed up shows/look-festival.olshow to backups/look-festival-20261121T143012Z.olshow

> Save Template "templates/wash.olshow"
Saved template to templates/wash.olshow

> Load Template "templates/wash.olshow"
Loaded `Wash` (8 fixtures, 2 fixture types) from templates/wash.olshow
```

- `Save As "path"` is the same write as `Save "path"`: it writes the desk
  (`<path>.oldesk`) and the show atomically, checkpoints the journal and
  autosaves. It does not change the path the console/App opened; that remains
  the caller's job. It is a readable spelling of the existing explicit-path
  save, not a new document owner.
- A save over an existing show creates a backup first, in a `backups/` folder
  beside the show: `shows/look.olshow` → `shows/backups/look-<UTC>.olshow`.
  At most ten backups are kept per show; the oldest is removed after a new one
  is complete. The existing `<show>.bak` generation is still written by the
  atomic store, so an immediate crash has both the previous generation and the
  last ten saves.
- The backup name uses the show's stem, a UTC timestamp
  (`YYYYMMDDTHHMMSSZ`) and `.olshow`. If two saves happen in the same second,
  `-2`, `-3` … is added before the extension. A backup is show-only; the desk
  is disposable and is not copied.
- A failed backup never blocks the save. It is reported as a warning in the
  command result, and the save proceeds. A full or read-only `backups/` folder
  must not turn a working desk into one that cannot save.
- `Backup` writes a show-only snapshot. With a path, that path is used; without
  one, the backup goes to `backups/<show-name>-<UTC>.olshow` in the current
  directory. It is the explicit "take one now" operation; ordinary `Save`
  already takes one automatically.
- `Files ["directory"]` lists `.olshow` files in the directory (the current
  directory when omitted), sorted by name, with size and UTC modification time.
  It reads directory metadata only; it does not parse every show to build the
  list. A missing directory or a path that is not a directory is an error.
- `Copy Show "source" "destination"` (also `Copy Show "source" To "destination"`)
  copies the show and, when present, its companion `.oldesk` file. The
  destination must not exist; an existing file is never overwritten. If the
  desk copy fails after the show copy, the new show is removed so no half-copy
  remains.
- `Rename Show "source" "destination"` moves the show and its companion desk.
  The destination must not exist. If the desk move fails, the show is moved back.
  A rename across filesystems fails rather than silently degrading into a copy.
- `Delete Show "path"` removes the show and its companion `.oldesk`. It leaves
  the `.bak` generation and the `backups/` folder intact: deletion is for the
  working document, not for the recovery material. There is no recursive
  delete and no wildcard.
- `Save Template "path"` writes the current show only, not the desk, and does
  not take a backup first. `Load Template "path"` loads that show without its
  companion desk, exactly like `Load Show`; the current desk is sanitised
  against the template. A template is an ordinary show file used as a starting
  point, not a new format and not a hidden marker in the document.
- `Copy Show`, `Rename Show` and `Delete Show` operate on the files on disk;
  they do not save the in-memory show first. `Save As` is the command that
  writes the current state to a new path.
- Templates are not forced into a fixed folder and there is no separate
  template listing: `Files "templates"` lists them. The operator can keep a
  template folder anywhere, and `Save Template` can write to it explicitly.
- No command rebinds the App's current show, journal or autosave path. After
  `Save As`, the console/App still points at the path it opened; the operator
  starts the next session with the new path. This is an explicit boundary, not
  an omission: the engine has no active-show field today, and adding one here
  would duplicate the App's ownership.
- Show and desk formats do not change. File management is path and bytes, not
  a new document version.

## Design

### A small file layer

`crates/engine/src/files.rs` owns the filesystem operations that are not a
document write:

```rust
pub const BACKUP_LIMIT: usize = 10;
pub const BACKUP_DIRECTORY: &str = "backups";

pub struct ShowFileInfo {
    pub path: PathBuf,
    pub name: String,
    pub bytes: u64,
    pub modified: Option<SystemTime>,
}

pub fn is_show_file_name(name: &str) -> bool;
pub fn list_show_files(directory: &Path) -> Result<Vec<ShowFileInfo>, FileError>;
pub fn backup_path(show_path: &Path, now: SystemTime) -> PathBuf;
pub fn backup(show_path: &Path, now: SystemTime) -> Result<Option<PathBuf>, FileError>;
pub fn copy_show(source: &Path, destination: &Path) -> Result<(), FileError>;
pub fn rename_show(source: &Path, destination: &Path) -> Result<(), FileError>;
pub fn delete_show(path: &Path) -> Result<Vec<PathBuf>, FileError>;
pub fn utc_stamp(time: SystemTime) -> String;
```

`FileError` keeps the path and the underlying I/O error together. It does not
replace `ShowError`: loading and saving a document still go through `Show`, and
a malformed source is reported by the existing loader.

Show files are recognised by the `.olshow` suffix, including
`.olshow.json`. The listing deliberately uses the extension rather than
parsing; a file with arbitrary JSON must not be presented as a show by a
metadata-only listing.

### UTC without a new dependency

The project has no calendar/time crate in the engine, and a timestamp for a
backup filename is not a reason to add one. `utc_stamp` converts
`SystemTime`'s Unix seconds with the civil-from-days algorithm:

```text
days = seconds / 86400
seconds_of_day = seconds % 86400
(y, m, d) = civil_from_days(days)
YYYYMMDDTHHMMSSZ
```

The pure function is tested against the Unix epoch and a leap day. Listing
uses the same parts with `YYYY-MM-DD HH:MM:SS UTC`. A modification time before
the epoch is displayed as `1970-01-01 00:00:00 UTC`; it is not a failure of the
file layer.

### Backup lifecycle

`backup()` is deliberately separate from `Show::save`:

1. If the show does not exist, return `None` (nothing to back up).
2. Check the source is at most `MAX_SHOW_BYTES`; read its bytes and write them
   through `store::write_atomic` to the chosen backup path. A new backup is
   therefore never a partially written file.
3. Prune the matching `backups/<stem>-*.olshow` files, oldest first, until at
   most ten remain. Pruning is best-effort: a backup that was created is not
   discarded because an unrelated file could not be removed.
4. Return the backup path for the command result.

The prefix is the sanitised show stem, so `look.olshow` and `look.olshow.json`
both use `look-…`; a stem with spaces or path separators cannot escape the
backup folder. The `backups` folder is a sibling of the show, not the current
directory, so moving a show folder moves its recovery material with it.

`Engine::save` calls this before the show write, but only for the show half of
a save and only when the target already exists. The desk is written first, as
it is today, so a desk failure leaves both the show and its backups untouched.
A backup failure is pushed as a warning after the successful save; it never
turns a successful document write into a failed command.

### File operations and companion desks

The desk lives beside the show via `Desk::path_for`, so a copy or rename that
moved only the show would leave the destination with the wrong handles. Both
operations therefore treat the pair as a unit:

```text
source.olshow   -> destination.olshow
source.oldesk   -> destination.oldesk   (when it exists)
```

`copy_show` checks the destination before writing, reads the source and writes
it atomically, then copies the desk with the same rule. If the second write
fails, the new show is removed; the source is untouched. `rename_show` uses
`fs::rename` for the same reason: on one filesystem it is atomic, and across
filesystems it fails instead of producing a copy that the operator did not ask
for. The desk is moved after the show and moved back if the desk move fails.

`delete_show` removes the show and the companion desk when present. It does not
remove `.bak` or `backups/`: those are recovery files, and a delete command that
also removed them would turn one typo into a lost show. It also does not
recursively delete directories or expand wildcards.

`list_show_files` sorts by file name, skips directories and non-show suffixes,
and returns file metadata. It does not recurse into subdirectories. A listing
is read-only and is exempt from the desk lock, while copy,
rename, delete, backup, save-as and template commands are mutating because they
change files on disk.

### Templates

A template has no extra fields. `Save Template` is `Show::save` with the desk
half omitted; `Load Template` is the existing show-only load. Keeping the
template as a normal `.olshow` means it can be copied, backed up, renamed and
opened with the same tools, and a show used as a template remains valid if it
is later opened as a full show.

The engine does not decide where templates live. A fixed `templates/` folder
would be wrong for portable show folders and would create a second path
convention beside the App's `--show`. The operator writes to the folder they
want and lists it with `Files`.

### Command surface

The parser adds no new document state:

```text
Save As "path"
Save Template "path"
Load Template "path"
Backup ["path"]
Files ["directory"]
Copy Show "source" ["To"] "destination"
Rename Show "source" ["To"] "destination"
Delete Show "path"
```

`Save As` and `Save Template` are distinct variants so the help and tests can
spell the intent, but they share the existing save/load implementations. The
file commands are available on every input surface because they are ordinary
commands; the GUI needs no new pane for this item.

## Implementation plan

1. Add `files.rs` with UTC conversion, listing, backup naming/retention,
   companion-aware copy/rename/delete and tests.
2. Wire `Engine::save` to take a backup before an existing show is overwritten;
   report failures as warnings.
3. Add the parser variants and help text; add `Files`, `Backup`, `Copy Show`,
   `Rename Show`, `Delete Show`, `Save As`, `Save Template` and `Load Template`
   dispatch arms.
4. Add engine tests for save-as, automatic and explicit backups, listing,
   copying, renaming, deleting and template load/save.
5. Update the design index, roadmap, phase notes, README, changelog and the spec
   ledger; regenerate `docs/COMPLETENESS.md`, `spec/matrix.json` and
   `spec/matrix.md`.
6. Run the full gate, review the diff with fresh eyes, commit and push.

## Tests

- **UTC** — epoch is `19700101T000000Z`; 2024-02-29 is
  `20240229T000000Z`; a listing formats `2024-02-29 00:00:00 UTC`.
- **Listing** — `.olshow` and `.olshow.json` appear, a `.txt` file and a
  directory do not, the order is by name, and a missing directory is an error.
- **Backup naming** — the folder is beside the show, the stem is sanitised,
  spaces and separators cannot escape, and two saves in the same second get
  distinct names.
- **Backup retention** — eleven backups leave ten, the oldest is removed, and a
  non-matching file in `backups/` is untouched.
- **Backup failure** — a backup directory that cannot be created or written
  reports an error; the file layer does not panic and the engine still saves.
- **Save-as** — writes the show and companion desk, checkpoints the journal and
  autosaves; saving over the same destination creates one backup.
- **Copy** — copies show and desk, refuses an existing destination, leaves the
  source alone, and removes the new show if the desk copy fails.
- **Rename** — moves show and desk, refuses an existing destination, and moves
  the show back if the desk move fails.
- **Delete** — removes show and desk, leaves `.bak` and `backups/`, and refuses
  a path that is a directory.
- **Templates** — `Save Template` writes no desk; `Load Template` loads the
  show and does not load a companion desk.
- **Parser** — all four new spellings parse to the intended variant, `Copy Show`
  accepts the optional `To`, and `Files`/`Backup` accept an omitted path.
- **No hardware claim** — this item touches no DMX path and no display. The
  hardware/display release gate is unchanged.

## Boundaries

- No bundled demo shows. Backups and templates are built; a demo library is a
  content decision, not file plumbing.
- No show history, description, automatic numbering, media export or grandMA2
  converter. Those need show metadata and a UI; they are not file plumbing.
- No overwrite/merge confirmation, no recursive delete, no wildcard, no
  undo for filesystem operations.
- No active-show rebinding, journal move or autosave path change. The App owns
  those paths and a second owner would be a regression.
- No SFTP, folder synchronisation, cloud storage, encryption or signed backups.
- No separate template folder or template marker in the show format.
- No GUI file browser. The commands work in the console, scripts and the GUI
  command line; a browser can follow once there is a layout design for it.

## Risks

- **Backups consume disk.** Ten copies per show is bounded, but a very large
  show multiplied across many shows is not free. The backup failure is a
  warning by design; the operator sees it and can free space.
- **Same-second names.** The `-2`, `-3` suffix is deterministic but not a
  global lock; two processes saving the same show at once can still race. The
  desk is a single-writer application, so this is outside the supported case.
- **Case-insensitive filesystems.** A destination that differs only by case may
  be reported as existing on Windows/macOS; refusing the copy/rename is safer
  than replacing a file the operator did not name.
- **Deletion is destructive.** It removes the working show and its desk. The
  `.bak` and `backups/` files remain, but the command has no interactive
  confirmation because the command line has no prompt state; the path is
  explicit and the help spells out what is removed.
- **Cross-filesystem rename.** It fails with the OS error instead of copying.
  The operator can use `Copy Show` and then delete explicitly.
- **The hardware/display proof remains the release gate.** No file operation
  changes that.
