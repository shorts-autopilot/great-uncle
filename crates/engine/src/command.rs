//! The command language shared by every input surface.
//!
//! Grammar (keywords are case-insensitive):
//!
//! ```text
//! Fixture 1 Thru 5                        select fixtures
//! Fixture 1 Thru 5 At 50                  set Dimmer (0 to 100)
//! Fixture 1 At ColorAdd_R 40              set any attribute of the mode
//! Fixture 1 At Red Full                   pretty attribute names work too
//! Fixture 1 Gobo 2 | Gobo "Rotate CW"     choose a channel set by index or name
//! Fixture 1 Gobo                          list the sets and the current one
//! Group 1 Gobo "Open"                     the same through a group
//! Color Red | Color "#FF8000"             set a colour by name or hex
//! Color Rgb 255 128 0 | Cmy 0 60 100      set a colour by components
//! Color Hsb 30 100 50 | Color 3200K       set a colour by HSB or temperature
//! Fixture 1 Color Red | Group 1 Color Red one fixture or a group
//! Color                                   list the selection's colours
//! At Palette Color 3                      apply a palette to the selection
//! Full | Off                              apply to the current selection
//! Clear | Clear Values | Clear All
//! Patch Fixture 1 At 1.001 Type "X" [Mode "4ch"]
//! Unpatch Fixture 1 Thru 3
//! Clone Fixture 1 At 1.101 [Count 8] [Step 4]  copy a fixture across the rig
//! Patch Check                             overlaps, gaps and overflow
//! Patch Import "rig.csv"                  replace the patch from a spreadsheet
//! Report Patch|Cues|Palettes|Status|Show [<sequence>] "file"
//! Import Gdtf "file.gdtf"
//! Import Mvr "rig.mvr"
//! Import Type "profile.olft"
//! Export Type "key" "profile.olft"
//! Copy Type "key" As "new key"
//! Type New "Generic 4ch"
//! Type "Generic 4ch" Mode "4ch"
//! Type "Generic 4ch" Channel 1 Dimmer "Dimmer"
//! Begin [label] / End                    group edits into one undo step
//! Macro 1 "cue" Blackout; Wait 2; Blackout Off
//! Macro 1 Add "Store Cue 1" / Macro 1 Delete / Macro [1] / Macro 1 Run
//! New [Show] ["name"]
//! Store Palette <Feature> <n> ["name"]    capture the programmer
//! Delete Palette <Feature> <n> | Palette
//! Store Cue 1 ["name"] [Fade 2] [Delay 0.5] [Mib [On|Off]] [Sequence 2]
//! Store Cue 1 Merge | Replace           merge into the cue, or hold only the programmer
//! Store Cue 2 Follow 1.5 | Time 0.5      automatic triggers
//! Update [Cue 2] [Sequence 1]            merge the programmer into a cue
//! Cue 2 Block | Unblock                 stop / resume tracking
//! Cue 2 Shield Pan [Off] | Shield       assert an attribute, or list the shields
//! Cue 2 Move 5 | Cue 2 Copy 5 [Sequence 2]
//! Cue 2 Show                            print the cue's absolute state
//! Cue 2 Trigger Go | Follow 1 | Time 2   retime without re-storing
//! Delete Cue 1 [Sequence 2] | Sequence [2]
//! Assign Executor 1 Sequence 1
//! Executor 1 At 75 | Full | Off           executor master fader
//! Executor 1 Mode Go | Fader | Flash | Solo
//! Executor 1 Flash [Off] | Solo [Off]     hold or release a button
//! Go | Back | Pause | Release [Executor n | Sequence n]
//! Group 1 | Group 1 At 50 | Group 1 Full | Group 1 Off
//! Store Group 1 ["name"] | Delete Group 1 | Group
//! World 1 | World All                          activate / deactivate a world
//! Store World 1 ["name"] | Delete World 1 | World
//! Filter 1 | Filter All                       activate / deactivate a filter
//! Store Filter 1 ["name"] Dimmer Color        names must be quoted here
//! Delete Filter 1 | Filter
//! Master Grand At 75 | Master Rate At 200 | Master Group 1 At 50 | Master
//! Effect 1 On [Fixture 1 Thru 4] | Effect 1 Off | Effect Off | Effect
//! Store Effect 1 ["name"] [Attributes Pan Tilt] [Waveform Sine]
//!              [Rate 1 | 1/4] [Size 50] [Phase Pan 0 Tilt 90] [Spread 0]
//! Effect 1 Attributes Pan Tilt | Effect 1 Attribute Pan Size 30 Phase 90
//! Effect 1 Rate 1/4 | Effect 1 Tempo Off | Tempo 128 | Tap Tempo
//! Delete Effect 1 | Effect
//! Undo | Redo
//! Store Timecode 1 ["name"] [Fps 25] [Source Internal | Mtc "port" | Osc]
//! Delete Timecode 1 | Timecode [1]
//! Timecode 1 Add 00:00:10:00 Sequence 1 Cue 2
//! Timecode 1 Track 2 "FX" | Timecode 1 Track 2
//! Timecode 1 Track 2 Add 00:00:10:00 Effect 3 On Group 2
//! Timecode 1 Track 2 Remove 00:00:10:00
//! Timecode 1 Marker 00:02:00:00 "Chorus"
//! Timecode 1 Remove 00:00:10:00 | Timecode 1 At 00:00:10:00
//! Timecode 1 Start | Pause | Stop
//! Blackout [Fade 2] | Blackout Off
//! Blind | Blind Off                      keep the programmer off the stage
//! Preview [Cue 14] [Sequence 2] | Preview Off
//! Highlight | Highlight Off              selected fixtures at full intensity
//! Lock [code] | Unlock [code]
//! Sheet | Status | Probe | Help
//! Save [Show|Desk] "file" | Load [Show] "file"
//! ```
//!
//! `universe.address` uses a dot: `1.001` is universe 1, address 1.

use std::fmt;

use crate::color::{self, ColorSpec};
use crate::playback::{Curve, ExecutorMode, Trigger};
use crate::show::{InputPriority, InputTrigger, LossPolicy, MergeMode};
use crate::timecode::TimecodeSource;

#[derive(Debug, Clone, PartialEq)]
pub enum Token {
    Word(String),
    Number(String),
    Quoted(String),
    Plus,
    Thru,
}

impl fmt::Display for Token {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Token::Word(word) => write!(formatter, "`{word}`"),
            Token::Number(number) => write!(formatter, "`{number}`"),
            Token::Quoted(text) => write!(formatter, "`\"{text}\"`"),
            Token::Plus => write!(formatter, "`+`"),
            Token::Thru => write!(formatter, "`Thru`"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Level {
    /// 0..100.
    Percent(f64),
    Full,
}

/// Where a level comes from: a literal, or a palette in a feature pool.
#[derive(Debug, Clone, PartialEq)]
pub enum LevelSource {
    Level(Level),
    Palette { feature: String, number: u16 },
}

/// A trigger as written on the command line: `Go`, `Follow 1.5` or `Time 2`.
///
/// The show stores the time separately so a `Go` cue keeps the number while it
/// is not used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TriggerArg {
    pub trigger: Trigger,
    pub time: f64,
}

/// How `Store Cue` treats what the cue already holds.
///
/// `Merge` is the default: the programmer's attributes are written into the
/// cue and everything else it stored stays. `Replace` is the explicit "make
/// this cue match the programmer" form.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StoreMode {
    #[default]
    Merge,
    Replace,
}

/// Which playback a `Go`, `Back`, `Pause` or `Release` addresses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackRef {
    Sequence(u16),
    Executor(u16),
}

/// Which report `Report` writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportKind {
    Patch,
    Cues,
    Palettes,
    Status,
    Show,
}

/// A rate after `Rate`: a free rate in hertz, or a musical division such as
/// `1/4`. The engine turns the division into a `Tempo`.
#[derive(Debug, Clone, PartialEq)]
pub enum EffectRate {
    Hz(f64),
    Division(String),
}

/// An action on a timecode track.
#[derive(Debug, Clone, PartialEq)]
pub enum TimecodeAction {
    Cue {
        sequence: u16,
        cue: f64,
    },
    Effect {
        effect: u16,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        on: bool,
    },
}

/// A channel set after an attribute: `Gobo 2` or `Gobo "Rotate CW"`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SetTarget {
    /// 1-based, in DMX order.
    Index(usize),
    /// Exact, case-insensitive name.
    Name(String),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Select(Vec<u16>),
    SetLevel {
        fixtures: Option<Vec<u16>>,
        attribute: Option<String>,
        source: LevelSource,
    },
    /// `Fixture 1 Gobo [2 | "Rotate CW"]`: list a channel's sets, or choose one.
    SetChannelSet {
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        attribute: String,
        target: Option<SetTarget>,
    },
    Off {
        fixtures: Option<Vec<u16>>,
    },
    Clear {
        all: bool,
        values: bool,
    },
    Patch {
        fixture: u16,
        universe: u16,
        address: u16,
        type_key: Option<String>,
        mode: Option<String>,
    },
    Unpatch {
        fixtures: Vec<u16>,
    },
    /// `Clone Fixture 1 At 1.101 [Count 8] [Step 4]`.
    Clone {
        fixture: u16,
        universe: u16,
        address: u16,
        count: Option<u16>,
        step: Option<u16>,
    },
    /// `Patch Check`: overlaps, overflow and gaps, without changing anything.
    PatchCheck,
    /// `Dmx` / `Dmx In` / `Dmx <n>`: input, merge and meter listing.
    ListDmx {
        universe: Option<u16>,
    },
    SetDmxMerge {
        universe: u16,
        mode: MergeMode,
    },
    SetDmxPriority {
        universe: u16,
        priority: InputPriority,
    },
    SetDmxLoss {
        universe: u16,
        loss: LossPolicy,
    },
    SetDmxTimeout {
        universe: u16,
        ms: u64,
    },
    SetDmxTrigger {
        universe: u16,
        trigger: InputTrigger,
    },
    ClearDmxTrigger {
        universe: u16,
    },
    /// `Dmx <n> Snapshot`: capture the raw input frame into the programmer.
    DmxSnapshot {
        universe: u16,
    },
    /// `Patch Import "rig.csv"`: replace the patch from a spreadsheet.
    ImportPatch {
        path: String,
    },
    ImportGdtf {
        path: String,
    },
    /// `Import Mvr "rig.mvr"`: import a rig and its profiles.
    ImportMvr {
        path: String,
    },
    /// `Import Type "profile.olft"`: add or replace a portable profile.
    ImportType {
        path: String,
    },
    /// `Export Type "key" "profile.olft"`: write a portable profile.
    ExportType {
        type_key: String,
        path: String,
    },
    /// `Copy Type "key" As "new key"`.
    CopyType {
        type_key: String,
        new_key: String,
    },
    /// `Type New "Generic 4ch"`.
    TypeNew {
        key: String,
    },
    /// `Type "key" Mode "4ch"`.
    TypeMode {
        type_key: String,
        mode: String,
    },
    /// `Type "key" Channel 1 Dimmer "Dimmer"`.
    TypeChannel {
        type_key: String,
        offset: u16,
        attribute: String,
        name: Option<String>,
    },
    /// `Begin [label]`: open one undo group.
    Begin {
        label: Option<String>,
    },
    /// `End`: close the open undo group.
    End,
    /// `Macro <n> "name" <statements…>`: store or replace a macro.
    MacroStore {
        number: u16,
        name: String,
        statements: Vec<String>,
    },
    /// `Macro <n> Add <statement>`.
    MacroAdd {
        number: u16,
        statement: String,
    },
    /// `Macro <n> Delete`.
    MacroDelete {
        number: u16,
    },
    /// `Macro [n]`: list every macro, or one macro's statements.
    MacroList {
        number: Option<u16>,
    },
    /// `Macro <n> Run`.
    MacroRun {
        number: u16,
    },
    New {
        name: Option<String>,
    },
    StorePalette {
        feature: String,
        number: u16,
        name: Option<String>,
    },
    DeletePalette {
        feature: String,
        number: u16,
    },
    ListPalettes,
    StoreCue {
        sequence: u16,
        number: f64,
        name: Option<String>,
        fade: Option<f64>,
        delay: Option<f64>,
        mib: Option<bool>,
        trigger: Option<TriggerArg>,
        mode: StoreMode,
    },
    /// `Update [Cue 2] [Sequence 1]`: merge the programmer into a cue, defaulting
    /// to the cue currently running on that sequence.
    Update {
        sequence: u16,
        cue: Option<f64>,
    },
    /// `Cue 2 Block` / `Cue 2 Unblock`.
    CueBlock {
        sequence: u16,
        number: f64,
        on: bool,
    },
    /// `Cue 2 Shield Pan [Off]`, or no attribute to list the shields.
    CueShield {
        sequence: u16,
        number: f64,
        attribute: Option<String>,
        off: bool,
    },
    /// `Cue 2 Show`: the absolute state, not the stored delta.
    CueShow {
        sequence: u16,
        number: f64,
    },
    /// `Cue 2 Move 5`: renumber a cue, keeping every look where it was.
    CueMove {
        sequence: u16,
        number: f64,
        to: f64,
    },
    /// `Cue 2 Copy 5 [Sequence 2]`: deep-copy a cue, timing and shield included.
    CueCopy {
        sequence: u16,
        number: f64,
        to: f64,
        target: Option<u16>,
    },
    /// `Cue 2 Trigger Follow 1.5`: retime a stored cue without re-storing it.
    CueTrigger {
        sequence: u16,
        number: f64,
        trigger: TriggerArg,
    },
    /// `Cue 2 Timing ColorAdd_R Fade 0.5 …`: per-attribute timing. With no
    /// attribute and no clauses it lists the cue's timing table.
    CueTiming {
        sequence: u16,
        number: f64,
        attribute: Option<String>,
        fade: Option<f64>,
        delay: Option<f64>,
        curve: Option<Curve>,
        /// Drop the override and follow the cue again.
        off: bool,
    },
    CueStagger {
        sequence: u16,
        number: f64,
        stagger: f64,
    },
    /// `Fixture 1 Default …`: list or change a fixture's home look. No
    /// attribute means "list"; `off` drops the stored override.
    SetHome {
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        attribute: Option<String>,
        level: Option<Level>,
        off: bool,
    },
    /// `Fixture 1 Home`, `Group 2 Home` or bare `Home`: recall the base look
    /// into the programmer.
    Home {
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    },
    /// `Color <spec>`: set a colour through the fixture's own system, or list
    /// the selection's rendered colour when no spec is given.
    SetColor {
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
        spec: Option<ColorSpec>,
    },
    SetExecutorMode {
        executor: u16,
        mode: ExecutorMode,
    },
    SetExecutorFlash {
        executor: u16,
        on: bool,
    },
    SetExecutorSolo {
        executor: u16,
        on: bool,
    },
    DeleteCue {
        sequence: u16,
        number: f64,
    },
    ListSequences {
        only: Option<u16>,
    },
    AssignExecutor {
        /// `Assign Page 2 Executor 1 …`; `None` is the active page.
        page: Option<u16>,
        executor: u16,
        sequence: u16,
    },
    SetExecutorLevel {
        executor: u16,
        level: Level,
    },
    ListExecutors {
        only: Option<u16>,
    },
    ListPages,
    SetPage {
        number: u16,
    },
    StepPage {
        next: bool,
    },
    StorePage {
        number: u16,
        name: Option<String>,
    },
    DeletePage {
        number: u16,
    },
    SelectGroup(u16),
    SetLevelGroup {
        group: u16,
        attribute: Option<String>,
        source: LevelSource,
    },
    OffGroup {
        group: u16,
    },
    StoreGroup {
        number: u16,
        name: Option<String>,
    },
    DeleteGroup {
        number: u16,
    },
    ListGroups,
    StoreWorld {
        number: u16,
        name: Option<String>,
    },
    DeleteWorld {
        number: u16,
    },
    /// `None` deactivates the active world.
    ActivateWorld {
        number: Option<u16>,
    },
    ListWorlds,
    StoreFilter {
        number: u16,
        name: Option<String>,
        features: Vec<String>,
    },
    DeleteFilter {
        number: u16,
    },
    ActivateFilter {
        number: Option<u16>,
    },
    ListFilters,
    SetGrandMaster {
        level: Level,
    },
    SetRateMaster {
        level: Level,
    },
    SetGroupMaster {
        group: u16,
        level: Level,
    },
    ListMasters,
    StoreEffect {
        number: u16,
        name: Option<String>,
        targets: Option<Vec<String>>,
        waveform: Option<String>,
        rate: Option<EffectRate>,
        size: Option<f64>,
        phases: Vec<(String, f64)>,
        spread: Option<f64>,
    },
    DeleteEffect {
        number: u16,
    },
    ListEffects {
        only: Option<u16>,
    },
    EffectOn {
        effect: u16,
        fixtures: Option<Vec<u16>>,
    },
    EffectOff {
        effect: Option<u16>,
    },
    /// `Effect 1 Attributes Pan Tilt`: replace the target list.
    SetEffectAttributes {
        effect: u16,
        attributes: Vec<String>,
    },
    /// `Effect 1 Attribute Pan Size 30 Phase 90`: edit one target.
    SetEffectTarget {
        effect: u16,
        attribute: String,
        size: Option<f64>,
        phase: Option<f64>,
    },
    SetEffectRate {
        effect: u16,
        rate: EffectRate,
    },
    /// `Effect 1 Tempo Off`: keep the current musical rate as a free rate.
    SetEffectFreeRate {
        effect: u16,
    },
    SetTempo {
        bpm: f64,
    },
    TapTempo,
    StoreTimecode {
        number: u16,
        name: Option<String>,
        fps: Option<u8>,
        source: Option<TimecodeSource>,
    },
    DeleteTimecode {
        number: u16,
    },
    /// `Timecode 1 Track 2 ["FX"]`: create, rename or list one track.
    TimecodeTrack {
        timecode: u16,
        track: u8,
        name: Option<String>,
    },
    AddTimecodeEvent {
        timecode: u16,
        track: Option<u8>,
        at: String,
        action: TimecodeAction,
    },
    RemoveTimecodeEvent {
        timecode: u16,
        track: Option<u8>,
        at: String,
    },
    TimecodeMarker {
        timecode: u16,
        at: String,
        name: String,
    },
    StartTimecode {
        timecode: u16,
    },
    PauseTimecode {
        timecode: u16,
    },
    StopTimecode {
        timecode: u16,
    },
    LocateTimecode {
        timecode: u16,
        at: String,
    },
    ListTimecodes {
        only: Option<u16>,
    },
    Undo,
    Redo,
    Go {
        target: Option<PlaybackRef>,
    },
    Back {
        target: Option<PlaybackRef>,
    },
    Pause {
        target: Option<PlaybackRef>,
    },
    Release {
        target: Option<PlaybackRef>,
    },
    Blackout {
        off: bool,
        fade: Option<f64>,
    },
    Blind {
        on: bool,
    },
    Preview {
        cue: Option<f64>,
        sequence: Option<u16>,
        off: bool,
    },
    Highlight {
        on: bool,
    },
    Lock {
        code: Option<String>,
    },
    Unlock {
        code: Option<String>,
    },
    Sheet,
    Status,
    /// `Report <kind> [<sequence>] "<file>"`.
    Report {
        kind: ReportKind,
        sequence: Option<u16>,
        path: String,
    },
    /// `Probe`: report what every candidate serial port does with DMX frames.
    Probe,
    Save {
        path: String,
    },
    SaveShow {
        path: String,
    },
    SaveDesk {
        path: String,
    },
    Load {
        path: String,
        /// `Load Show` reads the document only and keeps this desk.
        show_only: bool,
    },
    Help,
}

