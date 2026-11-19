//! ENTTEC DMX USB Pro / DMX King (Pro-compatible mode).
//!
//! Widget API 1.44 framing over a 57600 8N1 virtual COM port:
//!
//! ```text
//! 0x7E | label | length LSB | length MSB | data... | 0xE7
//! ```
//!
//! Label 6 is "output only send DMX packet"; the payload is the start code
//! followed by the 512 slots, so the length is 513.

use crate::serial::{self, SerialLink, SerialSettings};
use crate::{DmxResult, DmxTransport, DMX_SLOTS, DMX_START_CODE};

pub const START_OF_MESSAGE: u8 = 0x7E;
pub const END_OF_MESSAGE: u8 = 0xE7;
pub const LABEL_SEND_DMX: u8 = 6;
/// DMX King enhanced mode, output universe 1 (same payload as label 6).
pub const LABEL_SEND_DMX_KING_UNIVERSE_1: u8 = 100;

/// The widget ignores payloads shorter than this (`DMX_START_CODE + 24 slots`).
pub const MIN_PAYLOAD: usize = 25;

pub struct EnttecProTransport {
    link: Box<dyn SerialLink>,
    label: u8,
    settings: SerialSettings,
    configured: bool,
    frame: Vec<u8>,
    describe: String,
}

impl EnttecProTransport {
    pub fn open(path: &str) -> DmxResult<Self> {
        Self::open_with_label(path, LABEL_SEND_DMX)
    }

    pub fn open_with_label(path: &str, label: u8) -> DmxResult<Self> {
        Ok(Self {
            link: serial::open(path)?,
            label,
            settings: SerialSettings::enttec_pro(),
            configured: false,
            frame: Vec::with_capacity(DMX_SLOTS + 6),
            describe: format!("enttec:{path}"),
        })
    }

    fn ensure_configured(&mut self) -> DmxResult<()> {
        if !self.configured {
            self.link.configure(&self.settings)?;
            self.configured = true;
        }
        Ok(())
    }
}

/// Encode one widget message exactly as it goes out on the wire.
pub fn encode_frame(label: u8, slots: &[u8; DMX_SLOTS]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(DMX_SLOTS + 6);
    encode_into(&mut frame, label, slots);
    frame
}

fn encode_into(frame: &mut Vec<u8>, label: u8, slots: &[u8; DMX_SLOTS]) {
    let payload_len = (DMX_SLOTS + 1) as u16;
    debug_assert!(usize::from(payload_len) >= MIN_PAYLOAD);
    frame.clear();
    frame.push(START_OF_MESSAGE);
    frame.push(label);
    frame.extend_from_slice(&payload_len.to_le_bytes());
    frame.push(DMX_START_CODE);
    frame.extend_from_slice(slots);
    frame.push(END_OF_MESSAGE);
}

impl DmxTransport for EnttecProTransport {
    fn describe(&self) -> String {
        self.describe.clone()
    }

    fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> DmxResult<()> {
        self.ensure_configured()?;
        encode_into(&mut self.frame, self.label, slots);
        debug_assert_eq!(self.frame.len(), DMX_SLOTS + 6);
        self.link.write_all(&self.frame)?;
        self.link.drain()?;
        Ok(())
    }

    fn take_warning(&mut self) -> Option<String> {
        self.link.take_warning()
    }

    fn shutdown(&mut self) -> DmxResult<()> {
        self.send_frame(&[0; DMX_SLOTS])
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct RecordingLink {
        written: Arc<Mutex<Vec<u8>>>,
        settings: Arc<Mutex<Option<SerialSettings>>>,
    }

    impl SerialLink for RecordingLink {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            *self.settings.lock().unwrap() = Some(*settings);
            Ok(())
        }
        fn send_break(&mut self, _hold: Duration) -> DmxResult<()> {
            panic!("the widget generates DMX timing; the host must not send a break");
        }
        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }
        fn drain(&mut self) -> DmxResult<()> {
            Ok(())
        }
    }

    #[test]
    fn frame_layout_matches_widget_api() {
        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 7;
        slots[511] = 200;
        let frame = encode_frame(LABEL_SEND_DMX, &slots);

        assert_eq!(frame.len(), DMX_SLOTS + 6);
        assert_eq!(frame[0], START_OF_MESSAGE);
        assert_eq!(frame[1], LABEL_SEND_DMX);
        assert_eq!(&frame[2..4], &[0x01, 0x02], "513 = 0x0201 little endian");
        assert_eq!(frame[4], DMX_START_CODE);
        assert_eq!(frame[5], 7);
        assert_eq!(frame[5 + 511], 200);
        assert_eq!(frame[frame.len() - 1], END_OF_MESSAGE);
    }

    #[test]
    fn transport_uses_57600_8n1_and_one_write_per_frame() {
        let link = RecordingLink::default();
        let written = link.written.clone();
        let settings = link.settings.clone();
        let mut transport = EnttecProTransport {
            link: Box::new(link),
            label: LABEL_SEND_DMX,
            settings: SerialSettings::enttec_pro(),
            configured: false,
            frame: Vec::new(),
            describe: "enttec:test".into(),
        };

        transport.send_frame(&[1; DMX_SLOTS]).unwrap();

        let settings = (*settings.lock().unwrap()).expect("configured");
        assert_eq!(settings.baud, 57_600);
        assert_eq!(settings.stop_bits, 1);
        assert_eq!(settings.break_us, 0);
        assert_eq!(written.lock().unwrap().len(), DMX_SLOTS + 6);
    }

    #[test]
    fn dmxking_label_is_configurable() {
        let frame = encode_frame(LABEL_SEND_DMX_KING_UNIVERSE_1, &[0; DMX_SLOTS]);
        assert_eq!(frame[1], 100);
    }
}
