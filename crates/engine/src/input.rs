//! Per-universe DMX input merge state.
//!
//! The mixer is deliberately separate from the renderer and from the input
//! transport. It sees only complete frames, a monotonic clock and the
//! universe's merge policy, which makes every edge case testable without a
//! device or a runtime.
//!
//! **LTP ownership.** HTP needs no history. LTP needs to remember which source
//! owns each slot and whether either source changed since the last merge. A
//! local cue moving one channel therefore owns only that channel, not the
//! whole universe, and an unchanged input does not reclaim a slot that a local
//! change already took.
//!
//! **Loss.** A stale input is still stored, because `Hold` and `Snapshot`
//! need it. The effective contribution is computed from the age at render
//! time: held, removed immediately, or scaled down over the fade time.

use std::time::{Duration, Instant};

use dmx::{DmxFrame, DMX_SLOTS};

use crate::show::{InputPriority, InputTrigger, LossPolicy, MergeMode, Universe};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Owner {
    Local,
    Input,
}

/// One universe's input and merge state. Session only; never saved.
#[derive(Clone)]
pub struct InputMixer {
    universe: u16,
    last: Option<[u8; DMX_SLOTS]>,
    received_at: Option<Instant>,
    local_previous: Option<[u8; DMX_SLOTS]>,
    local_changed: [bool; DMX_SLOTS],
    input_changed: [bool; DMX_SLOTS],
    owner: [Owner; DMX_SLOTS],
    /// The trigger configuration last seen, its previous level and whether
    /// the slot has gone low since that configuration took effect.
    trigger_config: Option<InputTrigger>,
    trigger_level: Option<u8>,
    trigger_armed: bool,
    /// A stale frame has already been rendered under the current loss
    /// policy, so `tick` does not need to force another one.
    loss_rendered: bool,
}

impl InputMixer {
    pub fn new(universe: u16) -> Self {
        Self {
            universe,
            last: None,
            received_at: None,
            local_previous: None,
            local_changed: [false; DMX_SLOTS],
            input_changed: [false; DMX_SLOTS],
            owner: [Owner::Local; DMX_SLOTS],
            trigger_config: None,
            trigger_level: None,
            trigger_armed: false,
            loss_rendered: false,
        }
    }

    pub fn universe(&self) -> u16 {
        self.universe
    }

    /// The last raw frame, including a stale one.
    pub fn last(&self) -> Option<&[u8; DMX_SLOTS]> {
        self.last.as_ref()
    }

    pub fn received_at(&self) -> Option<Instant> {
        self.received_at
    }

    pub fn age(&self, now: Instant) -> Option<Duration> {
        self.received_at
            .map(|received| now.saturating_duration_since(received))
    }

    /// Accept a raw frame. Returns the sequence to `Go` when the trigger slot
    /// crossed its threshold.
    ///
    /// A non-zero start code is not a dimmer frame and must be filtered by the
    /// caller before this point.
    pub fn receive(&mut self, frame: &DmxFrame, config: &Universe, now: Instant) -> Option<u16> {
        let previous = self.last;
        for (index, value) in frame.slots.iter().enumerate() {
            if previous.map(|old| old[index]) != Some(*value) {
                self.input_changed[index] = true;
            }
        }
        self.last = Some(frame.slots);
        self.received_at = Some(now);
        self.loss_rendered = false;
        self.trigger(config)
    }

    fn trigger(&mut self, config: &Universe) -> Option<u16> {
        let Some(trigger) = config.input_trigger else {
            self.trigger_config = None;
            self.trigger_level = None;
            self.trigger_armed = false;
            return None;
        };
        let frame = self.last?;
        let level = frame[usize::from(trigger.address.saturating_sub(1)).min(DMX_SLOTS - 1)];
        if self.trigger_config != Some(trigger) {
            self.trigger_config = Some(trigger);
            self.trigger_level = Some(level);
            self.trigger_armed = level < trigger.threshold;
            return None;
        }
        let previous = self.trigger_level;
        self.trigger_level = Some(level);
        if level >= trigger.threshold {
            if previous.is_some_and(|value| value < trigger.threshold) && self.trigger_armed {
                self.trigger_armed = false;
                return Some(trigger.sequence);
            }
        } else {
            self.trigger_armed = true;
        }
        None
    }

