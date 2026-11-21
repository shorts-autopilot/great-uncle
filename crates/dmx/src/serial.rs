//! Serial link abstraction.
//!
//! DMX needs settings that ordinary terminal APIs cannot express: 250000 baud
//! is not a standard termios speed, and the frame timing depends on an explicit
//! break + mark-after-break. On Linux that means `termios2` with `BOTHER` and
//! `TIOCSBRK`/`TIOCCBRK`, so the link is built on raw `libc` calls rather than
//! a cross-platform serial crate.

use std::time::Duration;

use crate::DmxResult;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Parity {
    None,
    Even,
    Odd,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SerialSettings {
    pub baud: u32,
    pub stop_bits: u8,
    pub parity: Parity,
    /// How long the line is held in break before the mark-after-break pause.
    pub break_us: u32,
    /// Idle time between the rising edge of the break and the start code.
    pub mark_after_break_us: u32,
}

impl SerialSettings {
    /// ANSI E1.11: 250 kbaud, 8 data bits, 2 stop bits, no parity.
    pub const fn dmx512() -> Self {
        Self {
            baud: 250_000,
            stop_bits: 2,
            parity: Parity::None,
            break_us: 176,
            mark_after_break_us: 12,
        }
    }

    /// ENTTEC DMX USB Pro: 57600 8N1, the widget generates DMX timing itself.
    pub const fn enttec_pro() -> Self {
        Self {
            baud: 57_600,
            stop_bits: 1,
            parity: Parity::None,
            break_us: 0,
            mark_after_break_us: 0,
        }
    }

    /// MIDI Time Code: 31250 8N1, no break.
    pub const fn mtc() -> Self {
        Self {
            baud: 31_250,
            stop_bits: 1,
            parity: Parity::None,
            break_us: 0,
            mark_after_break_us: 0,
        }
    }
}

/// A bidirectional byte link with explicit line control.
pub trait SerialLink: Send {
    fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()>;
    /// Hold break for `hold`, then return to the mark (idle) state.
    fn send_break(&mut self, hold: Duration) -> DmxResult<()>;
    fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()>;
    fn drain(&mut self) -> DmxResult<()>;
    /// Read whatever bytes are available. `Ok(0)` means the port is idle; the
    /// caller decides how long to wait before polling again.
    fn read(&mut self, _buffer: &mut [u8]) -> DmxResult<usize> {
        Err(crate::DmxError::Unsupported(
            "this serial link cannot read".into(),
        ))
    }
    fn take_warning(&mut self) -> Option<String> {
        None
    }
}

/// Open a serial port for raw DMX use.
pub fn open(path: &str) -> DmxResult<Box<dyn SerialLink>> {
    platform::open(path)
}

#[cfg(target_os = "linux")]
mod platform {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    use super::{Parity, SerialLink, SerialSettings};
    use crate::{DmxError, DmxResult};

    // Linux UAPI values; libc does not expose termios2 on every target.
    const TCGETS2: libc::c_ulong = 0x802C_542A;
    const TCSETS2: libc::c_ulong = 0x402C_542B;
    const TIOCEXCL: libc::c_ulong = 0x540C;
    const TIOCSBRK: libc::c_ulong = 0x5427;
    const TIOCCBRK: libc::c_ulong = 0x5428;
    /// Baud rate lives in `c_ispeed`/`c_ospeed` rather than in `CBAUD`.
    const BOTHER: libc::tcflag_t = 0o010_000;
    const CBAUD: libc::tcflag_t = 0o010_017;
    const CRTSCTS: libc::tcflag_t = 0o020_000_000_000;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct Termios2 {
        c_iflag: libc::tcflag_t,
        c_oflag: libc::tcflag_t,
        c_cflag: libc::tcflag_t,
        c_lflag: libc::tcflag_t,
        c_line: libc::cc_t,
        c_cc: [libc::cc_t; 19],
        c_ispeed: libc::speed_t,
        c_ospeed: libc::speed_t,
    }

    pub struct PosixSerial {
        fd: OwnedFd,
        path: String,
        warning: Option<String>,
    }

    pub fn open(path: &str) -> DmxResult<Box<dyn SerialLink>> {
        let c_path = CString::new(path)
            .map_err(|_| DmxError::Unsupported(format!("invalid serial path: {path}")))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(DmxError::io(path, std::io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        // Best effort: keep other processes off the port.
        unsafe { libc::ioctl(fd.as_raw_fd(), TIOCEXCL) };
        Ok(Box::new(PosixSerial {
            fd,
            path: path.to_owned(),
            warning: None,
        }))
    }

    impl PosixSerial {
        fn err(&self) -> DmxError {
            DmxError::io(&self.path, std::io::Error::last_os_error())
        }
    }

    impl SerialLink for PosixSerial {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            let mut t: Termios2 = unsafe { std::mem::zeroed() };
            if unsafe { libc::ioctl(fd, TCGETS2, &mut t) } < 0 {
                return Err(self.err());
            }

            // Raw: no input/output translation, no echo, no signals, no flow control.
            t.c_iflag = 0;
            t.c_oflag = 0;
            t.c_lflag = 0;
            t.c_cflag &=
                !(CBAUD | libc::CSIZE | libc::CSTOPB | libc::PARENB | libc::PARODD | CRTSCTS);
            t.c_cflag |= BOTHER | libc::CS8 | libc::CREAD | libc::CLOCAL;
            if settings.stop_bits == 2 {
                t.c_cflag |= libc::CSTOPB;
            }
            match settings.parity {
                Parity::None => {}
                Parity::Even => t.c_cflag |= libc::PARENB,
                Parity::Odd => t.c_cflag |= libc::PARENB | libc::PARODD,
            }
            t.c_ispeed = settings.baud;
            t.c_ospeed = settings.baud;
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;

            if unsafe { libc::ioctl(fd, TCSETS2, &t) } < 0 {
                return Err(self.err());
            }
            unsafe { libc::tcflush(fd, libc::TCIOFLUSH) };
            Ok(())
        }

        fn send_break(&mut self, hold: Duration) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            if unsafe { libc::ioctl(fd, TIOCSBRK) } < 0 {
                let error = std::io::Error::last_os_error();
                let soft = matches!(
                    error.raw_os_error(),
                    Some(libc::ENOTTY) | Some(libc::EINVAL) | Some(libc::ENOSYS)
                );
                if soft {
                    // Pseudo terminals and a few adapters cannot drive break.
                    // Keep sending: the frame is still framed by the start code.
                    self.warning.get_or_insert_with(|| {
                        format!(
                            "{}: link cannot drive a break; frames are sent without DMX break",
                            self.path
                        )
                    });
                    return Ok(());
                }
                return Err(self.err());
            }
            std::thread::sleep(hold);
            loop {
                if unsafe { libc::ioctl(fd, TIOCCBRK) } == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(DmxError::io(&self.path, error));
                }
            }
        }

        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            let mut written = 0;
            while written < bytes.len() {
                let rc = unsafe {
                    libc::write(
                        fd,
                        bytes[written..].as_ptr() as *const libc::c_void,
                        bytes.len() - written,
                    )
                };
                if rc < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(DmxError::io(&self.path, error));
                }
                written += rc as usize;
            }
            Ok(())
        }

        fn drain(&mut self) -> DmxResult<()> {
            loop {
                if unsafe { libc::tcdrain(self.fd.as_raw_fd()) } == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                // A signal must not cost a frame: tcdrain is restartable.
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(DmxError::io(&self.path, error));
                }
            }
        }

        fn read(&mut self, buffer: &mut [u8]) -> DmxResult<usize> {
            loop {
                let count = unsafe {
                    libc::read(
                        self.fd.as_raw_fd(),
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                    )
                };
                if count >= 0 {
                    return Ok(count as usize);
                }
                let error = std::io::Error::last_os_error();
                match error.kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    std::io::ErrorKind::WouldBlock => return Ok(0),
                    _ => return Err(DmxError::io(&self.path, error)),
                }
            }
        }

        fn take_warning(&mut self) -> Option<String> {
            self.warning.take()
        }
    }
}

