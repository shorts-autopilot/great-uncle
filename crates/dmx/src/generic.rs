//! Generic DMX over a bare USB-serial adapter (FTDI, CH340, Prolific…).
//!
//! The host generates all DMX timing: break, mark-after-break, then the start
//! code and 512 slots as one continuous write at 250000 8N2.

use std::time::Duration;

use crate::serial::{self, SerialLink, SerialSettings};
use crate::{DmxResult, DmxTransport, DMX_SLOTS, DMX_START_CODE};

pub struct GenericSerialTransport {
    link: Box<dyn SerialLink>,
    settings: SerialSettings,
    configured: bool,
    frame: Vec<u8>,
    describe: String,
}

impl GenericSerialTransport {
    pub fn open(path: &str, settings: SerialSettings) -> DmxResult<Self> {
        Ok(Self {
            link: serial::open(path)?,
            settings,
            configured: false,
            frame: vec![0; DMX_SLOTS + 1],
            describe: format!("generic:{path}"),
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

impl DmxTransport for GenericSerialTransport {
    fn describe(&self) -> String {
        self.describe.clone()
    }

    fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> DmxResult<()> {
        self.ensure_configured()?;

        if self.settings.break_us > 0 {
            self.link
                .send_break(Duration::from_micros(u64::from(self.settings.break_us)))?;
        }
        if self.settings.mark_after_break_us > 0 {
            std::thread::sleep(Duration::from_micros(u64::from(
                self.settings.mark_after_break_us,
            )));
        }

        self.frame[0] = DMX_START_CODE;
        self.frame[1..].copy_from_slice(slots);
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
    use crate::serial::Parity;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    struct RecordingLink {
        inner: Arc<Mutex<Recorded>>,
    }

    #[derive(Default)]
    struct Recorded {
        configured: Vec<SerialSettings>,
        breaks: usize,
        bytes: Vec<u8>,
        drained: usize,
    }

    impl SerialLink for RecordingLink {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            self.inner.lock().unwrap().configured.push(*settings);
            Ok(())
        }
        fn send_break(&mut self, _hold: Duration) -> DmxResult<()> {
            self.inner.lock().unwrap().breaks += 1;
            Ok(())
        }
        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            self.inner.lock().unwrap().bytes.extend_from_slice(bytes);
            Ok(())
        }
        fn drain(&mut self) -> DmxResult<()> {
            self.inner.lock().unwrap().drained += 1;
            Ok(())
        }
    }

    fn transport() -> (GenericSerialTransport, Arc<Mutex<Recorded>>) {
        let link = RecordingLink::default();
        let recorded = link.inner.clone();
        let settings = SerialSettings {
            baud: 250_000,
            stop_bits: 2,
            parity: Parity::None,
            break_us: 176,
            mark_after_break_us: 12,
        };
        let transport = GenericSerialTransport {
            link: Box::new(link),
            settings,
            configured: false,
            frame: vec![0; DMX_SLOTS + 1],
            describe: "generic:test".into(),
        };
        (transport, recorded)
    }

    #[test]
    fn configures_once_and_frames_with_start_code() {
        let (mut transport, recorded) = transport();
        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 10;
        slots[511] = 255;

        transport.send_frame(&slots).unwrap();
        transport.send_frame(&slots).unwrap();

        let state = recorded.lock().unwrap();
        assert_eq!(state.configured.len(), 1, "port configured once");
        assert_eq!(state.breaks, 2, "one break per frame");
        assert_eq!(state.drained, 2);
        assert_eq!(state.bytes.len(), 2 * (DMX_SLOTS + 1));
        assert_eq!(state.bytes[0], DMX_START_CODE);
        assert_eq!(state.bytes[1], 10);
        assert_eq!(state.bytes[DMX_SLOTS], 255);
    }

    #[test]
    fn shutdown_sends_blackout_frame() {
        let (mut transport, recorded) = transport();
        let mut slots = [255u8; DMX_SLOTS];
        slots[3] = 1;
        transport.send_frame(&slots).unwrap();
        transport.shutdown().unwrap();

        let state = recorded.lock().unwrap();
        let last = &state.bytes[state.bytes.len() - (DMX_SLOTS + 1)..];
        assert_eq!(last.len(), DMX_SLOTS + 1);
        assert_eq!(last[0], DMX_START_CODE);
        assert!(last[1..].iter().all(|value| *value == 0));
    }
}
