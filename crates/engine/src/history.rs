//! Undo and redo over show snapshots.
//!
//! Every command that changes the show records the show *before* the change,
//! together with the command text that made it. Undo swaps the current show for
//! the previous snapshot and redo swaps it back, so the operator can step
//! through their own edits without a per-command inverse for each feature.
//!
//! A snapshot is a full [`Show`]. Shows are small (fixtures, pools and cue data,
//! not DMX frames), the depth is capped, and a snapshot is exact: undo restores
//! pools, cues, masters and timecode events together, with no chance of a
//! partial inverse leaving a show that does not validate.
//!
//! Selection, running playbacks, effects and the timecode player are console
//! state, not show data, so they are deliberately not restored.

use crate::show::Show;

/// How many edits can be undone. Old snapshots fall off the back.
pub const MAX_UNDO: usize = 64;

#[derive(Debug)]
struct Step {
    show: Show,
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
    pub fn record(&mut self, show: Show, label: String) {
        self.undo.push(Step { show, label });
        if self.undo.len() > MAX_UNDO {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    /// Restore the previous state, returning the command that is being undone.
    pub fn undo(&mut self, current: &mut Show) -> Option<String> {
        let step = self.undo.pop()?;
        let label = step.label;
        self.redo.push(Step {
            show: std::mem::replace(current, step.show),
            label: label.clone(),
        });
        Some(label)
    }

    /// Re-apply the last undone change.
    pub fn redo(&mut self, current: &mut Show) -> Option<String> {
        let step = self.redo.pop()?;
        let label = step.label;
        self.undo.push(Step {
            show: std::mem::replace(current, step.show),
            label: label.clone(),
        });
        Some(label)
    }

    /// Forget everything, e.g. after loading a different show.
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
    use crate::test_support::demo_show;

    #[test]
    fn undo_and_redo_swap_the_show() {
        let mut history = History::default();
        let mut show = demo_show();
        let original = show.clone();

        show.name = "Edited".into();
        history.record(original.clone(), "New Show".into());
        assert_eq!(history.undo_depth(), 1);

        assert_eq!(history.undo(&mut show), Some("New Show".to_string()));
        assert_eq!(show, original);
        assert_eq!(history.undo_depth(), 0);
        assert_eq!(history.redo_depth(), 1);
        assert_eq!(history.undo(&mut show), None, "nothing left to undo");

        assert_eq!(history.redo(&mut show), Some("New Show".to_string()));
        assert_eq!(show.name, "Edited");
        assert_eq!(history.redo(&mut show), None);
    }

    #[test]
    fn a_new_change_drops_the_redo_branch() {
        let mut history = History::default();
        let mut show = demo_show();
        let before = show.clone();
        show.name = "First".into();
        history.record(before, "First change".into());
        history.undo(&mut show);
        assert_eq!(history.redo_depth(), 1);

        let before = show.clone();
        show.name = "Second".into();
        history.record(before, "Second change".into());
        assert_eq!(history.redo_depth(), 0);
        assert_eq!(history.undo_depth(), 1);
    }

    #[test]
    fn the_depth_is_capped() {
        let mut history = History::default();
        let mut show = demo_show();
        for step in 0..(MAX_UNDO + 5) {
            let before = show.clone();
            show.name = format!("Step {step}");
            history.record(before, format!("Step {step}"));
        }
        assert_eq!(history.undo_depth(), MAX_UNDO);
        while history.undo(&mut show).is_some() {}
        assert_eq!(
            show.name, "Step 4",
            "the oldest snapshots fell off the back"
        );
    }

    #[test]
    fn clear_forgets_both_directions() {
        let mut history = History::default();
        history.record(demo_show(), "change".into());
        history.clear();
        assert_eq!(history.undo_depth(), 0);
        assert_eq!(history.redo_depth(), 0);
    }
}
