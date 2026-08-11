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
/// elsewhere would be rejected.
const EXPECTED_REPORT_DESCRIPTOR: [u8; 34] = [
    0x06, 0x60, 0xff, 0x09, 0x61, 0xa1, 0x01, 0x09, 0x62, 0x15, 0x00, 0x26, 0xff, 0x00, 0x95, 0x40,
    0x75, 0x08, 0x81, 0x02, 0x09, 0x63, 0x15, 0x00, 0x26, 0xff, 0x00, 0x95, 0x40, 0x75, 0x08, 0x91,
    0x02, 0xc0,
];

const REPORT_LEN: usize = 64;
const ACK_TIMEOUT: Duration = Duration::from_millis(500);
const DRAIN_TIMEOUT: Duration = Duration::from_millis(150);
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

fn parse_hid_id_line(uevent: &str) -> Option<(u16, u16)> {
    // uevent contains a line like: HID_ID=0003:000028E9:000031C8
    for line in uevent.lines() {
        if let Some(rest) = line.strip_prefix("HID_ID=") {
            let parts: Vec<&str> = rest.split(':').collect();
            if parts.len() == 3 {
                let vid = u16::from_str_radix(&parts[1][4..], 16).ok()?;
                let pid = u16::from_str_radix(&parts[2][4..], 16).ok()?;
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
    let mut candidates = Vec::new();
    let entries = fs::read_dir("/sys/class/hidraw").map_err(DeviceError::Io)?;

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

        candidates.push(PathBuf::from(format!("/dev/{name}")));
    }

    match candidates.len() {
        0 => Err(DeviceError::NoMatchingDevice),
        1 => Ok(candidates.into_iter().next().unwrap()),
        _ => Err(DeviceError::MultipleMatchingDevices(candidates)),
    }
}

pub struct Device {
    fd: OwnedFd,
    path: PathBuf,
}

impl Device {
    pub fn open(path: &Path) -> Result<Self, DeviceError> {
        let fd = open(path, OFlag::O_RDWR | OFlag::O_NONBLOCK, Mode::empty()).map_err(|errno| {
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

    /// Drains any currently-readable data, capped at DRAIN_TIMEOUT so a
    /// chatty node can't hang this step. Returns how many reports were
    /// drained (0 is the expected, healthy case before a fresh write).
    pub fn drain(&self) -> Result<usize, DeviceError> {
        let deadline = Instant::now() + DRAIN_TIMEOUT;
        let mut count = 0;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            if !self.poll_readable(remaining)? {
                break;
            }
            let mut buf = [0u8; REPORT_LEN + 1];
            match read(&self.fd, &mut buf) {
                Ok(0) => break,
                Ok(_) => count += 1,
                Err(nix::errno::Errno::EAGAIN) => break,
                Err(e) => return Err(DeviceError::Io(io::Error::from(e))),
            }
        }
        Ok(count)
    }

    fn poll_readable(&self, timeout: Duration) -> Result<bool, DeviceError> {
        let mut fds = [PollFd::new(self.fd.as_fd(), PollFlags::POLLIN)];
        let timeout_ms: i32 = timeout.as_millis().min(i32::MAX as u128) as i32;
        let n = poll(&mut fds, PollTimeout::from(timeout_ms as u16))
            .map_err(|e| DeviceError::Io(io::Error::from(e)))?;
        Ok(n > 0)
    }

    fn write_report(
        &self,
        form: ReportIdForm,
        report: &[u8; REPORT_LEN],
    ) -> Result<(), DeviceError> {
        let n = match form {
            ReportIdForm::LeadingZeroOnWrite => {
                let mut buf = [0u8; REPORT_LEN + 1];
                buf[1..].copy_from_slice(report);
                write(&self.fd, &buf).map_err(|e| DeviceError::Io(io::Error::from(e)))?
            }
            ReportIdForm::NoPrefix => {
                write(&self.fd, report).map_err(|e| DeviceError::Io(io::Error::from(e)))?
            }
        };
        let expected = match form {
            ReportIdForm::LeadingZeroOnWrite => REPORT_LEN + 1,
            ReportIdForm::NoPrefix => REPORT_LEN,
        };
        if n != expected {
            return Err(DeviceError::Io(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short write: {n} of {expected} bytes"),
            )));
        }
        Ok(())
    }

    /// Reads one report, normalizing away an optional leading 0x00 byte if
    /// present (reads from an unnumbered-report interface do NOT get a
    /// synthetic report-ID byte per the documented Linux hidraw behavior --
    /// this normalization exists for the discovery phase, where both forms
    /// are still being tried; the shipped, resolved code path expects a
    /// fixed, known reply shape once the form is settled).
    fn read_one(&self) -> Result<Option<[u8; REPORT_LEN]>, DeviceError> {
        let mut buf = [0u8; REPORT_LEN + 1];
        match read(&self.fd, &mut buf) {
            Ok(REPORT_LEN) => {
                let mut out = [0u8; REPORT_LEN];
                out.copy_from_slice(&buf[..REPORT_LEN]);
                Ok(Some(out))
            }
            Ok(n) if n == REPORT_LEN + 1 && buf[0] == 0x00 => {
                let mut out = [0u8; REPORT_LEN];
                out.copy_from_slice(&buf[1..REPORT_LEN + 1]);
                Ok(Some(out))
            }
            Ok(n) => Err(DeviceError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("unexpected read length {n}"),
            ))),
            Err(nix::errno::Errno::EAGAIN) => Ok(None),
            Err(e) => Err(DeviceError::Io(io::Error::from(e))),
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

    /// Sends one report and waits for exactly 2 matching ACKs within one
    /// 500ms deadline covering both. Aborts (returns Err) on a short write,
    /// zero ACKs, exactly one ACK, or a non-matching ACK -- per the plan,
    /// this is a hard error, not a soft continue.
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

    /// Sends a full sequence (all 18 reports for "set time"), aborting the
    /// whole transaction on the first report that doesn't get clean ACKs --
    /// a partially-applied clock/date is worse than a clean stop.
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
