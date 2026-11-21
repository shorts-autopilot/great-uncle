# 23 · Windows serial backend

**Rank** 6 (follow-up) · **Order** 23 · **Size** M · **Show —, desk —** · **Depends on** 01 (output runtime), 17 (release readiness)

## Problem

The output layer is only a real desk on Linux and macOS. Windows builds and
runs the virtual transport, but `serial.rs` has no COM implementation: a
`generic:COM3` or `enttec:COM3` output reports `unsupported` before it opens a
port. Every Windows user with a USB-DMX adapter is therefore limited to the
loopback, even though ENTTEC Pro and most generic adapters present themselves
as an ordinary COM port.

This is not only a missing convenience. The serial backend is the last
platform-specific piece of the output path, and it is inserted below the
transport layer that already owns start codes, break/MAB and ENTTEC framing.
Adding it must not create a second framing path, a second output thread or a
show/desk field. It also must not turn a missing driver into a silent blackout:
if Windows cannot drive a line break, a generic DMX frame is not valid and the
transport has to say so.

The hardware proof is still the release gate. This item can make the backend
build, test its pure mapping, and give a Windows operator a probe; it cannot
claim that a particular adapter, driver or PC achieves DMX timing.

## Behaviour when this is done

```
--output 1=generic:COM3
--output 1=generic:\\.\COM10
--output 1=enttec:COM3
--input 1=enttec:COM4
```

```text
> openlight --probe
COM3  generic
  3 frames in 94 ms (31.9 Hz), break 176 µs, MAB 12 µs, jitter 2.1 ms
COM10  generic
  unavailable: ...: The system cannot find the file specified.

> openlight --output 1=generic:COM3
Output 1  generic:COM3  online  universe 1
```

- `generic:COM3` opens the Windows COM device, configures 250000 baud,
  8 data bits, 2 stop bits, no parity, binary mode and no flow control, then
  frames every universe send with break, mark-after-break, start code and 512
  slots exactly as the Linux and macOS backends do.
- `enttec:COM3` uses the same link at 57600 8N1. The widget owns DMX timing, so
  the host sends no break; the existing ENTTEC envelope is unchanged.
