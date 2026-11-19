//! DMX512 output transports.
//!
//! Every transport receives whole frames of [`DMX_SLOTS`] slot values and owns
//! all wire-level framing (start code, break, MAB, widget envelopes). The
//! engine never talks to a device directly, which keeps the output stage
//! testable with the [`VirtualTransport`](virtual_port::VirtualTransport).

pub mod enttec;
pub mod error;
pub mod generic;
pub mod serial;
pub mod udmx;
pub mod virtual_port;

pub use error::{DmxError, DmxResult};

/// DMX512 addresses per universe.
pub const DMX_SLOTS: usize = 512;

/// DMX512 start code for a dimmer frame.
pub const DMX_START_CODE: u8 = 0x00;

/// One output path for one universe.
pub trait DmxTransport: Send {
    /// Human readable device description, e.g. `generic:/dev/ttyUSB0`.
    fn describe(&self) -> String;

    /// Send one frame. `slots` holds all 512 values; slot 0 is DMX address 1.
    fn send_frame(&mut self, slots: &[u8; DMX_SLOTS]) -> DmxResult<()>;

    /// Non-fatal condition discovered while sending, reported once by the
    /// output thread so it can surface in the status window.
    fn take_warning(&mut self) -> Option<String> {
        None
    }

    /// Best effort final frame when output stops.
    fn shutdown(&mut self) -> DmxResult<()> {
        Ok(())
    }
}

/// Build a transport from a spec string.
///
/// | spec | transport |
/// |---|---|
/// | `virtual` | in-memory loopback, for tests and demos |
/// | `generic:/dev/ttyUSB0` | bare USB-serial adapter at 250000 8N2 with break |
/// | `enttec:/dev/ttyUSB0` | ENTTEC DMX USB Pro protocol (DMX King compatible) |
/// | `udmx` | Anyma uDMX USB interface |
pub fn open(spec: &str) -> DmxResult<Box<dyn DmxTransport>> {
    let spec = spec.trim();
    let (kind, argument) = match spec.split_once(':') {
        Some((kind, argument)) => (kind, Some(argument)),
        None => (spec, None),
    };
    match (kind, argument) {
        ("virtual", None) => Ok(Box::new(virtual_port::VirtualTransport::new())),
        ("virtual", Some(_)) => Err(DmxError::Unsupported(format!(
            "`virtual` takes no argument: {spec}"
        ))),
        ("generic", Some(path)) => Ok(Box::new(generic::GenericSerialTransport::open(
            path,
            serial::SerialSettings::dmx512(),
        )?)),
        ("enttec", Some(path)) => Ok(Box::new(enttec::EnttecProTransport::open(path)?)),
        ("udmx", None) => open_udmx(),
        ("generic" | "enttec", None) => Err(DmxError::Unsupported(format!(
            "`{kind}` needs a serial port, e.g. `{kind}:/dev/ttyUSB0`"
        ))),
        ("udmx", Some(_)) => Err(DmxError::Unsupported(format!(
            "`udmx` takes no argument: {spec}"
        ))),
        (other, _) => Err(DmxError::Unsupported(format!(
            "unknown output transport `{other}` (expected virtual, generic, enttec or udmx)"
        ))),
    }
}

#[cfg(feature = "usb")]
fn open_udmx() -> DmxResult<Box<dyn DmxTransport>> {
    Ok(Box::new(udmx::usb::open()?))
}

#[cfg(not(feature = "usb"))]
fn open_udmx() -> DmxResult<Box<dyn DmxTransport>> {
    Err(DmxError::Unsupported(
        "this build has no USB support; rebuild with `--features usb`".into(),
    ))
}
