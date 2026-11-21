//! In-memory DMX input.
//!
//! The handle is the producer side; a test, a demo or a simulator sends whole
//! frames. The reader side implements [`DmxInput`] and is moved to the input
//! thread, so the two halves can live in different threads without sharing a
//! lock.

use std::sync::mpsc::{sync_channel, Receiver, SyncSender};
use std::time::Duration;

use crate::input::{DmxFrame, DmxInput};
use crate::{DmxResult, DMX_SLOTS};

/// The producer side of a virtual input.
#[derive(Clone)]
pub struct VirtualInputHandle {
    sender: SyncSender<DmxFrame>,
}

impl VirtualInputHandle {
    /// Send one complete frame. Blocks only when the reader is far behind.
    pub fn send(&self, frame: DmxFrame) -> DmxResult<()> {
        self.sender
            .send(frame)
            .map_err(|_| crate::DmxError::Unsupported("virtual input is closed".into()))
    }

    /// Send a dimmer frame (start code zero).
    pub fn send_slots(&self, slots: [u8; DMX_SLOTS]) -> DmxResult<()> {
        self.send(DmxFrame::dimmer(slots))
    }
}

/// The reader side of a virtual input.
pub struct VirtualInput {
    receiver: Receiver<DmxFrame>,
    closed: bool,
}

impl VirtualInput {
    /// Create a linked reader and producer.
    pub fn new() -> (Self, VirtualInputHandle) {
        Self::with_capacity(16)
    }

    /// Create a linked reader and producer with a bounded queue.
    pub fn with_capacity(capacity: usize) -> (Self, VirtualInputHandle) {
        let (sender, receiver) = sync_channel(capacity.max(1));
        (
            Self {
                receiver,
                closed: false,
            },
            VirtualInputHandle { sender },
        )
    }
}

impl Default for VirtualInput {
    fn default() -> Self {
        Self::new().0
    }
}

impl DmxInput for VirtualInput {
    fn describe(&self) -> String {
        "virtual".into()
    }

    fn poll(&mut self, timeout: Duration) -> DmxResult<Option<DmxFrame>> {
        if self.closed {
            return Ok(None);
        }
        match self.receiver.recv_timeout(timeout) {
            Ok(frame) => Ok(Some(frame)),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                self.closed = true;
                Ok(None)
            }
        }
    }

    fn reopen(&mut self) -> DmxResult<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delivers_frames_in_order() {
        let (mut input, handle) = VirtualInput::new();
        let mut first = [0u8; DMX_SLOTS];
        first[0] = 7;
        let mut second = [0u8; DMX_SLOTS];
        second[0] = 9;

        handle.send_slots(first).unwrap();
        handle.send(DmxFrame::new(0xCF, second)).unwrap();

        assert_eq!(
            input.poll(Duration::from_millis(10)).unwrap(),
            Some(DmxFrame::dimmer(first))
        );
        assert_eq!(
            input.poll(Duration::from_millis(10)).unwrap(),
            Some(DmxFrame::new(0xCF, second))
        );
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);
    }

    #[test]
    fn a_closed_handle_ends_the_poll() {
        let (mut input, handle) = VirtualInput::new();
        drop(handle);
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);
    }
}
