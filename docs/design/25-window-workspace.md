# 25 · Window workspace

**Rank** 9 (follow-up) · **Order** 25 · **Size** L · **No format change** · **Depends on** 18 (desktop GUI), 24 (file menu)

## Problem

The desktop window is a fixed five-pane viewer. F1–F5 swap Sheet, Playback,
Pools, Timecode and Status; a permanent right rail holds nine faders and a grand
master. That is sufficient to inspect the engine, but it is not the way a
grandMA3 operator works. A grandMA3 screen starts as a user-defined area: the
operator chooses windows from an Add Window picker, places them on a snap grid,
resizes them, and builds the working screen from data pools, sheets, playback
controls and status views.

The fixed pane layout also makes every new data view a code change in the pane
enum. It cannot express two sheets side by side, a pool strip below a playback
window, or a large fixture sheet beside a small command history. This item
replaces the pane switcher with a real workspace while preserving the existing
rule that every UI action is command text and every draw reads an owned
snapshot.

## Behaviour when this is done

The window opens on an empty **Screen 1**:

```text
┌─────────────────────────────────────────────────────────────────────┐
│ OPENLIGHT   look.olshow          BLACKOUT   LOCK       20:14:05    │
├────┬───────────────────────────────────────────────────────────────┤
│ +  │ ·    ·    ·    ·    ·    ·    ·    ·    ·    ·    ·    ·      │
│ ▣  │ ·                                                             │
│ ⌫  │ ·                    Screen 1 is empty                       │
│    │ ·                  [ ADD WINDOW ]                            │
│    │ ·                                                             │
├────┴───────────────────────────────────────────────────────────────┤
│ Admin[Fixture]>                                                     │
│ Ready.                                                              │
└─────────────────────────────────────────────────────────────────────┘
```

- The screen starts blank on every launch. There are no prefilled panes and no
  permanent fader rail.
- `ADD WINDOW` or a click on an empty area opens the Add Window picker. The
  picker has **Common**, **Pools**, **Playback**, **Tools** and **All** tabs,
  echoing the grandMA3 Add Window categories that this desk can populate.
- A window snaps to a 12 × 8 grid. It can be moved by its title bar and resized
  from its lower-right corner. Windows do not overlap; a move or resize that
  would overlap is rejected and the last valid position is kept.
- Every window has a title bar and an `×` close button. The left control bar
  has `ADD`, `CLEAR` and `RESET`; `CLEAR` empties the screen without touching
  the show, and `RESET` also closes the picker and returns to the blank screen.
- Window layout is session state, not show or desk data. Closing the window and
  reopening it starts blank again. Saved views and layouts are a separate
  grandMA3 feature and are not claimed here.
- The command line stays at the bottom, always visible. It remains the only
  mutation path: selecting a fixture, recalling a pool, moving a fader or
  pressing a transport button all write the same command the console accepts.

## Window catalogue at this point

The official grandMA3 Add Window list is much larger than the data this engine
has. The picker exposes every window that can be filled from the current
`Snapshot` without inventing a second data model:

| Window | Current source | Actions |
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

The picker's five tabs are a smaller adaptation of the manual's seven:
Command Line History also appears on **Tools**, and Playbacks/Masters appear on
**Playback**, for quicker access. Two further adaptations are explicit: the
desk's generic **Pools** window lists its existing pool rows instead of the
manual's resource/store pools, and **Patch** and **Status** are exposed as
windows even though the manual does not list them under Add Window. They are
useful views over the same snapshot, not claims of feature parity.

The catalogue is additive: when a deferred subsystem gets a snapshot listing,
it gets a `WindowKind` and a picker entry without changing the workspace model.

## Design

### A headless workspace model

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
`WindowTab` supplies the static picker slices. The workspace supplies `add`,
`add_at`, `remove`, `clear`, `move_to`, `resize`, `window_at`, and the read-only
window list.

- `add_at` first tries the clicked grid cell, then the first free rectangle.
  It returns `None` when the screen is full; the GUI reports that rather than
  silently dropping the window.
- `move_to` and `resize` refuse rectangles outside the screen grid, below the
  window's minimum, or overlapping another window. They return `bool`; the GUI
  keeps the last valid layout.
- `window_at` is used to stop a click inside a window from also opening the
  Add Window picker.
