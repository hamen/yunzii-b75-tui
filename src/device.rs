//! Device discovery and the send+ACK transport over `/dev/hidraw*`.
//!
//! See `PROTOCOL.md`'s "Interface identity" section for how this exact
//! interface was identified in Phase 0 (`hamen/yunzii-b75-tui` PR #1):
//! VID 0x28E9 / PID 0x31C8, the config channel's report descriptor decodes
//! to usage page 0xFF60 / usage 0x61, 64-byte in/out reports, no Report ID
//! item (an unnumbered-report interface).

use std::fs;
use std::io;
use std::os::fd::{AsFd, AsRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nix::fcntl::{OFlag, open};
use nix::poll::{PollFd, PollFlags, PollTimeout, poll};
use nix::sys::stat::Mode;
use nix::unistd::{read, write};

const VENDOR_ID: u16 = 0x28E9;
const PRODUCT_ID: u16 = 0x31C8;

/// The exact report-descriptor bytes for the config channel, captured and
/// verified in Phase 0 (`fields.json`'s `linuxInterfaceIdentity`). Matching
/// against these exact bytes -- not a general HID report-descriptor parser
/// -- is an accepted, documented limitation for this milestone: a firmware
/// revision with a byte-identical config channel but different padding
/// elsewhere would be rejected. If this ever needs re-deriving: read
/// `/sys/class/hidraw/hidrawN/device/report_descriptor` directly on Linux
/// (`xxd -p` that file) for the interface with usage page `0xFF60` / usage
/// `0x61` -- this is exactly how these bytes were originally captured.
const EXPECTED_REPORT_DESCRIPTOR: [u8; 34] = [
    0x06, 0x60, 0xff, 0x09, 0x61, 0xa1, 0x01, 0x09, 0x62, 0x15, 0x00, 0x26, 0xff, 0x00, 0x95, 0x40,
    0x75, 0x08, 0x81, 0x02, 0x09, 0x63, 0x15, 0x00, 0x26, 0xff, 0x00, 0x95, 0x40, 0x75, 0x08, 0x91,
    0x02, 0xc0,
];

const REPORT_LEN: usize = 64;
const ACK_TIMEOUT: Duration = Duration::from_millis(500);
// Iteration cap, NOT a timing mechanism: a correctly-behaving O_NONBLOCK fd
// returns EAGAIN immediately once truly empty, so drain() should never need
// many iterations. This bounds a pathologically chatty node -- it must
// never make drain() sleep-wait on the normal (empty) path (an earlier
// version used poll() with a timeout here, which meant the happy path
// always blocked for the full timeout even with nothing to drain).
const DRAIN_MAX_ITERATIONS: usize = 64;
// Phase 0's WebHID capture observed 2 identical `inputreport` events per
// outbound report -- but empirical testing against real hardware via native
// hidraw (this milestone) shows only ONE real ACK arrives at that layer.
// The "2 ACKs" pattern was a WebHID/Chrome-side artifact (e.g. dispatched
// to two internal listeners), not a fact about the wire protocol itself.
const EXPECTED_ACKS_PER_WRITE: usize = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReportIdForm {
    /// hidraw write() gets a leading 0x00 "report number" byte prepended
    /// (65 bytes total); read() returns the 64-byte report with NO such
    /// prefix. This is the documented Linux kernel behavior for unnumbered-
    /// report HID devices, and is CONFIRMED correct against real hardware
    /// (see PROTOCOL.md) -- this is the form used by default.
    LeadingZeroOnWrite,
    /// No prefix on either direction. CONFIRMED NOT to work against real
    /// hardware (produces a malformed reply, not silence) -- kept only as a
    /// debug option (`--debug-no-prefix`) for re-running the discovery
    /// experiment if the device's behavior ever needs re-checking.
    NoPrefix,
}

#[derive(Debug)]
pub enum DeviceError {
    NoMatchingDevice,
    MultipleMatchingDevices(Vec<PathBuf>),
    PermissionDenied(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for DeviceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DeviceError::NoMatchingDevice => write!(
                f,
                "no /dev/hidraw* node matches the Yunzii B75 Pro Max config channel (VID {VENDOR_ID:04x}, PID {PRODUCT_ID:04x}). Is the keyboard plugged in via USB-C?"
            ),
            DeviceError::MultipleMatchingDevices(paths) => write!(
                f,
                "more than one /dev/hidraw* node matches the expected identity, refusing to guess: {paths:?}"
            ),
            DeviceError::PermissionDenied(path) => write!(
                f,
                "permission denied opening {path:?}. Install udev/99-yunzii-b75.rules to /etc/udev/rules.d/, then: sudo udevadm control --reload-rules && sudo udevadm trigger, then replug the keyboard."
            ),
            DeviceError::Io(e) => write!(f, "I/O error: {e}"),
        }
    }
}

