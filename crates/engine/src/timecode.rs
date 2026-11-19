//! Timecode shows and the running timecode player.
//!
//! A timecode show is a numbered list of events. Each event names a frame
//! position (`HH:MM:SS:FF` at the show's frame rate) and the cue it starts, so
//! the cue list follows a track instead of an operator pressing `Go`.
//!
//! The show data is saved; the player is not. Playing, pausing, locating and
//! stopping are console state, like a running cue: loading a show starts with
//! every timecode stopped, exactly where Phase 1 put playback.
//!
//! Time is expressed in **frames** so a position is exact and can be compared.
//! Wall-clock time only enters through [`TimecodePlayer::advance`], which the
//! engine tick calls with an explicit instant, so tests never sleep.

use std::time::Instant;

use serde::{Deserialize, Serialize};

/// Frame rate used when a timecode show does not name one (the EBU default).
pub const DEFAULT_FPS: u8 = 25;
pub const MIN_FPS: u8 = 1;
pub const MAX_FPS: u8 = 60;
pub const MAX_TIMECODES: usize = 1024;
pub const MAX_TIMECODE_EVENTS: usize = 65_536;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimecodeShow {
    pub number: u16,
    #[serde(default)]
    pub name: String,
    /// Frames per second used to read and write this show's positions.
    #[serde(default = "default_fps")]
    pub fps: u8,
    /// Kept sorted by position; validated on save and load.
    #[serde(default)]
    pub events: Vec<TimecodeEvent>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TimecodeEvent {
    /// `HH:MM:SS:FF` at the show's frame rate.
    pub at: String,
    /// Sequence whose cue is started when the playhead reaches `at`.
    pub sequence: u16,
    pub cue: f64,
}

const fn default_fps() -> u8 {
    DEFAULT_FPS
}

impl TimecodeShow {
    pub fn new(number: u16) -> Self {
        Self {
            number,
            name: format!("Timecode {number}"),
            fps: DEFAULT_FPS,
            events: Vec::new(),
        }
    }

    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Timecode {}", self.number)
        } else {
            format!("Timecode {} \"{}\"", self.number, self.name)
        }
    }

    /// Frame rate, never zero, so parsing and formatting cannot divide by it.
    pub fn frame_rate(&self) -> u8 {
        self.fps.max(MIN_FPS)
    }

    /// Frames elapsed at `seconds` into the show.
    pub fn frames_at(&self, seconds: f64) -> u64 {
        if !seconds.is_finite() || seconds <= 0.0 {
            return 0;
        }
        (seconds * f64::from(self.frame_rate())).floor() as u64
    }

    pub fn position(&self, event: &TimecodeEvent) -> Result<u64, String> {
        parse(&event.at, self.frame_rate())
    }
}

/// Parse `HH:MM:SS:FF` into a frame count. The frame field must be inside the
/// show's frame rate, so `00:00:00:25` at 25 fps is rejected.
pub fn parse(text: &str, fps: u8) -> Result<u64, String> {
    let text = text.trim();
    let parts: Vec<&str> = text.split(':').collect();
    if parts.len() != 4 {
        return Err(format!("`{text}` is not a timecode (use `HH:MM:SS:FF`)"));
    }
    let mut values = [0u64; 4];
    for (slot, part) in parts.iter().enumerate() {
        if part.is_empty() || !part.chars().all(|character| character.is_ascii_digit()) {
            return Err(format!("`{text}` is not a timecode (use `HH:MM:SS:FF`)"));
        }
        values[slot] = part
            .parse()
            .map_err(|_| format!("`{part}` is not a timecode field"))?;
    }
    let [hours, minutes, seconds, frames] = values;
    if hours > 23 {
        return Err(format!("hours {hours} are outside 0..=23"));
    }
    if minutes > 59 {
        return Err(format!("minutes {minutes} are outside 0..=59"));
    }
    if seconds > 59 {
        return Err(format!("seconds {seconds} are outside 0..=59"));
    }
    let fps = u64::from(fps.max(MIN_FPS));
    if frames >= fps {
        return Err(format!(
            "frame {frames} is outside 0..={} at {fps} fps",
            fps - 1
        ));
    }
    Ok((hours * 3600 + minutes * 60 + seconds) * fps + frames)
}