    /// Forget the trigger edge state, so the next frame becomes a baseline.
    /// A configuration change calls this even when the slot is already high.
    pub fn reset_trigger(&mut self) {
        self.trigger_config = None;
        self.trigger_level = None;
        self.trigger_armed = false;
    }

    /// Merge the effective input into `local` for this universe.
    pub fn merge(&mut self, local: &mut [u8; DMX_SLOTS], config: &Universe, now: Instant) {
        self.mark_local(local);
        if config.merge == MergeMode::Off {
            // Turning the merge off releases the input's LTP ownership, so
            // turning it back on starts from the local look until the input
            // actually changes again.
            self.owner = [Owner::Local; DMX_SLOTS];
            self.local_changed = [false; DMX_SLOTS];
            self.input_changed = [false; DMX_SLOTS];
            return;
        }
        let effective = self.effective(config, now);
        if config.merge == MergeMode::Htp {
            if let Some(input) = effective {
                for (slot, value) in local.iter_mut().enumerate() {
                    *value = (*value).max(input[slot]);
                }
            }
            // HTP has no owner. Switching to LTP later should start from the
            // local look, not from an LTP owner left over before HTP.
            self.owner = [Owner::Local; DMX_SLOTS];
            self.local_changed = [false; DMX_SLOTS];
            self.input_changed = [false; DMX_SLOTS];
            return;
        }

        let Some(input) = effective else {
            // A fresh render already holds the local look; input ownership is
            // released by not writing any slot.
            self.owner = [Owner::Local; DMX_SLOTS];
            self.local_changed = [false; DMX_SLOTS];
            self.input_changed = [false; DMX_SLOTS];
            return;
        };

        for slot in 0..DMX_SLOTS {
            let local_changed = self.local_changed[slot];
            let input_changed = self.input_changed[slot];
            let winner = match (local_changed, input_changed) {
                (true, true) => match config.priority {
                    InputPriority::Local => Owner::Local,
                    InputPriority::Input => Owner::Input,
                },
                (true, false) => Owner::Local,
                (false, true) => Owner::Input,
                (false, false) => self.owner[slot],
            };
            self.owner[slot] = winner;
            if winner == Owner::Input {
                local[slot] = input[slot];
            }
        }
        self.local_changed = [false; DMX_SLOTS];
        self.input_changed = [false; DMX_SLOTS];
    }

    /// The input values that contribute right now, after the loss policy.
    fn effective(&mut self, config: &Universe, now: Instant) -> Option<[u8; DMX_SLOTS]> {
        let input = self.last?;
        let received = self.received_at?;
        let age = now.saturating_duration_since(received);
        let timeout = Duration::from_millis(config.input_timeout_ms.max(1));
        if age <= timeout {
            return Some(input);
        }
        let over = age - timeout;
        match config.loss {
            LossPolicy::Hold => Some(input),
            LossPolicy::Zero => {
                self.loss_rendered = true;
                None
            }
            LossPolicy::Fade(seconds) => {
                if !seconds.is_finite() || seconds <= 0.0 {
                    self.loss_rendered = true;
                    return None;
                }
                let remaining = 1.0 - over.as_secs_f64() / seconds;
                if remaining <= 0.0 {
                    self.loss_rendered = true;
                    return None;
                }
                let mut faded = input;
                for value in &mut faded {
                    *value = (f64::from(*value) * remaining).round() as u8;
                }
                // A loss fade is a decay, not a new operator value. It keeps
                // whichever slot the input already owned, so a local change
                // during the fade wins immediately instead of being hidden
                // until the fade completes.
                Some(faded)
            }
        }
    }

