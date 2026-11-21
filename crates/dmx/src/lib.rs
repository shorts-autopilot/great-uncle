//! DMX512 output and input transports.
//!
//! Every transport receives or returns whole frames of [`DMX_SLOTS`] slot
//! values and owns all wire-level framing (start code, break, MAB, widget
//! envelopes). The engine never talks to a device directly, which keeps both
//! stages testable with the virtual transports.

pub mod enttec;
pub mod enttec_input;
pub mod error;
pub mod generic;
pub mod input;
pub mod probe;
pub mod serial;
pub mod udmx;
pub mod virtual_input;
pub mod virtual_port;

pub use error::{DmxError, DmxResult};
pub use input::{DmxFrame, DmxInput};

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

    /// Reopen the underlying device after a failure.
    ///
    /// The default reports that reconnecting is not possible, which is what a
    /// transport with nothing to reopen should do.
    fn reopen(&mut self) -> DmxResult<()> {
        Err(crate::DmxError::Unsupported(
            "reconnect is not supported".into(),
        ))
    }
}

/// Build an input reader from a spec string.
///
/// | spec | reader |
/// |---|---|
/// | `virtual` | in-memory frames, for tests and demos |
/// | `enttec:/dev/ttyUSB0` | ENTTEC DMX USB Pro input (DMX King compatible) |
///
/// `generic:` is deliberately refused: a bare serial receiver needs break
/// detection, which has not been verified on hardware. See [`open`] for output.
pub fn open_input(spec: &str) -> DmxResult<Box<dyn DmxInput>> {
    let spec = spec.trim();
    let (kind, argument) = match spec.split_once(':') {
        Some((kind, argument)) => (kind, Some(argument)),
        None => (spec, None),
    };
    match (kind, argument) {
        ("virtual", None) => Ok(Box::new(virtual_input::VirtualInput::new().0)),
        ("virtual", Some(_)) => Err(DmxError::Unsupported(format!(
            "`virtual` takes no argument: {spec}"
        ))),
        ("enttec", Some(path)) => Ok(Box::new(enttec_input::EnttecProInput::open(path)?)),
        ("generic", Some(_)) => Err(DmxError::Unsupported(
            "generic DMX input is not supported: a bare serial receiver needs break detection \
             that has not been verified on hardware; use `enttec:<port>` or `virtual`"
                .into(),
        )),
        ("generic" | "enttec", None) => Err(DmxError::Unsupported(format!(
            "`{kind}` needs a serial port, e.g. `{kind}:/dev/ttyUSB0` or `{kind}:COM3`"
        ))),
        (other, _) => Err(DmxError::Unsupported(format!(
            "unknown input transport `{other}` (expected virtual or enttec)"
        ))),
    }
}

/// Build an output transport from a spec string.
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
            "`{kind}` needs a serial port, e.g. `{kind}:/dev/ttyUSB0` or `{kind}:COM3`"
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

#[cfg(test)]
mod input_tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn opens_the_virtual_reader() {
        let mut input = open_input("virtual").unwrap();
        assert_eq!(input.describe(), "virtual");
        assert_eq!(input.poll(Duration::from_millis(1)).unwrap(), None);
    }

    #[test]
    fn generic_input_reports_the_hardware_gate() {
        let error = match open_input("generic:/dev/ttyUSB0") {
            Ok(_) => panic!("generic input must report the hardware gate"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("break detection"), "{error}");
    }

    #[test]
    fn an_unknown_input_names_the_supported_readers() {
        let error = match open_input("nope") {
            Ok(_) => panic!("unknown input must fail"),
            Err(error) => error,
        };
        assert!(error.to_string().contains("virtual or enttec"), "{error}");
    }
}
