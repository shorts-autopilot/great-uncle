# 25 · Window workspace

**Rank** 9 (follow-up) · **Order** 25 · **Size** L · **No format change** · **Depends on** 18 (desktop GUI), 24 (file menu)

**State** built, then deliberately stripped back to the canvas. The workspace
model, the 16-window catalogue and the command-only model are tested; the
surrounding UI is being rebuilt one piece at a time on the operator's direction.
Nothing in this document prescribes the next UI piece.

## Problem

The desktop window was a fixed five-pane viewer. F1–F5 swapped Sheet, Playback,
Pools, Timecode and Status; a permanent right rail held nine faders and a grand
master. That was sufficient to inspect the engine, but it is not the way a
grandMA3 operator works. A grandMA3 screen starts as a user-defined area: the
operator chooses windows from an Add Window picker, places them on a snap grid,
resizes them, and builds the working screen from data pools, sheets, playback
controls and status views.

The fixed pane layout also made every new data view a code change in the pane
enum. It could not express two sheets side by side, a pool strip below a
playback window, or a large fixture sheet beside a small command history. This
item replaced the pane switcher with a real workspace while preserving the
existing rule that every UI action is command text and every draw reads an
owned snapshot.

That workspace was built, and then the chrome that came with it was removed on
purpose: the window now opens on the blank canvas, and the rest of the UI is
rebuilt deliberately rather than carried forward all at once.

## Current state

The window opens on an empty **Screen 1**:

```text
┌─────────────────────────────────────────────────────────────────────┐
│                                                                     │
│                                                                     │
│                         SCREEN 1 IS EMPTY                           │
│                    Add windows to build a workspace                 │
│                                                                     │
│                                                                     │
└─────────────────────────────────────────────────────────────────────┘
```

The 12 × 8 grid is drawn as dotted cell edges: every line is where a window
can start or end.

- There is no top bar, left control rail or global command bar. Clicking empty
  canvas opens the **Add Window** picker at that point, with category tabs for
  the catalogue.
- The screen, its 12 × 8 dotted grid, and the empty-state text remain.
- A window that exists still draws as a tile: frame, title bar, `×` close
  button and lower-right resize grip. It can be dragged by its title bar and
  resized on the grid, and moves and resizes never overlap.
- The picker's tiles show an empty placeholder box inline with the window
  name; the name is the only text in a tile. Choosing one adds that window at
  the clicked cell through the model below rather than inventing placement.
- The window layout is session state, not show or desk data. Closing and
  reopening the window starts blank again. Saved views and layouts are a
  separate grandMA3 feature and are not claimed here.

## The model that remains

`crates/desktop/src/workspace.rs` owns layout only. It never imports the
engine:

```rust
pub const COLUMNS: u16 = 12;
pub const ROWS: u16 = 8;

pub struct WindowId(u64);

pub struct GridRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

pub enum WindowKind {
    FixtureSheet, SequenceSheet, Playbacks, Pools, Presets, Groups, Worlds,
    Filters, Effects, Timecode, CommandLineHistory, DmxSheet, Masters,
    RunningPlaybacks, Patch, Status,
}

pub struct Window {
    pub id: WindowId,
    pub kind: WindowKind,
    pub rect: GridRect,
}

pub struct Workspace {
    windows: Vec<Window>,
    next_id: u64,
}
```

`WindowKind` supplies its title, description, default size and minimum size;
`WindowTab` keeps the static picker slices (Common, Pools, Playback, Tools,
All). The workspace supplies `add`, `add_at`, `remove`, `clear`, `move_to`,
`resize`, `window_at`, and the read-only window list.

- `add_at` first tries the clicked grid cell, then the first free rectangle.
  It returns `None` when the screen is full; the caller reports that rather than
  silently dropping the window.
- `move_to` and `resize` refuse rectangles outside the screen grid, below the
  window's minimum, or overlapping another window. They return `bool`; the GUI
  keeps the last valid layout.
- `window_at` answers what is under a grid cell. It was used to keep a click
  inside a window from also opening the picker; a rebuilt add flow needs the
  same check.
- IDs are monotonic and never reused, so egui widget IDs and scroll positions
  remain stable while windows are rearranged.
- `Workspace::new()` is empty. There is no factory view and no default pane.

`Model` keeps `add_window_at`, `remove_window` and a bounded 100-entry command
history. Layout changes execute no command and do not mark the snapshot dirty.