impl std::error::Error for DeviceError {}

impl From<io::Error> for DeviceError {
    fn from(e: io::Error) -> Self {
        DeviceError::Io(e)
    }
}

impl DeviceError {
    /// Enriches a plain I/O error with a hint that the device node may have
    /// changed since it was opened (`/dev/hidrawN` numbering isn't stable
    /// across a disconnect/replug -- a TOCTOU race for hot-pluggable
    /// devices), and that the fix is to re-run discovery, not retry the
    /// same handle. Call this on errors from operations AFTER `Device::open`
    /// succeeded -- `open()`'s own errors (`PermissionDenied`,
    /// `NoMatchingDevice`) already have their own specific messages.
    pub fn with_reconnect_hint(self, path: &Path) -> Self {
        match self {
            DeviceError::Io(e) => DeviceError::Io(io::Error::new(
                e.kind(),
                format!(
                    "{e} -- the device node {path:?} may have changed (e.g. unplugged/replugged mid-operation); re-run discovery rather than retrying this handle"
                ),
            )),
            other => other,
        }
    }
}

fn parse_hid_id_line(uevent: &str) -> Option<(u16, u16)> {
    // uevent contains a line like: HID_ID=0003:000028E9:000031C8
    // Parse each field as a full u32 and truncate to u16, rather than
    // slicing a fixed [4..] offset -- avoids a panic if a field is ever
    // shorter than expected (real sysfs always zero-pads to 8 hex digits,
    // but this doesn't rely on that).
    for line in uevent.lines() {
        if let Some(rest) = line.strip_prefix("HID_ID=") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                let vid = u32::from_str_radix(parts[1], 16).ok()? as u16;
                let pid = u32::from_str_radix(parts[2], 16).ok()? as u16;
                return Some((vid, pid));
            }
        }
    }
    None
}

/// Enumerates `/sys/class/hidraw/*`, requiring BOTH the VID/PID match (from
/// `uevent`) AND the exact report-descriptor byte match as separate
/// conditions -- the descriptor alone doesn't encode VID/PID.
pub fn find_device() -> Result<PathBuf, DeviceError> {
    find_device_under(Path::new("/sys/class/hidraw"), Path::new("/dev"))
}

/// Same logic as `find_device`, but with the sysfs and device-node roots
/// injectable -- lets tests exercise the zero-match, multiple-match, and
/// descriptor-mismatch paths against a fake directory tree, without real
/// hardware.
fn find_device_under(sys_hidraw_root: &Path, dev_root: &Path) -> Result<PathBuf, DeviceError> {
    let mut candidates = Vec::new();
    let entries = fs::read_dir(sys_hidraw_root).map_err(DeviceError::Io)?;

    for entry in entries.flatten() {
        let sys_path = entry.path();
        let name = match sys_path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n.to_string(),
            None => continue,
        };

        let uevent_path = sys_path.join("device/uevent");
        let uevent = match fs::read_to_string(&uevent_path) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let (vid, pid) = match parse_hid_id_line(&uevent) {
            Some(v) => v,
            None => continue,
        };
        if vid != VENDOR_ID || pid != PRODUCT_ID {
            continue;
        }

        let descriptor_path = sys_path.join("device/report_descriptor");
        let descriptor = match fs::read(&descriptor_path) {
            Ok(d) => d,
            Err(_) => continue,
        };
        if descriptor != EXPECTED_REPORT_DESCRIPTOR {
            continue;
        }

        candidates.push(dev_root.join(&name));
    }

    match candidates.len() {
        0 => Err(DeviceError::NoMatchingDevice),
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => Err(DeviceError::MultipleMatchingDevices(candidates)),
    }
}