/// Format a frame count as `HH:MM:SS:FF`.
pub fn format(frames: u64, fps: u8) -> String {
    let fps = u64::from(fps.max(MIN_FPS));
    let total_seconds = frames / fps;
    format!(
        "{:02}:{:02}:{:02}:{:02}",
        total_seconds / 3600,
        (total_seconds / 60) % 60,
        total_seconds % 60,
        frames % fps
    )
}

/// What is playing: which timecode show, where the playhead is, and whether it
/// is moving. The player survives commands; it is not part of the show file.
#[derive(Debug)]
pub struct TimecodePlayer {
    show: Option<u16>,
    running: bool,
    paused: bool,
    /// Exact playhead in frames at the active show's rate. It is fractional so
    /// repeated advances do not lose part-frames to rounding.
    position: f64,
    /// First event frame that has not fired yet.
    cursor: u64,
    /// Wall clock at which `position` was current.
    anchor: Instant,
}

impl Default for TimecodePlayer {
    fn default() -> Self {
        Self {
            show: None,
            running: false,
            paused: false,
            position: 0.0,
            cursor: 0,
            anchor: Instant::now(),
        }
    }
}

impl TimecodePlayer {
    pub fn active(&self) -> Option<u16> {
        self.show
    }

    pub fn is_running(&self) -> bool {
        self.running
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    pub fn position(&self) -> u64 {
        self.position.floor() as u64
    }

    /// Start (or restart) a show at `from`. Events at exactly `from` fire on
    /// the next advance, so starting a show whose first event is at frame zero
    /// does not wait a tick.
    pub fn start(&mut self, show: u16, from: u64, now: Instant) {
        self.show = Some(show);
        self.running = true;
        self.paused = false;
        self.position = from as f64;
        self.cursor = from;
        self.anchor = now;
    }

    /// Exact playhead in frames, including the part-frame the clock has reached.
    fn exact_at(&self, timecode: &TimecodeShow, now: Instant) -> f64 {
        if !self.running {
            return self.position;
        }
        let elapsed = now.saturating_duration_since(self.anchor).as_secs_f64();
        self.position + elapsed * f64::from(timecode.frame_rate())
    }

    /// Where the playhead is now, without changing anything.
    pub fn position_at(&self, timecode: &TimecodeShow, now: Instant) -> u64 {
        self.exact_at(timecode, now).floor().max(0.0) as u64
    }

    /// Freeze the playhead. Returns false when it was not running.
    pub fn pause(&mut self, timecode: &TimecodeShow, now: Instant) -> bool {
        if !self.running {
            return false;
        }
        self.position = self.exact_at(timecode, now);
        self.anchor = now;
        self.running = false;
        self.paused = true;
        true
    }

    /// Stop and rewind to the start. Locating afterwards is still possible.
    pub fn stop(&mut self) {
        self.running = false;
        self.paused = false;
        self.position = 0.0;
        self.cursor = 0;
    }

    /// Move the playhead. Points at or before the new position never fire, so a
    /// locate is not a burst of cues.
    pub fn locate(&mut self, show: u16, frames: u64, now: Instant) {
        self.show = Some(show);
        self.position = frames as f64;
        self.cursor = frames.saturating_add(1);
        self.anchor = now;
    }

    /// Advance to `now` and return the events the playhead passed, in order.
    pub fn advance(&mut self, timecode: &TimecodeShow, now: Instant) -> Vec<TimecodeEvent> {
        if !self.running {
            return Vec::new();
        }
        let exact = self.exact_at(timecode, now);
        let current = exact.floor().max(0.0) as u64;
        let mut fired = Vec::new();
        for event in &timecode.events {
            let Ok(at) = timecode.position(event) else {
                continue;
            };
            if at > current {
                break;
            }
            if at < self.cursor {
                continue;
            }
            fired.push(event.clone());
            self.cursor = at.saturating_add(1);
        }
        self.position = exact;
        self.anchor = now;
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn event(at: &str, cue: f64) -> TimecodeEvent {
        TimecodeEvent {
            at: at.to_string(),
            sequence: 1,
            cue,
        }
    }

    fn show() -> TimecodeShow {
        TimecodeShow {
            number: 1,
            name: "Song".into(),
            fps: 25,
            events: vec![event("00:00:00:00", 1.0), event("00:00:01:00", 2.0)],
        }
    }

    #[test]
    fn positions_round_trip_at_any_rate() {
        assert_eq!(parse("00:00:00:00", 25), Ok(0));
        assert_eq!(parse("00:00:01:00", 25), Ok(25));
        assert_eq!(parse("01:02:03:04", 25), Ok(3_723 * 25 + 4));
        assert_eq!(format(3_723 * 25 + 4, 25), "01:02:03:04");
        assert_eq!(format(29, 30), "00:00:00:29");
        assert_eq!(format(30, 30), "00:00:01:00");
    }

    #[test]
    fn bad_positions_are_rejected_with_the_reason() {
        assert!(parse("00:00:01", 25).unwrap_err().contains("HH:MM:SS:FF"));
        assert!(parse("00:00:01:25", 25).unwrap_err().contains("frame 25"));
        assert!(parse("24:00:00:00", 25).unwrap_err().contains("hours"));
        assert!(parse("00:60:00:00", 25).unwrap_err().contains("minutes"));
        assert!(parse("00:00:xx:00", 25).is_err());
    }

    #[test]
    fn advancing_fires_each_event_once_and_in_order() {
        let timecode = show();
        let start = Instant::now();
        let mut player = TimecodePlayer::default();
        player.start(1, 0, start);

        assert_eq!(player.advance(&timecode, start).len(), 1, "event at 0");
        assert!(player
            .advance(&timecode, start + Duration::from_millis(500))
            .is_empty());
        assert_eq!(
            player.position_at(&timecode, start + Duration::from_millis(500)),
            12
        );

        let fired = player.advance(&timecode, start + Duration::from_secs(1));
        assert_eq!(fired, vec![event("00:00:01:00", 2.0)]);
        assert!(
            player
                .advance(&timecode, start + Duration::from_secs(5))
                .is_empty(),
            "events fire once"
        );
    }

    #[test]
    fn pause_stops_the_clock_and_locate_skips_earlier_events() {
        let timecode = show();
        let start = Instant::now();
        let mut player = TimecodePlayer::default();
        player.start(1, 0, start);
        let _ = player.advance(&timecode, start);
        assert!(player.pause(&timecode, start + Duration::from_millis(600)));
        assert!(!player.is_running());
        assert!(player.is_paused());
        assert_eq!(
            player.position_at(&timecode, start + Duration::from_secs(9)),
            15
        );
        assert!(!player.pause(&timecode, start));

        player.start(1, 0, start);
        player.locate(1, 25, start);
        assert!(
            player
                .advance(&timecode, start + Duration::from_millis(10))
                .is_empty(),
            "locating onto an event does not fire it"
        );
        player.stop();
        assert_eq!(player.position(), 0);
        assert!(!player.is_paused());
    }

    #[test]
    fn frames_at_rounds_down() {
        let timecode = show();
        assert_eq!(timecode.frames_at(0.0), 0);
        assert_eq!(timecode.frames_at(0.04), 1);
        assert_eq!(timecode.frames_at(1.0), 25);
        assert_eq!(timecode.frames_at(f64::NAN), 0);
    }
}