## Window catalogue

No body is drawn at the moment. The **Actions** column records what each body
is meant to offer when the rebuild reaches it, so the catalogue is not a claim
about the current window.

| Window | Intended source | Intended actions |
|---|---|---|
| **Fixture Sheet** | `Listings.sheet` | Select, Home, Default, At |
| **Sequence Sheet** | `Listings.playback` | Select sequence, Go |
| **Playbacks** | `DeskView.executors` | Nine faders, flash, grand master |
| **Pools** | `Listings.pools` | Use/Off, delete |
| **Presets** | pool rows where kind is `Palette` | Use/Off, delete |
| **Groups** | pool rows where kind is `Group` | Use/Off, delete |
| **Worlds** | pool rows where kind is `World` | Use/Off, delete |
| **Filters** | pool rows where kind is `Filter` | Use/Off, delete |
| **Effects** | pool rows where kind is `Effect` | Use/Off, delete |
| **Timecode Viewer** | `Listings.timecode` | Start, pause, stop, locate, add |
| **Command Line History** | `Model.history` | Click a line to recall it into the input |
| **DMX Sheet** | `Snapshot.frame` | Read-only slot values |
| **Masters** | `DeskView` | Page previous/next, read grand/rate/tempo |
| **Running Playbacks** | `Snapshot.playing` | Read-only progress |
| **Patch** | `Listings.sheet` | Read-only patch table |
| **Status** | `Listings.status` | Read-only status lines |

### The official catalogue and this build's scope