impl Command {
    /// Whether running this command changes persisted state.
    ///
    /// The desk lock refuses everything that does. The match lists the
    /// non-mutating commands explicitly and treats anything unclassified as
    /// mutating, so a new command is locked out until someone classifies it:
    /// the failure mode is an over-strict lock, never an unlocked desk.
    pub fn mutates(&self) -> bool {
        match self {
            Command::Help
            | Command::Status
            | Command::Sheet
            | Command::ListPalettes
            | Command::ListSequences { .. }
            | Command::ListExecutors { .. }
            | Command::ListPages
            | Command::SetPage { .. }
            | Command::StepPage { .. }
            | Command::ListGroups
            | Command::ListWorlds
            | Command::ListFilters
            | Command::ListMasters
            | Command::ListEffects { .. }
            | Command::ListTimecodes { .. }
            | Command::Unlock { .. }
            | Command::PatchCheck
            | Command::ListDmx { .. }
            | Command::Probe
            | Command::ExportType { .. }
            | Command::MacroList { .. }
            | Command::Begin { .. }
            | Command::End
            | Command::Report { .. } => false,
            // Listing a channel's sets changes nothing.
            Command::SetChannelSet { target: None, .. } => false,
            // Listing a cue's timing table changes nothing.
            Command::CueTiming {
                attribute: None,
                fade: None,
                delay: None,
                curve: None,
                off: false,
                ..
            } => false,
            // Listing a cue's shields or absolute state changes nothing.
            Command::CueShield {
                attribute: None, ..
            }
            | Command::CueShow { .. } => false,
            // Listing a fixture's home changes nothing.
            Command::SetHome {
                attribute: None, ..
            } => false,
            // Listing a fixture's colour changes nothing.
            Command::SetColor { spec: None, .. } => false,
            // Lifting a blackout is always allowed: refusing it is a hazard.
            Command::Blackout { off: true, .. } => false,
            // A preview is a view, never an output or a saved intent.
            Command::Preview { .. } => false,
            _ => true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandError {
    pub message: String,
    pub hint: Option<String>,
}

impl CommandError {
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: None,
        }
    }

    pub fn with_hint(message: impl Into<String>, hint: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            hint: Some(hint.into()),
        }
    }
}

impl fmt::Display for CommandError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}", self.message)?;
        if let Some(hint) = &self.hint {
            write!(formatter, " ({hint})")?;
        }
        Ok(())
    }
}

impl std::error::Error for CommandError {}

/// Split a line into `;` separated statements, ignoring separators in quotes.
pub fn split_statements(line: &str) -> Vec<&str> {
    let mut statements = Vec::new();
    let mut start = 0;
    let mut quote: Option<char> = None;
    for (index, character) in line.char_indices() {
        match quote {
            Some(active) if character == active => quote = None,
            Some(_) => {}
            None if character == '"' || character == '\'' => quote = Some(character),
            None if character == ';' => {
                statements.push(line[start..index].trim());
                start = index + character.len_utf8();
            }
            None => {}
        }
    }
    statements.push(line[start..].trim());
    statements
        .into_iter()
        .filter(|part| !part.is_empty())
        .collect()
}

/// Parse a macro line before the generic `;` splitter sees it.
///
/// A macro definition deliberately contains semicolons, so it needs the raw
/// line. `None` means this is not a macro command and normal parsing applies.
pub fn parse_macro_line(line: &str) -> Result<Option<Command>, CommandError> {
    let line = line.trim();
    let mut first = line.splitn(2, char::is_whitespace);
    let Some(head) = first.next() else {
        return Ok(None);
    };
    if !head.eq_ignore_ascii_case("macro") {
        return Ok(None);
    }
    let rest = first.next().unwrap_or("").trim();
    if rest.is_empty() {
        return Ok(Some(Command::MacroList { number: None }));
    }

    let digits = rest
        .find(|character: char| !character.is_ascii_digit())
        .unwrap_or(rest.len());
    if digits == 0 {
        return Err(CommandError::with_hint(
            "macro needs a number",
            "`Macro 1 Run` or `Macro 1 \"name\" <statements…>`",
        ));
    }
    let number: u32 = rest[..digits]
        .parse()
        .map_err(|_| CommandError::new(format!("`{}` is not a macro number", &rest[..digits])))?;
    if number == 0 || number > u32::from(u16::MAX) {
        return Err(CommandError::new(format!(
            "macro number {number} is out of range"
        )));
    }
    let number = number as u16;
    let rest = rest[digits..].trim_start();
    if rest.is_empty() {
        return Ok(Some(Command::MacroList {
            number: Some(number),
        }));
    }
    if rest.eq_ignore_ascii_case("run") {
        return Ok(Some(Command::MacroRun { number }));
    }
    if rest.eq_ignore_ascii_case("delete") {
        return Ok(Some(Command::MacroDelete { number }));
    }
    if let Some(statement) = strip_word(rest, "add") {
        if statement.trim().is_empty() {
            return Err(CommandError::new("`Macro Add` needs a statement"));
        }
        if split_statements(statement).len() != 1 {
            return Err(CommandError::with_hint(
                "`Macro Add` takes one statement",
                "use `Macro <n> \"name\" …` to replace the whole macro",
            ));
        }
        return Ok(Some(Command::MacroAdd {
            number,
            statement: statement.trim().to_string(),
        }));
    }

    let Some((name, remainder)) = take_quoted(rest) else {
        return Err(CommandError::with_hint(
            "macro name must be quoted",
            "`Macro 1 \"Blackout cue\" Blackout; Wait 2; Blackout Off`",
        ));
    };
    let statements: Vec<String> = split_statements(remainder)
        .into_iter()
        .map(|statement| statement.trim().to_string())
        .filter(|statement| !statement.is_empty())
        .collect();
    if statements.is_empty() {
        return Err(CommandError::new("a macro needs at least one statement"));
    }
    Ok(Some(Command::MacroStore {
        number,
        name,
        statements,
    }))
}

fn strip_word<'a>(text: &'a str, word: &str) -> Option<&'a str> {
    let end = text
        .find(|character: char| character.is_whitespace())
        .unwrap_or(text.len());
    if !text[..end].eq_ignore_ascii_case(word) {
        return None;
    }
    Some(text[end..].trim_start())
}

fn take_quoted(text: &str) -> Option<(String, &str)> {
    let quote = text.chars().next()?;
    if quote != '"' && quote != '\'' {
        return None;
    }
    let mut value = String::new();
    let mut escaped = false;
    for (index, character) in text.char_indices().skip(1) {
        if escaped {
            value.push(character);
            escaped = false;
            continue;
        }
        if character == '\\' {
            escaped = true;
            continue;
        }
        if character == quote {
            return Some((value, &text[index + character.len_utf8()..]));
        }
        value.push(character);
    }
    None
}

pub fn lex(line: &str) -> Result<Vec<Token>, CommandError> {
    let chars: Vec<char> = line.chars().collect();
    let mut tokens = Vec::new();
    let mut index = 0;

    while index < chars.len() {
        let character = chars[index];
        if character.is_whitespace() {
            index += 1;
            continue;
        }
        match character {
            '+' => {
                tokens.push(Token::Plus);
                index += 1;
            }
            '"' | '\'' => {
                let quote = character;
                index += 1;
                let mut text = String::new();
                let mut closed = false;
                while index < chars.len() {
                    let current = chars[index];
                    if current == '\\' && index + 1 < chars.len() {
                        text.push(chars[index + 1]);
                        index += 2;
                        continue;
                    }
                    if current == quote {
                        closed = true;
                        index += 1;
                        break;
                    }
                    text.push(current);
                    index += 1;
                }
                if !closed {
                    return Err(CommandError::with_hint(
                        "unterminated quoted string",
                        "close the string with the same quote character",
                    ));
                }
                tokens.push(Token::Quoted(text));
            }
            digit if digit.is_ascii_digit() => {
                let start = index;
                // `1.001` is a patch address and `00:00:10:00` is a timecode;
                // both are one token that the parser validates.
                while index < chars.len()
                    && (chars[index].is_ascii_digit()
                        || chars[index] == '.'
                        || chars[index] == ':'
                        || chars[index] == '/')
                {
                    index += 1;
                }
                // `4ch` is a name, `50%` is a number followed by a suffix.
                if index < chars.len() && (chars[index].is_alphabetic() || chars[index] == '_') {
                    while index < chars.len() && is_word_char(chars[index]) {
                        index += 1;
                    }
                    tokens.push(Token::Word(chars[start..index].iter().collect()));
                } else {
                    tokens.push(Token::Number(chars[start..index].iter().collect()));
                }
            }
            _ => {
                let start = index;
                while index < chars.len() && is_word_char(chars[index]) {
                    index += 1;
                }
                if index == start {
                    return Err(CommandError::new(format!(
                        "unexpected character `{character}`"
                    )));
                }
                let word: String = chars[start..index].iter().collect();
                if word.eq_ignore_ascii_case("thru") {
                    tokens.push(Token::Thru);
                } else {
                    tokens.push(Token::Word(word));
                }
            }
        }
    }

    Ok(tokens)
}

fn is_word_char(character: char) -> bool {
    !character.is_whitespace() && !matches!(character, '+' | '"' | '\'' | ';')
}

/// `3200K` / `3200k` — a whole number directly followed by K.
fn kelvin_word(word: &str) -> Option<u32> {
    let digits = word.strip_suffix('k').or_else(|| word.strip_suffix('K'))?;
    if digits.is_empty() || !digits.chars().all(|character| character.is_ascii_digit()) {
        return None;
    }
    digits.parse().ok()
}

fn hex_error(text: &str) -> CommandError {
    CommandError::with_hint(
        format!("`{text}` is not a colour"),
        "use exactly six hex digits: `#FF8000`",
    )
}

fn unknown_colour(name: &str) -> CommandError {
    let names: Vec<&str> = color::NAMED_COLOURS.iter().map(|(name, _)| *name).collect();
    CommandError::with_hint(
        format!("unknown colour `{name}`"),
        format!("named colours: {}", names.join(", ")),
    )
}

pub fn parse(tokens: &[Token]) -> Result<Command, CommandError> {
    let mut parser = Parser {
        tokens,
        position: 0,
    };
    parser.command()
}

/// Keywords that may never be read as an object name.
const RESERVED: &[&str] = &[
    "fade",
    "delay",
    "sequence",
    "in",
    "at",
    "full",
    "off",
    "palette",
    "executor",
    "page",
    "pages",
    "fixture",
    "type",
    "mode",
    "thru",
    "values",
    "all",
    "show",
    "group",
    "world",
    "filter",
    "master",
    "grand",
    "rate",
    "effect",
    "attribute",
    "attributes",
    "waveform",
    "size",
    "phase",
    "tempo",
    "tap",
    "spread",
    "on",
    "mib",
    "merge",
    "replace",
    "block",
    "unblock",
    "shield",
    "move",
    "copy",
    "update",
    "timecode",
    "fps",
    "source",
    "track",
    "marker",
    "add",
    "remove",
    "start",
    "stop",
    "undo",
    "redo",
    "trigger",
    "follow",
    "flash",
    "solo",
    "mode",
    "fader",
    "go",
    "timing",
    "stagger",
    "curve",
    "default",
    "home",
    "blind",
    "preview",
    "highlight",
    "color",
    "dmx",
    "priority",
    "loss",
    "hold",
    "zero",
    "timeout",
    "threshold",
    "snapshot",
];

struct Parser<'a> {
    tokens: &'a [Token],
    position: usize,
}

