//! Timecode shows and the running timecode player.
//!
//! A timecode show is a numbered set of tracks. Each track is a list of events;
//! an event names a frame position (`HH:MM:SS:FF` at the show's frame rate) and
//! an action: start a cue, or turn an effect on/off. Markers are labels on the
//! same timeline and have no behaviour.
//!
//! The show data is saved; the player is not. Playing, pausing, locating and
//! stopping are console state, like a running cue: loading a show starts with
//! every timecode stopped, exactly where Phase 1 put playback.
//!
//! Time is expressed in **frames** so a position is exact and can be compared.
//! Wall-clock time only enters through [`TimecodePlayer::advance`] or
//! [`TimecodePlayer::follow_external`], which the engine tick calls with an
//! explicit instant, so tests never sleep.

use std::time::Instant;

use serde::{Deserialize, Deserializer, Serialize};

/// Frame rate used when a timecode show does not name one (the EBU default).
pub const DEFAULT_FPS: u8 = 25;
pub const MIN_FPS: u8 = 1;
pub const MAX_FPS: u8 = 60;
pub const MAX_TIMECODES: usize = 1024;
pub const MAX_TIMECODE_TRACKS: usize = 256;
pub const MAX_TIMECODE_EVENTS: usize = 65_536;

/// How a running timecode gets its position.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(tag = "kind", rename_all = "lowercase")]
#[non_exhaustive]
pub enum TimecodeSource {
    /// The engine's own clock, unchanged from Phase 4.
    #[default]
    Internal,
    /// MIDI Time Code on a serial port at 31 250 8N1.
    Mtc { port: String },
    /// Positions from the OSC listener (item 12's transport).
    Osc,
}

impl TimecodeSource {
    pub fn label(&self) -> String {
        match self {
            Self::Internal => "Internal".to_string(),
            Self::Mtc { port } => format!("Mtc \"{port}\""),
            Self::Osc => "Osc".to_string(),
        }
    }

    pub fn is_external(&self) -> bool {
        !matches!(self, Self::Internal)
    }
}

/// A label on the timeline. Markers have no behaviour.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Marker {
    pub at: String,
    #[serde(default)]
    pub name: String,
}

/// One timeline. Track 1 is the default for `Timecode <n> Add …`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Track {
    /// 1-based and unique within the timecode show.
    pub number: u8,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub events: Vec<Event>,
}

impl Track {
    pub fn display_name(&self) -> String {
        if self.name.trim().is_empty() {
            format!("Track {}", self.number)
        } else {
            format!("Track {} \"{}\"", self.number, self.name)
        }
    }
}

