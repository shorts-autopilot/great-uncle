//! Structured listings shared by every front end.
//!
//! The terminal shell formats its windows from these rows through the command
//! line; the desktop GUI renders the same rows directly. Keeping the rows here
//! means a pane cannot invent a column or a second interpretation of the show:
//! if the GUI needs a value, the engine produces it and the CLI listing grows
//! it too.
//!
//! A listing is a *snapshot of values*, never a handle into the engine. Every
//! field is owned so the UI can drop the engine lock before drawing.

use crate::render::ValueSource;

/// One attribute cell on the sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetCell {
    /// `P`, `Pb`, `H` or `-`.
    pub source: ValueSource,
    /// Pretty attribute name, e.g. `Dimmer` or `Red`.
    pub attribute: String,
    /// 0..=100, the rendered level.
    pub level: u8,
}

impl SheetCell {
    /// The one cell formatter both `Sheet` and the window use.
    pub fn label(&self) -> String {
        format!("{} {} {}%", self.source.label(), self.attribute, self.level)
    }
}

/// One fixture row on the sheet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SheetRow {
    pub number: u16,
    pub name: String,
    pub type_key: String,
    pub mode: String,
    pub universe: u16,
    pub address: u16,
    pub cells: Vec<SheetCell>,
    /// RGB swatch computed from the rendered colour channels, when the mode has
    /// any. This is the rig's colour, not a palette's.
    pub colour: Option<[u8; 3]>,
    /// Why the fixture's mode could not be read, if it could not.
    pub error: Option<String>,
}

impl SheetRow {
    /// `1.001`, the patch address as the console prints it.
    pub fn patch(&self) -> String {
        format!("{}.{:03}", self.universe, self.address)
    }

    /// The per-attribute column: cells, or the mode error.
    pub fn output(&self) -> String {
        match &self.error {
            Some(error) => error.clone(),
            None => self
                .cells
                .iter()
                .map(SheetCell::label)
                .collect::<Vec<_>>()
                .join("  "),
        }
    }
}

/// A running cue, as a row sees it.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct RowRun {
    /// 0..=1 through the fade.
    pub progress: f64,
    pub paused: bool,
    pub done: bool,
}

/// One cue on the playback pane.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaybackRow {
    pub sequence: u16,
    pub sequence_name: String,
    pub cue: f64,
    pub cue_name: String,
    pub trigger: String,
    pub fade: f64,
    pub delay: f64,
    pub mib: bool,
    pub blocked: bool,
    /// Stored delta values in the cue.
    pub values: usize,
    /// Set when this cue is the one a sequence or executor playback is on.
    pub running: Option<RowRun>,
}

/// One pool entry: a palette, group, world, filter or effect.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PoolRow {
    /// `Palette`, `Group`, `World`, `Filter` or `Effect`.
    pub kind: String,
    pub number: u16,
    pub name: String,
    /// Fixtures, features or rate, depending on the kind.
    pub detail: String,
    /// Active on the desk (world, filter) or running (effect).
    pub active: bool,
}

/// What a timecode row describes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimecodeRowKind {
    /// A timecode show's summary line.
    Show,
    Event,
    Marker,
}

impl TimecodeRowKind {
    pub fn label(self) -> &'static str {
        match self {
            TimecodeRowKind::Show => "Show",
            TimecodeRowKind::Event => "Event",
            TimecodeRowKind::Marker => "Marker",
        }
    }
}

/// One timecode show, event or marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimecodeRow {
    pub kind: TimecodeRowKind,
    pub timecode: u16,
    /// The show name on a summary row, the track name on an event row.
    pub name: String,
    /// 0 on summary and marker rows.
    pub track: u8,
    pub at: String,
    pub action: String,
}

/// Every listing a front end draws, produced at one instant under one lock.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Listings {
    pub sheet: Vec<SheetRow>,
    pub playback: Vec<PlaybackRow>,
    pub pools: Vec<PoolRow>,
    pub timecode: Vec<TimecodeRow>,
    /// The same lines `Status` prints, in the same order.
    pub status: Vec<String>,
}
