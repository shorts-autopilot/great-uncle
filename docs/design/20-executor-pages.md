# 20 · Executor pages

**Rank** 3 (follow-up) · **Order** 20 · **Size** L · **Show v13, desk v1** · **Depends on** 01 (executors), 02 (faders), 04 (journal)

## Problem

The executor namespace is flat. Every assigned cue list has one global number,
one fader and one set of buttons, and the GUI shows numbers 1 to 9 forever. A
musical needs a bank per song, a theatre a bank per scene, a busking rig a bank
per fixture group, and a show with forty cue lists has forty always-visible
handles. Reassigning `Executor 1` to the next song destroys the previous
binding, and there is no way to keep the first song running while the second is
started.

The cost is practical: the operator either numbers executors into sparse
ranges (`11`, `21`, `31`) and pages mentally, or overwrites the running song's
fader to reach the next one. Neither keeps a cue list addressable after its
scene has passed.

## Behaviour when this is done

```
Store Page 2 "Specials"                  create or rename a page
Assign Page 2 Executor 1 Sequence 4      assign without leaving the desk's page
Page 2                                   switch the desk's page
Assign Executor 1 Sequence 1             assign on the active page
Executor 1 At 75                         the active page's executor 1
Go Executor 1                            run the active page's executor 1
Page Next / Page Prev                    cycle through the stored pages
Page                                     list the pages
Delete Page 2                            remove a page and its assignments
```

```text
> Page
Pages (active 1 "Main")
1  Main       2 executors
2  Specials   1 executor

> Page 2
Page 2 "Specials"

> Go Executor 1
Executor 1 cue 1 running (page 2 "Specials")

> Page 1
Page 1 "Main"   the first cue kept running while page 2 was on the desk
```

- An executor is identified by a page and a number. Numbers are unique within
  a page and may repeat across pages; page 1 keeps every number a v12 show
  used, so old commands and old shows address the same executors until a page
  is switched.
- Switching pages is a handle change, not a playback change. Every run keeps
  running, keeps its cue and keeps its fader. Handles belong to
  `(page, number)`, so page 2's executor 1 has its own fader, flash and solo;
  switching back finds page 1 exactly as it was left.
- Solo remains a merge rule and therefore global: a solo on page 2 silences a
  run on page 1, and the page listing marks the page that holds it. Flash and
  faders only affect their own target.
- Every show has at least one page. `Show::new` starts with page 1 "Main";
  `Delete Page` refuses the last page. Assigning to a page that does not exist
  creates it in the same undo step, named `Page <n>`.
- `Store Page` and `Delete Page` are show edits and are undoable. Deleting a
  page drops its assignments and releases its runs; undo restores the
  programming but not a playback position that has already been released.
- A page name is a label, never an address. Commands use page numbers, so a
  show saved with different names still opens to the same commands.
- `Executor` lists only the active page; `Page` lists every page with its
  executor count; `Status` names the active page. The GUI fader bank and the
  web remote's executor section mirror the active page, and the remote's
  command box can switch pages.
- Old documents keep working: a v12 show's executors become page 1, and a v1
  desk's flat fader map becomes page 1's handles. A saved run remembers its
  page as well as its executor.

## Design

### Show v13

```rust
pub struct Page {
    pub number: u16,
    pub name: String,
}

pub struct Executor {
    /// The page this executor lives on. One is implicit for old documents.
    #[serde(default = "page_one", skip_serializing_if = "is_page_one")]
    pub page: u16,
    pub number: u16,
    pub sequence: u16,
    pub name: String,
    pub mode: ExecutorMode,
}

pub struct Show {
    pub pages: Vec<Page>,
    pub executors: Vec<Executor>,
    // … unchanged
}
```

- Executors stay one flat list with a page field rather than moving inside
  `Page`. Every existing consumer already iterates the list, validation and
  upgrade stay one pass, and a page remains what it is on a desk: a named
  selection of assignments, not a second owner of them. `Show::executors_on`
  is the only new access pattern.
