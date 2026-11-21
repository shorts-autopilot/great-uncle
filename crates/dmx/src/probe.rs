//! Hardware probe: prove what a serial port actually does with DMX frames.
//!
//! The probe opens a port with the same transport the console uses, sends three
//! whole frames back to back and measures how long they took. That is the only
//! code path in the project that can put a number on the electrical timing
//! story, and the report is written to be pasted into a bug report.
//!
//! A device that is not a DMX interface may accept the `open` and then block on
//! the first write. The frames run on their own thread so a two-second timeout
//! can report `unknown (no DMX framing observed)` instead of hanging the
//! console forever.

use std::sync::mpsc::{self, RecvTimeoutError};
use std::time::{Duration, Instant};

use crate::{serial::SerialSettings, DmxResult, DmxTransport, DMX_SLOTS};

/// How a probe is run. The defaults match the hardware report in the docs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Whole frames to send. Three is enough to see a rate and a jitter.
    pub frames: usize,
    /// Wall-clock cap for all frames of one port.
    pub timeout: Duration,
}

impl Default for ProbeOptions {
    fn default() -> Self {
        Self {
            frames: 3,
            timeout: Duration::from_secs(2),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeOutcome {
    /// Frames left the port. `elapsed` is measured from just before the first
    /// frame to just after the last, so the rate is what the link sustained.
    Framed {
        frames: usize,
        elapsed: Duration,
        break_us: u32,
        mark_after_break_us: u32,
        jitter: Duration,
    },
    /// The port opened but the frames did not finish inside the timeout. That
    /// is the signature of a serial device that does not speak DMX.
    NoFraming,
    /// The transport refused to open, or failed while sending.
    Unavailable(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeReport {
    /// The port path, or the spec itself for a non-serial transport.
    pub port: String,
    /// `generic`, `enttec`, `virtual`, …
    pub kind: String,
    /// A non-fatal warning from the transport, e.g. a pty that cannot break.
    pub warning: Option<String>,
    pub outcome: ProbeOutcome,
}

impl ProbeReport {
    /// The two-line report the console prints, one port at a time.
    pub fn lines(&self) -> Vec<String> {
        let mut lines = vec![format!("{}  {}", self.port, self.kind)];
        match &self.outcome {
            ProbeOutcome::Framed {
                frames,
                elapsed,
                break_us,
                mark_after_break_us,
                jitter,
            } => {
                let millis = elapsed.as_secs_f64() * 1000.0;
                let hz = if elapsed.is_zero() {
                    0.0
                } else {
                    *frames as f64 / elapsed.as_secs_f64()
                };
                lines.push(format!(
                    "  {frames} frames in {millis:.0} ms ({hz:.1} Hz), break {break_us} µs, \
                     MAB {mark_after_break_us} µs, jitter {:.1} ms",
                    jitter.as_secs_f64() * 1000.0
                ));
            }
            ProbeOutcome::NoFraming => {
                lines.push("  unknown (no DMX framing observed)".into());
            }
            ProbeOutcome::Unavailable(error) => {
                lines.push(format!("  unavailable: {error}"));
            }
        }
        if let Some(warning) = &self.warning {
            lines.push(format!("  warning: {warning}"));
        }
        lines
    }
}

/// Ports worth probing on this platform, as transport specs. The configured
/// outputs are added by the caller, so a port that is not auto-detected can
/// still be named explicitly.
pub fn candidates() -> Vec<String> {
    let mut specs = Vec::new();
    #[cfg(target_os = "linux")]
    {
        collect("/dev/serial/by-id", None, &mut specs);
        collect("/dev", Some("ttyUSB"), &mut specs);
        collect("/dev", Some("ttyACM"), &mut specs);
    }
    #[cfg(target_os = "macos")]
    {
        collect("/dev", Some("cu."), &mut specs);
    }
    specs.sort();
    specs.dedup();
    specs
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn collect(directory: &str, prefix: Option<&str>, specs: &mut Vec<String>) {
    let Ok(entries) = std::fs::read_dir(directory) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if let Some(prefix) = prefix {
            if !name.starts_with(prefix) {
                continue;
            }
        }
        specs.push(format!("generic:{directory}/{name}"));
    }
}

/// Probe every spec, keeping the order and dropping duplicates.
pub fn run(specs: &[String], options: ProbeOptions) -> Vec<ProbeReport> {
    let mut seen = Vec::new();
    let mut reports = Vec::new();
    for spec in specs {
        if seen.iter().any(|known| known == spec) {
            continue;
        }
        seen.push(spec.clone());
        reports.push(probe_spec(spec, options));
    }
    reports
}

/// Probe one transport spec, e.g. `generic:/dev/ttyUSB0` or `enttec:/dev/ttyUSB0`.
pub fn probe_spec(spec: &str, options: ProbeOptions) -> ProbeReport {
    probe_with(spec, options, crate::open)
}

fn probe_with<F>(spec: &str, options: ProbeOptions, open: F) -> ProbeReport
where
    F: FnOnce(&str) -> DmxResult<Box<dyn DmxTransport>>,
{
    let (kind, port) = split_spec(spec);
    let report = |outcome| ProbeReport {
        port: port.to_string(),
        kind: kind.to_string(),
        warning: None,
        outcome,
    };
    let mut transport = match open(spec) {
        Ok(transport) => transport,
        Err(error) => return report(ProbeOutcome::Unavailable(error.to_string())),
    };
    let frames = options.frames.max(1);
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let start = Instant::now();
        let mut completed = Vec::with_capacity(frames);
        let mut warning = None;
        for _ in 0..frames {
            if let Err(error) = transport.send_frame(&[0u8; DMX_SLOTS]) {
                let _ = sender.send(Event::Failed(error.to_string()));
                return;
            }
            completed.push(Instant::now());
            if let Some(text) = transport.take_warning() {
                warning = Some(text);
            }
        }
        let _ = sender.send(Event::Done {
            elapsed: completed.last().copied().unwrap_or(start) - start,
            completed,
            warning,
        });
    });

    match receiver.recv_timeout(options.timeout) {
        Ok(Event::Done {
            elapsed,
            completed,
            warning,
        }) => {
            let jitter = jitter(&completed);
            let (break_us, mark_after_break_us) = settings_for(kind);
            ProbeReport {
                port: port.to_string(),
                kind: kind.to_string(),
                warning,
                outcome: ProbeOutcome::Framed {
                    frames,
                    elapsed,
                    break_us,
                    mark_after_break_us,
                    jitter,
                },
            }
        }
        Ok(Event::Failed(error)) => report(ProbeOutcome::Unavailable(error)),
        Err(RecvTimeoutError::Timeout) => report(ProbeOutcome::NoFraming),
        Err(RecvTimeoutError::Disconnected) => {
            report(ProbeOutcome::Unavailable("probe thread stopped".into()))
        }
    }
}

enum Event {
    Done {
        elapsed: Duration,
        completed: Vec<Instant>,
        warning: Option<String>,
    },
    Failed(String),
}

/// Spread between the shortest and longest gap between two completed frames.
fn jitter(completed: &[Instant]) -> Duration {
    let gaps: Vec<Duration> = completed
        .windows(2)
        .map(|pair| pair[1].duration_since(pair[0]))
        .collect();
    match (gaps.iter().min().copied(), gaps.iter().max().copied()) {
        (Some(min), Some(max)) => max - min,
        _ => Duration::ZERO,
    }
}

fn split_spec(spec: &str) -> (&str, &str) {
    match spec.split_once(':') {
        Some((kind, argument)) => (kind, argument),
        None => (spec, spec),
    }
}

/// The framing a transport is expected to generate, so the report can name it.
fn settings_for(kind: &str) -> (u32, u32) {
    let settings = match kind {
        "generic" => SerialSettings::dmx512(),
        "enttec" => SerialSettings::enttec_pro(),
        _ => return (0, 0),
    };
    (settings.break_us, settings.mark_after_break_us)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    struct Fake {
        frames: Arc<Mutex<usize>>,
        block: bool,
    }

    impl DmxTransport for Fake {
        fn describe(&self) -> String {
            "fake".into()
        }
        fn send_frame(&mut self, _slots: &[u8; DMX_SLOTS]) -> DmxResult<()> {
            if self.block {
                std::thread::sleep(Duration::from_millis(300));
            }
            *self.frames.lock().unwrap() += 1;
            Ok(())
        }
    }

    #[test]
    fn three_frames_are_measured_and_reported() {
        let frames = Arc::new(Mutex::new(0));
        let count = frames.clone();
        let report = probe_with(
            "generic:/dev/fake",
            ProbeOptions {
                frames: 3,
                timeout: Duration::from_secs(1),
            },
            move |_| {
                Ok(Box::new(Fake {
                    frames: count,
                    block: false,
                }) as Box<dyn DmxTransport>)
            },
        );
        assert_eq!(report.port, "/dev/fake");
        assert_eq!(report.kind, "generic");
        assert!(matches!(
            report.outcome,
            ProbeOutcome::Framed {
                frames: 3,
                break_us: 176,
                mark_after_break_us: 12,
                ..
            }
        ));
        assert_eq!(*frames.lock().unwrap(), 3);
        let lines = report.lines();
        assert_eq!(lines.len(), 2);
        assert!(lines[0].contains("/dev/fake"), "{lines:?}");
        assert!(lines[1].contains("3 frames in"), "{lines:?}");
        assert!(lines[1].contains("break 176 µs"), "{lines:?}");
    }

    #[test]
    fn a_blocked_write_reports_no_framing_inside_the_timeout() {
        let report = probe_with(
            "generic:/dev/fake",
            ProbeOptions {
                frames: 3,
                timeout: Duration::from_millis(50),
            },
            |_| {
                Ok(Box::new(Fake {
                    frames: Arc::new(Mutex::new(0)),
                    block: true,
                }) as Box<dyn DmxTransport>)
            },
        );
        assert_eq!(report.outcome, ProbeOutcome::NoFraming);
        assert_eq!(report.lines()[1], "  unknown (no DMX framing observed)");
    }

    #[test]
    fn an_unopenable_port_reports_the_error() {
        let report = probe_with("generic:/dev/missing", ProbeOptions::default(), |_| {
            Err(crate::DmxError::Unsupported("no such port".into()))
        });
        assert_eq!(
            report.outcome,
            ProbeOutcome::Unavailable("no such port".into())
        );
        assert!(report.lines()[1].contains("unavailable: no such port"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_real_character_device_is_probed_through_generic_open() {
        use std::ffi::CStr;
        use std::os::fd::RawFd;

        let master: RawFd = unsafe {
            libc::open(
                c"/dev/ptmx".as_ptr(),
                libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
            )
        };
        if master < 0 {
            eprintln!("skipped: this machine has no pseudo terminals");
            return;
        }
        unsafe {
            if libc::grantpt(master) != 0 || libc::unlockpt(master) != 0 {
                libc::close(master);
                eprintln!("skipped: this machine has no pseudo terminals");
                return;
            }
        }
        // `ptsname_r`, not `ptsname`: the test harness runs tests in parallel
        // and `ptsname` returns a shared static buffer.
        let mut name = [0 as libc::c_char; 128];
        if unsafe { libc::ptsname_r(master, name.as_mut_ptr(), name.len()) } != 0 {
            unsafe { libc::close(master) };
            eprintln!("skipped: this machine has no pseudo terminals");
            return;
        }
        let path = unsafe { CStr::from_ptr(name.as_ptr()) }
            .to_string_lossy()
            .into_owned();

        let report = probe_spec(
            &format!("generic:{path}"),
            ProbeOptions {
                frames: 3,
                timeout: Duration::from_secs(2),
            },
        );
        unsafe { libc::close(master) };

        assert!(
            matches!(report.outcome, ProbeOutcome::Framed { frames: 3, .. }),
            "{report:?}"
        );
        assert_eq!(report.port, path);
    }

    #[test]
    fn run_keeps_order_and_drops_duplicates() {
        let reports = run(
            &[
                "generic:/a".into(),
                "generic:/a".into(),
                "generic:/b".into(),
            ],
            ProbeOptions::default(),
        );
        assert_eq!(reports.len(), 2);
        assert_eq!(reports[0].port, "/a");
        assert_eq!(reports[1].port, "/b");
    }
}