/// Retries a syscall on EINTR (a signal, e.g. terminal resize or job
/// control, interrupting a blocking or poll wait) instead of surfacing it
/// as a failure -- without this, a signal arriving mid-transaction could
/// abort a healthy 18-report "set time" sequence partway through, leaving
/// a partially-applied clock/date, which is exactly the failure mode the
/// abort-on-first-real-error design is trying to avoid.
fn retry_eintr<T>(mut f: impl FnMut() -> nix::Result<T>) -> Result<T, DeviceError> {
    loop {
        match f() {
            Err(nix::errno::Errno::EINTR) => continue,
            other => return other.map_err(|e| DeviceError::Io(io::Error::from(e))),
        }
    }
}

/// Builds the exact bytes that get written to the hidraw node for a given
/// report-ID form. Pure and side-effect-free so it's unit-testable without
/// hardware -- the confirmed-correct 65-byte "0x00 || report" layout is
/// this milestone's main transport fact and was previously only exercised
/// live against the real device.
fn frame_for_write(form: ReportIdForm, report: &[u8; REPORT_LEN]) -> Vec<u8> {
    match form {
        ReportIdForm::LeadingZeroOnWrite => {
            let mut buf = Vec::with_capacity(REPORT_LEN + 1);
            buf.push(0x00);
            buf.extend_from_slice(report);
            buf
        }
        ReportIdForm::NoPrefix => report.to_vec(),
    }
}

pub struct Device {
    fd: OwnedFd,
    path: PathBuf,
}

impl Device {
    pub fn open(path: &Path) -> Result<Self, DeviceError> {
        let fd = open(
            path,
            OFlag::O_RDWR | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::empty(),
        )
        .map_err(|errno| {
            if errno == nix::errno::Errno::EACCES {
                DeviceError::PermissionDenied(path.to_path_buf())
            } else {
                DeviceError::Io(io::Error::from(errno))
            }
        })?;
        Ok(Device {
            fd,
            path: path.to_path_buf(),
        })
    }