impl Parser<'_> {
    fn peek(&self) -> Option<&Token> {
        self.tokens.get(self.position)
    }

    fn bump(&mut self) -> Option<Token> {
        let token = self.tokens.get(self.position).cloned();
        if token.is_some() {
            self.position += 1;
        }
        token
    }

    fn at_end(&self) -> bool {
        self.position >= self.tokens.len()
    }

    fn eat_word(&mut self, keyword: &str) -> bool {
        if matches!(self.peek(), Some(Token::Word(word)) if word.eq_ignore_ascii_case(keyword)) {
            self.position += 1;
            return true;
        }
        false
    }

    fn expect_end(&self) -> Result<(), CommandError> {
        match self.peek() {
            Some(token) => Err(CommandError::with_hint(
                format!("unexpected {token}"),
                "type `Help` for the command list",
            )),
            None => Ok(()),
        }
    }

    fn command(&mut self) -> Result<Command, CommandError> {
        let Some(token) = self.peek().cloned() else {
            return Err(CommandError::new("empty command"));
        };
        match token {
            Token::Word(word) => self.word_command(&word),
            Token::Number(_) => {
                let fixtures = self.fixture_list()?;
                self.selection_or_action(fixtures)
            }
            other => Err(CommandError::with_hint(
                format!("unexpected {other}"),
                "a command starts with a keyword or a fixture number",
            )),
        }
    }

    fn word_command(&mut self, word: &str) -> Result<Command, CommandError> {
        self.position += 1;
        if word.eq_ignore_ascii_case("fixture") {
            let fixtures = self.fixture_list()?;
            return self.selection_or_action(fixtures);
        }
        if word.eq_ignore_ascii_case("begin") {
            let label = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::Begin { label });
        }
        if word.eq_ignore_ascii_case("end") {
            self.expect_end()?;
            return Ok(Command::End);
        }
        if word.eq_ignore_ascii_case("at") {
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                attribute,
                source,
            });
        }
        if word.eq_ignore_ascii_case("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Full),
            });
        }
        if word.eq_ignore_ascii_case("off") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Off { fixtures });
        }
        if word.eq_ignore_ascii_case("home") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Home {
                fixtures,
                group: None,
            });
        }
        if word.eq_ignore_ascii_case("color") {
            return self.color_action(None, None);
        }
        if word.eq_ignore_ascii_case("clear") {
            let mut all = false;
            let mut values = false;
            if self.eat_word("all") {
                all = true;
                values = true;
            } else if self.eat_word("values") {
                values = true;
            }
            self.expect_end()?;
            return Ok(Command::Clear { all, values });
        }
        if word.eq_ignore_ascii_case("clone") {
            return self.clone_fixture();
        }
        if word.eq_ignore_ascii_case("patch") {
            return self.patch();
        }
        if word.eq_ignore_ascii_case("dmx") {
            return self.dmx();
        }
        if word.eq_ignore_ascii_case("unpatch") {
            self.eat_word("fixture");
            let fixtures = self.fixture_list()?;
            self.expect_end()?;
            return Ok(Command::Unpatch { fixtures });
        }
        if word.eq_ignore_ascii_case("import") {
            if self.eat_word("mvr") {
                let path = self.path_token()?;
                self.expect_end()?;
                return Ok(Command::ImportMvr { path });
            }
            if self.eat_word("type") {
                let path = self.path_token()?;
                self.expect_end()?;
                return Ok(Command::ImportType { path });
            }
            self.eat_word("gdtf");
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::ImportGdtf { path });
        }
        if word.eq_ignore_ascii_case("export") {
            self.eat_word("type");
            let type_key = self.name_token("fixture type")?;
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::ExportType { type_key, path });
        }
        if word.eq_ignore_ascii_case("copy") {
            self.eat_word("type");
            let type_key = self.name_token("fixture type")?;
            self.eat_word("as");
            let new_key = self.name_token("new fixture type")?;
            self.expect_end()?;
            return Ok(Command::CopyType { type_key, new_key });
        }
        if word.eq_ignore_ascii_case("type") {
            return self.type_command();
        }
        if word.eq_ignore_ascii_case("new") {
            self.eat_word("show");
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::New { name });
        }
        if word.eq_ignore_ascii_case("store") {
            return self.store();
        }
        if word.eq_ignore_ascii_case("delete") {
            return self.delete();
        }
        if word.eq_ignore_ascii_case("assign") {
            return self.assign();
        }
        if word.eq_ignore_ascii_case("go") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Go { target });
        }
        if word.eq_ignore_ascii_case("back") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Back { target });
        }
        if word.eq_ignore_ascii_case("pause") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Pause { target });
        }
        if word.eq_ignore_ascii_case("release") {
            let target = self.playback_ref()?;
            self.expect_end()?;
            return Ok(Command::Release { target });
        }
        if word.eq_ignore_ascii_case("palette") || word.eq_ignore_ascii_case("palettes") {
            self.expect_end()?;
            return Ok(Command::ListPalettes);
        }
        if word.eq_ignore_ascii_case("cue") {
            return self.cue_command();
        }
        if word.eq_ignore_ascii_case("update") {
            return self.update();
        }
        if word.eq_ignore_ascii_case("sequence") || word.eq_ignore_ascii_case("sequences") {
            let only = if matches!(self.peek(), Some(Token::Number(_))) {
                Some(self.whole_number("sequence number")?)
            } else {
                None
            };
            self.expect_end()?;
            return Ok(Command::ListSequences { only });
        }
        if word.eq_ignore_ascii_case("executor") || word.eq_ignore_ascii_case("executors") {
            return self.executor();
        }
        if word.eq_ignore_ascii_case("page") || word.eq_ignore_ascii_case("pages") {
            return self.page_command();
        }
        if word.eq_ignore_ascii_case("group") || word.eq_ignore_ascii_case("groups") {
            return self.group_command();
        }
        if word.eq_ignore_ascii_case("world") || word.eq_ignore_ascii_case("worlds") {
            return self.world_command();
        }
        if word.eq_ignore_ascii_case("filter") || word.eq_ignore_ascii_case("filters") {
            return self.filter_command();
        }
        if word.eq_ignore_ascii_case("master") || word.eq_ignore_ascii_case("masters") {
            return self.master_command();
        }
        if word.eq_ignore_ascii_case("effect") || word.eq_ignore_ascii_case("effects") {
            return self.effect_command();
        }
        if word.eq_ignore_ascii_case("tempo") {
            let bpm = self.number_value("tempo")?;
            self.expect_end()?;
            return Ok(Command::SetTempo { bpm });
        }
        if word.eq_ignore_ascii_case("tap") {
            self.eat_word("tempo");
            self.expect_end()?;
            return Ok(Command::TapTempo);
        }
        if word.eq_ignore_ascii_case("timecode") || word.eq_ignore_ascii_case("timecodes") {
            return self.timecode_command();
        }
        if word.eq_ignore_ascii_case("undo") {
            self.expect_end()?;
            return Ok(Command::Undo);
        }
        if word.eq_ignore_ascii_case("redo") {
            self.expect_end()?;
            return Ok(Command::Redo);
        }
        if word.eq_ignore_ascii_case("blackout") || word.eq_ignore_ascii_case("black") {
            if self.eat_word("off") {
                self.expect_end()?;
                return Ok(Command::Blackout {
                    off: true,
                    fade: None,
                });
            }
            let fade = if self.eat_word("fade") {
                Some(self.number_value("fade seconds")?)
            } else {
                None
            };
            self.expect_end()?;
            return Ok(Command::Blackout { off: false, fade });
        }
        if word.eq_ignore_ascii_case("blind") {
            let on = !self.eat_word("off");
            self.expect_end()?;
            return Ok(Command::Blind { on });
        }
        if word.eq_ignore_ascii_case("highlight") || word.eq_ignore_ascii_case("highlt") {
            let on = !self.eat_word("off");
            self.expect_end()?;
            return Ok(Command::Highlight { on });
        }
        if word.eq_ignore_ascii_case("preview") || word.eq_ignore_ascii_case("prvw") {
            if self.eat_word("off") {
                self.expect_end()?;
                return Ok(Command::Preview {
                    cue: None,
                    sequence: None,
                    off: true,
                });
            }
            let cue = if self.eat_word("cue") {
                Some(self.cue_number()?)
            } else {
                None
            };
            let sequence = if self.eat_word("sequence") {
                Some(self.whole_number("sequence number")?)
            } else {
                None
            };
            self.expect_end()?;
            return Ok(Command::Preview {
                cue,
                sequence,
                off: false,
            });
        }
        if word.eq_ignore_ascii_case("lock") {
            let code = self.optional_code()?;
            self.expect_end()?;
            return Ok(Command::Lock { code });
        }
        if word.eq_ignore_ascii_case("unlock") {
            let code = self.optional_code()?;
            self.expect_end()?;
            return Ok(Command::Unlock { code });
        }
        if word.eq_ignore_ascii_case("sheet") || word.eq_ignore_ascii_case("fixtures") {
            self.expect_end()?;
            return Ok(Command::Sheet);
        }
        if word.eq_ignore_ascii_case("status") {
            self.expect_end()?;
            return Ok(Command::Status);
        }
        if word.eq_ignore_ascii_case("probe") {
            self.expect_end()?;
            return Ok(Command::Probe);
        }
        if word.eq_ignore_ascii_case("report") {
            return self.report();
        }
        if word.eq_ignore_ascii_case("output") {
            self.eat_word("status");
            self.expect_end()?;
            return Ok(Command::Status);
        }
        if word.eq_ignore_ascii_case("save") {
            if self.eat_word("show") {
                let path = self.path_token()?;
                self.expect_end()?;
                return Ok(Command::SaveShow { path });
            }
            if self.eat_word("desk") {
                let path = self.path_token()?;
                self.expect_end()?;
                return Ok(Command::SaveDesk { path });
            }
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::Save { path });
        }
        if word.eq_ignore_ascii_case("load") {
            let show_only = self.eat_word("show");
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::Load { path, show_only });
        }
        if word.eq_ignore_ascii_case("help") || word == "?" {
            self.expect_end()?;
            return Ok(Command::Help);
        }
        Err(CommandError::with_hint(
            format!("unknown command `{word}`"),
            "type `Help` for the command list",
        ))
    }

    fn selection_or_action(&mut self, fixtures: Vec<u16>) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::Select(fixtures));
        }
        if self.eat_word("at") {
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                attribute,
                source,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetLevel {
                fixtures: Some(fixtures),
                attribute: None,
                source: LevelSource::Level(Level::Full),
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::Off {
                fixtures: Some(fixtures),
            });
        }
        if self.eat_word("home") {
            self.expect_end()?;
            return Ok(Command::Home {
                fixtures: Some(fixtures),
                group: None,
            });
        }
        if self.eat_word("color") {
            return self.color_action(Some(fixtures), None);
        }
        if self.eat_word("default") {
            return self.default_action(Some(fixtures), None);
        }
        if matches!(self.peek(), Some(Token::Word(_))) {
            return self.channel_set_action(Some(fixtures), None);
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "after a fixture list use `At <level>`, `Full`, `Off`, `Home`, `Color`, `Default` or `Gobo 2`",
        ))
    }

    /// `[<attribute> [<set>]]`: list a channel's sets, or choose one by index
    /// or quoted name. A bare number is always a 1-based index; a level needs
    /// `At`, so the two can never be confused.
    fn channel_set_action(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    ) -> Result<Command, CommandError> {
        let attribute = self.name_token("attribute")?;
        if self.at_end() {
            return Ok(Command::SetChannelSet {
                fixtures,
                group,
                attribute,
                target: None,
            });
        }
        let target = match self.bump() {
            Some(Token::Number(raw)) => {
                if raw.contains('.') {
                    return Err(CommandError::with_hint(
                        format!("`{raw}` is not a set index"),
                        "a level needs `At`: `Fixture 1 At Gobo 40`",
                    ));
                }
                let index: usize = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a set index")))?;
                if index == 0 {
                    return Err(CommandError::with_hint(
                        "set index 0 is not allowed",
                        "sets are numbered from 1; `Fixture 1 Gobo` lists them",
                    ));
                }
                SetTarget::Index(index)
            }
            Some(Token::Quoted(name)) if !name.trim().is_empty() => SetTarget::Name(name),
            Some(Token::Quoted(_)) => {
                return Err(CommandError::new("a set name cannot be empty"));
            }
            Some(other) => {
                return Err(CommandError::with_hint(
                    format!("unexpected {other} after `{attribute}`"),
                    "use a 1-based set index or a quoted set name",
                ));
            }
            None => unreachable!("at_end was checked above"),
        };
        self.expect_end()?;
        Ok(Command::SetChannelSet {
            fixtures,
            group,
            attribute,
            target: Some(target),
        })
    }

    /// `Color [<spec>]`: a colour intent, or the listing when nothing follows.
    fn color_action(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    ) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::SetColor {
                fixtures,
                group,
                spec: None,
            });
        }
        let spec = self.color_spec()?;
        self.expect_end()?;
        Ok(Command::SetColor {
            fixtures,
            group,
            spec: Some(spec),
        })
    }

    /// One colour: a name, a hex value, `Rgb`/`Cmy`/`Hsb` components or a
    /// kelvin temperature. Everything is validated here so the engine only
    /// sees values it can write.
    fn color_spec(&mut self) -> Result<ColorSpec, CommandError> {
        let Some(token) = self.bump() else {
            return Err(CommandError::with_hint(
                "expected a colour",
                "use a name, `#RRGGBB`, `Rgb 255 128 0`, `Cmy 0 60 100`, `Hsb 30 100 50` or `3200K`",
            ));
        };
        match token {
            Token::Word(word) => {
                if word.eq_ignore_ascii_case("rgb") {
                    let red = self.byte_component("red")?;
                    let green = self.byte_component("green")?;
                    let blue = self.byte_component("blue")?;
                    return Ok(ColorSpec::Rgb(red, green, blue));
                }
                if word.eq_ignore_ascii_case("cmy") {
                    let cyan = self.number_component("cyan", 0.0, 100.0)?;
                    let magenta = self.number_component("magenta", 0.0, 100.0)?;
                    let yellow = self.number_component("yellow", 0.0, 100.0)?;
                    return Ok(ColorSpec::Cmy(cyan, magenta, yellow));
                }
                if word.eq_ignore_ascii_case("hsb") {
                    let hue = self.number_component("hue", 0.0, 360.0)?;
                    let saturation = self.number_component("saturation", 0.0, 100.0)?;
                    let brightness = self.number_component("brightness", 0.0, 100.0)?;
                    return Ok(ColorSpec::Hsb(hue, saturation, brightness));
                }
                if word.starts_with('#') {
                    if color::parse_hex(&word).is_none() {
                        return Err(hex_error(&word));
                    }
                    return Ok(ColorSpec::Hex(word));
                }
                if let Some(kelvin) = kelvin_word(&word) {
                    if !(u32::from(color::MIN_KELVIN)..=u32::from(color::MAX_KELVIN))
                        .contains(&kelvin)
                    {
                        return Err(CommandError::new(format!(
                            "colour temperature {kelvin}K is outside {}..{}K",
                            color::MIN_KELVIN,
                            color::MAX_KELVIN
                        )));
                    }
                    return Ok(ColorSpec::Kelvin(kelvin as u16));
                }
                // A name may be several words: `Color Warm White`. Join the
                // rest, so a typo is reported as one unknown colour rather
                // than as an unexpected token.
                let mut name = word;
                while let Some(Token::Word(next)) = self.peek().cloned() {
                    self.position += 1;
                    name.push(' ');
                    name.push_str(&next);
                }
                if color::named(&name).is_none() {
                    return Err(unknown_colour(&name));
                }
                Ok(ColorSpec::Named(name))
            }
            Token::Quoted(text) => {
                if text.trim().starts_with('#') {
                    if color::parse_hex(&text).is_none() {
                        return Err(hex_error(&text));
                    }
                    Ok(ColorSpec::Hex(text))
                } else if color::named(&text).is_some() {
                    Ok(ColorSpec::Named(text))
                } else {
                    Err(unknown_colour(&text))
                }
            }
            Token::Number(raw) => Err(CommandError::with_hint(
                format!("`{raw}` is not a colour"),
                "a kelvin value needs a K: `3200K`; otherwise use a name or a component form",
            )),
            other => Err(CommandError::with_hint(
                format!("unexpected {other} after `Color`"),
                "use a name, `#RRGGBB`, `Rgb 255 128 0`, `Cmy 0 60 100`, `Hsb 30 100 50` or `3200K`",
            )),
        }
    }

    fn number_component(&mut self, what: &str, min: f64, max: f64) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what}")))?;
                if !value.is_finite() || value < min || value > max {
                    return Err(CommandError::new(format!(
                        "{what} {raw} is outside {min}..{max}"
                    )));
                }
                Ok(value)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a {what}, got {other}"
            ))),
            None => Err(CommandError::with_hint(
                format!("expected a {what}"),
                "`Rgb` takes three 0..255 bytes; `Cmy` and `Hsb` take three 0..100 values (hue 0..360)",
            )),
        }
    }

    fn byte_component(&mut self, what: &str) -> Result<u8, CommandError> {
        let value = self.number_component(what, 0.0, 255.0)?;
        if (value - value.round()).abs() > 1e-9 {
            return Err(CommandError::new(format!(
                "{what} {value} must be a whole number"
            )));
        }
        Ok(value.round() as u8)
    }

    /// `Default [<attribute> <level> | <attribute> Off]`: a home override, or
    /// the listing when nothing follows.
    fn default_action(
        &mut self,
        fixtures: Option<Vec<u16>>,
        group: Option<u16>,
    ) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::SetHome {
                fixtures,
                group,
                attribute: None,
                level: None,
                off: false,
            });
        }
        if self.eat_word("off") {
            return Err(CommandError::with_hint(
                "`Default Off` needs an attribute",
                "use `Default Dimmer Off` to drop one override",
            ));
        }
        let attribute = self.name_token("attribute")?;
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::SetHome {
                fixtures,
                group,
                attribute: Some(attribute),
                level: None,
                off: true,
            });
        }
        let level = self.level()?;
        self.expect_end()?;
        Ok(Command::SetHome {
            fixtures,
            group,
            attribute: Some(attribute),
            level: Some(level),
            off: false,
        })
    }

    fn optional_fixture_list(&mut self) -> Result<Option<Vec<u16>>, CommandError> {
        self.eat_word("fixture");
        if matches!(self.peek(), Some(Token::Number(_))) {
            Ok(Some(self.fixture_list()?))
        } else {
            Ok(None)
        }
    }

    fn fixture_list(&mut self) -> Result<Vec<u16>, CommandError> {
        let mut fixtures = Vec::new();
        loop {
            let first = self.fixture_number()?;
            if matches!(self.peek(), Some(Token::Thru)) {
                self.position += 1;
                let last = self.fixture_number()?;
                let (low, high) = (first.min(last), first.max(last));
                if u32::from(high) - u32::from(low) > 4096 {
                    return Err(CommandError::new(format!(
                        "range {first} Thru {last} is too large"
                    )));
                }
                fixtures.extend(low..=high);
            } else {
                fixtures.push(first);
            }
            if matches!(self.peek(), Some(Token::Plus)) {
                self.position += 1;
                continue;
            }
            break;
        }
        fixtures.sort_unstable();
        fixtures.dedup();
        Ok(fixtures)
    }

    fn fixture_number(&mut self) -> Result<u16, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                let number: u32 = raw.parse().map_err(|_| {
                    CommandError::new(format!("`{raw}` is not a whole fixture number"))
                })?;
                if number == 0 || number > u32::from(u16::MAX) {
                    return Err(CommandError::new(format!(
                        "fixture number {number} is out of range"
                    )));
                }
                Ok(number as u16)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a fixture number, got {other}"
            ))),
            None => Err(CommandError::new(
                "expected a fixture number, e.g. `Fixture 1 Thru 4`",
            )),
        }
    }

    /// A positive whole number such as a palette, sequence or executor number.
    fn whole_number(&mut self, what: &str) -> Result<u16, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                if raw.contains('.') {
                    return Err(CommandError::new(format!(
                        "{what} `{raw}` must be a whole number"
                    )));
                }
                let number: u32 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what}")))?;
                if number == 0 || number > u32::from(u16::MAX) {
                    return Err(CommandError::new(format!(
                        "{what} {number} is out of range"
                    )));
                }
                Ok(number as u16)
            }
            Some(other) => Err(CommandError::new(format!("expected a {what}, got {other}"))),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }

    fn cue_number(&mut self) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) => {
                let number: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a cue number")))?;
                if !number.is_finite() || number < 0.0 {
                    return Err(CommandError::new(format!(
                        "cue number {raw} is not a finite positive number"
                    )));
                }
                Ok(number)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a cue number, got {other}"
            ))),
            None => Err(CommandError::new(
                "expected a cue number, e.g. `Store Cue 1`",
            )),
        }
    }

    fn level(&mut self) -> Result<Level, CommandError> {
        if self.eat_word("full") {
            return Ok(Level::Full);
        }
        match self.bump() {
            Some(Token::Number(raw)) => {
                self.eat_word("percent");
                if matches!(self.peek(), Some(Token::Word(word)) if word == "%") {
                    self.position += 1;
                }
                let percent: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a level")))?;
                Ok(Level::Percent(percent))
            }
            Some(other) => Err(CommandError::with_hint(
                format!("expected a level, got {other}"),
                "use a percentage from 0 to 100, `Full`, or `Palette <Feature> <n>`",
            )),
            None => Err(CommandError::with_hint(
                "expected a level",
                "use a percentage from 0 to 100, `Full`, or `Palette <Feature> <n>`",
            )),
        }
    }

    /// `[<Attribute>] <level>` or `[<Attribute>] Palette <Feature> <n>`.
    fn level_source(&mut self) -> Result<(Option<String>, LevelSource), CommandError> {
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            return Ok((None, LevelSource::Palette { feature, number }));
        }
        let attribute = self.optional_attribute();
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            return Ok((attribute, LevelSource::Palette { feature, number }));
        }
        let level = self.level()?;
        Ok((attribute, LevelSource::Level(level)))
    }

    fn optional_attribute(&mut self) -> Option<String> {
        match self.peek() {
            Some(Token::Word(word))
                if !word.eq_ignore_ascii_case("full")
                    && !word.eq_ignore_ascii_case("palette")
                    && !word.eq_ignore_ascii_case("off") =>
            {
                let word = word.clone();
                self.position += 1;
                Some(word)
            }
            _ => None,
        }
    }

    /// `Report <kind> [<sequence>] <file>`. Only a cues report takes a target.
    fn report(&mut self) -> Result<Command, CommandError> {
        let name = self.name_token("report kind")?;
        let kind = match name.to_ascii_lowercase().as_str() {
            "patch" => ReportKind::Patch,
            "cues" | "cue" => ReportKind::Cues,
            "palettes" | "palette" => ReportKind::Palettes,
            "status" => ReportKind::Status,
            "show" => ReportKind::Show,
            _ => {
                return Err(CommandError::with_hint(
                    format!("unknown report kind `{name}`"),
                    "use `Report Patch`, `Cues`, `Palettes`, `Status` or `Show`",
                ))
            }
        };
        let sequence = if kind == ReportKind::Cues && matches!(self.peek(), Some(Token::Number(_)))
        {
            Some(self.whole_number("sequence number")?)
        } else {
            None
        };
        let path = self.path_token()?;
        self.expect_end()?;
        Ok(Command::Report {
            kind,
            sequence,
            path,
        })
    }

    /// `Clone Fixture <n> At <universe>.<address> [Count <k>] [Step <slots>]`.
    fn type_command(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("new") {
            let key = self.name_token("fixture type")?;
            self.expect_end()?;
            return Ok(Command::TypeNew { key });
        }
        let type_key = self.name_token("fixture type")?;
        if self.eat_word("mode") {
            let mode = self.name_token("mode")?;
            self.expect_end()?;
            return Ok(Command::TypeMode { type_key, mode });
        }
        if self.eat_word("channel") {
            let offset = self.whole_number("channel offset")?;
            let attribute = self.name_token("attribute")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::TypeChannel {
                type_key,
                offset,
                attribute,
                name,
            });
        }
        Err(CommandError::with_hint(
            format!("expected `Mode` or `Channel` after `{type_key}`"),
            "type `Help` for the type commands",
        ))
    }

    fn clone_fixture(&mut self) -> Result<Command, CommandError> {
        self.eat_word("fixture");
        let fixture = self.fixture_number()?;
        self.eat_word("at");
        let (universe, address) = self.patch_address()?;
        let mut count = None;
        let mut step = None;
        loop {
            if self.eat_word("count") {
                count = Some(self.whole_number("clone count")?);
            } else if self.eat_word("step") {
                step = Some(self.whole_number("clone step")?);
            } else {
                break;
            }
        }
        self.expect_end()?;
        Ok(Command::Clone {
            fixture,
            universe,
            address,
            count,
            step,
        })
    }

    fn dmx(&mut self) -> Result<Command, CommandError> {
        if self.peek().is_none() {
            return Ok(Command::ListDmx { universe: None });
        }
        if self.eat_word("in") {
            self.expect_end()?;
            return Ok(Command::ListDmx { universe: None });
        }
        let universe = self.whole_number("universe number")?;
        if self.peek().is_none() {
            return Ok(Command::ListDmx {
                universe: Some(universe),
            });
        }
        if self.eat_word("merge") {
            let mode = if self.eat_word("off") {
                MergeMode::Off
            } else if self.eat_word("htp") {
                MergeMode::Htp
            } else if self.eat_word("ltp") {
                MergeMode::Ltp
            } else {
                return Err(CommandError::with_hint(
                    "`Dmx <universe> Merge` needs `Off`, `Htp` or `Ltp`",
                    "for example `Dmx 1 Merge Htp`",
                ));
            };
            self.expect_end()?;
            return Ok(Command::SetDmxMerge { universe, mode });
        }
        if self.eat_word("priority") {
            let priority = if self.eat_word("local") {
                InputPriority::Local
            } else if self.eat_word("input") {
                InputPriority::Input
            } else {
                return Err(CommandError::with_hint(
                    "`Dmx <universe> Priority` needs `Local` or `Input`",
                    "for example `Dmx 1 Priority Input`",
                ));
            };
            self.expect_end()?;
            return Ok(Command::SetDmxPriority { universe, priority });
        }
        if self.eat_word("loss") {
            let loss = if self.eat_word("hold") {
                LossPolicy::Hold
            } else if self.eat_word("zero") {
                LossPolicy::Zero
            } else if self.eat_word("fade") {
                let seconds = self.number_value("fade seconds")?;
                if !seconds.is_finite() || !(0.0..=60.0).contains(&seconds) {
                    return Err(CommandError::with_hint(
                        format!("fade {seconds} is outside 0..60 seconds"),
                        "for example `Dmx 1 Loss Fade 2`",
                    ));
                }
                LossPolicy::Fade(seconds)
            } else {
                return Err(CommandError::with_hint(
                    "`Dmx <universe> Loss` needs `Hold`, `Zero` or `Fade <seconds>`",
                    "for example `Dmx 1 Loss Fade 2`",
                ));
            };
            self.expect_end()?;
            return Ok(Command::SetDmxLoss { universe, loss });
        }
        if self.eat_word("timeout") {
            let seconds = self.number_value("timeout seconds")?;
            if !seconds.is_finite() || !(0.001..=60.0).contains(&seconds) {
                return Err(CommandError::with_hint(
                    format!("timeout {seconds} is outside 0.001..60 seconds"),
                    "for example `Dmx 1 Timeout 1.5`",
                ));
            }
            let ms = (seconds * 1000.0).round() as u64;
            self.expect_end()?;
            return Ok(Command::SetDmxTimeout {
                universe,
                ms: ms.max(1),
            });
        }
        if self.eat_word("trigger") {
            if self.eat_word("off") {
                self.expect_end()?;
                return Ok(Command::ClearDmxTrigger { universe });
            }
            let address = self.whole_number("trigger address")?;
            if !(1..=512).contains(&address) {
                return Err(CommandError::with_hint(
                    format!("trigger address {address} is outside 1..512"),
                    "DMX addresses are 1-based",
                ));
            }
            self.eat_word("sequence");
            let sequence = self.whole_number("sequence number")?;
            let threshold = if self.eat_word("threshold") {
                let value = self.whole_number("trigger threshold")?;
                if !(1..=255).contains(&value) {
                    return Err(CommandError::with_hint(
                        format!("trigger threshold {value} is outside 1..255"),
                        "a trigger at zero would fire continuously",
                    ));
                }
                value as u8
            } else {
                128
            };
            self.expect_end()?;
            return Ok(Command::SetDmxTrigger {
                universe,
                trigger: InputTrigger {
                    address,
                    threshold,
                    sequence,
                },
            });
        }
        if self.eat_word("snapshot") {
            self.expect_end()?;
            return Ok(Command::DmxSnapshot { universe });
        }
        Err(CommandError::with_hint(
            format!("unknown DMX action for universe {universe}"),
            "use `Merge`, `Priority`, `Loss`, `Timeout`, `Trigger` or `Snapshot`",
        ))
    }

    fn patch(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("check") {
            self.expect_end()?;
            return Ok(Command::PatchCheck);
        }
        if self.eat_word("import") {
            let path = self.path_token()?;
            self.expect_end()?;
            return Ok(Command::ImportPatch { path });
        }
        self.eat_word("fixture");
        let fixture = self.fixture_number()?;
        self.eat_word("at");
        let (universe, address) = self.patch_address()?;

        let mut type_key = None;
        let mut mode = None;
        loop {
            if self.eat_word("type") {
                type_key = Some(self.name_token("fixture type")?);
            } else if self.eat_word("mode") {
                mode = Some(self.name_token("mode")?);
            } else if self.eat_word("at") {
                // `At` may be repeated by habit; the address is already read.
            } else {
                break;
            }
        }
        self.expect_end()?;
        Ok(Command::Patch {
            fixture,
            universe,
            address,
            type_key,
            mode,
        })
    }

    fn patch_address(&mut self) -> Result<(u16, u16), CommandError> {
        let Some(Token::Number(raw)) = self.bump() else {
            return Err(CommandError::with_hint(
                "expected a patch address",
                "use `universe.address`, e.g. `1.001`",
            ));
        };
        let Some((universe, address)) = raw.split_once('.') else {
            return Err(CommandError::with_hint(
                format!("patch address `{raw}` needs a universe and an address"),
                "use `universe.address`, e.g. `1.001`",
            ));
        };
        let universe: u16 = universe
            .parse()
            .map_err(|_| CommandError::new(format!("`{universe}` is not a universe number")))?;
        let address: u16 = address
            .parse()
            .map_err(|_| CommandError::new(format!("`{address}` is not a DMX address")))?;
        if address == 0 || address > 512 {
            return Err(CommandError::new(format!(
                "DMX address {address} is outside 1..=512"
            )));
        }
        Ok((universe, address))
    }

    fn group_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListGroups);
        }
        let number = self.whole_number("group number")?;
        if self.at_end() {
            return Ok(Command::SelectGroup(number));
        }
        if self.eat_word("at") {
            let (attribute, source) = self.level_source()?;
            self.expect_end()?;
            return Ok(Command::SetLevelGroup {
                group: number,
                attribute,
                source,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetLevelGroup {
                group: number,
                attribute: None,
                source: LevelSource::Level(Level::Full),
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::OffGroup { group: number });
        }
        if self.eat_word("home") {
            self.expect_end()?;
            return Ok(Command::Home {
                fixtures: None,
                group: Some(number),
            });
        }
        if self.eat_word("color") {
            return self.color_action(None, Some(number));
        }
        if self.eat_word("default") {
            return self.default_action(None, Some(number));
        }
        if matches!(self.peek(), Some(Token::Word(_))) {
            return self.channel_set_action(None, Some(number));
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "after a group use `At <level>`, `Full`, `Off`, `Home`, `Color`, `Default` or `Gobo 2`",
        ))
    }

    fn world_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListWorlds);
        }
        if self.eat_word("all") || self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::ActivateWorld { number: None });
        }
        let number = self.whole_number("world number")?;
        self.expect_end()?;
        Ok(Command::ActivateWorld {
            number: Some(number),
        })
    }

    fn filter_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListFilters);
        }
        if self.eat_word("all") || self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::ActivateFilter { number: None });
        }
        let number = self.whole_number("filter number")?;
        self.expect_end()?;
        Ok(Command::ActivateFilter {
            number: Some(number),
        })
    }

    fn master_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListMasters);
        }
        if self.eat_word("grand") {
            let level = self.master_level()?;
            return Ok(Command::SetGrandMaster { level });
        }
        if self.eat_word("rate") {
            let level = self.master_level()?;
            return Ok(Command::SetRateMaster { level });
        }
        if self.eat_word("group") {
            let group = self.whole_number("group number")?;
            let level = self.master_level()?;
            return Ok(Command::SetGroupMaster { group, level });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Master Grand`, `Master Rate` or `Master Group <n>`",
        ))
    }

    fn master_level(&mut self) -> Result<Level, CommandError> {
        if self.eat_word("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(level);
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Level::Full);
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Level::Percent(0.0));
        }
        Err(CommandError::with_hint(
            "expected a master level",
            "use `At <level>`, `Full` or `Off`",
        ))
    }

    fn effect_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListEffects { only: None });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::EffectOff { effect: None });
        }
        let number = self.whole_number("effect number")?;
        if self.at_end() {
            return Ok(Command::ListEffects { only: Some(number) });
        }
        if self.eat_word("on") {
            let fixtures = self.optional_fixture_list()?;
            self.expect_end()?;
            return Ok(Command::EffectOn {
                effect: number,
                fixtures,
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::EffectOff {
                effect: Some(number),
            });
        }
        if self.eat_word("attributes") {
            let attributes = self.effect_attribute_list()?;
            self.expect_end()?;
            return Ok(Command::SetEffectAttributes {
                effect: number,
                attributes,
            });
        }
        if self.eat_word("attribute") {
            let attribute = self.name_token("attribute")?;
            let mut size = None;
            let mut phase = None;
            loop {
                if self.eat_word("size") {
                    size = Some(self.number_value("size")?);
                } else if self.eat_word("phase") {
                    phase = Some(self.phase_value()?);
                } else {
                    break;
                }
            }
            if size.is_none() && phase.is_none() {
                return Err(CommandError::with_hint(
                    format!("`Effect {number} Attribute {attribute}` changes nothing"),
                    "add `Size <percent>` or `Phase <degrees>`",
                ));
            }
            self.expect_end()?;
            return Ok(Command::SetEffectTarget {
                effect: number,
                attribute,
                size,
                phase,
            });
        }
        if self.eat_word("rate") {
            let rate = self.effect_rate()?;
            self.expect_end()?;
            return Ok(Command::SetEffectRate {
                effect: number,
                rate,
            });
        }
        if self.eat_word("tempo") {
            if self.eat_word("off") {
                self.expect_end()?;
                return Ok(Command::SetEffectFreeRate { effect: number });
            }
            return Err(CommandError::with_hint(
                "an effect's tempo can only be turned off",
                "use `Effect 1 Rate 1/4` to make it musical",
            ));
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Effect <n> On`, `Effect <n> Off` or `Effect <n>`",
        ))
    }

    fn timecode_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListTimecodes { only: None });
        }
        let number = self.whole_number("timecode number")?;
        if self.at_end() {
            return Ok(Command::ListTimecodes { only: Some(number) });
        }
        let mut track = None;
        if self.eat_word("track") {
            let value = self.whole_number("track number")?;
            track = Some(
                u8::try_from(value)
                    .map_err(|_| CommandError::new(format!("track {value} is outside 1..=255")))?,
            );
        }
        if self.eat_word("add") {
            let at = self.timecode_token()?;
            let action = self.timecode_action()?;
            self.expect_end()?;
            return Ok(Command::AddTimecodeEvent {
                timecode: number,
                track,
                at,
                action,
            });
        }
        if self.eat_word("remove") {
            let at = self.timecode_token()?;
            self.expect_end()?;
            return Ok(Command::RemoveTimecodeEvent {
                timecode: number,
                track,
                at,
            });
        }
        if self.eat_word("marker") {
            let at = self.timecode_token()?;
            let name = self.name_token("marker name")?;
            if name.trim().is_empty() {
                return Err(CommandError::new("marker name is empty"));
            }
            self.expect_end()?;
            return Ok(Command::TimecodeMarker {
                timecode: number,
                at,
                name,
            });
        }
        if let Some(track) = track {
            let name = if self.at_end() {
                None
            } else {
                Some(self.name_token("track name")?)
            };
            self.expect_end()?;
            return Ok(Command::TimecodeTrack {
                timecode: number,
                track,
                name,
            });
        }
        if self.eat_word("start") {
            self.expect_end()?;
            return Ok(Command::StartTimecode { timecode: number });
        }
        if self.eat_word("pause") {
            self.expect_end()?;
            return Ok(Command::PauseTimecode { timecode: number });
        }
        if self.eat_word("stop") {
            self.expect_end()?;
            return Ok(Command::StopTimecode { timecode: number });
        }
        if self.eat_word("at") {
            let at = self.timecode_token()?;
            self.expect_end()?;
            return Ok(Command::LocateTimecode {
                timecode: number,
                at,
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Timecode 1 Add|Remove <timecode>`, `Track <n>`, `Marker`, `Start`, `Pause`, `Stop` or `At <timecode>`",
        ))
    }

    fn timecode_action(&mut self) -> Result<TimecodeAction, CommandError> {
        if self.eat_word("sequence") {
            let sequence = self.whole_number("sequence number")?;
            self.eat_word("cue");
            let cue = self.cue_number()?;
            return Ok(TimecodeAction::Cue { sequence, cue });
        }
        if self.eat_word("effect") {
            let effect = self.whole_number("effect number")?;
            let mut fixtures = None;
            let mut group = None;
            let on = if self.eat_word("off") {
                false
            } else {
                self.eat_word("on");
                if self.eat_word("group") {
                    group = Some(self.whole_number("group number")?);
                } else {
                    fixtures = self.optional_fixture_list()?;
                }
                true
            };
            return Ok(TimecodeAction::Effect {
                effect,
                fixtures,
                group,
                on,
            });
        }
        Err(CommandError::with_hint(
            "a timecode event needs an action",
            "use `Sequence <n> Cue <n>` or `Effect <n> On|Off`",
        ))
    }

    fn timecode_source(&mut self) -> Result<TimecodeSource, CommandError> {
        if self.eat_word("internal") {
            return Ok(TimecodeSource::Internal);
        }
        if self.eat_word("mtc") {
            let port = self.path_token()?;
            if port.trim().is_empty() {
                return Err(CommandError::new("the MTC port is empty"));
            }
            return Ok(TimecodeSource::Mtc { port });
        }
        if self.eat_word("osc") {
            return Ok(TimecodeSource::Osc);
        }
        Err(CommandError::with_hint(
            format!(
                "`{}` is not a timecode source",
                self.peek()
                    .cloned()
                    .map(|token| token.to_string())
                    .unwrap_or_else(|| "end of command".into())
            ),
            "use `Internal`, `Mtc \"<port>\"` or `Osc`",
        ))
    }

    fn timecode_token(&mut self) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Number(text)) | Some(Token::Word(text)) => Ok(text),
            Some(other) => Err(CommandError::with_hint(
                format!("expected a timecode, found {other}"),
                "write positions as `HH:MM:SS:FF`, e.g. `00:01:30:00`",
            )),
            None => Err(CommandError::with_hint(
                "expected a timecode",
                "write positions as `HH:MM:SS:FF`, e.g. `00:01:30:00`",
            )),
        }
    }

    fn store(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("page") {
            let number = self.whole_number("page number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StorePage { number, name });
        }
        if self.eat_word("timecode") {
            let number = self.whole_number("timecode number")?;
            let name = self.optional_name()?;
            let mut fps = None;
            let mut source = None;
            loop {
                if self.eat_word("fps") {
                    let value = self.whole_number("frame rate")?;
                    fps = Some(u8::try_from(value).map_err(|_| {
                        CommandError::new(format!("frame rate {value} is outside 1..=60"))
                    })?);
                } else if self.eat_word("source") {
                    source = Some(self.timecode_source()?);
                } else {
                    break;
                }
            }
            self.expect_end()?;
            return Ok(Command::StoreTimecode {
                number,
                name,
                fps,
                source,
            });
        }
        if self.eat_word("group") {
            let number = self.whole_number("group number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StoreGroup { number, name });
        }
        if self.eat_word("world") {
            let number = self.whole_number("world number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StoreWorld { number, name });
        }
        if self.eat_word("filter") {
            let number = self.whole_number("filter number")?;
            // A filter name must be quoted so bare pool names are unambiguous.
            let name = self.quoted_name();
            let mut features = Vec::new();
            while let Some(Token::Word(word)) = self.peek().cloned() {
                self.position += 1;
                features.push(word);
            }
            self.expect_end()?;
            if features.is_empty() {
                return Err(CommandError::with_hint(
                    format!("filter {number} needs at least one feature"),
                    "name the pools it allows, e.g. `Store Filter 1 Dimmer Color`",
                ));
            }
            return Ok(Command::StoreFilter {
                number,
                name,
                features,
            });
        }
        if self.eat_word("effect") {
            let number = self.whole_number("effect number")?;
            let name = self.optional_name()?;
            let mut targets = None;
            let mut waveform = None;
            let mut rate = None;
            let mut size = None;
            let mut phases = Vec::new();
            let mut spread = None;
            loop {
                if self.eat_word("attributes") {
                    targets = Some(self.effect_attribute_list()?);
                } else if self.eat_word("attribute") {
                    targets = Some(vec![self.name_token("attribute")?]);
                } else if self.eat_word("waveform") {
                    waveform = Some(self.name_token("waveform")?);
                } else if self.eat_word("rate") {
                    rate = Some(self.effect_rate()?);
                } else if self.eat_word("size") {
                    size = Some(self.number_value("size")?);
                } else if self.eat_word("phase") {
                    phases.extend(self.effect_phase_pairs()?);
                } else if self.eat_word("spread") {
                    spread = Some(self.number_value("spread")?);
                } else {
                    break;
                }
            }
            self.expect_end()?;
            return Ok(Command::StoreEffect {
                number,
                name,
                targets,
                waveform,
                rate,
                size,
                phases,
                spread,
            });
        }
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            let name = self.optional_name()?;
            self.expect_end()?;
            return Ok(Command::StorePalette {
                feature,
                number,
                name,
            });
        }
        if self.eat_word("cue") {
            let number = self.cue_number()?;
            let mut name = self.optional_name()?;
            let mut fade = None;
            let mut delay = None;
            let mut mib = None;
            let mut trigger = None;
            let mut sequence = 1;
            let mut mode = StoreMode::Merge;
            loop {
                if self.eat_word("fade") {
                    fade = Some(self.seconds("fade")?);
                } else if self.eat_word("trigger") {
                    trigger = Some(self.trigger()?);
                } else if self.eat_word("follow") {
                    trigger = Some(TriggerArg {
                        trigger: Trigger::Follow,
                        time: self.seconds("follow")?,
                    });
                } else if self.eat_word("time") {
                    trigger = Some(TriggerArg {
                        trigger: Trigger::Time,
                        time: self.seconds("time")?,
                    });
                } else if self.eat_word("delay") {
                    delay = Some(self.seconds("delay")?);
                } else if self.eat_word("mib") {
                    if self.eat_word("off") {
                        mib = Some(false);
                    } else {
                        self.eat_word("on");
                        mib = Some(true);
                    }
                } else if self.eat_word("merge") {
                    mode = StoreMode::Merge;
                } else if self.eat_word("replace") {
                    mode = StoreMode::Replace;
                } else if self.eat_word("sequence") {
                    sequence = self.whole_number("sequence number")?;
                } else if self.eat_word("in") {
                    self.eat_word("sequence");
                    sequence = self.whole_number("sequence number")?;
                } else if let Some(candidate) = self.optional_name()? {
                    if name.is_some() {
                        return Err(CommandError::with_hint(
                            format!("unexpected name `{candidate}`"),
                            "quote the cue name once",
                        ));
                    }
                    name = Some(candidate);
                } else {
                    break;
                }
            }
            self.expect_end()?;
            return Ok(Command::StoreCue {
                sequence,
                number,
                name,
                fade,
                delay,
                mib,
                trigger,
                mode,
            });
        }
        Err(CommandError::with_hint(
            "`Store` needs an object",
            "`Store Palette <Feature> <n>` or `Store Cue <n>`",
        ))
    }

    fn delete(&mut self) -> Result<Command, CommandError> {
        if self.eat_word("page") {
            let number = self.whole_number("page number")?;
            self.expect_end()?;
            return Ok(Command::DeletePage { number });
        }
        if self.eat_word("group") {
            let number = self.whole_number("group number")?;
            self.expect_end()?;
            return Ok(Command::DeleteGroup { number });
        }
        if self.eat_word("world") {
            let number = self.whole_number("world number")?;
            self.expect_end()?;
            return Ok(Command::DeleteWorld { number });
        }
        if self.eat_word("filter") {
            let number = self.whole_number("filter number")?;
            self.expect_end()?;
            return Ok(Command::DeleteFilter { number });
        }
        if self.eat_word("effect") {
            let number = self.whole_number("effect number")?;
            self.expect_end()?;
            return Ok(Command::DeleteEffect { number });
        }
        if self.eat_word("timecode") {
            let number = self.whole_number("timecode number")?;
            self.expect_end()?;
            return Ok(Command::DeleteTimecode { number });
        }
        if self.eat_word("palette") {
            let feature = self.name_token("palette feature")?;
            let number = self.whole_number("palette number")?;
            self.expect_end()?;
            return Ok(Command::DeletePalette { feature, number });
        }
        if self.eat_word("cue") {
            let number = self.cue_number()?;
            let mut sequence = 1;
            if self.eat_word("sequence") {
                sequence = self.whole_number("sequence number")?;
            } else if self.eat_word("in") {
                self.eat_word("sequence");
                sequence = self.whole_number("sequence number")?;
            }
            self.expect_end()?;
            return Ok(Command::DeleteCue { sequence, number });
        }
        Err(CommandError::with_hint(
            "`Delete` needs an object",
            "`Delete Palette <Feature> <n>` or `Delete Cue <n>`",
        ))
    }

    fn assign(&mut self) -> Result<Command, CommandError> {
        let page = if self.eat_word("page") {
            Some(self.whole_number("page number")?)
        } else {
            None
        };
        if !self.eat_word("executor") {
            return Err(CommandError::with_hint(
                "`Assign` needs an executor",
                "`Assign Executor 1 Sequence 1` or `Assign Page 2 Executor 1 Sequence 1`",
            ));
        }
        let executor = self.whole_number("executor number")?;
        self.eat_word("to");
        if !self.eat_word("sequence") {
            return Err(CommandError::with_hint(
                "`Assign` needs an object",
                "`Assign Executor 1 Sequence 1`",
            ));
        }
        let sequence = self.whole_number("sequence number")?;
        self.expect_end()?;
        Ok(Command::AssignExecutor {
            page,
            executor,
            sequence,
        })
    }

    fn page_command(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListPages);
        }
        if self.eat_word("next") {
            self.expect_end()?;
            return Ok(Command::StepPage { next: true });
        }
        if self.eat_word("prev") || self.eat_word("previous") {
            self.expect_end()?;
            return Ok(Command::StepPage { next: false });
        }
        let number = self.whole_number("page number")?;
        self.expect_end()?;
        Ok(Command::SetPage { number })
    }

    fn executor(&mut self) -> Result<Command, CommandError> {
        if self.at_end() {
            return Ok(Command::ListExecutors { only: None });
        }
        let number = self.whole_number("executor number")?;
        if self.at_end() {
            return Ok(Command::ListExecutors { only: Some(number) });
        }
        if self.eat_word("at") {
            let level = self.level()?;
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level,
            });
        }
        if self.eat_word("full") {
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level: Level::Full,
            });
        }
        if self.eat_word("off") {
            self.expect_end()?;
            return Ok(Command::SetExecutorLevel {
                executor: number,
                level: Level::Percent(0.0),
            });
        }
        if self.eat_word("mode") {
            let word = self.name_token("executor mode")?;
            let mode = [
                ExecutorMode::Go,
                ExecutorMode::Fader,
                ExecutorMode::Flash,
                ExecutorMode::Solo,
            ]
            .into_iter()
            .find(|mode| mode.label().eq_ignore_ascii_case(&word))
            .ok_or_else(|| {
                CommandError::with_hint(
                    format!("`{word}` is not an executor mode"),
                    "use `Go`, `Fader`, `Flash` or `Solo`",
                )
            })?;
            self.expect_end()?;
            return Ok(Command::SetExecutorMode {
                executor: number,
                mode,
            });
        }
        if self.eat_word("flash") {
            let on = !self.eat_word("off");
            self.eat_word("on");
            self.expect_end()?;
            return Ok(Command::SetExecutorFlash {
                executor: number,
                on,
            });
        }
        if self.eat_word("solo") {
            let on = !self.eat_word("off");
            self.eat_word("on");
            self.expect_end()?;
            return Ok(Command::SetExecutorSolo {
                executor: number,
                on,
            });
        }
        Err(CommandError::with_hint(
            format!(
                "unexpected {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Executor <n> At <level>`, `Mode <mode>`, `Flash`, `Solo` or `Executor <n>`",
        ))
    }

    /// `Cue 2 Trigger Follow 1.5`, `Cue 2 Timing Pan Fade 3`, `Cue 2 Stagger
    /// 0.2`: these are show data, so each is an edit with its own undo step
    /// rather than a re-store that captures values.
    fn cue_command(&mut self) -> Result<Command, CommandError> {
        let number = self.cue_number()?;
        if self.eat_word("trigger") {
            let trigger = self.trigger()?;
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueTrigger {
                sequence,
                number,
                trigger,
            });
        }
        if self.eat_word("stagger") {
            let stagger = self.seconds("stagger")?;
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueStagger {
                sequence,
                number,
                stagger,
            });
        }
        if self.eat_word("block") {
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueBlock {
                sequence,
                number,
                on: true,
            });
        }
        if self.eat_word("unblock") {
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueBlock {
                sequence,
                number,
                on: false,
            });
        }
        if self.eat_word("show") {
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueShow { sequence, number });
        }
        if self.eat_word("move") {
            let to = self.cue_number()?;
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueMove {
                sequence,
                number,
                to,
            });
        }
        if self.eat_word("copy") {
            let to = self.cue_number()?;
            let target = if self.eat_word("sequence") {
                Some(self.whole_number("sequence number")?)
            } else if self.eat_word("in") {
                self.eat_word("sequence");
                Some(self.whole_number("sequence number")?)
            } else {
                None
            };
            self.expect_end()?;
            return Ok(Command::CueCopy {
                sequence: 1,
                number,
                to,
                target,
            });
        }
        if self.eat_word("shield") {
            let attribute = self.optional_attribute_name();
            let off = self.eat_word("off");
            if off && attribute.is_none() {
                return Err(CommandError::with_hint(
                    "`Shield Off` needs an attribute",
                    "use `Cue 2 Shield Pan Off`, or `Cue 2 Shield` to list",
                ));
            }
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueShield {
                sequence,
                number,
                attribute,
                off,
            });
        }
        if self.eat_word("timing") {
            let attribute = self.optional_attribute_name();
            let mut fade = None;
            let mut delay = None;
            let mut curve = None;
            let mut off = false;
            loop {
                if self.eat_word("fade") {
                    fade = Some(self.seconds("fade")?);
                } else if self.eat_word("delay") {
                    delay = Some(self.seconds("delay")?);
                } else if self.eat_word("curve") {
                    curve = Some(self.curve()?);
                } else if self.eat_word("off") {
                    off = true;
                } else {
                    break;
                }
            }
            let has_clause = fade.is_some() || delay.is_some() || curve.is_some() || off;
            if attribute.is_none() && has_clause {
                return Err(CommandError::with_hint(
                    "`Timing` needs an attribute for a change",
                    "use `Cue 2 Timing ColorAdd_R Fade 0.5`, or `Cue 2 Timing` to list",
                ));
            }
            if off && (fade.is_some() || delay.is_some() || curve.is_some()) {
                return Err(CommandError::with_hint(
                    "`Off` drops the whole override",
                    "use `Cue 2 Timing ColorAdd_R Off` on its own",
                ));
            }
            let sequence = self.sequence_suffix()?;
            self.expect_end()?;
            return Ok(Command::CueTiming {
                sequence,
                number,
                attribute,
                fade,
                delay,
                curve,
                off,
            });
        }
        Err(CommandError::with_hint(
            format!("cue {number} needs an action"),
            "use `Cue 2 Show`, `Cue 2 Trigger Follow 1.5`, `Cue 2 Timing ColorAdd_R Fade 0.5`, or `Sequence 1` to list the cues",
        ))
    }

    /// `Update [Cue <n>] [Sequence <n>]`: merge the programmer into a named cue,
    /// or into the cue that sequence is running.
    fn update(&mut self) -> Result<Command, CommandError> {
        let cue = if self.eat_word("cue") {
            Some(self.cue_number()?)
        } else {
            None
        };
        let mut sequence = 1;
        if self.eat_word("sequence") {
            sequence = self.whole_number("sequence number")?;
        } else if self.eat_word("in") {
            self.eat_word("sequence");
            sequence = self.whole_number("sequence number")?;
        }
        self.expect_end()?;
        Ok(Command::Update { sequence, cue })
    }

    fn sequence_suffix(&mut self) -> Result<u16, CommandError> {
        if self.eat_word("sequence") {
            return self.whole_number("sequence number");
        }
        if self.eat_word("in") {
            self.eat_word("sequence");
            return self.whole_number("sequence number");
        }
        Ok(1)
    }

    /// The attribute a timing clause is about. Clause keywords are not names,
    /// so `Cue 2 Timing` and `Cue 2 Timing Sequence 2` stay unambiguous.
    fn optional_attribute_name(&mut self) -> Option<String> {
        let clause = ["fade", "delay", "curve", "off", "sequence", "in"];
        match self.peek().cloned() {
            Some(Token::Quoted(name)) => {
                self.position += 1;
                Some(name)
            }
            Some(Token::Word(word))
                if !clause
                    .iter()
                    .any(|keyword| word.eq_ignore_ascii_case(keyword)) =>
            {
                self.position += 1;
                Some(word)
            }
            _ => None,
        }
    }

    /// `Linear`, `EaseIn`, `EaseOut`, `EaseInOut` or `Snap`.
    fn curve(&mut self) -> Result<Curve, CommandError> {
        let word = self.name_token("curve")?;
        [
            Curve::Linear,
            Curve::EaseIn,
            Curve::EaseOut,
            Curve::EaseInOut,
            Curve::Snap,
        ]
        .into_iter()
        .find(|curve| curve.label().eq_ignore_ascii_case(&word))
        .ok_or_else(|| {
            CommandError::with_hint(
                format!("`{word}` is not a curve"),
                "use `Linear`, `EaseIn`, `EaseOut`, `EaseInOut` or `Snap`",
            )
        })
    }

    /// `Go`, `Follow <seconds>` or `Time <seconds>`.
    fn trigger(&mut self) -> Result<TriggerArg, CommandError> {
        if self.eat_word("go") {
            return Ok(TriggerArg {
                trigger: Trigger::Go,
                time: 0.0,
            });
        }
        if self.eat_word("follow") {
            return Ok(TriggerArg {
                trigger: Trigger::Follow,
                time: self.seconds("follow")?,
            });
        }
        if self.eat_word("time") {
            return Ok(TriggerArg {
                trigger: Trigger::Time,
                time: self.seconds("time")?,
            });
        }
        Err(CommandError::with_hint(
            format!(
                "expected a trigger, got {}",
                self.peek().cloned().unwrap_or(Token::Word(String::new()))
            ),
            "use `Go`, `Follow <seconds>` or `Time <seconds>`",
        ))
    }

    fn playback_ref(&mut self) -> Result<Option<PlaybackRef>, CommandError> {
        if self.eat_word("executor") {
            return Ok(Some(PlaybackRef::Executor(
                self.whole_number("executor number")?,
            )));
        }
        if self.eat_word("sequence") {
            return Ok(Some(PlaybackRef::Sequence(
                self.whole_number("sequence number")?,
            )));
        }
        Ok(None)
    }

    fn optional_name(&mut self) -> Result<Option<String>, CommandError> {
        match self.peek().cloned() {
            Some(Token::Quoted(name)) => {
                self.position += 1;
                Ok(Some(name))
            }
            Some(Token::Word(word))
                if !RESERVED
                    .iter()
                    .any(|keyword| word.eq_ignore_ascii_case(keyword)) =>
            {
                self.position += 1;
                Ok(Some(word))
            }
            _ => Ok(None),
        }
    }

    /// A quoted object name. Filters use this so bare words remain features.
    fn quoted_name(&mut self) -> Option<String> {
        match self.peek().cloned() {
            Some(Token::Quoted(name)) => {
                self.position += 1;
                Some(name)
            }
            _ => None,
        }
    }

    /// Whether the next word starts another effect option, so an attribute
    /// list knows where to stop.
    fn at_effect_option(&self) -> bool {
        matches!(self.peek(), Some(Token::Word(word))
            if ["attribute", "attributes", "waveform", "rate", "size", "phase", "spread", "tempo", "on", "off"]
                .iter()
                .any(|keyword| word.eq_ignore_ascii_case(keyword)))
    }

    fn effect_attribute_list(&mut self) -> Result<Vec<String>, CommandError> {
        let mut attributes = Vec::new();
        while let Some(Token::Word(word)) = self.peek().cloned() {
            if self.at_effect_option() {
                break;
            }
            self.position += 1;
            attributes.push(word);
        }
        if attributes.is_empty() {
            return Err(CommandError::with_hint(
                "`Attributes` needs at least one attribute",
                "list them by name, e.g. `Attributes Pan Tilt`",
            ));
        }
        Ok(attributes)
    }

    /// `Phase Pan 0 Tilt 90`, repeated until the next effect option.
    fn effect_phase_pairs(&mut self) -> Result<Vec<(String, f64)>, CommandError> {
        let mut phases = Vec::new();
        while !self.at_effect_option() {
            let Some(Token::Word(attribute)) = self.peek().cloned() else {
                break;
            };
            if !matches!(self.tokens.get(self.position + 1), Some(Token::Number(_))) {
                break;
            }
            self.position += 1;
            phases.push((attribute, self.phase_value()?));
        }
        if phases.is_empty() {
            return Err(CommandError::with_hint(
                "`Phase` needs an attribute and degrees",
                "use `Phase Pan 0 Tilt 90`",
            ));
        }
        Ok(phases)
    }

    /// `Rate 2` / `Rate 2hz` is free, `Rate 1/4` is musical.
    fn effect_rate(&mut self) -> Result<EffectRate, CommandError> {
        let Some(token) = self.bump() else {
            return Err(CommandError::new("expected a rate"));
        };
        let raw = match token {
            Token::Number(raw) | Token::Word(raw) => raw,
            other => return Err(CommandError::new(format!("expected a rate, got {other}"))),
        };
        if raw.contains('/') {
            return Ok(EffectRate::Division(raw));
        }
        let lower = raw.to_ascii_lowercase();
        let number = lower.strip_suffix("hz").unwrap_or(&lower);
        let value: f64 = number
            .parse()
            .map_err(|_| CommandError::new(format!("`{raw}` is not a rate")))?;
        if !value.is_finite() || value < 0.0 {
            return Err(CommandError::new(format!(
                "rate {raw} is not a positive number"
            )));
        }
        Ok(EffectRate::Hz(value))
    }

    fn phase_value(&mut self) -> Result<f64, CommandError> {
        let value = self.number_value("phase")?;
        if !value.is_finite() || !(0.0..=crate::effect::MAX_SPREAD_DEGREES).contains(&value) {
            return Err(CommandError::new(format!(
                "phase {value} is outside 0..={} degrees",
                crate::effect::MAX_SPREAD_DEGREES
            )));
        }
        Ok(value)
    }

    fn number_value(&mut self, what: &str) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) | Some(Token::Word(raw)) => {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what}")))?;
                if !value.is_finite() || value < 0.0 {
                    return Err(CommandError::new(format!(
                        "{what} {raw} is not a positive number"
                    )));
                }
                Ok(value)
            }
            Some(other) => Err(CommandError::new(format!("expected a {what}, got {other}"))),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }

    fn seconds(&mut self, what: &str) -> Result<f64, CommandError> {
        match self.bump() {
            Some(Token::Number(raw)) | Some(Token::Word(raw)) => {
                let value: f64 = raw
                    .parse()
                    .map_err(|_| CommandError::new(format!("`{raw}` is not a {what} time")))?;
                if !value.is_finite() || value < 0.0 {
                    return Err(CommandError::new(format!(
                        "{what} {raw} is not a positive number of seconds"
                    )));
                }
                Ok(value)
            }
            Some(other) => Err(CommandError::new(format!(
                "expected a {what} time in seconds, got {other}"
            ))),
            None => Err(CommandError::new(format!(
                "expected a {what} time in seconds"
            ))),
        }
    }

    /// A lock code: one token, quoted or bare. `Lock a b` is rejected because
    /// `expect_end` sees the leftover word.
    fn optional_code(&mut self) -> Result<Option<String>, CommandError> {
        match self.peek().cloned() {
            Some(Token::Quoted(code)) | Some(Token::Word(code)) | Some(Token::Number(code)) => {
                self.position += 1;
                Ok(Some(code))
            }
            _ => Ok(None),
        }
    }

    fn path_token(&mut self) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Quoted(path)) | Some(Token::Word(path)) | Some(Token::Number(path)) => {
                Ok(path)
            }
            Some(other) => Err(CommandError::with_hint(
                format!("expected a path, got {other}"),
                "quote paths that contain spaces",
            )),
            None => Err(CommandError::new("expected a path")),
        }
    }

    fn name_token(&mut self, what: &str) -> Result<String, CommandError> {
        match self.bump() {
            Some(Token::Quoted(name)) | Some(Token::Word(name)) => Ok(name),
            Some(other) => Err(CommandError::with_hint(
                format!("expected a {what}, got {other}"),
                "quote names that contain spaces",
            )),
            None => Err(CommandError::new(format!("expected a {what}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(line: &str) -> Command {
        parse(&lex(line).unwrap()).unwrap()
    }

    fn error(line: &str) -> CommandError {
        match lex(line).and_then(|tokens| parse(&tokens)) {
            Ok(command) => panic!("`{line}` parsed as {command:?}"),
            Err(error) => error,
        }
    }

    #[test]
    fn lexer_separates_numbers_words_quotes_and_operators() {
        let tokens =
            lex(r#"Patch Fixture 1 At 1.001 Type "OpenLight Demo Wash" + 4ch 50%"#).unwrap();
        assert_eq!(
            tokens,
            vec![
                Token::Word("Patch".into()),
                Token::Word("Fixture".into()),
                Token::Number("1".into()),
                Token::Word("At".into()),
                Token::Number("1.001".into()),
                Token::Word("Type".into()),
                Token::Quoted("OpenLight Demo Wash".into()),
                Token::Plus,
                Token::Word("4ch".into()),
                Token::Number("50".into()),
                Token::Word("%".into()),
            ]
        );
    }

    #[test]
    fn lexer_reports_unterminated_quotes() {
        assert!(lex("\"unfinished").is_err());
    }

    #[test]
    fn statements_split_on_semicolons_outside_quotes() {
        assert_eq!(
            split_statements("Fixture 1 At 50; Save \"a;b.json\""),
            vec!["Fixture 1 At 50", "Save \"a;b.json\""]
        );
        assert!(split_statements("  ;; ").is_empty());
    }

    #[test]
    fn parses_selection_and_levels() {
        assert_eq!(
            command("Fixture 1 Thru 5"),
            Command::Select(vec![1, 2, 3, 4, 5])
        );
        assert_eq!(
            command("Fixture 1 Thru 5 At 50"),
            Command::SetLevel {
                fixtures: Some(vec![1, 2, 3, 4, 5]),
                attribute: None,
                source: LevelSource::Level(Level::Percent(50.0))
            }
        );
        assert_eq!(
            command("1 + 3 At Full"),
            Command::SetLevel {
                fixtures: Some(vec![1, 3]),
                attribute: None,
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("Full"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("At 0"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Percent(0.0))
            }
        );
        assert_eq!(command("Off"), Command::Off { fixtures: None });
        assert_eq!(
            command("Off Fixture 3"),
            Command::Off {
                fixtures: Some(vec![3])
            }
        );
    }

    #[test]
    fn ranges_are_order_independent_and_deduplicated() {
        assert_eq!(command("5 Thru 1"), Command::Select(vec![1, 2, 3, 4, 5]));
        assert_eq!(command("2 + 2"), Command::Select(vec![2]));
    }

    #[test]
    fn parses_patch_with_type_and_mode() {
        assert_eq!(
            command(r#"Patch Fixture 7 At 2.101 Type "OpenLight Demo Wash" Mode "4ch""#),
            Command::Patch {
                fixture: 7,
                universe: 2,
                address: 101,
                type_key: Some("OpenLight Demo Wash".into()),
                mode: Some("4ch".into()),
            }
        );
    }

    #[test]
    fn parses_attributes_and_palettes() {
        assert_eq!(
            command("Fixture 1 At ColorAdd_R 40"),
            Command::SetLevel {
                fixtures: Some(vec![1]),
                attribute: Some("ColorAdd_R".into()),
                source: LevelSource::Level(Level::Percent(40.0))
            }
        );
        assert_eq!(
            command("At Dimmer Full"),
            Command::SetLevel {
                fixtures: None,
                attribute: Some("Dimmer".into()),
                source: LevelSource::Level(Level::Full)
            }
        );
        assert_eq!(
            command("Fixture 1 Thru 2 At Palette Color 3"),
            Command::SetLevel {
                fixtures: Some(vec![1, 2]),
                attribute: None,
                source: LevelSource::Palette {
                    feature: "Color".into(),
                    number: 3
                }
            }
        );
        assert_eq!(
            command("At Red 25"),
            Command::SetLevel {
                fixtures: None,
                attribute: Some("Red".into()),
                source: LevelSource::Level(Level::Percent(25.0))
            }
        );
    }

    #[test]
    fn parses_store_and_delete_objects() {
        assert_eq!(
            command(r#"Store Palette Color 2 "Deep Blue""#),
            Command::StorePalette {
                feature: "Color".into(),
                number: 2,
                name: Some("Deep Blue".into()),
            }
        );
        assert_eq!(
            command("Store Cue 1.5 Fade 2 Delay 0.5 Sequence 3"),
            Command::StoreCue {
                sequence: 3,
                number: 1.5,
                name: None,
                fade: Some(2.0),
                delay: Some(0.5),
                mib: None,
                trigger: None,
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command(r#"Store Cue 2 "Look" In Sequence 2"#),
            Command::StoreCue {
                sequence: 2,
                number: 2.0,
                name: Some("Look".into()),
                fade: None,
                delay: None,
                mib: None,
                trigger: None,
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command("Store Cue 3 Mib Fade 1"),
            Command::StoreCue {
                sequence: 1,
                number: 3.0,
                name: None,
                fade: Some(1.0),
                delay: None,
                mib: Some(true),
                trigger: None,
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command("Store Cue 3 Mib Off"),
            Command::StoreCue {
                sequence: 1,
                number: 3.0,
                name: None,
                fade: None,
                delay: None,
                mib: Some(false),
                trigger: None,
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command("Store Cue 2 Follow 1.5 Fade 3"),
            Command::StoreCue {
                sequence: 1,
                number: 2.0,
                name: None,
                fade: Some(3.0),
                delay: None,
                mib: None,
                trigger: Some(TriggerArg {
                    trigger: Trigger::Follow,
                    time: 1.5,
                }),
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command("Cue 3 Trigger Time 2 Sequence 2"),
            Command::CueTrigger {
                sequence: 2,
                number: 3.0,
                trigger: TriggerArg {
                    trigger: Trigger::Time,
                    time: 2.0,
                },
            }
        );
        assert_eq!(
            command("Cue 2 Timing ColorAdd_R Fade 0.5 Delay 1 Curve EaseOut Sequence 3"),
            Command::CueTiming {
                sequence: 3,
                number: 2.0,
                attribute: Some("ColorAdd_R".into()),
                fade: Some(0.5),
                delay: Some(1.0),
                curve: Some(Curve::EaseOut),
                off: false,
            }
        );
        assert_eq!(
            command("Cue 2 Stagger 0.2"),
            Command::CueStagger {
                sequence: 1,
                number: 2.0,
                stagger: 0.2,
            }
        );
        assert_eq!(
            command("Cue 2 Timing ColorAdd_R"),
            Command::CueTiming {
                sequence: 1,
                number: 2.0,
                attribute: Some("ColorAdd_R".into()),
                fade: None,
                delay: None,
                curve: None,
                off: false,
            }
        );
        assert_eq!(
            command("Cue 2 Timing"),
            Command::CueTiming {
                sequence: 1,
                number: 2.0,
                attribute: None,
                fade: None,
                delay: None,
                curve: None,
                off: false,
            }
        );
        assert_eq!(
            command("Cue 2 Timing ColorAdd_R Off"),
            Command::CueTiming {
                sequence: 1,
                number: 2.0,
                attribute: Some("ColorAdd_R".into()),
                fade: None,
                delay: None,
                curve: None,
                off: true,
            }
        );
        assert_eq!(
            command("Cue 4 Trigger Go"),
            Command::CueTrigger {
                sequence: 1,
                number: 4.0,
                trigger: TriggerArg {
                    trigger: Trigger::Go,
                    time: 0.0,
                },
            }
        );
        assert_eq!(
            command("Executor 2 Mode Solo"),
            Command::SetExecutorMode {
                executor: 2,
                mode: ExecutorMode::Solo,
            }
        );
        assert_eq!(
            command("Executor 3 Flash Off"),
            Command::SetExecutorFlash {
                executor: 3,
                on: false,
            }
        );
        assert_eq!(
            command("Executor 1 Solo"),
            Command::SetExecutorSolo {
                executor: 1,
                on: true,
            }
        );
        assert_eq!(
            command("Delete Cue 4 Sequence 2"),
            Command::DeleteCue {
                sequence: 2,
                number: 4.0
            }
        );
        assert_eq!(
            command("Delete Palette Beam 1"),
            Command::DeletePalette {
                feature: "Beam".into(),
                number: 1
            }
        );
    }

    #[test]
    fn parses_cue_editing_commands() {
        assert_eq!(
            command("Store Cue 4 Replace"),
            Command::StoreCue {
                sequence: 1,
                number: 4.0,
                name: None,
                fade: None,
                delay: None,
                mib: None,
                trigger: None,
                mode: StoreMode::Replace,
            }
        );
        assert_eq!(
            command("Store Cue 4 Merge"),
            Command::StoreCue {
                sequence: 1,
                number: 4.0,
                name: None,
                fade: None,
                delay: None,
                mib: None,
                trigger: None,
                mode: StoreMode::Merge,
            }
        );
        assert_eq!(
            command("Update"),
            Command::Update {
                sequence: 1,
                cue: None
            }
        );
        assert_eq!(
            command("Update Cue 2"),
            Command::Update {
                sequence: 1,
                cue: Some(2.0)
            }
        );
        assert_eq!(
            command("Update Sequence 3"),
            Command::Update {
                sequence: 3,
                cue: None
            }
        );
        assert_eq!(
            command("Cue 2 Block"),
            Command::CueBlock {
                sequence: 1,
                number: 2.0,
                on: true
            }
        );
        assert_eq!(
            command("Cue 2 Unblock Sequence 2"),
            Command::CueBlock {
                sequence: 2,
                number: 2.0,
                on: false
            }
        );
        assert_eq!(
            command("Cue 2 Shield Pan"),
            Command::CueShield {
                sequence: 1,
                number: 2.0,
                attribute: Some("Pan".into()),
                off: false
            }
        );
        assert_eq!(
            command("Cue 2 Shield Pan Off"),
            Command::CueShield {
                sequence: 1,
                number: 2.0,
                attribute: Some("Pan".into()),
                off: true
            }
        );
        assert_eq!(
            command("Cue 2 Shield"),
            Command::CueShield {
                sequence: 1,
                number: 2.0,
                attribute: None,
                off: false
            }
        );
        assert_eq!(
            command("Cue 2 Show"),
            Command::CueShow {
                sequence: 1,
                number: 2.0
            }
        );
        assert_eq!(
            command("Cue 2 Move 5"),
            Command::CueMove {
                sequence: 1,
                number: 2.0,
                to: 5.0
            }
        );
        assert_eq!(
            command("Cue 2 Copy 5 Sequence 2"),
            Command::CueCopy {
                sequence: 1,
                number: 2.0,
                to: 5.0,
                target: Some(2)
            }
        );
    }

    #[test]
    fn rejects_incomplete_cue_editing_commands() {
        assert!(error("Cue 2 Shield Off")
            .message
            .contains("needs an attribute"));
        assert!(error("Update Cue").message.contains("cue number"));
        assert!(error("Cue 2 Move").message.contains("cue number"));
        assert!(error("Cue 2 Copy 5 Sequence")
            .message
            .contains("sequence number"));
    }

    #[test]
    fn parses_pages_and_page_assignments() {
        assert_eq!(command("Page"), Command::ListPages);
        assert_eq!(command("Page 2"), Command::SetPage { number: 2 });
        assert_eq!(command("Page Next"), Command::StepPage { next: true });
        assert_eq!(command("Page Prev"), Command::StepPage { next: false });
        assert_eq!(
            command("Store Page 2 \"Specials\""),
            Command::StorePage {
                number: 2,
                name: Some("Specials".into())
            }
        );
        assert_eq!(
            command("Store Page 3"),
            Command::StorePage {
                number: 3,
                name: None
            }
        );
        assert_eq!(command("Delete Page 2"), Command::DeletePage { number: 2 });
        assert_eq!(
            command("Assign Page 2 Executor 1 Sequence 3"),
            Command::AssignExecutor {
                page: Some(2),
                executor: 1,
                sequence: 3
            }
        );
        assert_eq!(
            command("Assign Executor 1 Sequence 3"),
            Command::AssignExecutor {
                page: None,
                executor: 1,
                sequence: 3
            }
        );
        assert!(!Command::ListPages.mutates());
        assert!(!Command::SetPage { number: 2 }.mutates());
        assert!(!Command::StepPage { next: true }.mutates());
        assert!(Command::StorePage {
            number: 2,
            name: None
        }
        .mutates());
        assert!(Command::DeletePage { number: 2 }.mutates());
        assert!(Command::AssignExecutor {
            page: Some(2),
            executor: 1,
            sequence: 1
        }
        .mutates());
        assert!(error("Page next 2").message.contains("unexpected"));
        assert!(error("Store Page").message.contains("page number"));
    }

    #[test]
    fn parses_executors_and_playback_commands() {
        assert_eq!(
            command("Assign Executor 2 Sequence 1"),
            Command::AssignExecutor {
                page: None,
                executor: 2,
                sequence: 1
            }
        );
        assert_eq!(
            command("Executor 2 At 75"),
            Command::SetExecutorLevel {
                executor: 2,
                level: Level::Percent(75.0)
            }
        );
        assert_eq!(
            command("Executor 2 Full"),
            Command::SetExecutorLevel {
                executor: 2,
                level: Level::Full
            }
        );
        assert_eq!(command("Executor"), Command::ListExecutors { only: None });
        assert_eq!(
            command("Go Executor 2"),
            Command::Go {
                target: Some(PlaybackRef::Executor(2))
            }
        );
        assert_eq!(
            command("Go Sequence 1"),
            Command::Go {
                target: Some(PlaybackRef::Sequence(1))
            }
        );
        assert_eq!(command("Go"), Command::Go { target: None });
        assert_eq!(command("Back"), Command::Back { target: None });
        assert_eq!(command("Pause"), Command::Pause { target: None });
        assert_eq!(command("Release"), Command::Release { target: None });
        assert_eq!(
            command("Clear All"),
            Command::Clear {
                all: true,
                values: true
            }
        );
        assert_eq!(
            command("Clear Values"),
            Command::Clear {
                all: false,
                values: true
            }
        );
        assert_eq!(
            command("New Show \"Gig\""),
            Command::New {
                name: Some("Gig".into())
            }
        );
    }

    #[test]
    fn parse_errors_have_actionable_hints() {
        let failure = error("Patch Fixture 1 At 1");
        assert!(
            failure
                .hint
                .as_deref()
                .unwrap()
                .contains("universe.address"),
            "{failure}"
        );
        let failure = error("Patch Fixture 1 At 1.1000");
        assert!(failure.message.contains("outside 1..=512"), "{failure}");
        let failure = error("Nonsense");
        assert!(failure.hint.unwrap().contains("Help"));
        let failure = error("Thru 4");
        assert!(failure.hint.unwrap().contains("starts with"));
        assert!(error("Store Cue 1 Fade -1").message.contains("positive"));
        assert!(error("Store Cue 2 Follow").message.contains("follow time"));
        assert!(error("Cue 2 Trigger Follow")
            .message
            .contains("follow time"));
        assert!(error("Cue 2 Trigger Follow -1").message.contains("-1"));
        assert!(error("Cue 2 Timing ColorAdd_R Curve Bounce")
            .message
            .contains("Bounce"));
        assert!(error("Cue 2 Timing Fade 1").message.contains("attribute"));
        assert!(error("Cue 2 Timing ColorAdd_R Off Fade 1")
            .message
            .contains("Off"));
        assert!(error("Cue 2 Stagger -0.5").message.contains("-0.5"));
        assert!(error("Cue 2 Trigger Sometimes")
            .message
            .contains("Sometimes"));
        assert!(error("Executor 1 Mode Wiggle").message.contains("Wiggle"));
        assert!(error("Store Palette Color 0")
            .message
            .contains("out of range"));
        assert!(error("Go Executor").message.contains("expected"));
    }

    #[test]
    fn at_without_selection_is_valid_at_parse_time() {
        assert_eq!(
            command("At 25"),
            Command::SetLevel {
                fixtures: None,
                attribute: None,
                source: LevelSource::Level(Level::Percent(25.0))
            }
        );
    }

    #[test]
    fn parses_undo_groups_and_raw_macro_lines() {
        assert_eq!(command("Begin"), Command::Begin { label: None });
        assert_eq!(
            command(r#"Begin setup"#),
            Command::Begin {
                label: Some("setup".into())
            }
        );
        assert_eq!(command("End"), Command::End);
        assert_eq!(
            parse_macro_line(r#"Macro 1 "Blackout cue" Blackout; Wait 2; Blackout Off"#).unwrap(),
            Some(Command::MacroStore {
                number: 1,
                name: "Blackout cue".into(),
                statements: vec!["Blackout".into(), "Wait 2".into(), "Blackout Off".into()]
            })
        );
        assert_eq!(
            parse_macro_line(r#"Macro 2 Add Store Cue 1"#).unwrap(),
            Some(Command::MacroAdd {
                number: 2,
                statement: "Store Cue 1".into()
            })
        );
        assert_eq!(
            parse_macro_line("Macro 2 Run").unwrap(),
            Some(Command::MacroRun { number: 2 })
        );
        assert_eq!(
            parse_macro_line("Macro 2 Delete").unwrap(),
            Some(Command::MacroDelete { number: 2 })
        );
        assert_eq!(
            parse_macro_line("Macro").unwrap(),
            Some(Command::MacroList { number: None })
        );
        assert_eq!(
            parse_macro_line("Macro 2").unwrap(),
            Some(Command::MacroList { number: Some(2) })
        );
        assert!(parse_macro_line("Fixture 1").unwrap().is_none());
        assert!(parse_macro_line("Macro 1 Run").unwrap().is_some());
        assert!(!command("Begin").mutates());
        assert!(matches!(
            parse_macro_line(r#"Macro 1 "x" Wait 2"#).unwrap(),
            Some(Command::MacroStore { .. })
        ));
        assert!(parse_macro_line(r#"Macro 1 "x""#)
            .unwrap_err()
            .message
            .contains("statement"));
    }

    #[test]
    fn parses_mvr_and_fixture_type_commands() {
        assert_eq!(
            command(r#"Import Mvr "rig show.mvr""#),
            Command::ImportMvr {
                path: "rig show.mvr".into()
            }
        );
        assert_eq!(
            command(r#"Import Type "generic.olft""#),
            Command::ImportType {
                path: "generic.olft".into()
            }
        );
        assert_eq!(
            command(r#"Export Type "Generic 4ch" "generic.olft""#),
            Command::ExportType {
                type_key: "Generic 4ch".into(),
                path: "generic.olft".into()
            }
        );
        assert_eq!(
            command(r#"Copy Type "Generic 4ch" As "Wide""#),
            Command::CopyType {
                type_key: "Generic 4ch".into(),
                new_key: "Wide".into()
            }
        );
        assert_eq!(
            command(r#"Type New "Generic 4ch""#),
            Command::TypeNew {
                key: "Generic 4ch".into()
            }
        );
        assert_eq!(
            command(r#"Type "Generic 4ch" Mode "4ch""#),
            Command::TypeMode {
                type_key: "Generic 4ch".into(),
                mode: "4ch".into()
            }
        );
        assert_eq!(
            command(r#"Type "Generic 4ch" Channel 2 ColorAdd_R "Red""#),
            Command::TypeChannel {
                type_key: "Generic 4ch".into(),
                offset: 2,
                attribute: "ColorAdd_R".into(),
                name: Some("Red".into())
            }
        );
        assert!(!command(r#"Export Type "Generic 4ch" "x.olft""#).mutates());
        assert!(command(r#"Import Mvr "x.mvr""#).mutates());
        assert!(command(r#"Type New "Generic 4ch""#).mutates());
    }

    #[test]
    fn parses_paths() {
        assert_eq!(
            command(r#"Import Gdtf "examples/gdtf/a b.gdtf""#),
            Command::ImportGdtf {
                path: "examples/gdtf/a b.gdtf".into()
            }
        );
        assert_eq!(
            command("Save target/show.json"),
            Command::Save {
                path: "target/show.json".into()
            }
        );
    }

    #[test]
    fn save_and_load_name_their_document() {
        assert_eq!(
            command(r#"Save Show "a.olshow""#),
            Command::SaveShow {
                path: "a.olshow".into()
            }
        );
        assert_eq!(
            command(r#"Save Desk "a.oldesk""#),
            Command::SaveDesk {
                path: "a.oldesk".into()
            }
        );
        assert_eq!(
            command(r#"Load Show "a.olshow""#),
            Command::Load {
                path: "a.olshow".into(),
                show_only: true
            }
        );
        assert_eq!(
            command(r#"Load "a.olshow""#),
            Command::Load {
                path: "a.olshow".into(),
                show_only: false
            }
        );
    }

    #[test]
    fn parses_home_and_default() {
        assert_eq!(
            command("Fixture 1 Home"),
            Command::Home {
                fixtures: Some(vec![1]),
                group: None,
            }
        );
        assert_eq!(
            command("Home"),
            Command::Home {
                fixtures: None,
                group: None,
            }
        );
        assert_eq!(
            command("Group 2 Home"),
            Command::Home {
                fixtures: None,
                group: Some(2),
            }
        );
        assert_eq!(
            command("Fixture 1 Default"),
            Command::SetHome {
                fixtures: Some(vec![1]),
                group: None,
                attribute: None,
                level: None,
                off: false,
            }
        );
        assert_eq!(
            command("Fixture 1 Default Dimmer 50"),
            Command::SetHome {
                fixtures: Some(vec![1]),
                group: None,
                attribute: Some("Dimmer".into()),
                level: Some(Level::Percent(50.0)),
                off: false,
            }
        );
        assert_eq!(
            command("Group 2 Default Dimmer Full"),
            Command::SetHome {
                fixtures: None,
                group: Some(2),
                attribute: Some("Dimmer".into()),
                level: Some(Level::Full),
                off: false,
            }
        );
        assert_eq!(
            command("Group 2 Default Dimmer Off"),
            Command::SetHome {
                fixtures: None,
                group: Some(2),
                attribute: Some("Dimmer".into()),
                level: None,
                off: true,
            }
        );
        assert!(error("Fixture 1 Default Off").message.contains("attribute"));
        assert!(!command("Fixture 1 Default").mutates());
        assert!(command("Fixture 1 Default Dimmer 50").mutates());
        assert!(command("Home").mutates());
    }

    #[test]
    fn parses_group_commands() {
        assert_eq!(command("Group"), Command::ListGroups);
        assert_eq!(command("Group 2"), Command::SelectGroup(2));
        assert_eq!(
            command("Group 2 At 40"),
            Command::SetLevelGroup {
                group: 2,
                attribute: None,
                source: LevelSource::Level(Level::Percent(40.0))
            }
        );
        assert_eq!(
            command("Group 2 At Palette Color 1"),
            Command::SetLevelGroup {
                group: 2,
                attribute: None,
                source: LevelSource::Palette {
                    feature: "Color".into(),
                    number: 1
                }
            }
        );
        assert_eq!(command("Group 2 Off"), Command::OffGroup { group: 2 });
        assert_eq!(
            command(r#"Store Group 3 "Front""#),
            Command::StoreGroup {
                number: 3,
                name: Some("Front".into())
            }
        );
        assert_eq!(
            command("Delete Group 3"),
            Command::DeleteGroup { number: 3 }
        );
    }

    #[test]
    fn parses_worlds_and_filters() {
        assert_eq!(command("World"), Command::ListWorlds);
        assert_eq!(
            command("World 2"),
            Command::ActivateWorld { number: Some(2) }
        );
        assert_eq!(
            command("World All"),
            Command::ActivateWorld { number: None }
        );
        assert_eq!(
            command("Store World 2 Front"),
            Command::StoreWorld {
                number: 2,
                name: Some("Front".into())
            }
        );
        assert_eq!(
            command("Delete World 2"),
            Command::DeleteWorld { number: 2 }
        );

        assert_eq!(command("Filter"), Command::ListFilters);
        assert_eq!(
            command("Filter 3"),
            Command::ActivateFilter { number: Some(3) }
        );
        assert_eq!(
            command("Store Filter 3 \"Intensity\" Dimmer Color"),
            Command::StoreFilter {
                number: 3,
                name: Some("Intensity".into()),
                features: vec!["Dimmer".into(), "Color".into()]
            }
        );
        assert_eq!(
            command("Store Filter 3 Dimmer"),
            Command::StoreFilter {
                number: 3,
                name: None,
                features: vec!["Dimmer".into()]
            }
        );
        assert_eq!(
            command("Delete Filter 3"),
            Command::DeleteFilter { number: 3 }
        );
        assert!(error("Store Filter 3")
            .message
            .contains("at least one feature"));
    }

    #[test]
    fn parses_timecode_and_history_commands() {
        assert_eq!(command("Timecode"), Command::ListTimecodes { only: None });
        assert_eq!(
            command("Timecode 2"),
            Command::ListTimecodes { only: Some(2) }
        );
        assert_eq!(
            command(r#"Store Timecode 2 "Song" Fps 30"#),
            Command::StoreTimecode {
                number: 2,
                name: Some("Song".into()),
                fps: Some(30),
                source: None,
            }
        );
        assert_eq!(
            command("Store Timecode 2 Source Mtc \"/dev/ttyACM0\""),
            Command::StoreTimecode {
                number: 2,
                name: None,
                fps: None,
                source: Some(TimecodeSource::Mtc {
                    port: "/dev/ttyACM0".into()
                }),
            }
        );
        assert_eq!(
            command("Store Timecode 2 Fps 25 Source Osc"),
            Command::StoreTimecode {
                number: 2,
                name: None,
                fps: Some(25),
                source: Some(TimecodeSource::Osc),
            }
        );
        assert_eq!(
            command("Timecode 2 Track 3 \"FX\""),
            Command::TimecodeTrack {
                timecode: 2,
                track: 3,
                name: Some("FX".into()),
            }
        );
        assert_eq!(
            command("Timecode 2 Track 3 Add 00:01:30:12 Effect 4 On Group 5"),
            Command::AddTimecodeEvent {
                timecode: 2,
                track: Some(3),
                at: "00:01:30:12".into(),
                action: TimecodeAction::Effect {
                    effect: 4,
                    fixtures: None,
                    group: Some(5),
                    on: true,
                },
            }
        );
        assert_eq!(
            command("Timecode 2 Track 3 Add 00:01:31:00 Effect 4 Off"),
            Command::AddTimecodeEvent {
                timecode: 2,
                track: Some(3),
                at: "00:01:31:00".into(),
                action: TimecodeAction::Effect {
                    effect: 4,
                    fixtures: None,
                    group: None,
                    on: false,
                },
            }
        );
        assert_eq!(
            command("Timecode 2 Add 00:01:32:00 Effect 4 On Fixture 1 Thru 2"),
            Command::AddTimecodeEvent {
                timecode: 2,
                track: None,
                at: "00:01:32:00".into(),
                action: TimecodeAction::Effect {
                    effect: 4,
                    fixtures: Some(vec![1, 2]),
                    group: None,
                    on: true,
                },
            }
        );
        assert_eq!(
            command("Timecode 2 Track 3 Remove 00:01:31:00"),
            Command::RemoveTimecodeEvent {
                timecode: 2,
                track: Some(3),
                at: "00:01:31:00".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 Marker 00:02:00:00 \"Chorus\""),
            Command::TimecodeMarker {
                timecode: 2,
                at: "00:02:00:00".into(),
                name: "Chorus".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 Add 00:01:30:12 Sequence 3 Cue 4.5"),
            Command::AddTimecodeEvent {
                timecode: 2,
                track: None,
                at: "00:01:30:12".into(),
                action: TimecodeAction::Cue {
                    sequence: 3,
                    cue: 4.5
                },
            }
        );
        assert_eq!(
            command("Timecode 2 Remove 00:01:30:12"),
            Command::RemoveTimecodeEvent {
                timecode: 2,
                track: None,
                at: "00:01:30:12".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 At 00:00:05:00"),
            Command::LocateTimecode {
                timecode: 2,
                at: "00:00:05:00".into(),
            }
        );
        assert_eq!(
            command("Timecode 2 Start"),
            Command::StartTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Timecode 2 Pause"),
            Command::PauseTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Timecode 2 Stop"),
            Command::StopTimecode { timecode: 2 }
        );
        assert_eq!(
            command("Delete Timecode 2"),
            Command::DeleteTimecode { number: 2 }
        );
        assert_eq!(command("Undo"), Command::Undo);
        assert_eq!(command("Redo"), Command::Redo);
        assert!(error("Timecode 1 Add 00:00:00:00 Sequence 1")
            .message
            .contains("cue"));
        assert!(error("Store Timecode 1 Source Ltc")
            .hint
            .as_deref()
            .unwrap_or_default()
            .contains("Internal"));
    }

    #[test]
    fn parses_master_commands() {
        assert_eq!(command("Master"), Command::ListMasters);
        assert_eq!(
            command("Master Grand At 75"),
            Command::SetGrandMaster {
                level: Level::Percent(75.0)
            }
        );
        assert_eq!(
            command("Master Grand Off"),
            Command::SetGrandMaster {
                level: Level::Percent(0.0)
            }
        );
        assert_eq!(
            command("Master Rate At 200"),
            Command::SetRateMaster {
                level: Level::Percent(200.0)
            }
        );
        assert_eq!(
            command("Master Group 4 Full"),
            Command::SetGroupMaster {
                group: 4,
                level: Level::Full
            }
        );
    }

    #[test]
    fn parses_effect_commands() {
        assert_eq!(command("Effect"), Command::ListEffects { only: None });
        assert_eq!(command("Effect 2"), Command::ListEffects { only: Some(2) });
        assert_eq!(
            command("Effect 2 On"),
            Command::EffectOn {
                effect: 2,
                fixtures: None
            }
        );
        assert_eq!(
            command("Effect 2 On Fixture 1 Thru 3"),
            Command::EffectOn {
                effect: 2,
                fixtures: Some(vec![1, 2, 3])
            }
        );
        assert_eq!(
            command("Effect 2 Off"),
            Command::EffectOff { effect: Some(2) }
        );
        assert_eq!(command("Effect Off"), Command::EffectOff { effect: None });

        assert_eq!(
            command("Store Effect 2 Pulse Attribute ColorAdd_R Waveform Square Rate 2 Size 25 Spread 90"),
            Command::StoreEffect {
                number: 2,
                name: Some("Pulse".into()),
                targets: Some(vec!["ColorAdd_R".into()]),
                waveform: Some("Square".into()),
                rate: Some(EffectRate::Hz(2.0)),
                size: Some(25.0),
                phases: Vec::new(),
                spread: Some(90.0)
            }
        );
        assert_eq!(
            command(r#"Store Effect 2 "Default""#),
            Command::StoreEffect {
                number: 2,
                name: Some("Default".into()),
                targets: None,
                waveform: None,
                rate: None,
                size: None,
                phases: Vec::new(),
                spread: None
            }
        );
        assert_eq!(
            command(r#"Store Effect 1 "Circle" Rate 1/4 Attributes Pan Tilt Phase Pan 0 Tilt 90"#),
            Command::StoreEffect {
                number: 1,
                name: Some("Circle".into()),
                targets: Some(vec!["Pan".into(), "Tilt".into()]),
                waveform: None,
                rate: Some(EffectRate::Division("1/4".into())),
                size: None,
                phases: vec![("Pan".into(), 0.0), ("Tilt".into(), 90.0)],
                spread: None
            }
        );
        assert_eq!(
            command("Effect 1 Attributes Pan Tilt"),
            Command::SetEffectAttributes {
                effect: 1,
                attributes: vec!["Pan".into(), "Tilt".into()]
            }
        );
        assert_eq!(
            command("Effect 1 Attribute Pan Size 30 Phase 120"),
            Command::SetEffectTarget {
                effect: 1,
                attribute: "Pan".into(),
                size: Some(30.0),
                phase: Some(120.0)
            }
        );
        assert_eq!(
            command("Effect 1 Rate 2hz"),
            Command::SetEffectRate {
                effect: 1,
                rate: EffectRate::Hz(2.0)
            }
        );
        assert_eq!(
            command("Effect 1 Rate 1/4"),
            Command::SetEffectRate {
                effect: 1,
                rate: EffectRate::Division("1/4".into())
            }
        );
        assert_eq!(
            command("Effect 1 Tempo Off"),
            Command::SetEffectFreeRate { effect: 1 }
        );
        assert_eq!(command("Tempo 128"), Command::SetTempo { bpm: 128.0 });
        assert_eq!(command("Tap"), Command::TapTempo);
        assert_eq!(command("Tap Tempo"), Command::TapTempo);
        assert!(error("Effect 1 Attribute Pan Phase 400")
            .message
            .contains("outside 0..=360"));
        assert!(error("Effect 1 Attribute Pan Phase -10")
            .message
            .contains("not a positive number"));
        assert_eq!(
            command("Delete Effect 2"),
            Command::DeleteEffect { number: 2 }
        );
    }

    #[test]
    fn parses_blackout_and_locking() {
        assert_eq!(
            command("Blackout"),
            Command::Blackout {
                off: false,
                fade: None
            }
        );
        assert_eq!(
            command("Blackout Fade 2"),
            Command::Blackout {
                off: false,
                fade: Some(2.0)
            }
        );
        assert_eq!(
            command("Blackout Off"),
            Command::Blackout {
                off: true,
                fade: None
            }
        );
        assert_eq!(
            command("Lock 4711"),
            Command::Lock {
                code: Some("4711".into())
            }
        );
        assert_eq!(command("Lock"), Command::Lock { code: None });
        assert_eq!(
            command("Unlock 4711"),
            Command::Unlock {
                code: Some("4711".into())
            }
        );
        assert!(lex("Blackout Fade -1")
            .and_then(|tokens| parse(&tokens))
            .is_err());
        assert!(
            lex("Lock a b").and_then(|tokens| parse(&tokens)).is_err(),
            "one code, not a phrase"
        );
    }

    #[test]
    fn the_lock_allow_list_is_a_classification() {
        // Questions and listings are always answerable.
        for line in [
            "Help",
            "Status",
            "Sheet",
            "Palette",
            "Sequence",
            "Executor",
            "Group",
            "World",
            "Filter",
            "Master",
            "Effect",
            "Timecode",
            "Preview",
            "Preview Off",
            "Fixture 1 Gobo",
            "Patch Check",
            "Report Status \"status.txt\"",
        ] {
            assert!(!command(line).mutates(), "{line} must not mutate");
        }
        assert!(!command("Unlock 1").mutates());
        assert!(
            !command("Blackout Off").mutates(),
            "lifting is a safety action"
        );
        // Everything that changes the show, the desk or the document.
        for line in [
            "Fixture 1 At 50",
            "Store Cue 1",
            "Delete Cue 1",
            "Patch Fixture 1 At 1.001",
            "Clone Fixture 1 At 1.101",
            "New",
            "Save \"a.olshow\"",
            "Import Gdtf \"a.gdtf\"",
            "Go",
        ] {
            let parsed = lex(line).and_then(|tokens| parse(&tokens));
            assert!(parsed.is_ok(), "{line} must parse");
            assert!(parsed.unwrap().mutates(), "{line} must mutate");
        }
        assert!(command("Blackout").mutates());
        assert!(command("Lock").mutates());
        assert!(command("Blind").mutates());
        assert!(command("Highlight").mutates());
    }

    #[test]
    fn parses_clone_patch_tools_and_reports() {
        assert_eq!(
            command("Clone Fixture 1 At 1.101"),
            Command::Clone {
                fixture: 1,
                universe: 1,
                address: 101,
                count: None,
                step: None
            }
        );
        assert_eq!(
            command("Clone Fixture 2 At 2.010 Count 8 Step 6"),
            Command::Clone {
                fixture: 2,
                universe: 2,
                address: 10,
                count: Some(8),
                step: Some(6)
            }
        );
        assert_eq!(command("Patch Check"), Command::PatchCheck);
        assert_eq!(
            command(r#"Patch Import "rig.csv""#),
            Command::ImportPatch {
                path: "rig.csv".into()
            }
        );
        assert_eq!(
            command(r#"Report Patch "patch.csv""#),
            Command::Report {
                kind: ReportKind::Patch,
                sequence: None,
                path: "patch.csv".into()
            }
        );
        assert_eq!(
            command(r#"Report Cues 3 "cues.csv""#),
            Command::Report {
                kind: ReportKind::Cues,
                sequence: Some(3),
                path: "cues.csv".into()
            }
        );
        assert_eq!(
            command(r#"Report Cues "cues.csv""#),
            Command::Report {
                kind: ReportKind::Cues,
                sequence: None,
                path: "cues.csv".into()
            }
        );
        assert_eq!(
            command(r#"Report Show "show.md""#),
            Command::Report {
                kind: ReportKind::Show,
                sequence: None,
                path: "show.md".into()
            }
        );
        assert!(error("Clone Fixture 1 At 1.101 Count 0")
            .message
            .contains("clone count 0"));
        assert!(error(r#"Report Nope "x""#)
            .hint
            .unwrap()
            .contains("Report Patch"));
        assert!(!command(r#"Report Status "status.txt""#).mutates());
        assert!(!command("Patch Check").mutates());
        assert!(command("Clone Fixture 1 At 1.101").mutates());
        assert!(command(r#"Patch Import "rig.csv""#).mutates());
    }

    #[test]
    fn parses_channel_sets() {
        assert_eq!(
            command("Fixture 1 Gobo"),
            Command::SetChannelSet {
                fixtures: Some(vec![1]),
                group: None,
                attribute: "Gobo".into(),
                target: None
            }
        );
        assert_eq!(
            command("Fixture 1 Gobo 2"),
            Command::SetChannelSet {
                fixtures: Some(vec![1]),
                group: None,
                attribute: "Gobo".into(),
                target: Some(SetTarget::Index(2))
            }
        );
        assert_eq!(
            command(r#"Fixture 1 Gobo "Rotate CW""#),
            Command::SetChannelSet {
                fixtures: Some(vec![1]),
                group: None,
                attribute: "Gobo".into(),
                target: Some(SetTarget::Name("Rotate CW".into()))
            }
        );
        assert_eq!(
            command(r#"Group 1 Gobo "Open""#),
            Command::SetChannelSet {
                fixtures: None,
                group: Some(1),
                attribute: "Gobo".into(),
                target: Some(SetTarget::Name("Open".into()))
            }
        );
        assert!(lex("Fixture 1 Gobo 0")
            .and_then(|tokens| parse(&tokens))
            .is_err());
        assert!(lex("Fixture 1 Gobo 2.5")
            .and_then(|tokens| parse(&tokens))
            .is_err());
        assert!(
            lex("Fixture 1 Gobo Rotate")
                .and_then(|tokens| parse(&tokens))
                .is_err(),
            "a name with a space needs quotes"
        );
        assert!(command("Fixture 1 Gobo 2").mutates());
        assert!(
            !command("Fixture 1 Gobo").mutates(),
            "the listing is a view"
        );
    }

    #[test]
    fn parses_blind_preview_and_highlight() {
        assert_eq!(command("Blind"), Command::Blind { on: true });
        assert_eq!(command("Blind Off"), Command::Blind { on: false });
        assert_eq!(command("Highlight"), Command::Highlight { on: true });
        assert_eq!(command("Highlight Off"), Command::Highlight { on: false });
        assert_eq!(
            command("Preview"),
            Command::Preview {
                cue: None,
                sequence: None,
                off: false
            }
        );
        assert_eq!(
            command("Preview Cue 14"),
            Command::Preview {
                cue: Some(14.0),
                sequence: None,
                off: false
            }
        );
        assert_eq!(
            command("Preview Cue 14 Sequence 2"),
            Command::Preview {
                cue: Some(14.0),
                sequence: Some(2),
                off: false
            }
        );
        assert_eq!(
            command("Preview Sequence 2"),
            Command::Preview {
                cue: None,
                sequence: Some(2),
                off: false
            }
        );
        assert_eq!(
            command("Preview Off"),
            Command::Preview {
                cue: None,
                sequence: None,
                off: true
            }
        );
        assert!(lex("Blind maybe")
            .and_then(|tokens| parse(&tokens))
            .is_err());
        assert!(lex("Preview Cue")
            .and_then(|tokens| parse(&tokens))
            .is_err());
    }

    #[test]
    fn parses_every_colour_form() {
        assert_eq!(
            command("Color Red"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Named("Red".into())),
            }
        );
        assert_eq!(
            command("Color Warm White"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Named("Warm White".into())),
            },
            "an unquoted name joins its words"
        );
        assert_eq!(
            command(r#"Fixture 2 Color "Warm White""#),
            Command::SetColor {
                fixtures: Some(vec![2]),
                group: None,
                spec: Some(ColorSpec::Named("Warm White".into())),
            }
        );
        assert_eq!(
            command(r##"Color "#ff8000""##),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Hex("#ff8000".into())),
            }
        );
        assert_eq!(
            command("Color #FF8000"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Hex("#FF8000".into())),
            }
        );
        assert_eq!(
            command("Group 3 Color Rgb 255 128 0"),
            Command::SetColor {
                fixtures: None,
                group: Some(3),
                spec: Some(ColorSpec::Rgb(255, 128, 0)),
            }
        );
        assert_eq!(
            command("Fixture 1 Color Cmy 0 60 100"),
            Command::SetColor {
                fixtures: Some(vec![1]),
                group: None,
                spec: Some(ColorSpec::Cmy(0.0, 60.0, 100.0)),
            }
        );
        assert_eq!(
            command("Color Hsb 30 100 50.5"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Hsb(30.0, 100.0, 50.5)),
            }
        );
        assert_eq!(
            command("Color 3200K"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: Some(ColorSpec::Kelvin(3200)),
            }
        );
        assert_eq!(
            command("Color"),
            Command::SetColor {
                fixtures: None,
                group: None,
                spec: None,
            }
        );
        assert_eq!(
            command("Fixture 1 Color"),
            Command::SetColor {
                fixtures: Some(vec![1]),
                group: None,
                spec: None,
            }
        );
        assert!(command("Color Red").mutates());
        assert!(!command("Color").mutates(), "the listing is a view");
        assert!(!command("Fixture 1 Color").mutates());
    }

    #[test]
    fn parses_dmx_input_and_merge_commands() {
        assert_eq!(command("Dmx"), Command::ListDmx { universe: None });
        assert_eq!(command("Dmx In"), Command::ListDmx { universe: None });
        assert_eq!(command("Dmx 2"), Command::ListDmx { universe: Some(2) });
        assert_eq!(
            command("Dmx 1 Merge Htp"),
            Command::SetDmxMerge {
                universe: 1,
                mode: MergeMode::Htp
            }
        );
        assert_eq!(
            command("Dmx 1 Priority Input"),
            Command::SetDmxPriority {
                universe: 1,
                priority: InputPriority::Input
            }
        );
        assert_eq!(
            command("Dmx 1 Loss Fade 2"),
            Command::SetDmxLoss {
                universe: 1,
                loss: LossPolicy::Fade(2.0)
            }
        );
        assert_eq!(
            command("Dmx 1 Timeout 1.5"),
            Command::SetDmxTimeout {
                universe: 1,
                ms: 1_500
            }
        );
        assert_eq!(
            command("Dmx 1 Trigger 10 Sequence 2 Threshold 200"),
            Command::SetDmxTrigger {
                universe: 1,
                trigger: InputTrigger {
                    address: 10,
                    threshold: 200,
                    sequence: 2
                }
            }
        );
        assert_eq!(
            command("Dmx 1 Trigger Off"),
            Command::ClearDmxTrigger { universe: 1 }
        );
        assert_eq!(
            command("Dmx 1 Snapshot"),
            Command::DmxSnapshot { universe: 1 }
        );

        assert!(!command("Dmx").mutates());
        assert!(!command("Dmx 1").mutates());
        assert!(command("Dmx 1 Merge Htp").mutates());
        assert!(command("Dmx 1 Snapshot").mutates());
    }

    #[test]
    fn rejects_bad_dmx_commands() {
        for (line, needle) in [
            ("Dmx 1 Merge", "Off"),
            ("Dmx 1 Merge Sideways", "Off"),
            ("Dmx 1 Priority", "Local"),
            ("Dmx 1 Loss Fade", "fade"),
            ("Dmx 1 Loss Fade 90", "0..60"),
            ("Dmx 1 Timeout 0", "0.001"),
            ("Dmx 1 Trigger 0 Sequence 1", "trigger address"),
            (
                "Dmx 1 Trigger 1 Sequence 1 Threshold 0",
                "trigger threshold",
            ),
            ("Dmx 1 Nonsense", "unknown DMX action"),
        ] {
            let error = error(line);
            assert!(
                error.to_string().contains(needle),
                "`{line}` should name `{needle}`: {error}"
            );
        }
    }

    #[test]
    fn rejects_bad_colours_by_name() {
        for (line, needle) in [
            (r##"Color "#FF80""##, "#FF80"),
            ("Color #GG8000", "#GG8000"),
            ("Color Chartreuse", "Chartreuse"),
            ("Color Rgb 300 0 0", "red 300"),
            ("Color Rgb 10.5 0 0", "whole number"),
            ("Color Rgb 10 20", "blue"),
            ("Color Cmy 0 101 0", "magenta 101"),
            ("Color Hsb 400 0 0", "hue 400"),
            ("Color 3200", "`3200` is not a colour"),
            ("Color 900K", "900K"),
        ] {
            let error = error(line);
            assert!(
                error.to_string().contains(needle),
                "`{line}` should name `{needle}`: {error}"
            );
        }
        assert!(
            error("Color Red 5").to_string().contains("unexpected"),
            "trailing tokens are not part of a name"
        );
    }
}
