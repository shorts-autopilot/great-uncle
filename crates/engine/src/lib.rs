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
//! * [`fixture_type`] — GDTF-derived fixture types, modes and channel functions.
//! * [`gdtf`] — `.gdtf` archive import.
//! * [`command`] — lexer and parser for the command line.
//! * [`levels`] — normalised level maps shared by every layer.
//! * [`palette`] — typed preset pools.
//! * [`playback`] — cues, sequences, executors and running fades.
//! * [`render`] — programmer and playback to DMX universes.
//! * [`output`] — 60 Hz engine tick + 30 Hz frame output thread.

pub mod command;
pub mod engine;
pub mod fixture_type;
pub mod gdtf;
pub mod levels;
pub mod output;
pub mod palette;
pub mod patch;
pub mod playback;
pub mod registry;
pub mod render;
pub mod show;

#[cfg(test)]
pub(crate) mod test_support;

pub use command::CommandError;
pub use engine::{CommandOutcome, Engine};
pub use fixture_type::{Channel, ChannelFunction, FixtureType, Mode, DIMMER};
pub use levels::Levels;
pub use output::{OutputStatus, Runtime, RuntimeConfig, TransportBinding};
pub use palette::Palette;
pub use playback::{Cue, Executor, Playback, Sequence, Target};
pub use show::{Fixture, Show, Universe};