    /// Drains any currently-readable data WITHOUT waiting for anything new
    /// -- reads directly on the non-blocking fd until EAGAIN, no poll().
    /// This must never sleep on the happy (nothing to drain) path, so it
    /// intentionally does not wait for readability first; it just tries.
    /// Returns how many reports were drained (0 is the expected, healthy
    /// case before a fresh write). If the node is still readable after
    /// DRAIN_MAX_ITERATIONS, that's surfaced as an error rather than
    /// silently stopping and letting the next write proceed against a
    /// node that's still spewing leftover data.
    pub fn drain(&self) -> Result<usize, DeviceError> {
        let mut count = 0;
        for _ in 0..DRAIN_MAX_ITERATIONS {
            let mut buf = [0u8; REPORT_LEN];
            match retry_eintr(|| read(&self.fd, &mut buf)) {
                Ok(0) => return Ok(count),
                Ok(REPORT_LEN) => count += 1,
                // A short read during drain is not "one drained report" --
                // it means the fd is in a state read_one() would also
                // reject. Surface it rather than silently miscounting.
                Ok(n) => {
                    return Err(DeviceError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!(
                            "drain: expected a {REPORT_LEN}-byte read or EAGAIN, got {n} bytes"
                        ),
                    )));
                }
                Err(DeviceError::Io(e))
                    if e.raw_os_error() == Some(nix::errno::Errno::EAGAIN as i32) =>
                {
                    return Ok(count);
                }
                Err(e) => return Err(e),
            }
        }
        Err(DeviceError::Io(io::Error::other(format!(
            "device still readable after draining {DRAIN_MAX_ITERATIONS} reports -- something is wrong, refusing to proceed with a write against a node that won't go quiet"
        ))))
    }

    /// Waits up to `timeout` for the fd to become readable (`POLLIN`) or
    /// writable (`POLLOUT`). EINTR restarts the wait with the REMAINING
    /// time, not the original timeout -- a naive "retry with the same
    /// timeout" would let repeated signals push the total wait arbitrarily
    /// past the caller's intended deadline.
    fn poll_for(&self, flags: PollFlags, timeout: Duration) -> Result<bool, DeviceError> {
        let deadline = Instant::now() + timeout;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Ok(false);
            }
            // Clamp (not truncate) to u16::MAX -- a plain `as u16` cast on a
            // larger value would silently wrap to a much SHORTER timeout
            // (e.g. 70000ms -> 4464ms), a real correctness bug, not just an
            // edge case. Not reachable today (ACK_TIMEOUT is 500ms) but
            // this function shouldn't rely on that staying true.
            let timeout_ms = remaining.as_millis().min(u16::MAX as u128) as u16;
            let mut fds = [PollFd::new(self.fd.as_fd(), flags)];
            match poll(&mut fds, PollTimeout::from(timeout_ms)) {
                Ok(n) => return Ok(n > 0),
                Err(nix::errno::Errno::EINTR) => continue,
                Err(e) => return Err(DeviceError::Io(io::Error::from(e))),
            }
        }
    }

    fn poll_readable(&self, timeout: Duration) -> Result<bool, DeviceError> {
        self.poll_for(PollFlags::POLLIN, timeout)
    }

    fn poll_writable(&self, timeout: Duration) -> Result<bool, DeviceError> {
        self.poll_for(PollFlags::POLLOUT, timeout)
    }

    /// Writes one report, retrying on `EAGAIN` (the device's send queue is
    /// momentarily full -- expected under a non-blocking fd, not an error)
    /// by waiting for `POLLOUT` up to `ACK_TIMEOUT`, rather than treating
    /// the first `EAGAIN` as a hard failure and aborting a healthy
    /// transaction.
    fn write_report(
        &self,
        form: ReportIdForm,
        report: &[u8; REPORT_LEN],
    ) -> Result<(), DeviceError> {
        let buf = frame_for_write(form, report);
        let deadline = Instant::now() + ACK_TIMEOUT;
        loop {
            match retry_eintr(|| write(&self.fd, &buf)) {
                Ok(n) if n == buf.len() => return Ok(()),
                Ok(n) => {
                    return Err(DeviceError::Io(io::Error::new(
                        io::ErrorKind::WriteZero,
                        format!("short write: {n} of {} bytes", buf.len()),
                    )));
                }
                Err(DeviceError::Io(e))
                    if e.raw_os_error() == Some(nix::errno::Errno::EAGAIN as i32) =>
                {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() || !self.poll_writable(remaining)? {
                        return Err(DeviceError::Io(io::Error::new(
                            io::ErrorKind::TimedOut,
                            "write() kept returning EAGAIN (device send queue full?) past the deadline",
                        )));
                    }
                }
                Err(e) => return Err(e),
            }
        }
    }

    /// Reads one report. Confirmed against real hardware (see PROTOCOL.md's
    /// "Linux hidraw write/read byte layout"): reads from this unnumbered-
    /// report interface are always exactly 64 bytes, with NO leading
    /// report-ID byte -- asymmetric with `write()`, which needs one. A read
    /// of any other length is an error, not silently tolerated: the
    /// resolved protocol has one known reply shape, and a mismatch means
    /// something is actually wrong, not just "the other form."
    fn read_one(&self) -> Result<Option<[u8; REPORT_LEN]>, DeviceError> {
        let mut buf = [0u8; REPORT_LEN];
        match retry_eintr(|| read(&self.fd, &mut buf)) {
            Ok(REPORT_LEN) => Ok(Some(buf)),
            Ok(n) => Err(DeviceError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected a {REPORT_LEN}-byte read, got {n}"),
            ))),
            Err(DeviceError::Io(e))
                if e.raw_os_error() == Some(nix::errno::Errno::EAGAIN as i32) =>
            {
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Confirms `ack` is byte-identical to `sent` except offset 6 (status:
    /// 0x00 in the request, 0x55 in a real ACK) -- per Phase 0's evidence.
    fn is_valid_ack(sent: &[u8; REPORT_LEN], ack: &[u8; REPORT_LEN]) -> bool {
        if ack[6] != 0x55 {
            return false;
        }
        for i in 0..REPORT_LEN {
            if i == 6 {
                continue;
            }
            if sent[i] != ack[i] {
                return false;
            }
        }
        true
    }

    /// Sends one report and waits for exactly EXPECTED_ACKS_PER_WRITE (1)
    /// matching ACK within the ACK_TIMEOUT deadline, then stops reading --
    /// it does NOT itself detect an unexpected extra ACK arriving after
    /// that (an unexpected 2nd report would surface as a drained/warned
    /// leftover on the *next* write via `send_sequence`'s drain step, not
    /// as an error from this function). Aborts (returns Err) on a short
    /// write, zero ACKs, or a non-matching ACK -- per the plan, this is a
    /// hard error, not a soft continue. (Phase 0's WebHID capture saw 2
    /// ACKs per report; confirmed empirically in this milestone that only 1
    /// real ACK arrives at the native hidraw layer -- see PROTOCOL.md's
    /// "Linux hidraw write/read byte layout" section.)
    pub fn send_and_await_acks(
        &self,
        form: ReportIdForm,
        report: &[u8; REPORT_LEN],
    ) -> Result<(), DeviceError> {
        let debug = std::env::var("YUNZII_DEBUG").is_ok();
        if debug {
            eprintln!(
                "DEBUG send: {}",
                report
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<Vec<_>>()
                    .join(" ")
            );
        }
        self.write_report(form, report)?;

        let deadline = Instant::now() + ACK_TIMEOUT;
        let mut acks_received = 0;
        loop {
            if acks_received >= EXPECTED_ACKS_PER_WRITE {
                break;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if !self.poll_readable(remaining)? {
                break;
            }
            match self.read_one()? {
                Some(ack) if Self::is_valid_ack(report, &ack) => {
                    if debug {
                        eprintln!(
                            "DEBUG  ack: {}",
                            ack.iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    acks_received += 1;
                }
                Some(other) => {
                    if debug {
                        eprintln!(
                            "DEBUG  bad: {}",
                            other
                                .iter()
                                .map(|b| format!("{b:02x}"))
                                .collect::<Vec<_>>()
                                .join(" ")
                        );
                    }
                    return Err(DeviceError::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "received a report that doesn't match the expected ACK shape",
                    )));
                }
                None => continue,
            }
        }

        if acks_received != EXPECTED_ACKS_PER_WRITE {
            return Err(DeviceError::Io(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "expected {EXPECTED_ACKS_PER_WRITE} ACKs, got {acks_received} before the {ACK_TIMEOUT:?} deadline"
                ),
            )));
        }
        Ok(())
    }

    /// Sends an arbitrary sequence of reports in order (18 for "set time", 2
    /// for a page switch, 32 for clear-picture, or any other command's
    /// sequence), aborting the whole transaction on the first report that
    /// doesn't get clean ACKs -- a partially-applied command is worse than a
    /// clean stop. This function has no per-command knowledge; the caller
    /// (`protocol.rs`'s builders) decides what the reports mean.
    pub fn send_sequence(
        &self,
        form: ReportIdForm,
        reports: &[[u8; REPORT_LEN]],
    ) -> Result<(), DeviceError> {
        for (i, report) in reports.iter().enumerate() {
            let drained = self.drain()?;
            if drained > 0 {
                eprintln!("warning: drained {drained} unexpected report(s) before write #{i}");
            }
            self.send_and_await_acks(form, report)?;
        }
        Ok(())
    }
}