#[cfg(test)]
mod settings_tests {
    use super::{Parity, SerialSettings};

    #[test]
    fn mtc_uses_midi_serial_settings() {
        let settings = SerialSettings::mtc();
        assert_eq!(settings.baud, 31_250);
        assert_eq!(settings.stop_bits, 1);
        assert_eq!(settings.parity, Parity::None);
        assert_eq!(settings.break_us, 0);
        assert_eq!(settings.mark_after_break_us, 0);
    }
}

#[cfg(all(test, target_os = "linux"))]
mod pty_tests {
    //! End-to-end framing over a real character device.
    //!
    //! A pseudo terminal is not DMX hardware, but it does exercise the same
    //! code path as a USB-serial adapter: open, configure, break attempt, one
    //! continuous write and drain. It also proves the pty-tolerant break path
    //! keeps sending instead of failing.

    use std::ffi::CStr;
    use std::os::fd::RawFd;
    use std::time::{Duration, Instant};

    use super::SerialSettings;
    use crate::generic::GenericSerialTransport;
    use crate::{DmxTransport, DMX_SLOTS};

    /// Start code plus 512 slots.
    const FRAME_BYTES: usize = DMX_SLOTS + 1;

    fn master_and_slave_path() -> Option<(RawFd, String)> {
        let master = unsafe {
            libc::open(
                c"/dev/ptmx".as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if master < 0 {
            return None;
        }
        unsafe {
            if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
                libc::close(master);
                return None;
            }
            let flags = libc::fcntl(master, libc::F_GETFL);
            libc::fcntl(master, libc::F_SETFL, flags | libc::O_NONBLOCK);
            // `ptsname` writes to a process-global buffer, and the test harness
            // runs these tests on parallel threads. `ptsname_r` is the safe one.
            let mut name = [0 as libc::c_char; 128];
            if libc::ptsname_r(master, name.as_mut_ptr(), name.len()) != 0 {
                libc::close(master);
                return None;
            }
            let path = CStr::from_ptr(name.as_ptr()).to_string_lossy().into_owned();
            Some((master, path))
        }
    }

    fn read_at_least(master: RawFd, wanted: usize) -> Vec<u8> {
        let mut received = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut buffer = [0u8; 2048];
        while received.len() < wanted && Instant::now() < deadline {
            let count = unsafe {
                libc::read(
                    master,
                    buffer.as_mut_ptr() as *mut libc::c_void,
                    buffer.len(),
                )
            };
            if count > 0 {
                received.extend_from_slice(&buffer[..count as usize]);
            } else {
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        received
    }

    #[test]
    fn generic_frames_reach_a_character_device() {
        let Some((master, path)) = master_and_slave_path() else {
            eprintln!("skipped: this machine has no pseudo terminals");
            return;
        };

        let settings = SerialSettings::dmx512();
        let mut transport = GenericSerialTransport::open(&path, settings).expect("open pty slave");

        let mut slots = [0u8; DMX_SLOTS];
        slots[0] = 255;
        slots[7] = 17;
        slots[DMX_SLOTS - 1] = 3;
        transport.send_frame(&slots).unwrap();
        let warning = transport.take_warning();
        transport.shutdown().unwrap();

        let received = read_at_least(master, 2 * FRAME_BYTES);
        unsafe { libc::close(master) };

        assert!(
            received.len() >= FRAME_BYTES,
            "read {} bytes",
            received.len()
        );
        assert_eq!(received[0], 0, "start code precedes the slots");
        assert_eq!(received[1], 255, "address 1");
        assert_eq!(received[8], 17, "address 8");
        assert_eq!(received[DMX_SLOTS], 3, "address 512");

        if let Some(warning) = warning {
            assert!(
                warning.contains("break"),
                "the only expected pty warning is the missing DMX break: {warning}"
            );
            assert_eq!(received[0], 0, "frames are still framed by the start code");
        }
    }

    #[test]
    fn configuring_a_pty_uses_250000_8n2() {
        let Some((master, path)) = master_and_slave_path() else {
            eprintln!("skipped: this machine has no pseudo terminals");
            return;
        };
        let mut link = super::open(&path).expect("open pty slave");
        link.configure(&SerialSettings::dmx512())
            .expect("pty accepts termios2 with BOTHER");
        drop(link);
        unsafe { libc::close(master) };
    }
}

/// macOS: the same raw line discipline as Linux, through BSD `termios`.
///
/// Darwin has no `BOTHER`, so an arbitrary baud rate — 250000 for DMX and 31250
/// for MTC — needs `IOSSIOSPEED` after the ordinary settings. Break control is
/// `TIOCSBRK`/`TIOCCBRK`, which a USB-serial adapter driver honours exactly as
/// the Linux backend does.
#[cfg(target_os = "macos")]
mod platform {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::time::Duration;

    use super::{Parity, SerialLink, SerialSettings};
    use crate::{DmxError, DmxResult};

    /// `_IOW('I', 2, speed_t)`: the Darwin ioctl that accepts any baud rate.
    /// The size field is computed from `speed_t` rather than hard-coded, because
    /// it differs between the 32- and 64-bit headers.
    const IOSSIOSPEED: libc::c_ulong = iow(b'I', 2);

    const fn iow(group: u8, number: u8) -> libc::c_ulong {
        let size = std::mem::size_of::<libc::speed_t>() as u64;
        (0x8000_0000u64 | (size << 16) | ((group as u64) << 8) | number as u64) as libc::c_ulong
    }

    pub struct PosixSerial {
        fd: OwnedFd,
        path: String,
        warning: Option<String>,
    }

    pub fn open(path: &str) -> DmxResult<Box<dyn SerialLink>> {
        let c_path = CString::new(path)
            .map_err(|_| DmxError::Unsupported(format!("invalid serial path: {path}")))?;
        let fd = unsafe {
            libc::open(
                c_path.as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if fd < 0 {
            return Err(DmxError::io(path, std::io::Error::last_os_error()));
        }
        let fd = unsafe { OwnedFd::from_raw_fd(fd) };
        unsafe { libc::ioctl(fd.as_raw_fd(), libc::TIOCEXCL as libc::c_ulong) };
        Ok(Box::new(PosixSerial {
            fd,
            path: path.to_owned(),
            warning: None,
        }))
    }

    impl PosixSerial {
        fn err(&self) -> DmxError {
            DmxError::io(&self.path, std::io::Error::last_os_error())
        }
    }

    impl SerialLink for PosixSerial {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            let mut t: libc::termios = unsafe { std::mem::zeroed() };
            if unsafe { libc::tcgetattr(fd, &mut t) } < 0 {
                return Err(self.err());
            }
            unsafe { libc::cfmakeraw(&mut t) };
            t.c_cflag &=
                !(libc::CSIZE | libc::CSTOPB | libc::PARENB | libc::PARODD | libc::CRTSCTS);
            t.c_cflag |= libc::CS8 | libc::CREAD | libc::CLOCAL;
            if settings.stop_bits == 2 {
                t.c_cflag |= libc::CSTOPB;
            }
            match settings.parity {
                Parity::None => {}
                Parity::Even => t.c_cflag |= libc::PARENB,
                Parity::Odd => t.c_cflag |= libc::PARENB | libc::PARODD,
            }
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;
            if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &t) } < 0 {
                return Err(self.err());
            }
            // The standard termios speeds cannot express 250000, so the
            // adapter driver is asked for the exact rate after the line is raw.
            let speed: libc::speed_t = settings.baud as libc::speed_t;
            if unsafe { libc::ioctl(fd, IOSSIOSPEED, &speed) } < 0 {
                return Err(self.err());
            }
            unsafe { libc::tcflush(fd, libc::TCIOFLUSH) };
            Ok(())
        }

        fn send_break(&mut self, hold: Duration) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            if unsafe { libc::ioctl(fd, libc::TIOCSBRK as libc::c_ulong) } < 0 {
                let error = std::io::Error::last_os_error();
                let soft = matches!(
                    error.raw_os_error(),
                    Some(libc::ENOTTY) | Some(libc::EINVAL) | Some(libc::ENOSYS)
                );
                if soft {
                    self.warning.get_or_insert_with(|| {
                        format!(
                            "{}: link cannot drive a break; frames are sent without DMX break",
                            self.path
                        )
                    });
                    return Ok(());
                }
                return Err(self.err());
            }
            std::thread::sleep(hold);
            loop {
                if unsafe { libc::ioctl(fd, libc::TIOCCBRK as libc::c_ulong) } == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(DmxError::io(&self.path, error));
                }
            }
        }

        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            let fd = self.fd.as_raw_fd();
            let mut written = 0;
            while written < bytes.len() {
                let rc = unsafe {
                    libc::write(
                        fd,
                        bytes[written..].as_ptr() as *const libc::c_void,
                        bytes.len() - written,
                    )
                };
                if rc < 0 {
                    let error = std::io::Error::last_os_error();
                    if error.kind() == std::io::ErrorKind::Interrupted {
                        continue;
                    }
                    return Err(DmxError::io(&self.path, error));
                }
                written += rc as usize;
            }
            Ok(())
        }

