//! MIDI Time Code decoding and the serial reader thread.
//!
//! MTC is a stream of system real-time messages. Quarter-frame messages
//! (`0xF1`) carry one nibble each and cycle through eight message types; a
//! full-frame SysEx (`0xF0 0x7F … 0x01 0x01`) carries a whole position at once.
//! The decoder is byte-at-a-time so a torn or interleaved message is ignored
//! rather than interpreted as a wild jump.
//!
//! The reader owns its serial port and thread. It posts decoded positions to
//! the engine's bounded channel; the engine drains them on its own tick, so
//! neither side holds the other's lock.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TrySendError};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

/// The four frame rates in the MTC rate bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MtcRate {
    Fps24,
    Fps25,
    Fps30Drop,
    Fps30,
}

impl MtcRate {
    pub fn fps(self) -> u8 {
        match self {
            Self::Fps24 => 24,
            Self::Fps25 => 25,
            Self::Fps30Drop | Self::Fps30 => 30,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Fps24 => "24 fps",
            Self::Fps25 => "25 fps",
            Self::Fps30Drop => "29.97 fps drop-frame",
            Self::Fps30 => "30 fps",
        }
    }

    fn from_bits(bits: u8) -> Self {
        match bits & 0x3 {
            0 => Self::Fps24,
            1 => Self::Fps25,
            2 => Self::Fps30Drop,
            _ => Self::Fps30,
        }
    }
}

/// One decoded position. `frames` is an absolute count at the rate's nominal
/// frame rate (30 for drop-frame, where the label is converted with the drop
/// rule).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MtcPosition {
    pub rate: MtcRate,
    pub frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MtcMessage {
    Position(MtcPosition),
    Warning(String),
    /// The port could not be read; the reader has stopped.
    Unavailable(String),
}

/// Convert a drop-frame label to an absolute frame count at 30 fps.
pub fn drop_frame_to_frames(hours: u64, minutes: u64, seconds: u64, frames: u64) -> u64 {
    let total_minutes = hours * 60 + minutes;
    let dropped = 2 * (total_minutes - total_minutes / 10);
    (hours * 3600 + minutes * 60 + seconds) * 30 + frames - dropped
}

/// Convert an absolute frame count back to a drop-frame label.
pub fn frames_to_drop_frame(frames: u64) -> (u64, u64, u64, u64) {
    let mut minutes = frames / 1800;
    loop {
        let dropped = 2 * (minutes - minutes / 10);
        let start = minutes * 1800
            + if minutes == 0 || minutes.is_multiple_of(10) {
                0
            } else {
                2
            }
            - dropped;
        let next_minutes = minutes + 1;
        let next_dropped = 2 * (next_minutes - next_minutes / 10);
        let next = next_minutes * 1800
            + if next_minutes.is_multiple_of(10) {
                0
            } else {
                2
            }
            - next_dropped;
        if frames < start {
            minutes = minutes.saturating_sub(1);
            continue;
        }
        if frames >= next {
            minutes = next_minutes;
            continue;
        }
        let label = frames + dropped;
        return (
            label / 108_000,
            (label / 1800) % 60,
            (label % 1800) / 30,
            label % 30,
        );
    }
}

fn position(rate: MtcRate, hours: u64, minutes: u64, seconds: u64, frames: u64) -> Option<u64> {
    if hours > 23 || minutes > 59 || seconds > 59 {
        return None;
    }
    let fps = u64::from(rate.fps());
    if frames >= fps {
        return None;
    }
    if rate == MtcRate::Fps30Drop && !minutes.is_multiple_of(10) && frames < 2 && seconds == 0 {
        return None;
    }
    Some(match rate {
        MtcRate::Fps30Drop => drop_frame_to_frames(hours, minutes, seconds, frames),
        _ => (hours * 3600 + minutes * 60 + seconds) * fps + frames,
    })
}

/// Byte-at-a-time MTC decoder. Feed it serial bytes; it returns messages only
/// when a complete, consistent position has arrived.
#[derive(Default)]
pub struct MtcDecoder {
    quarter: [u8; 8],
    quarter_next: u8,
    quarter_pending: bool,
    sysex: Option<Vec<u8>>,
    song: Option<u8>,
    song_bytes: u8,
}