// OwnedFd closes itself on drop -- no manual Drop impl needed.

impl AsRawFd for Device {
    fn as_raw_fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }
}

impl std::fmt::Debug for Device {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Device").field("path", &self.path).finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// Builds a fake `/sys/class/hidraw/<name>/device/{uevent,report_descriptor}`
    /// tree under `root` for one node, so `find_device_under` can be tested
    /// without real hardware.
    fn write_fake_hidraw_node(root: &Path, name: &str, vid: u16, pid: u16, descriptor: &[u8]) {
        let device_dir = root.join(name).join("device");
        fs::create_dir_all(&device_dir).unwrap();
        fs::write(
            device_dir.join("uevent"),
            format!("DRIVER=hid-generic\nHID_ID=0003:{vid:08X}:{pid:08X}\n"),
        )
        .unwrap();
        fs::write(device_dir.join("report_descriptor"), descriptor).unwrap();
    }

    #[test]
    fn find_device_under_zero_matches() {
        let tmp = tempfile::tempdir().unwrap();
        // An empty hidraw root -- no nodes at all.
        fs::create_dir_all(tmp.path()).unwrap();
        let result = find_device_under(tmp.path(), Path::new("/dev"));
        assert!(matches!(result, Err(DeviceError::NoMatchingDevice)));
    }

