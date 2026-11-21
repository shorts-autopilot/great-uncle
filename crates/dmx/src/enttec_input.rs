//! ENTTEC DMX USB Pro input.
//!
//! The widget API wraps every message as
//! `0x7E | label | length LSB | length MSB | data... | 0xE7`. In receive mode
//! (label 8, payload `0`) a Pro sends label 5 unsolicited for every DMX frame:
//! one status byte followed by the start code and up to 512 slots.
//!
//! This reader owns its serial link and never sends DMX output. A Pro is an
//! input *or* output device at one time, so an input and an output binding
//! must name different widgets.

use std::time::{Duration, Instant};

use crate::input::{DmxFrame, DmxInput};
use crate::serial::{self, SerialLink, SerialSettings};
use crate::{DmxResult, DMX_SLOTS};

pub const START_OF_MESSAGE: u8 = 0x7E;
pub const END_OF_MESSAGE: u8 = 0xE7;
pub const LABEL_RECEIVED_DMX: u8 = 5;
pub const LABEL_RECEIVE_MODE: u8 = 8;
/// Maximum payload the widget API allows.
const MAX_PAYLOAD: usize = 600;
/// Stop growing the receive buffer if a device never emits an end delimiter.
const MAX_BUFFER: usize = 8 * 1024;

/// Build the label-8 request that puts the widget in "send always" receive
/// mode.
pub fn encode_receive_mode() -> Vec<u8> {
    vec![
        START_OF_MESSAGE,
        LABEL_RECEIVE_MODE,
        1,
        0,
        0,
        END_OF_MESSAGE,
    ]
}

pub struct EnttecProInput {
    link: Box<dyn SerialLink>,
    path: String,
    configured: bool,
    receiving: bool,
    buffer: Vec<u8>,
    warning: Option<String>,
    describe: String,
}

impl EnttecProInput {
    pub fn open(path: &str) -> DmxResult<Self> {
        let mut input = Self::with_link(serial::open(path)?, format!("enttec:in:{path}"));
        input.path = path.to_string();
        Ok(input)
    }

    /// Build a reader over an already-open link. Tests use this with a fake
    /// serial port; production goes through [`EnttecProInput::open`].
    pub fn with_link(link: Box<dyn SerialLink>, describe: impl Into<String>) -> Self {
        Self {
            link,
            path: String::new(),
            configured: false,
            receiving: false,
            buffer: Vec::new(),
            warning: None,
            describe: describe.into(),
        }
    }

    fn ensure_receiving(&mut self) -> DmxResult<()> {
        if !self.configured {
            self.link.configure(&SerialSettings::enttec_pro())?;
            self.configured = true;
        }
        if !self.receiving {
            self.link.write_all(&encode_receive_mode())?;
            self.link.drain()?;
            self.receiving = true;
        }
        Ok(())
    }

    /// Remove one complete message from the buffer. Returns `None` when more
    /// bytes are needed; a corrupt message is dropped and parsing continues.
    fn take_frame(&mut self) -> Option<DmxFrame> {
        loop {
            let Some(start) = self
                .buffer
                .iter()
                .position(|byte| *byte == START_OF_MESSAGE)
            else {
                self.buffer.clear();
                return None;
            };
            if start > 0 {
                self.buffer.drain(..start);
            }
            if self.buffer.len() < 4 {
                return None;
            }
            let label = self.buffer[1];
            let length = usize::from(u16::from_le_bytes([self.buffer[2], self.buffer[3]]));
            if length > MAX_PAYLOAD {
                self.warn("received a widget message with an invalid length");
                self.buffer.drain(..1);
                continue;
            }
            let total = 4 + length + 1;
            if self.buffer.len() < total {
                return None;
            }
            if self.buffer[total - 1] != END_OF_MESSAGE {
                self.warn("received a widget message with no end delimiter");
                self.buffer.drain(..1);
                continue;
            }
            let data: Vec<u8> = self.buffer[4..4 + length].to_vec();
            self.buffer.drain(..total);

            if label != LABEL_RECEIVED_DMX {
                continue;
            }
            if data.len() < 2 {
                self.warn("received an empty DMX input message");
                continue;
            }
            if data[0] != 0 {
                self.warn(format!(
                    "the widget reported a corrupted DMX receive (status {})",
                    data[0]
                ));
                continue;
            }
            let start_code = data[1];
            let mut slots = [0u8; DMX_SLOTS];
            let count = data.len().saturating_sub(2).min(DMX_SLOTS);
            slots[..count].copy_from_slice(&data[2..2 + count]);
            return Some(DmxFrame { start_code, slots });
        }
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.warning = Some(message.into());
    }

