//! In-memory loopback transport.
//!
//! Used by the engine's output tests, by CI and for hardware-free demos. It
//! records every frame in wire order (start code included) so tests can assert
//! on the exact bytes the serial path would have emitted.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{DmxResult, DmxTransport, DMX_SLOTS, DMX_START_CODE};

#[derive(Debug, Default)]
pub struct VirtualState {
    /// Frames sent since the transport was created.
    pub frames: u64,
    /// Most recent frame, start code included (513 bytes).
    pub last: Vec<u8>,
    /// Up to `capacity` recent frames, oldest first.
    pub history: VecDeque<Vec<u8>>,
}

pub struct VirtualTransport {
    state: Arc<Mutex<VirtualState>>,
    capacity: usize,
}

impl Default for VirtualTransport {
    fn default() -> Self {
        Self::new()
    }
}

impl VirtualTransport {
    pub fn new() -> Self {
        Self::with_history(0)
    }

    /// Keep the last `capacity` frames for inspection.
    pub fn with_history(capacity: usize) -> Self {
        Self {
            state: Arc::new(Mutex::new(VirtualState::default())),
            capacity,
        }
    }

    /// Shared handle so a console or test can read what was sent.
    pub fn handle(&self) -> Arc<Mutex<VirtualState>> {
        Arc::clone(&self.state)
    }
}

impl DmxTransport for VirtualTransport {
    fn describe(&self) -> String {
        "virtual".into()
    }

    /// A virtual port has nothing to reopen, so a reconnect test can drive the
    /// recovery path without hardware.
    fn reopen(&mut self) -> DmxResult<()> {
        Ok(())
    }

    fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> DmxResult<()> {
        let mut state = lock(&self.state);
        state.frames += 1;
        let mut frame = Vec::with_capacity(DMX_SLOTS + 1);
        frame.push(DMX_START_CODE);
        frame.extend_from_slice(slots);
        state.last = frame.clone();
        if self.capacity > 0 {
            if state.history.len() == self.capacity {
                state.history.pop_front();
            }
            state.history.push_back(frame);
        }
        Ok(())
    }

    /// Record the same blackout frame a real output sends when it stops.
    fn shutdown(&mut self) -> DmxResult<()> {
        self.send_frame(&[0; DMX_SLOTS])
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_frames_with_start_code() {
        let mut transport = VirtualTransport::with_history(2);
        let handle = transport.handle();

        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 255;
        transport.send_frame(&slots).unwrap();
        slots[0] = 1;
        transport.send_frame(&slots).unwrap();

        let state = handle.lock().unwrap();
        assert_eq!(state.frames, 2);
        assert_eq!(state.last.len(), DMX_SLOTS + 1);
        assert_eq!(state.last[0], DMX_START_CODE);
        assert_eq!(state.last[1], 1);
        assert_eq!(state.history.len(), 2);
        assert_eq!(state.history[0][1], 255);
    }

    #[test]
    fn history_is_bounded() {
        let mut transport = VirtualTransport::with_history(3);
        let handle = transport.handle();
        for value in 0..10u8 {
            let mut slots = [0u8; DMX_SLOTS];
            slots[0] = value;
            transport.send_frame(&slots).unwrap();
        }
        let state = handle.lock().unwrap();
        assert_eq!(state.history.len(), 3);
        assert_eq!(state.history.back().unwrap()[1], 9);
    }
}