    #[test]
    fn find_device_under_wrong_vid_pid_is_zero_matches() {
        let tmp = tempfile::tempdir().unwrap();
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw0",
            0xABCD,
            0x1234,
            &EXPECTED_REPORT_DESCRIPTOR,
        );
        let result = find_device_under(tmp.path(), Path::new("/dev"));
        assert!(matches!(result, Err(DeviceError::NoMatchingDevice)));
    }

    #[test]
    fn find_device_under_descriptor_mismatch_is_zero_matches() {
        let tmp = tempfile::tempdir().unwrap();
        // Right VID/PID, wrong descriptor -- must not match (a firmware
        // revision or a different interface under the same VID/PID).
        write_fake_hidraw_node(tmp.path(), "hidraw0", VENDOR_ID, PRODUCT_ID, &[0xAA, 0xBB]);
        let result = find_device_under(tmp.path(), Path::new("/dev"));
        assert!(matches!(result, Err(DeviceError::NoMatchingDevice)));
    }

    #[test]
    fn find_device_under_one_match() {
        let tmp = tempfile::tempdir().unwrap();
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw5",
            VENDOR_ID,
            PRODUCT_ID,
            &EXPECTED_REPORT_DESCRIPTOR,
        );
        let result = find_device_under(tmp.path(), Path::new("/dev")).unwrap();
        assert_eq!(result, Path::new("/dev/hidraw5"));
    }

    #[test]
    fn find_device_under_multiple_matches_refuses_to_guess() {
        let tmp = tempfile::tempdir().unwrap();
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw5",
            VENDOR_ID,
            PRODUCT_ID,
            &EXPECTED_REPORT_DESCRIPTOR,
        );
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw9",
            VENDOR_ID,
            PRODUCT_ID,
            &EXPECTED_REPORT_DESCRIPTOR,
        );
        let result = find_device_under(tmp.path(), Path::new("/dev"));
        match result {
            Err(DeviceError::MultipleMatchingDevices(paths)) => assert_eq!(paths.len(), 2),
            other => panic!("expected MultipleMatchingDevices, got {other:?}"),
        }
    }

    #[test]
    fn find_device_under_mixed_nodes_picks_only_the_real_match() {
        let tmp = tempfile::tempdir().unwrap();
        // The real device exposes 4 interfaces; only one should match.
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw4",
            VENDOR_ID,
            PRODUCT_ID,
            &[0x05, 0x01, 0x09, 0x06],
        ); // keyboard interface, wrong descriptor
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw5",
            VENDOR_ID,
            PRODUCT_ID,
            &EXPECTED_REPORT_DESCRIPTOR,
        ); // the config channel
        write_fake_hidraw_node(
            tmp.path(),
            "hidraw7",
            0xABCD,
            0x1234,
            &EXPECTED_REPORT_DESCRIPTOR,
        ); // right descriptor, wrong VID/PID (shouldn't happen in reality, but exercises the AND condition)
        let result = find_device_under(tmp.path(), Path::new("/dev")).unwrap();
        assert_eq!(result, Path::new("/dev/hidraw5"));
    }

    #[test]
    fn parses_hid_id_line() {
        let uevent = "DRIVER=hid-generic\nHID_ID=0003:000028E9:000031C8\nHID_NAME=YUNZII B75 PRO MAX Keyboard\n";
        assert_eq!(parse_hid_id_line(uevent), Some((0x28E9, 0x31C8)));
    }

    #[test]
    fn parses_hid_id_line_different_vendor() {
        let uevent = "HID_ID=0003:0000ABCD:00001234\n";
        assert_eq!(parse_hid_id_line(uevent), Some((0xABCD, 0x1234)));
    }

    #[test]
    fn returns_none_for_missing_hid_id() {
        let uevent = "DRIVER=hid-generic\nHID_NAME=Something\n";
        assert_eq!(parse_hid_id_line(uevent), None);
    }

    #[test]
    fn expected_descriptor_matches_phase0_recorded_hex() {
        // Cross-check against PROTOCOL.md's documented hex string, so this
        // constant can't silently drift from the doc it's supposed to match.
        let expected_hex = "0660ff0961a1010962150026ff009540750881020963150026ff00954075089102c0";
        let bytes: Vec<u8> = (0..expected_hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&expected_hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(EXPECTED_REPORT_DESCRIPTOR.to_vec(), bytes);
    }

    #[test]
    fn frame_for_write_leading_zero_form_is_65_bytes_with_zero_prefix() {
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0x41;
        report[7] = 19;
        let framed = frame_for_write(ReportIdForm::LeadingZeroOnWrite, &report);
        assert_eq!(framed.len(), REPORT_LEN + 1);
        assert_eq!(framed[0], 0x00);
        assert_eq!(&framed[1..], &report[..]);
    }

    #[test]
    fn frame_for_write_no_prefix_form_is_64_bytes_unchanged() {
        let mut report = [0u8; REPORT_LEN];
        report[0] = 0x41;
        report[7] = 19;
        let framed = frame_for_write(ReportIdForm::NoPrefix, &report);
        assert_eq!(framed.len(), REPORT_LEN);
        assert_eq!(&framed[..], &report[..]);
    }

    #[test]
    fn is_valid_ack_accepts_status_flip_only() {
        let mut sent = [0u8; REPORT_LEN];
        sent[0] = 0x41;
        sent[7] = 19;
        let mut ack = sent;
        ack[6] = 0x55;
        assert!(Device::is_valid_ack(&sent, &ack));
    }

    #[test]
    fn is_valid_ack_rejects_wrong_status_byte() {
        let sent = [0u8; REPORT_LEN];
        let mut ack = sent;
        ack[6] = 0x00; // not flipped to 0x55
        assert!(!Device::is_valid_ack(&sent, &ack));
    }

    #[test]
    fn is_valid_ack_rejects_mismatched_payload() {
        let sent = [0u8; REPORT_LEN];
        let mut ack = sent;
        ack[6] = 0x55;
        ack[7] = 99; // payload byte changed, not just status
        assert!(!Device::is_valid_ack(&sent, &ack));
    }
}