    fn mark_local(&mut self, local: &[u8; DMX_SLOTS]) {
        match self.local_previous {
            Some(previous) => {
                for (index, value) in local.iter().enumerate() {
                    if previous[index] != *value {
                        self.local_changed[index] = true;
                    }
                }
            }
            // The first local frame is the baseline, not a change: if an input
            // arrived before the first render, LTP lets it take the slots.
            None => self.local_changed = [false; DMX_SLOTS],
        }
        self.local_previous = Some(*local);
    }

    /// True while the engine must keep rendering even though no new input
    /// frame arrived: a fade is moving, or a `Zero`/finished fade has not yet
    /// been applied to a frame.
    pub fn needs_render(&self, config: &Universe, now: Instant) -> bool {
        if config.merge == MergeMode::Off || self.last.is_none() {
            return false;
        }
        let Some(received) = self.received_at else {
            return false;
        };
        let age = now.saturating_duration_since(received);
        let timeout = Duration::from_millis(config.input_timeout_ms.max(1));
        if age <= timeout {
            return false;
        }
        match config.loss {
            LossPolicy::Hold => false,
            LossPolicy::Zero => !self.loss_rendered,
            LossPolicy::Fade(seconds) => {
                if !seconds.is_finite() || seconds <= 0.0 {
                    return !self.loss_rendered;
                }
                let over = age - timeout;
                if over.as_secs_f64() < seconds {
                    true
                } else {
                    !self.loss_rendered
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::show::InputTrigger;

    fn config(mode: MergeMode) -> Universe {
        Universe {
            merge: mode,
            ..Universe::default()
        }
    }

    fn frame(slot: usize, value: u8) -> DmxFrame {
        let mut slots = [0u8; DMX_SLOTS];
        slots[slot] = value;
        DmxFrame::dimmer(slots)
    }

    #[test]
    fn htp_takes_the_max_per_slot() {
        let now = Instant::now();
        let mut mixer = InputMixer::new(1);
        let mut input = frame(0, 100);
        input.slots[1] = 10;
        mixer.receive(&input, &config(MergeMode::Htp), now);
        let mut local = [0u8; DMX_SLOTS];
        local[0] = 50;
        local[1] = 80;
        mixer.merge(&mut local, &config(MergeMode::Htp), now);
        assert_eq!(local[0], 100);
        assert_eq!(local[1], 80);
    }

    #[test]
    fn ltp_is_per_slot_and_does_not_let_one_local_change_take_the_universe() {
        let now = Instant::now();
        let config = config(MergeMode::Ltp);
        let mut mixer = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, now);

        let mut input = [0u8; DMX_SLOTS];
        input[0] = 10;
        input[1] = 20;
        input[2] = 30;
        mixer.receive(&DmxFrame::dimmer(input), &config, now);
        mixer.merge(&mut local, &config, now);
        assert_eq!(&local[..3], &[10, 20, 30], "input takes changed slots");

        local = [0u8; DMX_SLOTS];
        local[1] = 99;
        mixer.merge(&mut local, &config, now);
        assert_eq!(local[0], 10, "unchanged local slot still follows input");
        assert_eq!(local[1], 99, "the changed slot is local");
        assert_eq!(local[2], 30);
    }

    #[test]
    fn ltp_tie_follows_priority() {
        let now = Instant::now();
        let mut input = [0u8; DMX_SLOTS];
        input[0] = 10;
        let input = DmxFrame::dimmer(input);

        let local_config = config(MergeMode::Ltp);
        let mut local_first = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        local_first.merge(&mut local, &local_config, now);
        local[0] = 20;
        local_first.receive(&input, &local_config, now);
        local_first.merge(&mut local, &local_config, now);
        assert_eq!(local[0], 20, "local wins a same-frame tie by default");

        let mut input_config = config(MergeMode::Ltp);
        input_config.priority = InputPriority::Input;
        let mut input_first = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        input_first.merge(&mut local, &input_config, now);
        local[0] = 20;
        input_first.receive(&input, &input_config, now);
        input_first.merge(&mut local, &input_config, now);
        assert_eq!(local[0], 10, "input wins the same tie when configured");
    }

    #[test]
    fn ltp_keeps_the_input_owner_when_nothing_changes() {
        let now = Instant::now();
        let config = config(MergeMode::Ltp);
        let mut mixer = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, now);
        let mut input = frame(0, 200);
        input.slots[1] = 100;
        mixer.receive(&input, &config, now);
        mixer.merge(&mut local, &config, now);
        assert_eq!(&local[..2], &[200, 100]);

        // The renderer hands the mixer a fresh local frame every time; the
        // merged values from the previous frame are not the local base.
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, now);
        assert_eq!(&local[..2], &[200, 100], "unchanged input keeps the slots");
    }

    #[test]
    fn loss_policies_change_the_contribution_at_the_timeout() {
        let start = Instant::now();
        let mut input = frame(0, 200);
        input.slots[1] = 100;
        let mut local = [0u8; DMX_SLOTS];
        local[0] = 10;
        local[1] = 10;

        let mut hold = InputMixer::new(1);
        hold.receive(&input, &config(MergeMode::Htp), start);
        let later = start + Duration::from_millis(1_500);
        hold.merge(&mut local, &config(MergeMode::Htp), later);
        assert_eq!(local[0], 200, "hold keeps the last frame");

        let mut zero_config = config(MergeMode::Htp);
        zero_config.loss = LossPolicy::Zero;
        let mut zero = InputMixer::new(1);
        zero.receive(&input, &zero_config, start);
        let mut zero_local = [0u8; DMX_SLOTS];
        zero_local[0] = 10;
        zero.merge(&mut zero_local, &zero_config, later);
        assert_eq!(zero_local[0], 10, "zero removes the input at the timeout");
    }

    #[test]
    fn fade_scales_then_removes_the_input() {
        let start = Instant::now();
        let input = frame(0, 200);
        let mut config = config(MergeMode::Htp);
        config.loss = LossPolicy::Fade(2.0);
        let mut mixer = InputMixer::new(1);
        mixer.receive(&input, &config, start);

        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start + Duration::from_millis(500));
        assert_eq!(local[0], 200, "still fresh before the timeout");
        local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start + Duration::from_millis(1_500));
        assert!(
            (145..=155).contains(&local[0]),
            "quarter way through the fade"
        );
        local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start + Duration::from_millis(3_500));
        assert_eq!(local[0], 0, "the fade is complete");
    }

    #[test]
    fn a_fade_requests_renders_only_while_it_moves() {
        let start = Instant::now();
        let mut config = config(MergeMode::Htp);
        config.loss = LossPolicy::Fade(1.0);
        let mut mixer = InputMixer::new(1);
        mixer.receive(&frame(0, 200), &config, start);
        assert!(!mixer.needs_render(&config, start + Duration::from_millis(500)));
        assert!(mixer.needs_render(&config, start + Duration::from_millis(1_500)));
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start + Duration::from_millis(1_500));
        assert!(mixer.needs_render(&config, start + Duration::from_millis(1_900)));
        assert!(
            mixer.needs_render(&config, start + Duration::from_millis(2_100)),
            "the completed fade still needs one final render"
        );
        mixer.merge(&mut local, &config, start + Duration::from_millis(2_100));
        assert!(!mixer.needs_render(&config, start + Duration::from_millis(2_500)));
    }

    #[test]
    fn zero_needs_one_render_after_the_timeout() {
        let start = Instant::now();
        let mut config = config(MergeMode::Htp);
        config.loss = LossPolicy::Zero;
        let mut mixer = InputMixer::new(1);
        mixer.receive(&frame(0, 200), &config, start);
        assert!(!mixer.needs_render(&config, start + Duration::from_millis(500)));
        let later = start + Duration::from_millis(1_500);
        assert!(mixer.needs_render(&config, later));
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, later);
        assert!(!mixer.needs_render(&config, later));
    }

    #[test]
    fn a_local_change_wins_during_an_ltp_loss_fade() {
        let start = Instant::now();
        let mut config = config(MergeMode::Ltp);
        config.loss = LossPolicy::Fade(1.0);
        let mut mixer = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start);
        mixer.receive(&frame(0, 200), &config, start);
        mixer.merge(&mut local, &config, start);
        assert_eq!(local[0], 200, "the input owns the slot");

        local[0] = 50;
        mixer.merge(&mut local, &config, start + Duration::from_millis(1_500));
        assert_eq!(
            local[0], 50,
            "a local change is not hidden behind a fading input"
        );
        mixer.merge(&mut local, &config, start + Duration::from_millis(2_500));
        assert_eq!(local[0], 50, "the local look remains after the fade");
    }

    #[test]
    fn a_zero_loss_releases_ltp_ownership() {
        let start = Instant::now();
        let mut config = config(MergeMode::Ltp);
        config.loss = LossPolicy::Zero;
        let mut mixer = InputMixer::new(1);
        let mut local = [0u8; DMX_SLOTS];
        mixer.merge(&mut local, &config, start);
        let mut input = frame(0, 200);
        input.slots[1] = 100;
        mixer.receive(&input, &config, start);
        mixer.merge(&mut local, &config, start);
        assert_eq!(&local[..2], &[200, 100]);
        local[0] = 5;
        local[1] = 6;
        mixer.merge(&mut local, &config, start + Duration::from_millis(1_500));
        assert_eq!(&local[..2], &[5, 6], "local look returns after signal loss");
    }

    #[test]
    fn trigger_fires_once_on_a_rising_edge_and_rearms() {
        let now = Instant::now();
        let mut config = config(MergeMode::Off);
        config.input_trigger = Some(InputTrigger {
            address: 11,
            threshold: 128,
            sequence: 2,
        });
        let mut mixer = InputMixer::new(1);
        assert_eq!(mixer.receive(&frame(10, 0), &config, now), None);
        assert_eq!(mixer.receive(&frame(10, 200), &config, now), Some(2));
        assert_eq!(
            mixer.receive(&frame(10, 220), &config, now),
            None,
            "holding the trigger does not refire"
        );
        assert_eq!(mixer.receive(&frame(10, 20), &config, now), None);
        assert_eq!(mixer.receive(&frame(10, 200), &config, now), Some(2));
    }

    #[test]
    fn a_changed_threshold_rebaselines_a_high_trigger_slot() {
        let now = Instant::now();
        let mut config = config(MergeMode::Off);
        config.input_trigger = Some(InputTrigger {
            address: 1,
            threshold: 100,
            sequence: 2,
        });
        let mut mixer = InputMixer::new(1);
        assert_eq!(mixer.receive(&frame(0, 0), &config, now), None);
        assert_eq!(mixer.receive(&frame(0, 150), &config, now), Some(2));

        config.input_trigger = Some(InputTrigger {
            address: 1,
            threshold: 200,
            sequence: 2,
        });
        assert_eq!(
            mixer.receive(&frame(0, 250), &config, now),
            None,
            "a new threshold starts from the current level"
        );
        assert_eq!(mixer.receive(&frame(0, 250), &config, now), None);
        assert_eq!(mixer.receive(&frame(0, 0), &config, now), None);
        assert_eq!(mixer.receive(&frame(0, 250), &config, now), Some(2));
    }

    #[test]
    fn a_different_trigger_slot_starting_high_does_not_fire() {
        let now = Instant::now();
        let mut config = config(MergeMode::Off);
        config.input_trigger = Some(InputTrigger {
            address: 1,
            threshold: 10,
            sequence: 2,
        });
        let mut mixer = InputMixer::new(1);
        assert_eq!(mixer.receive(&frame(0, 255), &config, now), None);
        assert_eq!(mixer.receive(&frame(0, 255), &config, now), None);
    }
}