- A port above COM9 may be written as `generic:COM10` or
  `generic:\\.\COM10`; both resolve to the same device. A path that already
  starts with `\\.\` is not prefixed twice.
- `--probe` discovers the active COM ports from the Windows
  `HARDWARE\DEVICEMAP\SERIALCOMM` device map, so a port that Device Manager
  shows is offered without spelling it out. Configured `--output` specs are
  still probed even when the map is empty.
- Input uses the same COM link. `enttec:COM4` can receive widget label-5
  frames; `generic:` input remains unsupported for the same reason as on every
  other platform, because a bare receiver needs break detection that has not
  been proven on hardware.
- A failed write, a removed adapter or a closed handle is a normal transport
  error. The existing output thread reports it and calls `reopen`; the Windows
  link closes its old handle and opens the original COM name again. A device
  that is unplugged and replugged under the same COM name recovers, while a
  device that comes back under a different name needs a new binding.
- A line that cannot enter break is a hard error for `generic:`. Sending a
  DMX frame without break would be worse than reporting the failure: the
  receiver would not frame it as DMX. ENTTEC is unaffected because it never
  asks for break.
- No command, show field or desk field is added. The support matrix and the
  probe report change; the show and desk formats do not.

## Design

### A target-only Win32 dependency

```toml
[target.'cfg(windows)'.dependencies]
windows-sys = { version = "0.61", features = [
    "Win32_Devices_Communication",
    "Win32_Foundation",
    "Win32_Security",
    "Win32_Storage_FileSystem",
    "Win32_System_IO",
    "Win32_System_Registry",
] }
```

`serial.rs` keeps its existing `SerialLink` contract: configure, break, write,
drain, read and warning. The Windows module implements that contract directly
with `CreateFileW`, `GetCommState`/`SetCommState`, `SetCommTimeouts`,
`SetCommBreak`/`ClearCommBreak`, `WriteFile`, `ReadFile` and
`FlushFileBuffers`. No cross-platform serial crate is introduced: the project
needs explicit break control and an exact 250000 baud rate, not a terminal
compatibility layer, so the platform module stays the only unsafe code in the
DMX crate alongside the USB transport.

`windows-sys` is target-only, so a Linux or macOS build does not download or
compile it. `libc` becomes a Unix-only dependency for the same reason; Windows
uses no POSIX calls. The fallback module remains for every other target and
still refuses a named port rather than pretending to configure it.

### Opening and configuring a COM port

```rust
pub struct WindowsSerial {
    handle: OwnedHandle,
    path: String,
    warning: Option<String>,
}
```

- `wide_device_path` trims the operator's path, rejects an empty path and
  rejects an embedded NUL, then prefixes `\\.\` unless it is already present.
  The prefix is required for COM10 and above and harmless for lower numbers.
  The original path, not the device path, is kept for error messages and for
  `reopen`.
- `CreateFileW` uses `GENERIC_READ | GENERIC_WRITE`, no sharing, `OPEN_EXISTING`
  and `FILE_ATTRIBUTE_NORMAL`, which is the standard way to claim a COM port.
  The returned handle is converted to `OwnedHandle`, so it is closed exactly
  once on drop and the link remains `Send` for the output thread.
- `GetCommState` reads the driver's current `DCB`; then the backend overwrites
  only the fields DMX owns. `DCBlength` is set to the struct size, `BaudRate` to
  the requested rate, `ByteSize` to 8, parity from the settings, and stop bits
  to `TWOSTOPBITS` or `ONESTOPBIT`.
- The opaque `DCB::_bitfield` is set explicitly: binary mode on, parity bit on
  only for even/odd parity, all CTS/DSR/XON/XOFF/error handling off, DTR and
  RTS asserted. Asserting DTR/RTS matches a port left open by a terminal and
  keeps adapters that use those lines as a transceiver enable working; no
  hardware handshake is enabled. The XON/XOFF characters are zeroed so a
  driver cannot interpret data bytes as flow control.
- `SetupComm` requests 4 KiB in each direction. `SetCommTimeouts` makes a read
  return immediately when no bytes are available (`ReadIntervalTimeout =
  MAXDWORD`, both read totals zero) and gives a whole write two seconds
  (`WriteTotalTimeoutConstant = 2000`). Both settings are per handle and are
  reapplied by `reopen`.
- `PurgeComm(PURGE_TXCLEAR | PURGE_RXCLEAR)` discards anything stale after the
  line is configured. This is especially important when the desk reopens a
  device that was replugged with an old frame still queued.

The link reports the OS error through the existing `DmxError::io(path, …)`, so
a permission problem, a missing port and a driver rejection all keep the
operator's port name in the message.

### Break and mark-after-break

`GenericSerialTransport` already calls `send_break(hold)`, sleeps for the
mark-after-break, writes one contiguous start-code-plus-slots buffer, then
drains. On Windows `send_break` is:

1. `SetCommBreak(handle)`; failure is returned, never softened to a warning.
2. sleep for the requested hold (176 µs for DMX512).
3. `ClearCommBreak(handle)`; failure is returned.

The Windows scheduler cannot promise a precise 176 µs hold. `--probe` measures
the result, and a logic-analyser capture is still required before any row is
called verified. The design deliberately does not shrink the hold to a
theoretical minimum: a late clear is a longer break, which is legal, while an
early clear is a short or missing break, which is not.

`WriteFile` can complete after writing fewer bytes than requested. The link
loops until the whole frame is out, and a zero-byte completion with success is
reported as `TimedOut`. `FlushFileBuffers` is the Windows equivalent of
`tcdrain`: it waits for the driver's transmit queue, so the next break cannot
overtake the previous frame.

`EnttecProTransport` uses `SerialSettings::enttec_pro()` (`break_us = 0`) and
therefore never calls `send_break`; its 57600 8N1 link is ordinary binary COM
traffic. `EnttecProInput` uses the same read path and sends its label-8
receive-mode request through `write_all`.

### Probe and device discovery

`probe::candidates` currently knows Linux `by-id`, `ttyUSB` and `ttyACM`, and
macOS `cu.*`. Windows adds a registry reader for
`HKEY_LOCAL_MACHINE\HARDWARE\DEVICEMAP\SERIALCOMM`, the map Device Manager
updates for active ports. Values are `REG_SZ`, so the bytes are decoded as
UTF-16LE and a trailing NUL is removed. Each value becomes
`generic:<name>`, including names above COM9.

The registry read is best-effort: a missing key or a denied query yields no
candidates rather than a probe failure. The operator can always name a port in
`--output` or pass it explicitly, and the engine already adds configured
outputs to the probe list. The map is read at the moment of the probe; there is
no background watcher and no dependency on SetupAPI friendly names.

### CI and packaging

- The Windows platform job already runs the workspace tests with
  `--no-default-features`. Once the module exists, that job compiles the real
  COM code and runs the path, DCB and registry-decoding tests. The job comment
  is updated rather than claiming Windows has no serial backend.
- A Windows-only clippy step checks the target module with `-D warnings`, so a
  warning on a branch Linux clippy never compiles cannot slip through.
- The local cross-check is
  `cargo check -p openlight-dmx --target x86_64-pc-windows-gnu
  --no-default-features --all-targets`. It proves the target compiles; it is
  not a substitute for running the tests on Windows, which CI does.
- `packaging/windows/make-zip.ps1` builds the console and, unless `-NoGui`,
  the desktop binary, then writes `dist/openlight-<version>-<target>.zip` with
  the binaries, docs, examples, licence and the install scripts. It is a
  portable package, not a machine-wide MSI.
- `packaging/windows/install.ps1` copies the two executables into
  `%LOCALAPPDATA%\Programs\OpenLight` and creates Start Menu shortcuts for the
  current user. It needs no administrator rights. `uninstall.ps1` removes
  exactly those files and shortcuts. Neither script edits the machine PATH;
  the Start Menu entry is the supported way to launch the desk.
- No code signing, MSI, MSIX, auto-update or notarisation is added. A signed
  installer is a distribution decision, and SmartScreen may warn about an
  unsigned download.

### Safety

The Windows branch is `unsafe` because every Win32 call is a foreign function.
The rules are narrow:

- a handle is owned by `OwnedHandle` from the moment `CreateFileW` succeeds;
- every buffer pointer passed to `WriteFile`/`ReadFile` comes from a live
  slice, and the length is capped to `u32::MAX`;
- the `DCB` and `COMMTIMEOUTS` pointers are valid for the duration of the
  call;
- registry buffers are sized from `RegQueryInfoKeyW` and the returned lengths
  are bounded by those allocations before slicing;
- every successful registry open has exactly one `RegCloseKey`, including the
  early-return paths.

`cargo clippy` and the Windows test job are the enforcement; this module is not
excluded from the workspace lint.

## Tests

- **COM paths** — `COM3` gains the `\\.\` prefix, `\\.\COM10` is unchanged,
  surrounding whitespace is trimmed and an empty path is refused with a
  message naming the path rule.
- **DCB mapping** — DMX512 is 250000, 8 data bits, two stop bits, no parity,
  binary mode, no flow control and DTR/RTS enabled; ENTTEC is 57600 8N1; odd
  parity sets the parity bit. These run on the Windows CI job.
- **Registry decoding** — a UTF-16LE `COM10\0` value becomes `COM10`, with the
  trailing NUL removed.
- **Existing transport tests** — the generic and ENTTEC recording links still
  prove one break per generic frame, no break for ENTTEC, the start code and
  the 512-slot payload. The Windows module only supplies the byte link, so those
  framing tests stay valid.
- **Output recovery** — the existing output-thread tests prove that a failed
  transport is re-opened and that a failed reopen is reported. The COM link's
  `reopen` constructs a fresh handle from the original path.
- **Cross-compile** — the Windows target check above runs on the development
  host and in review; CI runs the tests natively.
- **Not tested here** — no Windows machine, USB-serial adapter, ENTTEC widget,
  logic analyser or COM driver is available. The module has been
  cross-compiled, not run against a device. The hardware matrix in item 1
  remains the release gate.

## Boundaries

- No Windows generic DMX input. The read path exists for ENTTEC and for future
  RDM, but the current `generic:` input refusal is unchanged.
- No SetupAPI enumeration, port friendly names or hot-plug watcher. The probe
  reads the active-port map once, and `reopen` retries by name.
- No change to Windows USB (`udmx`) support, installer signing, services,
  MSI/MSIX or auto-update.
- No claim that `SetCommBreak` produces an E1.11-compliant waveform on any
  particular driver. The probe and logic-analyser capture are the proof.
- No change to show or desk versions, commands or the engine's render path.

## Risks

- **Break timing is driver- and scheduler-dependent.** Windows may stretch the
  hold far beyond 176 µs. The probe reports it; a stretched break is safe for
  DMX receivers, but it reduces the achievable frame rate.
- **COM port naming.** The `\\.\` prefix covers normal COM names, but a
  vendor-specific device path may need its own syntax. The error keeps the
  original name and the docs show both accepted forms.
- **Registry discovery is not SetupAPI.** A port that is present but not in
  `SERIALCOMM` will not be auto-detected; naming it explicitly still works.
- **An unsigned ZIP is not a trust story.** SmartScreen and enterprise policy
  can block it. Signing is intentionally left to release engineering.
- **The hardware gate remains.** A compiling backend is not a verified output
  path. The adapter × OS × driver × Hz × jitter × break/MAB matrix and the
  one-hour soak still have to be run before `verified` is used.