/// What an event does when the playhead reaches it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "action", rename_all = "lowercase")]
pub enum Action {
    Cue {
        sequence: u16,
        cue: f64,
    },
    Effect {
        effect: u16,
        /// `None` means the desk selection at fire time.
        #[serde(default)]
        fixtures: Option<Vec<u16>>,
        on: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Event {
    /// `HH:MM:SS:FF` at the show's frame rate.
    pub at: String,
    pub action: Action,
}

/// The persisted shape, which also accepts the v11 flat `events` list.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimecodeWire {
    number: u16,
    #[serde(default)]
    name: String,
    #[serde(default = "default_fps")]
    fps: u8,
    #[serde(default)]
    source: TimecodeSource,
    #[serde(default)]
    tracks: Option<Vec<Track>>,
    #[serde(default)]
    markers: Vec<Marker>,
    #[serde(default)]
    events: Option<Vec<LegacyEvent>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyEvent {
    at: String,
    sequence: u16,
    cue: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TimecodeShow {
    pub number: u16,
    pub name: String,
    /// Frames per second used to read and write this show's positions.
    pub fps: u8,
    pub source: TimecodeSource,
    /// Kept sorted by track number; events are kept sorted within each track.
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub markers: Vec<Marker>,
}

impl<'de> Deserialize<'de> for TimecodeShow {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let wire = TimecodeWire::deserialize(deserializer)?;
        // v11 stored one flat `events` list. It becomes track 1 so every old
        // script keeps addressing the same timeline through the default track.
        let tracks = match wire.tracks {
            Some(tracks) => tracks,
            None => vec![Track {
                number: 1,
                name: "Cues".into(),
                events: wire
                    .events
                    .unwrap_or_default()
                    .into_iter()
                    .map(|event| Event {
                        at: event.at,
                        action: Action::Cue {
                            sequence: event.sequence,
                            cue: event.cue,
                        },
                    })
                    .collect(),
            }],
        };
        Ok(Self {
            number: wire.number,
            name: wire.name,
            fps: wire.fps,
            source: wire.source,
            tracks,
            markers: wire.markers,
        })
    }
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
            source: TimecodeSource::Internal,
            tracks: Vec::new(),
            markers: Vec::new(),
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

    pub fn track(&self, number: u8) -> Option<&Track> {
        self.tracks.iter().find(|track| track.number == number)
    }

    pub fn track_mut(&mut self, number: u8) -> Option<&mut Track> {
        self.tracks.iter_mut().find(|track| track.number == number)
    }

    /// Get a track, creating it with a default name when it does not exist.
    pub fn ensure_track(&mut self, number: u8) -> &mut Track {
        if let Some(index) = self.tracks.iter().position(|track| track.number == number) {
            return &mut self.tracks[index];
        }
        self.tracks.push(Track {
            number,
            name: if number == 1 {
                "Cues".into()
            } else {
                format!("Track {number}")
            },
            events: Vec::new(),
        });
        self.tracks.sort_by_key(|track| track.number);
        let index = self
            .tracks
            .iter()
            .position(|track| track.number == number)
            .expect("just inserted");
        &mut self.tracks[index]
    }

    pub fn event_count(&self) -> usize {
        self.tracks.iter().map(|track| track.events.len()).sum()
    }

    pub fn position(&self, event: &Event) -> Result<u64, String> {
        parse(&event.at, self.frame_rate())
    }

    /// Events in track order. The caller has already checked the cursor.
    fn events_at(&self, current: u64, cursor: u64) -> Vec<Event> {
        let mut tracks: Vec<&Track> = self.tracks.iter().collect();
        tracks.sort_by_key(|track| track.number);
        let mut fired = Vec::new();
        for track in tracks {
            for event in &track.events {
                let Ok(at) = self.position(event) else {
                    continue;
                };
                if at > current {
                    break;
                }
                if at < cursor {
                    continue;
                }
                fired.push(event.clone());
            }
        }
        fired
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

/// Which clock a player is following.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimecodeMode {
    Internal,
    External,
}

/// What is playing: which timecode show, where the playhead is, and whether it
/// is moving. The player survives commands; it is not part of the show file.
#[derive(Debug)]
pub struct TimecodePlayer {
    show: Option<u16>,
    running: bool,
    paused: bool,
    mode: TimecodeMode,
    /// Exact playhead in frames at the active show's rate. It is fractional so
    /// repeated advances do not lose part-frames to rounding.
    position: f64,
    /// First event frame that has not fired yet.
    cursor: u64,
    /// Wall clock at which `position` was current.
    anchor: Instant,
    /// Last accepted external position, if one has arrived.
    last_external: Option<u64>,
    /// When the last external position arrived.
    last_signal: Option<Instant>,
    stalled: bool,
    source_error: Option<String>,
    notice: Option<String>,
}

impl Default for TimecodePlayer {
    fn default() -> Self {
        Self {
            show: None,
            running: false,
            paused: false,
            mode: TimecodeMode::Internal,
            position: 0.0,
            cursor: 0,
            anchor: Instant::now(),
            last_external: None,
            last_signal: None,
            stalled: false,
            source_error: None,
            notice: None,
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

    pub fn is_external(&self) -> bool {
        self.mode == TimecodeMode::External
    }

    pub fn is_stalled(&self) -> bool {
        self.stalled
    }

    pub fn source_error(&self) -> Option<&str> {
        self.source_error.as_deref()
    }

    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    pub fn position(&self) -> u64 {
        self.position.floor().max(0.0) as u64
    }

    /// Start (or restart) a show on the internal clock at `from`.
    pub fn start(&mut self, show: u16, from: u64, now: Instant) {
        self.show = Some(show);
        self.running = true;
        self.paused = false;
        self.mode = TimecodeMode::Internal;
        self.position = from as f64;
        self.cursor = from;
        self.anchor = now;
        self.last_external = None;
        self.last_signal = None;
        self.stalled = false;
        self.source_error = None;
        self.notice = None;
    }

    /// Arm an external source. The first position after arming is a locate:
    /// it moves the playhead and fires nothing.
    pub fn start_external(&mut self, show: u16, from: u64, now: Instant) {
        self.start(show, from, now);
        self.mode = TimecodeMode::External;
        self.last_signal = Some(now);
    }

    /// The source failed to open or deliver a usable position. The playhead
    /// stops following; it never falls back to the internal clock.
    pub fn fail_source(&mut self, message: impl Into<String>) {
        self.running = false;
        self.paused = false;
        self.stalled = false;
        self.last_signal = None;
        self.source_error = Some(message.into());
    }

    pub fn set_notice(&mut self, message: impl Into<String>) {
        self.notice = Some(message.into());
    }

    pub fn mark_stalled(&mut self) {
        if self.running && self.is_external() {
            self.stalled = true;
        }
    }

    pub fn signal_age(&self, now: Instant) -> Option<std::time::Duration> {
        self.last_signal
            .map(|last| now.saturating_duration_since(last))
    }

    /// Exact playhead in frames, including the part-frame the clock has reached.
    fn exact_at(&self, timecode: &TimecodeShow, now: Instant) -> f64 {
        if !self.running {
            return self.position;
        }
        if self.mode == TimecodeMode::External && (self.stalled || self.last_external.is_none()) {
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
        self.mode = TimecodeMode::Internal;
        self.position = 0.0;
        self.cursor = 0;
        self.last_external = None;
        self.last_signal = None;
        self.stalled = false;
        self.source_error = None;
        self.notice = None;
    }

    /// Move the playhead. Points at or before the new position never fire, so a
    /// locate is not a burst of cues.
    pub fn locate(&mut self, show: u16, frames: u64, now: Instant) {
        self.show = Some(show);
        self.position = frames as f64;
        self.cursor = frames.saturating_add(1);
        self.anchor = now;
        self.last_external = Some(frames);
        self.stalled = false;
        self.last_signal = Some(now);
    }

    fn events_between(&mut self, timecode: &TimecodeShow, current: u64) -> Vec<Event> {
        let fired = timecode.events_at(current, self.cursor);
        if current >= self.cursor {
            self.cursor = current.saturating_add(1);
        }
        fired
    }

    /// Advance the internal clock to `now` and return the events the playhead
    /// passed, in track order.
    pub fn advance(&mut self, timecode: &TimecodeShow, now: Instant) -> Vec<Event> {
        if !self.running || self.mode == TimecodeMode::External {
            return Vec::new();
        }
        let exact = self.exact_at(timecode, now);
        let current = exact.floor().max(0.0) as u64;
        let fired = self.events_between(timecode, current);
        self.position = exact;
        self.anchor = now;
        fired
    }

    /// Follow one external position. Within ±2 frames of the expected position
    /// it is the clock and events between the cursor and it fire; further away
    /// it is a locate and nothing fires.
    pub fn follow_external(
        &mut self,
        timecode: &TimecodeShow,
        frames: u64,
        now: Instant,
    ) -> Vec<Event> {
        if !self.running || self.mode != TimecodeMode::External {
            return Vec::new();
        }
        let expected = if self.last_external.is_some() {
            Some(self.exact_at(timecode, now).floor().max(0.0) as u64)
        } else {
            None
        };
        self.last_signal = Some(now);
        self.stalled = false;
        self.last_external = Some(frames);
        let Some(expected) = expected else {
            // The first position after arming is a locate.
            self.position = frames as f64;
            self.cursor = frames.saturating_add(1);
            self.anchor = now;
            return Vec::new();
        };
        if frames.abs_diff(expected) > 2 {
            self.position = frames as f64;
            self.cursor = frames.saturating_add(1);
            self.anchor = now;
            return Vec::new();
        }
        let fired = self.events_between(timecode, frames);
        self.position = frames as f64;
        self.anchor = now;
        fired
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn action(cue: f64) -> Action {
        Action::Cue { sequence: 1, cue }
    }

    fn event(at: &str, cue: f64) -> Event {
        Event {
            at: at.to_string(),
            action: action(cue),
        }
    }

    fn show() -> TimecodeShow {
        let mut show = TimecodeShow::new(1);
        show.name = "Song".into();
        show.fps = 25;
        show.tracks.push(Track {
            number: 1,
            name: "Cues".into(),
            events: vec![event("00:00:00:00", 1.0), event("00:00:01:00", 2.0)],
        });
        show
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
    fn tracks_fire_independently_and_in_track_order() {
        let mut timecode = show();
        timecode.tracks.push(Track {
            number: 2,
            name: "FX".into(),
            events: vec![event("00:00:01:00", 9.0)],
        });
        let start = Instant::now();
        let mut player = TimecodePlayer::default();
        player.start(1, 0, start);
        let fired = player.advance(&timecode, start + Duration::from_secs(1));
        assert_eq!(fired.len(), 3, "track 1 twice plus track 2");
        assert_eq!(fired[1].action, action(2.0));
        assert_eq!(fired[2].action, action(9.0), "track 2 fires after track 1");
    }

    #[test]
    fn external_positions_locate_first_then_follow_within_two_frames() {
        let timecode = show();
        let start = Instant::now();
        let mut player = TimecodePlayer::default();
        player.start_external(1, 0, start);

        assert!(
            player.follow_external(&timecode, 0, start).is_empty(),
            "the first position after arming is a locate"
        );
        assert!(player
            .follow_external(&timecode, 1, start + Duration::from_millis(40))
            .is_empty());
        let fired = player.follow_external(&timecode, 25, start + Duration::from_secs(1));
        assert_eq!(fired, vec![event("00:00:01:00", 2.0)]);
        assert_eq!(player.position(), 25);
    }

    #[test]
    fn a_jump_larger_than_two_frames_locates_and_fires_nothing() {
        let timecode = show();
        let start = Instant::now();
        let mut player = TimecodePlayer::default();
        player.start_external(1, 0, start);
        let _ = player.follow_external(&timecode, 0, start);
        let fired = player.follow_external(&timecode, 10, start + Duration::from_millis(40));
        assert!(fired.is_empty());
        assert_eq!(player.position(), 10);
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