impl MtcDecoder {
    pub fn push(&mut self, byte: u8) -> Vec<MtcMessage> {
        let mut messages = Vec::new();
        match byte {
            0xF0 => {
                self.quarter_pending = false;
                self.song = None;
                self.sysex = Some(vec![byte]);
                return messages;
            }
            0xF1 => {
                self.quarter_pending = true;
                return messages;
            }
            0xF2 => {
                self.quarter_pending = false;
                self.song = Some(byte);
                self.song_bytes = 0;
                return messages;
            }
            0xF7 => {
                if let Some(mut sysex) = self.sysex.take() {
                    sysex.push(0xF7);
                    if let Some(message) = decode_full_frame(&sysex) {
                        messages.push(message);
                    }
                }
                self.quarter_pending = false;
                self.song = None;
                return messages;
            }
            _ => {}
        }

        if let Some(sysex) = self.sysex.as_mut() {
            if sysex.len() < 256 {
                sysex.push(byte);
            } else {
                self.sysex = None;
            }
            return messages;
        }
        if self.song.is_some() {
            self.song_bytes += 1;
            if self.song_bytes == 2 {
                self.song = None;
                messages.push(MtcMessage::Warning(
                    "MTC song position is not supported; ignored".into(),
                ));
            }
            return messages;
        }
        if !self.quarter_pending {
            return messages;
        }
        self.quarter_pending = false;
        let kind = byte >> 4;
        let value = byte & 0x0F;
        if kind == 0 {
            self.quarter = [0; 8];
            self.quarter_next = 0;
        }
        if kind != self.quarter_next {
            // A missing or out-of-order nibble means a torn message; start
            // again at the next zero rather than assembling a wrong position.
            self.quarter_next = 0;
            return messages;
        }
        self.quarter[usize::from(kind)] = value;
        self.quarter_next += 1;
        if self.quarter_next == 8 {
            self.quarter_next = 0;
            let q = self.quarter;
            let frames = (q[0] & 0xF) | ((q[1] & 0xF) << 4);
            let seconds = (q[2] & 0xF) | ((q[3] & 0xF) << 4);
            let minutes = (q[4] & 0xF) | ((q[5] & 0xF) << 4);
            let hours = (q[6] & 0xF) | ((q[7] & 1) << 4);
            let rate = MtcRate::from_bits((q[7] >> 1) & 0x3);
            if let Some(frames) = position(
                rate,
                u64::from(hours),
                u64::from(minutes),
                u64::from(seconds),
                u64::from(frames),
            ) {
                messages.push(MtcMessage::Position(MtcPosition { rate, frames }));
            }
        }
        messages
    }
}

fn decode_full_frame(sysex: &[u8]) -> Option<MtcMessage> {
    // F0 7F <device> 01 01 <hours> <minutes> <seconds> <frames> F7
    if sysex.len() != 10 || sysex[1] != 0x7F || sysex[3] != 0x01 || sysex[4] != 0x01 {
        return None;
    }
    let hours_byte = sysex[5];
    let rate = MtcRate::from_bits((hours_byte >> 5) & 0x3);
    let hours = u64::from(hours_byte & 0x1F);
    let frames = position(
        rate,
        hours,
        u64::from(sysex[6]),
        u64::from(sysex[7]),
        u64::from(sysex[8]),
    )?;
    Some(MtcMessage::Position(MtcPosition { rate, frames }))
}

/// A running MTC reader. Dropping it asks the thread to stop and joins it.
pub struct MtcReader {
    receiver: Receiver<MtcMessage>,
    stop: Arc<AtomicBool>,
    handle: Option<JoinHandle<()>>,
}

impl MtcReader {
    pub fn open(port: &str) -> Result<Self, String> {
        let mut link = dmx::serial::open(port).map_err(|error| error.to_string())?;
        link.configure(&dmx::serial::SerialSettings::mtc())
            .map_err(|error| error.to_string())?;
        let (sender, receiver) = mpsc::sync_channel(256);
        let stop = Arc::new(AtomicBool::new(false));
        let thread_stop = Arc::clone(&stop);
        let name = port.to_string();
        let handle = thread::Builder::new()
            .name("openlight-mtc".into())
            .spawn(move || {
                let mut decoder = MtcDecoder::default();
                let mut buffer = [0u8; 256];
                while !thread_stop.load(Ordering::Relaxed) {
                    match link.read(&mut buffer) {
                        Ok(0) => thread::sleep(Duration::from_millis(2)),
                        Ok(count) => {
                            for byte in &buffer[..count] {
                                for message in decoder.push(*byte) {
                                    match sender.try_send(message) {
                                        Ok(()) | Err(TrySendError::Full(_)) => {}
                                        Err(TrySendError::Disconnected(_)) => return,
                                    }
                                }
                            }
                        }
                        Err(error) => {
                            let _ =
                                sender.send(MtcMessage::Unavailable(format!("{name}: {error}")));
                            return;
                        }
                    }
                }
            })
            .map_err(|error| format!("cannot start the MTC reader: {error}"))?;
        Ok(Self {
            receiver,
            stop,
            handle: Some(handle),
        })
    }

