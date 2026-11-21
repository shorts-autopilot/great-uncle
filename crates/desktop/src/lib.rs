//! Desktop model, kept apart from the window so it can be tested headless.
//!
//! The model owns no show state and no lighting maths. It turns a click or a
//! key into the same command text the terminal would receive and hands it to
//! `App::execute`. The window is a drawing of `openlight_app::Snapshot` plus
//! this model.

pub mod model;
pub mod workspace;

pub use model::{CommandHistory, Model};
pub use workspace::{GridRect, Window, WindowId, WindowKind, WindowTab, Workspace, COLUMNS, ROWS};