    fn read_once(&mut self) -> DmxResult<usize> {
        let mut chunk = [0u8; 1024];
        let count = self.link.read(&mut chunk)?;
        if count > 0 {
            self.buffer.extend_from_slice(&chunk[..count]);
            if self.buffer.len() > MAX_BUFFER {
                self.warn("input buffer overflow; dropping bytes until the next message");
                let keep = self.buffer.len() - MAX_BUFFER;
                self.buffer.drain(..keep);
            }
        }
        Ok(count)
    }
}

impl DmxInput for EnttecProInput {
    fn describe(&self) -> String {
        self.describe.clone()
    }

    fn poll(&mut self, timeout: Duration) -> DmxResult<Option<DmxFrame>> {
        self.ensure_receiving()?;
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(frame) = self.take_frame() {
                return Ok(Some(frame));
            }
            let count = self.read_once()?;
            if count > 0 {
                continue;
            }
            let now = Instant::now();
            if now >= deadline {
                return Ok(None);
            }
            std::thread::sleep((deadline - now).min(Duration::from_millis(1)));
        }
    }

    fn take_warning(&mut self) -> Option<String> {
        self.warning.take()
    }

    fn reopen(&mut self) -> DmxResult<()> {
        if self.path.is_empty() {
            return Err(crate::DmxError::Unsupported(
                "this input has no serial path to reopen".into(),
            ));
        }
        self.link = serial::open(&self.path)?;
        self.configured = false;
        self.receiving = false;
        self.buffer.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct FakeLink {
        reads: Arc<Mutex<VecDeque<Vec<u8>>>>,
        written: Arc<Mutex<Vec<u8>>>,
        configured: Arc<Mutex<Option<SerialSettings>>>,
        read_error: Arc<Mutex<Option<String>>>,
    }

    impl SerialLink for FakeLink {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            *self.configured.lock().unwrap() = Some(*settings);
            Ok(())
        }

        fn send_break(&mut self, _hold: Duration) -> DmxResult<()> {
            panic!("input must not drive DMX break");
        }

        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            self.written.lock().unwrap().extend_from_slice(bytes);
            Ok(())
        }

        fn drain(&mut self) -> DmxResult<()> {
            Ok(())
        }

        fn read(&mut self, buffer: &mut [u8]) -> DmxResult<usize> {
            if let Some(error) = self.read_error.lock().unwrap().take() {
                return Err(crate::DmxError::Protocol(error));
            }
            let Some(mut chunk) = self.reads.lock().unwrap().pop_front() else {
                return Ok(0);
            };
            let count = chunk.len().min(buffer.len());
            buffer[..count].copy_from_slice(&chunk[..count]);
            if count < chunk.len() {
                chunk.drain(..count);
                self.reads.lock().unwrap().push_front(chunk);
            }
            Ok(count)
        }
    }

    fn message(label: u8, data: &[u8]) -> Vec<u8> {
        let mut out = vec![START_OF_MESSAGE, label];
        out.extend_from_slice(&(data.len() as u16).to_le_bytes());
        out.extend_from_slice(data);
        out.push(END_OF_MESSAGE);
        out
    }

    fn reader(reads: Vec<Vec<u8>>) -> (EnttecProInput, FakeLink) {
        let link = FakeLink::default();
        *link.reads.lock().unwrap() = reads.into();
        (
            EnttecProInput::with_link(Box::new(link.clone()), "enttec:in:test"),
            link,
        )
    }

    #[test]
    fn configures_and_requests_receive_mode_once() {
        let (mut input, link) = reader(Vec::new());
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);

        let configured = link.configured.lock().unwrap().unwrap();
        assert_eq!(configured.baud, 57_600);
        assert_eq!(configured.stop_bits, 1);
        assert_eq!(
            *link.written.lock().unwrap(),
            encode_receive_mode(),
            "one label-8 request, not one per poll"
        );
    }

    #[test]
    fn decodes_received_dmx_payload() {
        let mut payload = vec![0u8, 0u8]; // status, start code
        payload.resize(2 + DMX_SLOTS, 0);
        payload[2] = 10;
        payload[2 + DMX_SLOTS - 1] = 255;
        let (mut input, _) = reader(vec![message(LABEL_RECEIVED_DMX, &payload)]);
        let frame = input
            .poll(Duration::from_secs(1))
            .unwrap()
            .expect("a frame");
        assert_eq!(frame.start_code, 0);
        assert_eq!(frame.slots[0], 10);
        assert_eq!(frame.slots[DMX_SLOTS - 1], 255);
    }

    #[test]
    fn short_payload_is_padded() {
        let payload = [0u8, 0u8, 5u8, 6u8];
        let (mut input, _) = reader(vec![message(LABEL_RECEIVED_DMX, &payload)]);
        let frame = input.poll(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(&frame.slots[..2], &[5, 6]);
        assert!(frame.slots[2..].iter().all(|value| *value == 0));
    }

    #[test]
    fn alternate_start_code_is_passed_through() {
        let payload = [0u8, 0xCF, 1, 2];
        let (mut input, _) = reader(vec![message(LABEL_RECEIVED_DMX, &payload)]);
        let frame = input.poll(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(frame.start_code, 0xCF);
        assert_eq!(&frame.slots[..2], &[1, 2]);
    }

    #[test]
    fn corrupt_status_is_dropped_with_a_warning() {
        let corrupt = [1u8, 0, 255, 255];
        let good = [0u8, 0, 7];
        let (mut input, _) = reader(vec![
            message(LABEL_RECEIVED_DMX, &corrupt),
            message(LABEL_RECEIVED_DMX, &good),
        ]);
        let frame = input.poll(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(frame.slots[0], 7);
        assert!(input.take_warning().unwrap().contains("corrupted"));
    }

    #[test]
    fn resynchronises_after_a_bad_length() {
        let mut bad = vec![START_OF_MESSAGE, LABEL_RECEIVED_DMX, 0xFF, 0x02];
        bad.extend_from_slice(&[0u8; 8]);
        let good = [0u8, 0, 42];
        let (mut input, _) = reader(vec![bad, message(LABEL_RECEIVED_DMX, &good)]);
        let frame = input.poll(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(frame.slots[0], 42);
        assert!(input.take_warning().unwrap().contains("invalid length"));
    }

    #[test]
    fn resynchronises_after_a_missing_end_delimiter() {
        let mut bad = vec![START_OF_MESSAGE, LABEL_RECEIVED_DMX, 2, 0, 0, 0];
        bad.push(0xAA);
        let good = [0u8, 0, 9];
        let (mut input, _) = reader(vec![bad, message(LABEL_RECEIVED_DMX, &good)]);
        let frame = input.poll(Duration::from_secs(1)).unwrap().unwrap();
        assert_eq!(frame.slots[0], 9);
        assert!(input.take_warning().unwrap().contains("end delimiter"));
    }

    #[test]
    fn read_errors_are_reported() {
        let (mut input, link) = reader(Vec::new());
        *link.read_error.lock().unwrap() = Some("unplugged".into());
        assert!(input.poll(Duration::from_millis(1)).is_err());
    }
}
