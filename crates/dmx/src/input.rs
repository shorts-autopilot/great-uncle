//! DMX input transports.
//!
//! An input reader returns whole frames, never raw bytes. The engine merges a
//! frame with its own rendered frame and owns every policy decision; a
//! transport only has to turn a device into [`DmxFrame`] values. That keeps
//! the merge testable with the virtual reader and keeps wire-level framing out
//! of the engine.

use std::time::Duration;

use crate::{DmxResult, DMX_SLOTS};

/// One received DMX frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DmxFrame {
    /// DMX start code. `0` is a dimmer frame; other codes are not merged.
    pub start_code: u8,
    /// All 512 slot values. Slot 0 is DMX address 1.
    pub slots: [u8; DMX_SLOTS],
}

impl DmxFrame {
    pub fn new(start_code: u8, slots: [u8; DMX_SLOTS]) -> Self {
        Self { start_code, slots }
    }

    /// A dimmer frame (start code zero).
    pub fn dimmer(slots: [u8; DMX_SLOTS]) -> Self {
        Self {
            start_code: 0,
            slots,
        }
    }
}

impl Default for DmxFrame {
    fn default() -> Self {
        Self::dimmer([0; DMX_SLOTS])
    }
}

/// One input path for one universe.
pub trait DmxInput: Send {
    /// Human readable device description, e.g. `enttec:in:/dev/ttyUSB0`.
    fn describe(&self) -> String;

    /// Wait up to `timeout` for one frame. `Ok(None)` means the timeout
    /// expired with no complete frame; it is not an error.
    fn poll(&mut self, timeout: Duration) -> DmxResult<Option<DmxFrame>>;

    /// Non-fatal condition discovered while reading, reported once by the
    /// input thread so it can surface in the status window.
    fn take_warning(&mut self) -> Option<String> {
        None
    }

    /// Reopen the underlying device after a failure.
    fn reopen(&mut self) -> DmxResult<()> {
        Err(crate::DmxError::Unsupported(
            "reconnect is not supported".into(),
        ))
    }
}
