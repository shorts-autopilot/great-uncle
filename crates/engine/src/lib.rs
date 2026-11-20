//! Lighting engine.
//!
//! The engine is the single writer of show state. Every input surface (console
//! now, GUI and Lua later) turns user input into the same [`command`] text and
//! hands it to [`engine::Engine::execute`], so behaviour cannot drift between
//! surfaces.
//!
//! Layers:
//!
//! * [`show`] — versioned, validated, self-contained show file.
//! * [`desk`] — the handles: selection, programmer, masters and playback
//!   positions, saved beside the show rather than inside it.
//! * [`store`] — atomic, crash-safe writes shared by every document.
//! * [`csv`] — the patch spreadsheet, for `Patch Import` and `Report Patch`.
//! * [`fixture_type`] — GDTF-derived fixture types, modes and channel functions.
//! * [`gdtf`] — `.gdtf` archive import.
//! * [`command`] — lexer and parser for the command line.
//! * [`levels`] — normalised level maps shared by every layer.
//! * [`palette`] — typed preset pools.
//! * [`playback`] — cues, sequences, executors and running fades.
//! * [`pools`] — groups, worlds and filters.
//! * [`effect`] — stored waveforms and the running effect layer.
//! * [`timecode`] — timecode tracks, markers and the running player.
//! * [`mtc`] — MIDI Time Code decoding and the serial reader thread.
//! * [`history`] — undo and redo over show snapshots.
//! * [`journal`] — the crash journal and the autosave snapshot.
//! * [`render`] — programmer, playback, effects and masters to DMX universes.
//! * [`output`] — 60 Hz engine tick + 30 Hz frame output thread.

pub mod builder;
pub mod command;
pub mod csv;
pub mod desk;
pub mod effect;
pub mod engine;
pub mod fixture_type;
pub mod gdtf;
pub mod history;
pub mod journal;
pub mod levels;
pub mod mtc;
pub mod mvr;
pub mod output;
pub mod palette;
pub mod patch;
pub mod playback;
pub mod pools;
pub mod registry;
pub mod render;
pub mod show;
pub mod store;
pub mod timecode;

#[cfg(test)]
pub(crate) mod test_support;

pub use builder::FixtureTypeFile;
pub use command::CommandError;
pub use desk::{Desk, Masters};
pub use effect::{Effect, Waveform};
pub use engine::{CommandOutcome, Engine};
pub use fixture_type::{split_key, Channel, ChannelFunction, FixtureType, Mode, DIMMER};
pub use levels::Levels;
pub use output::{OutputStatus, RemoteStatus, Runtime, RuntimeConfig, TransportBinding};
pub use palette::Palette;
pub use playback::{Cue, Executor, Playback, Sequence, Target};
pub use pools::{Filter, Group, World};
pub use show::{DeskSeed, Fixture, LoadReport, Show, Universe, SHOW_VERSION};
pub use timecode::{
    Action as TimecodeAction, Event as TimecodeEvent, Marker as TimecodeMarker, TimecodePlayer,
    TimecodeShow, TimecodeSource, Track as TimecodeTrack,
};