        fn drain(&mut self) -> DmxResult<()> {
            loop {
                if unsafe { libc::tcdrain(self.fd.as_raw_fd()) } == 0 {
                    return Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    return Err(DmxError::io(&self.path, error));
                }
            }
        }

        fn read(&mut self, buffer: &mut [u8]) -> DmxResult<usize> {
            loop {
                let count = unsafe {
                    libc::read(
                        self.fd.as_raw_fd(),
                        buffer.as_mut_ptr() as *mut libc::c_void,
                        buffer.len(),
                    )
                };
                if count >= 0 {
                    return Ok(count as usize);
                }
                let error = std::io::Error::last_os_error();
                match error.kind() {
                    std::io::ErrorKind::Interrupted => continue,
                    std::io::ErrorKind::WouldBlock => return Ok(0),
                    _ => return Err(DmxError::io(&self.path, error)),
                }
            }
        }

        fn take_warning(&mut self) -> Option<String> {
            self.warning.take()
        }
    }
}

/// Windows: a native COM handle through `CreateFileW` and `DCB`.
///
/// A COM port is opened synchronously, configured for raw binary transfer and
/// driven with `SetCommBreak`/`ClearCommBreak` around the host-generated DMX
/// timing. The widget transports use the same link: ENTTEC only needs ordinary
/// 57600 8N1 traffic because the Pro generates DMX timing itself.
///
/// Windows cannot express an arbitrary baud rate through a terminal API, but
/// `DCB::BaudRate` is a plain integer and accepts 250000 directly. Read
/// timeouts are configured so an idle port returns immediately instead of
/// blocking the input thread.
#[cfg(target_os = "windows")]
mod platform {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
    use std::time::Duration;

