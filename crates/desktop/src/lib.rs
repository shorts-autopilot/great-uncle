//! Desktop model, kept apart from the window so it can be tested headless.
//!
//! The model owns no show state and no lighting maths. It turns a click or a
//! key into the same command text the terminal would receive and hands it to
//! `App::execute`. The window is a drawing of `openlight_app::Snapshot` plus
//! this model.

pub mod model;

pub use model::{Model, Pane};