    /// Everything the reader has posted since the last drain.
    pub fn drain(&self) -> Vec<MtcMessage> {
        let mut messages = Vec::new();
        while let Ok(message) = self.receiver.try_recv() {
            messages.push(message);
        }
        messages
    }
}

impl Drop for MtcReader {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quarter_frames(hours: u8, minutes: u8, seconds: u8, frames: u8, rate_bits: u8) -> Vec<u8> {
        let values = [
            frames & 0xF,
            frames >> 4,
            seconds & 0xF,
            seconds >> 4,
            minutes & 0xF,
            minutes >> 4,
            hours & 0xF,
            ((rate_bits & 0x3) << 1) | (hours >> 4),
        ];
        let mut bytes = Vec::new();
        for (kind, value) in values.into_iter().enumerate() {
            bytes.push(0xF1);
            bytes.push(((kind as u8) << 4) | (value & 0xF));
        }
        bytes
    }

    #[test]
    fn quarter_frames_reassemble_a_position() {
        let mut decoder = MtcDecoder::default();
        let mut messages = Vec::new();
        for byte in quarter_frames(1, 2, 3, 4, 1) {
            messages.extend(decoder.push(byte));
        }
        assert_eq!(
            messages,
            vec![MtcMessage::Position(MtcPosition {
                rate: MtcRate::Fps25,
                frames: 93_079
            })]
        );
    }

    #[test]
    fn a_full_frame_message_is_accepted() {
        let mut decoder = MtcDecoder::default();
        let mut messages = Vec::new();
        for byte in [0xF0, 0x7F, 0x7F, 0x01, 0x01, 0x21, 2, 3, 4, 0xF7] {
            messages.extend(decoder.push(byte));
        }
        assert_eq!(
            messages,
            vec![MtcMessage::Position(MtcPosition {
                rate: MtcRate::Fps25,
                frames: 93_079
            })]
        );
    }

    #[test]
    fn a_torn_message_is_ignored() {
        let mut decoder = MtcDecoder::default();
        let bytes = quarter_frames(1, 2, 3, 4, 1);
        let mut messages = Vec::new();
        for byte in &bytes[..14] {
            messages.extend(decoder.push(*byte));
        }
        messages.extend(decoder.push(0xF1));
        messages.extend(decoder.push(0x00));
        assert!(messages.is_empty());
    }

    #[test]
    fn song_position_is_ignored_with_a_warning() {
        let mut decoder = MtcDecoder::default();
        let mut messages = Vec::new();
        for byte in [0xF2, 0x01, 0x02] {
            messages.extend(decoder.push(byte));
        }
        assert_eq!(
            messages,
            vec![MtcMessage::Warning(
                "MTC song position is not supported; ignored".into()
            )]
        );
    }

    #[test]
    fn drop_frame_round_trips_across_a_minute_boundary() {
        assert_eq!(drop_frame_to_frames(0, 0, 59, 29), 1799);
        assert_eq!(drop_frame_to_frames(0, 1, 0, 2), 1800);
        assert_eq!(frames_to_drop_frame(1799), (0, 0, 59, 29));
        assert_eq!(frames_to_drop_frame(1800), (0, 1, 0, 2));
        assert_eq!(drop_frame_to_frames(0, 10, 0, 0), 17_982);
        assert_eq!(frames_to_drop_frame(17_982), (0, 10, 0, 0));
    }

    #[test]
    fn an_impossible_drop_frame_label_is_rejected() {
        assert_eq!(position(MtcRate::Fps30Drop, 0, 1, 0, 0), None);
        assert_eq!(position(MtcRate::Fps30Drop, 0, 1, 0, 2), Some(1800));
    }
}