- IDs are monotonic and never reused, so egui widget IDs and scroll positions
  remain stable while windows are rearranged.
- `Workspace::new()` is empty. There is no factory view and no default pane.

### Drawing and interaction

`gui.rs` draws the workspace inside one `CentralPanel`:

1. Fill the panel with the dark screen colour and paint the 12 × 8 dot grid.
2. If `workspace.windows()` is empty, paint the blank-screen message and a
   centred `ADD WINDOW` button.
3. For each window, compute its pixel rectangle from its grid rectangle and
   draw a dark tile with a title bar, close button, clipped body and a
   lower-right resize grip.
4. Wrap each body in `ui.push_id(window.id, …)`, so two Fixture Sheet windows
   do not share scroll/grid IDs.
5. A title-bar drag records the pointer origin and the window's original grid
   rectangle. Each frame maps the pointer delta to grid cells and asks
   `move_to`; a rejected cell leaves the window where it was.
6. A resize grip does the same for `resize`, clamped to the window's minimum.
7. An empty-area click records the clicked cell and opens the picker; the
   chosen kind is added at that cell when possible.

The existing `sheet`, `playback`, `pools`, `timecode` and `status` renderers
become window bodies rather than full-panel pages. `Playbacks` moves the old
right rail into a window body; `Masters`, `Running Playbacks`, `DMX Sheet`,
`Command Line History` and `Patch` are small read-only/new renderers over the
same snapshot. No window calls the engine directly.

### Theme and chrome

The fixed top bar, left control rail and bottom command bar remain, restyled
to the dark grandMA3-like palette: near-black screen, blue-grey panels, a
muted orange accent for active controls, red for close/lock, and monospace
tables. The control rail has:

- `ADD` — open the window picker.
- `CLEAR` — remove every window (not the show).
- `RESET` — clear and reopen the blank screen.
- The active page, selection count and a compact running-playback count.

The command prompt remains `openlight>` rather than pretending to be an MA
prompt; the UI does not claim MA compatibility, only the interaction pattern.

### Add Window picker

The picker is an anchored egui window with a tab row and a button grid. Each
button shows the window title and its default grid size. Hovering gives a
short description. Clicking adds the window and closes the picker. Closing the
picker or pressing Escape leaves the screen unchanged.

The tabs and their contents are static `WindowKind` slices; there is no runtime
registry and no plugin UI. That keeps the picker's scope visible in one place
and makes it obvious which grandMA3 windows are waiting for engine data.

### History window

`Model` gains a bounded `history: Vec<CommandHistory>` in addition to the
existing `echo`:

```rust
pub struct CommandHistory {
    pub command: String,
    pub message: String,
    pub error: bool,
}
```

Every non-empty `act` appends one entry after execution. The window shows the
last commands in order; the latest error is red. A click on a history row
places the command back in the input, matching the console's command-history
convenience without adding a second execution path.

## Implementation plan

1. Add `workspace.rs` with the grid, window catalog, placement rules and unit
   tests.
2. Add `CommandHistory` and `Workspace` to `Model`; remove the fixed `Pane`
   enum and pane-switch tests.
3. Replace `pane_bar`, `fader_bar` and the pane match in `gui.rs` with the
   blank workspace, window picker, tile chrome, drag/resize interaction and
   the left control rail.
4. Split the existing renderers into window bodies, add `Masters`, `Running
   Playbacks`, `DMX Sheet`, `Patch` and `Command Line History`.
5. Apply the dark theme, update the desktop guide and design index, and record
   the supported/deferred catalogue in the spec ledger.
6. Run the full gate, inspect the GUI model tests and the diff with fresh eyes,
   commit and push.

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
  workspace model, picker catalogue and command mapping are headless-tested;
  first-frame layout, drag feel and resize feel still need a real X11/Wayland
  desktop pass as part of the hardware/display release gate.

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
  survive a restart. Saved views are the next design; this item does not fake
  them with a hidden file.
- **Window bodies can be cramped.** Each kind has a minimum grid size, and
  tables scroll inside their own rectangle. A window at its minimum may show
  only headers until it is resized.
- **The real display remains unverified.** The GUI feature compiles and the
  headless model is tested, but no frame has been rendered on X11/Wayland here.
  The hardware/display proof remains the release gate.