- `MAX_PAGES` is 64. `MAX_EXECUTORS` stays 4096, counted over the whole show,
  because that is the document size limit the playbacks already live within.
- Validation adds: at least one page, page numbers non-zero and unique, page
  names non-empty, every executor's page present, and `(page, number)` unique.
  A duplicate number on a different page is legal and is the point.
- `Show::new` seeds page 1 "Main". `Show::page`, `page_mut`,
  `page_or_insert`, `executor(page, number)` and `executors_on(page)` are the
  accessors; the old `executor(number)` becomes `executor(page, number)` so no
  caller can forget the page.
- Upgrade from v12 and older inserts page 1 if the document has no pages, and
  leaves every executor on page 1 (the field's serde default). Page 1's
  `page` is skipped when writing, so a page-1-only show keeps the old compact
  shape. The upgrade report mentions the page only when the document actually
  had executors.

### Desk state

```rust
pub struct Desk {
    /// The page whose handles the surface is showing.
    pub page: u16,
    /// Handles by page, then executor number.
    pub executors: BTreeMap<u16, BTreeMap<u16, ExecutorDesk>>,
    // … unchanged
}

pub struct RunState {
    #[serde(default = "page_one")]
    pub page: u16,
    pub executor: Option<u16>,
    pub sequence: Option<u16>,
    pub cue: f64,
    pub paused: bool,
}
```

- The desk file stays v1 because the change is additive. `page` defaults to 1;
  the old flat `executors` map is read into page 1 and written empty; new
  handles are written under `page_executors`. `Masters::legacy_executors`
  from the first desk layout still migrates into page 1 as before.
- `Desk::sanitise` drops a handle whose `(page, executor)` no longer exists,
  and moves the active page to the lowest surviving page when its own page is
  gone, so a desk always points at a page the show has.
- `executor_state(page, number)`, `executor_master(page, number)` and the
  three setters take both coordinates. `soloed()` returns
  `Option<(page, number)>`, which is what makes two pages' executor 1
  independent handles while keeping solo global.
- The run snapshot records the page. Restoring a run resolves the executor on
  its recorded page, never on whatever page the desk happens to be showing
  now; a page switch while the desk file is written therefore cannot move a
  run to another list.

### Playback identity

```rust
pub enum Target {
    Sequence(u16),
    Executor { page: u16, number: u16 },
}
```

- The page is part of the target, so `Target::Executor { page: 1, number: 1 }`
  and `{ page: 2, number: 1 }` are separate runs in the same `BTreeMap`.
  `label()` prints `Executor 1` on page 1 and `Page 2 Executor 1` elsewhere,
  which keeps every existing message unchanged for single-page shows.
- `Playback::refresh` scales a run by `desk.executor_master(page, number)`;
  `prune` and `due` resolve the sequence through
  `show.executor(page, number)`. Page switching itself never touches
  `Playback`, which is what lets a cue keep running on the page that is no
  longer displayed.

### Commands

| Command | Effect |
|---|---|
| `Page` | list pages, marking the active page, counts and any solo |
| `Page <n>` | switch to a stored page |
| `Page Next` / `Page Prev` | switch to the next or previous stored page, wrapping |
| `Store Page <n> ["name"]` | create a page, or rename one; default `Page <n>` |
| `Delete Page <n>` | remove a page, its assignments and its handles; releases its runs |
| `Assign Page <p> Executor <n> Sequence <s>` | assign on page `p`, creating it if needed |
| `Assign Executor <n> Sequence <s>` | assign on the active page |
| `Executor <n> …`, `Go/Back/Pause/Release Executor <n>` | address the active page |

- `page` becomes a reserved word. No pool, fixture attribute or sequence is
  named `Page`, so nothing existing is shadowed.
- `Page <n>` on a page that is not stored is an error that lists the stored
  pages; only `Store Page` and `Assign Page` create pages, so a typo cannot
  silently make an empty bank.
- Page switching, page listing and `Executor`/`Status` listings are
  non-mutating for the lock. `Store Page`, `Delete Page` and `Assign Page`
  are show edits through `mutate`, so journal, undo and autosave see them.
- The parser keeps the existing `Verb Noun` and `Noun Verb` shapes: `Page`
  alone lists, `Page 2` switches, `Page Next` steps, `Store Page 2 "…"`
  stores, and `Assign Page 2 Executor 1 Sequence 1` is the assign verb with an
  explicit page before the executor.

### Listings, GUI and remote

- `Executor` prints `Page <n> "<name>"` above its table, so the same numbers
  on another page cannot be mistaken for these. The columns do not change.
- `Page` prints one row per page: number, name, executor count, and `solo`
  when a handle on that page is soloed.
- `Status` gains `Page: <n> "<name>"` beside the show and selection lines.
- `DeskView` gains `page` and `page_name`; its `executors` holds the active
  page's bank only. The GUI's nine faders and flash buttons keep sending the
  same `Executor <n>` commands, now against the active page, and the fader
  pane's header names the page. A page change refreshes the displayed fader
  positions from the new page's handles, so the surface never shows the old
  page's levels.
- The remote's `/state` JSON gains `page` and `page_name` and lists only the
  active page's executors; the page shows the page name and its buttons keep
  addressing the active page. No OSC address is added: `/openlight/executor/
  <n>/…` is the active page, and an OSC client can switch pages with
  `/openlight/command` text (`Page 2`).

### Undo, journal and load

- Pages live in the show and assignments live beside them, so one `mutate`
  step covers page creation, renaming, deletion and assignment. Undo after a
  page delete restores the page, its executors and their handles; the released
  runs stay released, which is the same rule the existing executor reassign
  path already follows.
- A page switch is desk state, not a show edit: it is saved in the desk file
  and never enters the journal or undo.
- Loading a v12 show inserts page 1 and records the upgrade. Loading a desk
  whose active page or handle page is gone falls back to the lowest page with
  a warning, never an error.

## Implementation plan

1. `show.rs`: `Page`, `SHOW_VERSION` 13, `MAX_PAGES`, show accessors,
   `validate_executors` page checks and the v12 upgrade; `Executor.page` with
   its serde default in `playback.rs`.
2. `playback.rs`: page-qualified `Target`, labels, `prune`, `due`, `refresh`;
   update the playback tests to construct targets through `Target::executor`.
3. `desk.rs`: active page, nested handles, page-aware accessors and solo,
   `RunState.page`, desk-file legacy and `page_executors`, sanitise fallback.
4. `command.rs`: `ListPages`, `SetPage`, `StepPage`, `StorePage`,
   `DeletePage`, `AssignExecutor.page`, parser arms, `mutates` classification
   and parser tests.
5. `engine.rs`: page switch/list/store/delete, page-aware executor handlers,
   `executor_list` header, page listing, status line, playback rows, remote
   JSON, desk run snapshot/restore and the help text.
6. `app`, `desktop` and `remote`: `DeskView.page`/`page_name`, active-page
   executor views, GUI header, remote JSON and HTML.
7. Docs: this design, the roadmap row, a `PHASE5.md` section, an unreleased
   `CHANGELOG.md` entry, and regenerated `COMPLETENESS.md`/matrix from
   `tools/spec_status.py`.

## Tests

- **command** — `Page`, `Page 2`, `Page Next`, `Page Prev`, `Store Page 2
  "Specials"`, `Delete Page 2` and both assign forms parse to the expected
  variants; a page switch, listing and `Executor` listing are non-mutating;
  store, delete and page assignment are mutating; `Page 0`, `Page` with a
  stray word and a missing name are rejected with hints.
- **show** — a v12 document with executors loads them on page 1 and saves as
  v13; a v13 page-1-only document round-trips; duplicate numbers on two pages
  validate; duplicate numbers on one page, an executor on a missing page and a
  nameless page are rejected; `Show::new` starts with page 1 "Main".
- **playback** — two pages' executor 1 hold independent runs, cues and labels;
  `refresh` applies each page's own fader and solo; `prune` drops only the
  run whose page or executor is gone; page 1 labels stay `Executor n`.
- **desk** — page 1 and page 2 handles with the same number keep independent
  fader, flash and solo; a solo on one page silences the other's run but not
  its own; `sanitise` drops handles for a deleted page and moves the active
  page; a flat v1 desk file loads its handles onto page 1 and writes
  `page_executors`; run states keep their page through save and load.
- **engine** — assigning page 2 does not create a page-1 executor; `Page 2`
  switches without disturbing a running page-1 cue; `Go Executor 1` runs page
  2's list; `Executor` lists only the active page; `Page` lists counts and
  marks the active page; `Delete Page 2` releases its run and undo restores
  the assignment; deleting the last page is refused; `Store Page` renames;
  a missing page is an error listing the pages; `Status` and the remote JSON
  name the active page; `Assign Page` on a new page creates it.
- **app/remote** — `DeskView` carries the page and only its executors; the
  remote state JSON has `page`, `page_name` and the active bank.

## Boundaries

- **No special executors.** Macros, chases, grand masters and multi-cue
  stacks are separate work; pages relocate ordinary cue-list executors only.
- **No executor copy or swap between pages.** `Assign Page <p> Executor <n>`
  overwrites that slot; moving a programming between pages is two assigns,
  with the old one released by reassignment. A dedicated move verb would need
  its own undo semantics and is not part of making pages addressable.
- **No page fader or page master.** A page is a selection, not a mixing
  layer; the grand, rate and group masters stay show-wide.
- **No automatic page creation from `Page <n>`.** Only `Store Page` and
  `Assign Page` create pages, so a mistyped page number is an error rather
  than a silent empty bank.
- **No page renumbering and no more than 64 pages.** Renaming is supported;
  renumbering would invalidate saved runs and command text.
- **No GUI page buttons.** The command line and the remote's command box
  switch pages; the fader bank follows. A page strip is a surface design of
  its own and is not needed to make pages addressable.
- **No all-pages executor view.** `Executor` is the active bank; `Page` is
  the index. A combined view would make the numbers ambiguous again.
- **No new OSC page address.** Existing executor addresses mean the active
  page; `/openlight/command` already carries arbitrary command text.

## Risks

- **Identity is now two numbers.** Every run key, message and desk snapshot
  must carry both, and a target built from the wrong page would orphan a run.
  The design closes that by deriving the page from the executor record at
  every resolve, never from "the current page" at refresh time, and by making
  `Show::executor` require both coordinates so no call site can omit one.
- **Deleting a page loses a playback position.** Releasing the runs is the
  safe choice: a cue list whose assignment is gone must not keep outputting.
  Undo restores the programming but not where the cue was; the message says
  how many runs were released.
- **A v1 desk's handles all land on page 1.** If the operator had already
  moved executor 1 to page 2 in the show and then loads an old desk, the page
  1 handle is dropped and the page 2 handle starts at its default. That is the
  same stale-handle rule the desk already applies, and it warns rather than
  guess where an old fader belonged.
- **A solo on an inactive page can silence the stage.** Solo is global by
  design, and the page listing marks it, but an operator looking only at the
  active bank can be surprised. The alternative — a per-page solo — would
  make solo mean two different things depending on what is displayed, which
  is worse.
- **`Page` is a new reserved word.** A hand-built fixture attribute or pool
  named exactly `Page` would no longer be addressable bare. Nothing in GDTF
  or the built-in profile vocabulary uses that name, and the command covers a
  desk-level concept that had no spelling at all.
- **A remote click can follow someone else's page change.** The web remote's
  buttons address the desk's active page, and the active page is shared by
  every client. If another client switches pages between a state refresh and
  a click, that click lands on the new page. The page name is part of the
  remote state, so the change is visible; making a click carry its own page
  would need a page-qualified playback verb and is not part of this item.
