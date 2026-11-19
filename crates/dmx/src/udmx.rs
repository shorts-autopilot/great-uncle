//! Anyma uDMX.
//!
//! The uDMX has no serial port: DMX values are written with vendor specific USB
//! control transfers (VID:PID `16c0:05dc`).
//!
//! * request 1 — set one channel: `wValue` = value, `wIndex` = channel (0-based)
//! * request 2 — set a channel range: `wValue` = channel count,
//!   `wIndex` = first channel (0-based), payload = the values
//!
//! Ranges are sent in chunks because control transfers are limited to 64 bytes.

use crate::{DmxResult, DmxTransport, DMX_SLOTS};

pub const UDMX_VENDOR_ID: u16 = 0x16C0;
pub const UDMX_PRODUCT_ID: u16 = 0x05DC;
pub const REQUEST_TYPE_OUT: u8 = 0x40;
pub const REQUEST_SET_CHANNEL_RANGE: u8 = 2;
pub const REQUEST_SET_SINGLE_CHANNEL: u8 = 1;
/// A full speed control transfer carries at most 64 bytes.
pub const MAX_CHUNK: usize = 64;

/// The USB boundary, isolated so the protocol can be tested without hardware.
pub trait UsbControl: Send {
    fn control_out(
        &mut self,
        request_type: u8,
        request: u8,
        value: u16,
        index: u16,
        data: &[u8],
    ) -> DmxResult<()>;
}

pub struct UdmxTransport {
    device: Box<dyn UsbControl>,
    chunk: usize,
}

impl UdmxTransport {
    pub fn new(device: Box<dyn UsbControl>) -> Self {
        Self {
            device,
            chunk: MAX_CHUNK,
        }
    }

    /// Override the range size, mainly so tests can keep frames small.
    pub fn with_chunk(mut self, chunk: usize) -> Self {
        self.chunk = chunk.clamp(1, DMX_SLOTS);
        self
    }
}

impl DmxTransport for UdmxTransport {
    fn describe(&self) -> String {
        "udmx".into()
    }

    fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> DmxResult<()> {
        let mut start = 0;
        while start < DMX_SLOTS {
            let end = (start + self.chunk).min(DMX_SLOTS);
            self.device.control_out(
                REQUEST_TYPE_OUT,
                REQUEST_SET_CHANNEL_RANGE,
                (end - start) as u16,
                start as u16,
                &slots[start..end],
            )?;
            start = end;
        }
        Ok(())
    }

    fn shutdown(&mut self) -> DmxResult<()> {
        self.send_frame(&[0; DMX_SLOTS])
    }
}

#[cfg(feature = "usb")]
pub mod usb {
    use std::time::Duration;

    use rusb::UsbContext as _;

    use super::{UsbControl, UDMX_PRODUCT_ID, UDMX_VENDOR_ID};
    use crate::{DmxError, DmxResult};

    pub struct RusbUdmx {
        handle: rusb::DeviceHandle<rusb::Context>,
    }

    /// Open the first attached uDMX interface.
    pub fn open_control() -> DmxResult<RusbUdmx> {
        let context = rusb::Context::new().map_err(|error| DmxError::Usb(error.to_string()))?;
        let devices = context
            .devices()
            .map_err(|error| DmxError::Usb(error.to_string()))?;

        for device in devices.iter() {
            let descriptor = device
                .device_descriptor()
                .map_err(|error| DmxError::Usb(error.to_string()))?;
            if descriptor.vendor_id() != UDMX_VENDOR_ID
                || descriptor.product_id() != UDMX_PRODUCT_ID
            {
                continue;
            }
            let handle = device
                .open()
                .map_err(|error| DmxError::Usb(error.to_string()))?;
            if let Ok(true) = handle.kernel_driver_active(0) {
                let _ = handle.detach_kernel_driver(0);
            }
            handle
                .claim_interface(0)
                .map_err(|error| DmxError::Usb(error.to_string()))?;
            return Ok(RusbUdmx { handle });
        }

        Err(DmxError::Usb(format!(
            "no uDMX device found ({UDMX_VENDOR_ID:04x}:{UDMX_PRODUCT_ID:04x})"
        )))
    }

    pub fn open() -> DmxResult<super::UdmxTransport> {
        Ok(super::UdmxTransport::new(Box::new(open_control()?)))
    }

    impl UsbControl for RusbUdmx {
        fn control_out(
            &mut self,
            request_type: u8,
            request: u8,
            value: u16,
            index: u16,
            data: &[u8],
        ) -> DmxResult<()> {
            let written = self
                .handle
                .write_control(
                    request_type,
                    request,
                    value,
                    index,
                    data,
                    Duration::from_millis(500),
                )
                .map_err(|error| DmxError::Usb(error.to_string()))?;
            if written != data.len() {
                return Err(DmxError::Protocol(format!(
                    "uDMX accepted {written} of {} bytes",
                    data.len()
                )));
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// `(request type, request, value, index, payload)`.
    type Transfer = (u8, u8, u16, u16, Vec<u8>);

    #[derive(Clone, Default)]
    struct MockUsb {
        transfers: Arc<Mutex<Vec<Transfer>>>,
    }

    impl UsbControl for MockUsb {
        fn control_out(
            &mut self,
            request_type: u8,
            request: u8,
            value: u16,
            index: u16,
            data: &[u8],
        ) -> DmxResult<()> {
            self.transfers.lock().unwrap().push((
                request_type,
                request,
                value,
                index,
                data.to_vec(),
            ));
            Ok(())
        }
    }

    #[test]
    fn sends_full_universe_as_zero_based_ranges() {
        let usb = MockUsb::default();
        let transfers = usb.transfers.clone();
        let mut transport = UdmxTransport::new(Box::new(usb)).with_chunk(256);

        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 9;
        slots[255] = 10;
        slots[256] = 11;
        slots[511] = 12;
        transport.send_frame(&slots).unwrap();

        let transfers = transfers.lock().unwrap();
        assert_eq!(transfers.len(), 2);
        let (request_type, request, value, index, data) = &transfers[0];
        assert_eq!(*request_type, 0x40);
        assert_eq!(*request, 2);
        assert_eq!(*value, 256, "wValue is the channel count");
        assert_eq!(*index, 0, "wIndex is the zero based first channel");
        assert_eq!(data.len(), 256);
        assert_eq!(data[0], 9);
        assert_eq!(data[255], 10);

        let (_, _, value, index, data) = &transfers[1];
        assert_eq!(*value, 256);
        assert_eq!(*index, 256);
        assert_eq!(data[0], 11);
        assert_eq!(data[255], 12);
    }

    #[test]
    fn default_chunk_respects_control_transfer_limit() {
        let usb = MockUsb::default();
        let transfers = usb.transfers.clone();
        let mut transport = UdmxTransport::new(Box::new(usb));
        transport.send_frame(&[0; DMX_SLOTS]).unwrap();

        let transfers = transfers.lock().unwrap();
        assert_eq!(transfers.len(), 8, "512 / 64");
        assert!(transfers.iter().all(|(_, _, _, _, data)| data.len() <= 64));
    }
}
