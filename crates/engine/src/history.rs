//! Undo and redo over desk snapshots.
//!
//! Every command that changes the show *or the programmer* records the state
//! before the change, together with the command text that made it. Undo swaps
//! the current state for the previous snapshot and redo swaps it back, so the
//! operator can step through their own edits without a per-command inverse.
//!
//! A snapshot is a full [`Show`] plus the programmer [`Levels`]. Shows are small
//! (fixtures, pools and cue data, not DMX frames), the depth is capped, and a
//! snapshot is exact: undo restores pools, cues, timecode events and the values
//! being edited together, with no chance of a partial inverse leaving a state
//! that does not validate.
//!
//! Selection, masters, running playbacks, effects and the timecode player are
//! handles rather than programming, so they are deliberately not restored.

use crate::levels::Levels;
use crate::show::Show;

/// How many edits can be undone. Old snapshots fall off the back.
pub const MAX_UNDO: usize = 64;

#[derive(Debug, Clone, PartialEq)]
pub struct Snapshot {
    pub show: Show,
    pub programmer: Levels,
}

#[derive(Debug)]
struct Step {
    snapshot: Snapshot,
    label: String,
}

#[derive(Debug, Default)]
pub struct History {
    undo: Vec<Step>,
    redo: Vec<Step>,
}

impl History {
    /// Remember the state before a change. A new change after an undo drops
    /// the redo branch, exactly like a text editor.
    pub fn record(&mut self, snapshot: Snapshot, label: String) {
        self.undo.push(Step { snapshot, label });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Restore the previous state, returning the command that is being undone.
    pub fn undo(&mut self, current: &mut Snapshot) -> Option<String> {
        let step = self.undo.pop()?;
        let label = step.label;
        self.redo.push(Step {
            snapshot: std::mem::replace(current, step.snapshot),
            label: label.clone(),
        });
        Some(label)
    }

    /// Re-apply the last undone change.
    pub fn redo(&mut self, current: &mut Snapshot) -> Option<String> {
        let step = self.redo.pop()?;
        let label = step.label;
        self.undo.push(Step {
            snapshot: std::mem::replace(current, step.snapshot),
            label: label.clone(),
        });
        Some(label)
    }

    pub fn clear(&mut self) {
        self.undo.clear();
        self.redo.clear();
    }

    pub fn undo_depth(&self) -> usize {
        self.undo.len()
    }

    pub fn redo_depth(&self) -> usize {
        self.redo.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::Show;

    fn snapshot(programmer: f64) -> Snapshot {
        let mut levels = Levels::new();
        levels.set(1, "Dimmer", programmer);
        Snapshot {
            show: Show::new(),
            programmer: levels,
        }
    }

    #[test]
    fn undo_and_redo_swap_show_and_programmer() {
        let mut history = History::default();
        let mut current = snapshot(0.6);
        history.record(snapshot(0.5), "first".into());

        assert_eq!(history.undo(&mut current).as_deref(), Some("first"));
        assert_eq!(current.programmer.get(1, "Dimmer"), Some(0.5));
        assert_eq!(history.redo(&mut current).as_deref(), Some("first"));
        assert_eq!(current.programmer.get(1, "Dimmer"), Some(0.6));
    }

    #[test]
    fn a_new_change_drops_the_redo_branch() {
        let mut history = History::default();
        let mut current = snapshot(0.2);
        history.record(snapshot(0.5), "first".into());
        history.undo(&mut current);
        assert_eq!(history.redo_depth(), 1);
        history.record(snapshot(0.7), "second".into());
        assert_eq!(history.redo_depth(), 0);
    }

    #[test]
    fn depth_is_capped() {
        let mut history = History::default();
        for step in 0..(MAX_UNDO + 5) {
            history.record(snapshot(step as f64 / 100.0), "edit".into());
        }
        assert_eq!(history.undo_depth(), MAX_UNDO);
    }
}