    use windows_sys::Win32::Devices::Communication::{
        ClearCommBreak, GetCommState, PurgeComm, SetCommBreak, SetCommState, SetCommTimeouts,
        SetupComm, COMMTIMEOUTS, DCB, EVENPARITY, NOPARITY, ODDPARITY, ONESTOPBIT, PURGE_RXCLEAR,
        PURGE_TXCLEAR, TWOSTOPBITS,
    };
    use windows_sys::Win32::Foundation::{
        GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FlushFileBuffers, ReadFile, WriteFile, FILE_ATTRIBUTE_NORMAL, OPEN_EXISTING,
    };

    use super::{Parity, SerialLink, SerialSettings};
    use crate::{DmxError, DmxResult};

    /// One COM handle. `OwnedHandle` closes it on drop and makes the link
    /// `Send`, which the output thread requires.
    pub struct WindowsSerial {
        handle: OwnedHandle,
        path: String,
        warning: Option<String>,
    }

    pub fn open(path: &str) -> DmxResult<Box<dyn SerialLink>> {
        let wide = wide_device_path(path)?;
        let handle = unsafe {
            CreateFileW(
                wide.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                0,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE || handle.is_null() {
            return Err(DmxError::io(path, std::io::Error::last_os_error()));
        }
        let handle = unsafe { OwnedHandle::from_raw_handle(handle as RawHandle) };
        Ok(Box::new(WindowsSerial {
            handle,
            path: path.to_owned(),
            warning: None,
        }))
    }

    /// A COM port above 9 needs the `\\.\` device prefix, and accepting it
    /// for every port keeps the command line consistent. An already-prefixed
    /// path is passed through unchanged.
    fn wide_device_path(path: &str) -> DmxResult<Vec<u16>> {
        let path = path.trim();
        if path.is_empty() {
            return Err(DmxError::Unsupported(
                "a COM port path cannot be empty".into(),
            ));
        }
        if path.contains('\0') {
            return Err(DmxError::Unsupported(format!(
                "invalid serial path: {path}"
            )));
        }
        let device = if path.starts_with(r"\\.\") {
            path.to_owned()
        } else {
            format!(r"\\.\{path}")
        };
        Ok(OsStr::new(&device)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect())
    }

    fn apply_dcb(dcb: &mut DCB, settings: &SerialSettings) {
        dcb.DCBlength = std::mem::size_of::<DCB>() as u32;
        dcb.BaudRate = settings.baud;
        dcb.ByteSize = 8;
        dcb.Parity = match settings.parity {
            Parity::None => NOPARITY,
            Parity::Even => EVENPARITY,
            Parity::Odd => ODDPARITY,
        };
        dcb.StopBits = if settings.stop_bits == 2 {
            TWOSTOPBITS
        } else {
            ONESTOPBIT
        };

        // DCB bit field, least significant bit first:
        // fBinary, fParity, fOutxCtsFlow, fOutxDsrFlow, fDtrControl[2],
        // fDsrSensitivity, fTXContinueOnXoff, fOutX, fInX, fErrorChar, fNull,
        // fRtsControl[2], fAbortOnError. Binary and parity are the data-mode
        // bits; every flow-control and error bit stays off, and DTR/RTS stay
        // asserted so adapters that use them as a transceiver enable work.
        let mut bits = 1u32;
        if settings.parity != Parity::None {
            bits |= 1 << 1;
        }
        bits |= 1 << 4; // DTR_CONTROL_ENABLE
        bits |= 1 << 12; // RTS_CONTROL_ENABLE
        dcb._bitfield = bits;
        dcb.XonLim = 0;
        dcb.XoffLim = 0;
        dcb.XonChar = 0;
        dcb.XoffChar = 0;
        dcb.ErrorChar = 0;
        dcb.EofChar = 0;
        dcb.EvtChar = 0;
    }

    impl WindowsSerial {
        fn handle(&self) -> HANDLE {
            self.handle.as_raw_handle() as HANDLE
        }

        fn err(&self) -> DmxError {
            DmxError::io(&self.path, std::io::Error::last_os_error())
        }
    }

    impl SerialLink for WindowsSerial {
        fn configure(&mut self, settings: &SerialSettings) -> DmxResult<()> {
            if !matches!(settings.stop_bits, 1 | 2) {
                return Err(DmxError::Unsupported(format!(
                    "{}: unsupported stop-bit count {}",
                    self.path, settings.stop_bits
                )));
            }
            let handle = self.handle();
            let mut dcb = DCB::default();
            if unsafe { GetCommState(handle, &mut dcb) } == 0 {
                return Err(self.err());
            }
            apply_dcb(&mut dcb, settings);
            if unsafe { SetCommState(handle, &dcb) } == 0 {
                return Err(self.err());
            }
            if unsafe { SetupComm(handle, 4096, 4096) } == 0 {
                return Err(self.err());
            }
            let timeouts = COMMTIMEOUTS {
                // Return immediately when a read has no bytes, but never let a
                // write wedge the output thread forever.
                ReadIntervalTimeout: u32::MAX,
                ReadTotalTimeoutMultiplier: 0,
                ReadTotalTimeoutConstant: 0,
                WriteTotalTimeoutMultiplier: 0,
                WriteTotalTimeoutConstant: 2_000,
            };
            if unsafe { SetCommTimeouts(handle, &timeouts) } == 0 {
                return Err(self.err());
            }
            if unsafe { PurgeComm(handle, PURGE_TXCLEAR | PURGE_RXCLEAR) } == 0 {
                return Err(self.err());
            }
            Ok(())
        }

        fn send_break(&mut self, hold: Duration) -> DmxResult<()> {
            let handle = self.handle();
            if unsafe { SetCommBreak(handle) } == 0 {
                return Err(self.err());
            }
            std::thread::sleep(hold);
            if unsafe { ClearCommBreak(handle) } == 0 {
                return Err(self.err());
            }
            Ok(())
        }

        fn write_all(&mut self, bytes: &[u8]) -> DmxResult<()> {
            let handle = self.handle();
            let mut written = 0;
            while written < bytes.len() {
                let chunk = (bytes.len() - written).min(u32::MAX as usize) as u32;
                let mut count = 0u32;
                let ok = unsafe {
                    WriteFile(
                        handle,
                        bytes[written..].as_ptr(),
                        chunk,
                        &mut count,
                        std::ptr::null_mut(),
                    )
                };
                if ok == 0 {
                    return Err(self.err());
                }
                if count == 0 {
                    return Err(DmxError::io(
                        &self.path,
                        std::io::Error::new(std::io::ErrorKind::TimedOut, "COM write timed out"),
                    ));
                }
                written += count as usize;
            }
            Ok(())
        }

        fn drain(&mut self) -> DmxResult<()> {
            if unsafe { FlushFileBuffers(self.handle()) } == 0 {
                return Err(self.err());
            }
            Ok(())
        }

        fn read(&mut self, buffer: &mut [u8]) -> DmxResult<usize> {
            if buffer.is_empty() {
                return Ok(0);
            }
            let mut count = 0u32;
            let ok = unsafe {
                ReadFile(
                    self.handle(),
                    buffer.as_mut_ptr(),
                    buffer.len().min(u32::MAX as usize) as u32,
                    &mut count,
                    std::ptr::null_mut(),
                )
            };
            if ok == 0 {
                return Err(self.err());
            }
            Ok(count as usize)
        }

        fn take_warning(&mut self) -> Option<String> {
            self.warning.take()
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        fn decoded(path: &[u16]) -> String {
            String::from_utf16_lossy(&path[..path.len().saturating_sub(1)])
        }

        #[test]
        fn com_paths_get_the_device_prefix() {
            assert_eq!(decoded(&wide_device_path("COM3").unwrap()), r"\\.\COM3");
            assert_eq!(
                decoded(&wide_device_path(r"\\.\COM10").unwrap()),
                r"\\.\COM10"
            );
            assert_eq!(decoded(&wide_device_path("COM3 ").unwrap()), r"\\.\COM3");
        }

        #[test]
        fn an_empty_com_path_is_refused() {
            let error = wide_device_path("  ").unwrap_err();
            assert!(error.to_string().contains("cannot be empty"), "{error}");
        }

        #[test]
        fn dmx_dcb_is_250000_8n2_without_flow_control() {
            let mut dcb = DCB::default();
            apply_dcb(&mut dcb, &SerialSettings::dmx512());
            assert_eq!(dcb.BaudRate, 250_000);
            assert_eq!(dcb.ByteSize, 8);
            assert_eq!(dcb.StopBits, TWOSTOPBITS);
            assert_eq!(dcb.Parity, NOPARITY);
            assert_eq!(dcb._bitfield & 1, 1, "binary mode");
            assert_eq!(dcb._bitfield & (1 << 1), 0, "parity disabled");
            assert_eq!(dcb._bitfield & (1 << 2), 0, "CTS flow control off");
            assert_eq!(dcb._bitfield & (1 << 3), 0, "DSR flow control off");
            assert_eq!((dcb._bitfield >> 4) & 3, 1, "DTR enabled");
            assert_eq!((dcb._bitfield >> 12) & 3, 1, "RTS enabled");
        }

        #[test]
        fn enttec_dcb_is_57600_8n1() {
            let mut dcb = DCB::default();
            apply_dcb(&mut dcb, &SerialSettings::enttec_pro());
            assert_eq!(dcb.BaudRate, 57_600);
            assert_eq!(dcb.StopBits, ONESTOPBIT);
            assert_eq!(dcb.Parity, NOPARITY);
        }

        #[test]
        fn parity_and_stop_bits_map_to_dcb_fields() {
            let mut dcb = DCB::default();
            apply_dcb(
                &mut dcb,
                &SerialSettings {
                    baud: 31_250,
                    stop_bits: 1,
                    parity: Parity::Odd,
                    break_us: 0,
                    mark_after_break_us: 0,
                },
            );
            assert_eq!(dcb.Parity, ODDPARITY);
            assert_eq!(dcb._bitfield & (1 << 1), 1, "parity enabled");
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "macos", target_os = "windows")))]
mod platform {
    use super::{SerialLink, SerialSettings};
    use crate::{DmxError, DmxResult};
    use std::time::Duration;

    pub fn open(path: &str) -> DmxResult<Box<dyn SerialLink>> {
        Err(DmxError::Unsupported(format!(
            "raw DMX framing is not implemented on this platform (requested {path}); \
             use `virtual` or an ENTTEC widget"
        )))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn the_fallback_refuses_a_serial_port_by_name() {
            match open("/dev/ttyUSB0") {
                Err(DmxError::Unsupported(message)) => {
                    assert!(message.contains("not implemented"), "{message}");
                    assert!(message.contains("/dev/ttyUSB0"), "{message}");
                }
                _ => panic!("the fallback must report Unsupported"),
            }
        }
    }

    struct Unsupported;

    impl SerialLink for Unsupported {
        fn configure(&mut self, _settings: &SerialSettings) -> DmxResult<()> {
            Ok(())
        }
        fn send_break(&mut self, _hold: Duration) -> DmxResult<()> {
            Ok(())
        }
        fn write_all(&mut self, _bytes: &[u8]) -> DmxResult<()> {
            Ok(())
        }
        fn drain(&mut self) -> DmxResult<()> {
            Ok(())
        }
        fn read(&mut self, _buffer: &mut [u8]) -> DmxResult<usize> {
            Err(crate::DmxError::Unsupported(
                "serial input is only implemented for Linux in this build".into(),
            ))
        }
    }
}