The complete Add Window list was read from the [grandMA3 2.5 manual](https://help.malighting.com/grandMA3/2.5/HTML/wvm_add_window.html).
The dark chrome and left icon rail were checked against the official
[onPC quick-start screenshot](https://help2.malighting.com/grandMA3/2.4/Storage/grandma3-user-manual-publication/img/grandma3_quick_start_guide/qsg_03_interface_v2-4.png);
no artwork, icons or manual text are copied into the build. The table records
every category and every window, so a deferred name is not mistaken for an
unexamined one. Names in the **Present** column have a `WindowKind`; the rest
are deliberately deferred until their data or surface exists.

| Category | Present in this build | Deferred, with what is missing |
|---|---|---|
| **Common** | Fixture Sheet, Sequence Sheet, Playbacks, Command Line History, DMX Sheet | Content Sheet, Phaser Editor, Recipe Editor, 3D Viewer, Layout Viewer, Selection Grid, Special Dialog, MAtricks Editor, Xkeys — need cue content, phaser/recipe, 3D, layouts, a selection grid, a special dialog, MAtricks state or Xkeys |
| **Data Pools** | Groups, Worlds and Filters as filtered pool windows; Sequences through Sequence Sheet; Timecodes through Timecode Viewer; Effects through the existing effect pool | Bitmaps, Executor Configurations, Generators, Layouts, Macros, MAtricks, Pages, Plugins, Quickeys, Shapes, Timers — each needs its chapter's data model |
| **Presets** | One generic Presets window over the existing palette pool | Dynamic, Phaser, All 2, All Presets, All 4, All 5, Dimmer, Position, Gobo, Color, Beam, Focus, Control, Shapers, Video — this engine has one palette pool, not the feature-group and media pools |
| **Pools** | The five live pool kinds: palette, group, world, filter and effect | Show data: Appearances, Data Pools, Gels, Scribbles, Tags, Timecode Slots, Universes, Users. Media: Gobos, Images, Meshes, Sounds, Symbols, Videos. User profile: Cameras, Encoder Bars, Render Qualities, Views — all need resource/media stores or a user profile |
| **Tools** | Fixture Sheet, Sequence Sheet, DMX Sheet, Timecode Viewer and Command Line History. The desk also exposes its Patch and Status listings as windows; the manual reaches those through other surfaces. | Content Sheet, At Filter, Recipe Editor, Selection Grid, sMArt, Special Dialog, 3D Viewer, Agenda Viewer, Clock Viewer, Layout Viewer, MAtricks Editor, Phaser Editor, RDM Devices Viewer, Sound Viewer — need content, programmer tools, scheduling, system clocks, 3D/layout/media or RDM state |
| **More** | Playbacks (the desk's fader surface), Masters, Running Playbacks and Command Line History | Bars: Align Bar, command wing Bar, Encoder Bar, Selection Bar, Step Bar. Playback: Custom Master Section, Xkeys. Info and system: Help, Info, Message Center, Oops, System Info, System Monitor, Trackpad — need bar surfaces, a custom-master editor, help/system data or the message/overlay model |
| **All** | The 16 supported windows, sorted alphabetically like the manual | The union of every deferred name above |

The original picker's five tabs are a smaller adaptation of the manual's seven:
Command Line History also appears on **Tools**, and Playbacks/Masters appear on
**Playback**, for quicker access. Two further adaptations are explicit: the
desk's generic **Pools** window lists its existing pool rows instead of the
manual's resource/store pools, and **Patch** and **Status** are exposed as
windows even though the manual does not list them under Add Window. They are
useful views over the same snapshot, not claims of feature parity.

The catalogue is additive: when a deferred subsystem gets a snapshot listing,
it gets a `WindowKind` and a picker entry without changing the workspace model.

## Design

### Drawing and interaction

`gui.rs` draws the workspace inside one `CentralPanel`:

1. Fill the panel with the dark screen colour and paint the 12 × 8 dotted cell
   edges.
2. If `workspace.windows()` is empty, paint the blank-screen message.
3. For each window, compute its pixel rectangle from its grid rectangle and
   draw a dark tile with a title bar, close button, clipped body and a
   lower-right resize grip. The body is the pane for that `WindowKind`, filled
   from the current `Snapshot`.
4. A title-bar drag records the pointer origin and the window's original grid
   rectangle. Each frame maps the pointer delta to grid cells and asks
   `move_to`; a rejected cell leaves the window where it was.
5. A resize grip does the same for `resize`, clamped to the window's minimum.
6. Close removes the window and clears any drag or resize state for it.
7. A click on empty canvas opens the Add Window picker at the pointer; a click
   inside a window does not. The hit test runs before the frame draws, so the
   window a close button removes still owns the click that removed it and the
   picker cannot open in the cell it just vacated. Choosing a tile places that
   window at the clicked cell and closes the picker; Escape or the picker's `×`
   closes it without adding anything.

There is no global command bar in `gui.rs`; the picker is the only add flow,
and the Command Line History pane carries its own prompt.

### Window bodies

`Desktop` keeps one `Snapshot` and rebuilds it when `App::revision()` moves and
at least every 250 ms, so the panes are live without holding the engine lock
while drawing. `window_body` maps each `WindowKind` to one drawing function:

| Pane | Source |
| --- | --- |
| Fixture Sheet, Patch | `listings.sheet`, with `Sel`/`Home`/`Def` and an `At` level |
| Sequence Sheet | `listings.playback`, one row per cue, plus `Go`/`Back`/`Pause`/`Release` |
| Playbacks | `desk.executors` as nine faders plus the grand master |
| Pools, Presets, Groups, Worlds, Filters, Effects | `listings.pools`, filtered by kind, with `Use`/`Off` and `Del` |
| Timecode Viewer | `listings.timecode`, with `Start`/`Pause`/`Stop`/`Locate`/`Add` |
| Command Line History | `model.history` with a prompt that calls `Model::submit` |
| DMX Sheet | `frame`, every rendered slot by address |
| Masters | `desk` page, grand, rate, tempo, selection, blind and blackout |
| Running Playbacks | `playing`, with target, cue, progress and state |
| Status | `listings.status`, the same lines the `Status` command prints |

Every control is a command string: the fader throttle turns a drag into
`Executor n At pc` at most every 33 ms, a cue row into `Go Sequence n`, and a
pool row into the matching `At Palette`, `Group`, `World`, `Filter` or
`Effect … On|Off` line. No pane writes to the engine directly, so the window
cannot become a second engine.

### Theme

The canvas keeps the dark grandMA3-like palette: near-black screen, blue-grey
window tiles, a muted orange accent and red for close. The theme also styles
egui's own widgets, so a rebuilt control inherits the same colours instead of
starting from the default light theme.

### Add Window picker

The picker is a fixed-size egui window placed at the pointer: five category
tabs (`Common`, `Pools`, `Playback`, `Tools`, `All`) over a four-column grid of
equal tiles. Each tile shows an empty placeholder box inline with the window
name, which is the only text in the tile; hovering outlines it and the pointer
becomes a hand. Choosing a tile calls `Model::add_window_at` with the kind and
the cell that opened the picker, so that cell is the window's upper-left
corner. The workspace clamps the rectangle inside the screen edge and falls
back to the first free slot when the cell is already taken, so every catalogue
tile opens its window while the screen has room. A successful placement closes
the picker; Escape or the window's `×` closes it without adding anything. The
tab selection resets to `Common` on the next open. `WindowTab` and
`WindowKind::ALL` remain the source for the tiles.

### History

`Model` keeps a bounded `history: Vec<CommandHistory>` in addition to the
existing `echo`:

```rust
pub struct CommandHistory {
    pub command: String,
    pub message: String,
    pub error: bool,
}
```

Every non-empty `act` appends one entry after execution. The model records the
latest error flag and message, and a history window can put a command back in
the input when the rebuild reaches it; there is no second execution path.

## Implementation history

1. Added `workspace.rs` with the grid, window catalogue, placement rules and
   unit tests.
2. Added `CommandHistory` and `Workspace` to `Model`; removed the fixed `Pane`
   enum and pane-switch tests.
3. Replaced `pane_bar`, `fader_bar` and the pane match in `gui.rs` with the
   workspace, window picker, tile chrome, drag/resize interaction and the left
   control rail; window bodies were split out of the old renderers.
4. Removed the chrome again: top bar, control rail, command bar, window bodies
   and every picker control. The canvas, its grid, the empty-state text, the
   window tile and the drag/resize/close gestures stayed, and the model was left
   untouched for the real add flow.
5. Updated the desktop guide, the phase note, the roadmap row and this design;
   regenerated the completeness ledger.
6. Changed the grid from centre dots to cell-edge lines, and brought the Add
   Window box back as an empty surface reached by clicking the canvas.
7. Made the grid dotted, anchored the picker at the clicked point, and filled
   it with the category tabs and catalogue tiles (display only).

## Tests

- **Blank start** — a new workspace has no windows, and adding/removing windows
  never runs a command or marks the snapshot dirty.
- **Placement** — a window added without a cell goes to the first free grid
  slot; a clicked cell is preferred when it fits; a full screen returns `None`.
- **No overlap** — moving or resizing onto another window fails and leaves the
  original rectangle intact.
- **Bounds and minimums** — a window cannot move outside 12 × 8 or shrink below
  its minimum; resizing clamps at the grid edge.
- **Identity** — every added window gets a unique ID, and closing one does not
  renumber the others.
- **Clear** — removing all windows returns to the empty-screen state without
  touching the model input or engine revision.
- **Catalogue** — every `WindowKind` appears in `All`, no kind is duplicated in
  a tab, and every default size fits the grid and its minimum.
- **History** — a successful command and a failed command each append one
  entry; the last entry records the error flag and message.
- **Existing widget tests** — pool commands, fader commands, cue commands and
  the command-line echo still produce exactly the same text.
- **Not tested here** — the window has no display in this environment. The
  workspace model, catalogue and command mapping are headless-tested; the
  canvas, first-frame layout, drag feel and resize feel still need a real
  X11/Wayland desktop pass as part of the hardware/display release gate.

## Boundaries

- No saved views, layouts, multiple screens, view buttons or quickeys. The
  screen is session-local and starts blank.
- No drag-and-drop of pool objects, no window settings pop-ups, no column
  editor or themes editor.
- No 3D viewer, layout viewer, content sheet, phaser/recipe editor, selection
  grid, special dialog, MAtricks editor, Xkeys, RDM, media or plugin windows.
- No engine, show or desk format change.
- No new command syntax; every action remains an existing command where one
  exists.

## Risks

- **Grid interaction is fiddly.** A move/resize gesture can be rejected on one
  frame and accepted later; the implementation keeps the original rectangle
  until a valid target is found, so the window never jumps into an overlap.
- **No persistence is a deliberate gap.** Operators may expect the screen to
  survive a restart. Saved views are a separate design; this item does not fake
  them with a hidden file.
- **The UI is incomplete on purpose.** The window cannot add a window yet, so a
  first-time user sees an empty screen with a hint. That is the rebuild's
  starting point, not a finished feature.
- **The real display remains unverified.** The GUI feature compiles and the
  headless model is tested, but no frame has been rendered on X11/Wayland here.
  The hardware/display proof remains the release gate.
