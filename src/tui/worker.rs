//! The threads that are allowed to block.
//!
//! Three threads, and the split is the whole design. The UI draws and reads
//! keys and touches neither the device nor a decoder. A **worker** owns jobs.
//! A separate **discovery** thread probes for the keyboard.
//!
//! Discovery is separate rather than another kind of job because a GIF upload
//! takes forty-five seconds, and a rescan queued behind one would either
//! starve or arrive in a burst afterwards. It also must not run *during* a
//! job: probing means draining the device, and draining mid-upload would eat
//! the acknowledgements the upload is waiting for.

use crate::device::{self, Device, DeviceError, ReportIdForm};
use crate::exec::{self, ExecCtx, ExecEvent, Phase, SystemClock, Transport};
use crate::plan;
use crate::protocol;
use crate::time;
use crate::tui::app::{DeviceState, Job, Pending, Update};
use crate::tui::preview;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// How often to look for the keyboard while it is not there.
const RESCAN: Duration = Duration::from_secs(2);

/// Cells reserved for the preview when one is built. Generous: the pane is
/// re-rendered from the plan's own frame anyway, and this only sets how much
/// detail is kept.
const PREVIEW_W: usize = 160;
const PREVIEW_H: usize = 48;

/// Looks for the keyboard, and *opens* it, because looking is not enough.
///
/// `find_device` cannot report a permission problem: `PermissionDenied` comes
/// out of `drain` (see `device.rs`). A discovery that only searched would
/// happily report Ready with a path while every action failed the moment it
/// opened the node -- which is exactly the failure the udev work was about,
/// displayed as success. So this opens and drains once, then closes.
pub fn probe() -> DeviceState {
    let path = match device::find_device() {
        Ok(p) => p,
        Err(DeviceError::NoMatchingDevice) => return DeviceState::NotFound,
        Err(DeviceError::MultipleMatchingDevices(paths)) => {
            return DeviceState::MultipleMatches(paths);
        }
        Err(DeviceError::PermissionDenied(p)) => return DeviceState::PermissionDenied(p),
        Err(e) => return DeviceState::ScanFailed(e.to_string()),
    };

    match Device::open(&path).and_then(|d| d.drain().map(|_| ())) {
        Ok(()) => DeviceState::Ready(path),
        Err(DeviceError::PermissionDenied(p)) => DeviceState::PermissionDenied(p),
        Err(e) => DeviceState::ScanFailed(e.to_string()),
    }
}

/// Probes on a timer, but only while idle and not already Ready.
pub fn spawn_discovery(tx: Sender<Update>, busy: Arc<AtomicBool>, ready: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        loop {
            if !busy.load(Ordering::Relaxed) && !ready.load(Ordering::Relaxed) {
                let state = probe();
                ready.store(state.is_ready(), Ordering::Relaxed);
                if tx.send(Update::Device(state)).is_err() {
                    return; // UI is gone
                }
            }
            std::thread::sleep(RESCAN);
        }
    });
}

/// Runs one job at a time and reports what it is doing.
pub fn spawn_worker(
    jobs: Receiver<Job>,
    tx: Sender<Update>,
    busy: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        for job in jobs {
            busy.store(true, Ordering::Relaxed);
            let result = run_job(job, &tx, &ready);
            busy.store(false, Ordering::Relaxed);
            if let Some(finished) = result
                && tx.send(Update::Finished(finished)).is_err()
            {
                return;
            }
        }
    });
}

/// `None` means the job reported its own completion (preview does).
fn run_job(
    job: Job,
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
) -> Option<Result<String, String>> {
    match job {
        Job::Preview { path, for_gif } => {
            let built = build_preview(&path, for_gif);
            let _ = tx.send(Update::Preview(Box::new(built)));
            None
        }
        Job::SetTime => Some(simple(tx, ready, "set the clock", |dev, notes| {
            // Snapshot AFTER the device is open and drained, exactly as the
            // CLI does: taking it earlier would send a time that is already
            // stale by however long opening took.
            let f = time::snapshot_local();
            dev.send_sequence(
                ReportIdForm::LeadingZeroOnWrite,
                &protocol::build_set_time_sequence(
                    f.hour,
                    f.minute,
                    f.second,
                    f.year2digit,
                    f.weekday,
                    f.month,
                    f.date,
                ),
                notes,
            )
        })),
        Job::SwitchPage(page) => Some(simple(tx, ready, "switched page", |dev, notes| {
            dev.send_sequence(
                ReportIdForm::LeadingZeroOnWrite,
                &protocol::build_page_switch_sequence(page),
                notes,
            )
        })),
        Job::ClearPicture => Some(simple(tx, ready, "cleared the picture", |dev, notes| {
            dev.send_sequence(
                ReportIdForm::LeadingZeroOnWrite,
                &protocol::build_clear_picture_sequence(),
                notes,
            )
        })),
        Job::UploadPicture(plan) => Some(upload(
            tx,
            ready,
            "Uploading picture",
            plan.total_reports,
            None,
            |cx| exec::execute_picture(&plan, cx),
            "the picture may be partially written -- re-run it, or clear the picture",
        )),
        Job::UploadGif(plan) => Some(upload(
            tx,
            ready,
            "Uploading GIF",
            plan.total_reports,
            Some(plan.est_secs),
            |cx| exec::execute_gif(&plan, cx),
            "the animation on the keyboard may be incomplete -- re-run set-gif to overwrite it",
        )),
    }
}

fn build_preview(path: &std::path::Path, for_gif: bool) -> Result<Pending, String> {
    if for_gif {
        let plan = plan::plan_gif_upload(path, None, None).map_err(|e| e.to_string())?;
        let first = plan.frames.first().cloned().unwrap_or_default();
        Ok(Pending::Gif {
            path: path.to_path_buf(),
            preview: preview::render(&first, PREVIEW_W, PREVIEW_H),
            plan,
            rate_override: None,
        })
    } else {
        let plan = plan::plan_picture_upload(path).map_err(|e| e.to_string())?;
        Ok(Pending::Picture {
            path: path.to_path_buf(),
            preview: preview::render(&plan.pixels, PREVIEW_W, PREVIEW_H),
            plan,
        })
    }
}

/// The short commands: open, send, close. No progress worth reporting.
fn simple(
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
    done_msg: &str,
    body: impl FnOnce(&Device, &mut dyn FnMut(String)) -> Result<(), DeviceError>,
) -> Result<String, String> {
    let (path, dev) = open(ready)?;
    let mut notes = |m: String| {
        let _ = tx.send(Update::Note(m));
    };
    body(&dev, &mut notes)
        .map(|()| done_msg.to_string())
        .map_err(|e| {
            ready.store(false, Ordering::Relaxed);
            e.with_reconnect_hint(&path).to_string()
        })
}

/// The long ones: progress, and a cancel flag the UI can trip.
#[allow(clippy::too_many_arguments)]
fn upload(
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
    label: &str,
    total: usize,
    est_secs: Option<usize>,
    body: impl FnOnce(&mut ExecCtx) -> Result<(), DeviceError>,
    partial_note: &str,
) -> Result<String, String> {
    let (path, dev) = open(ready)?;

    let cancel = Arc::new(AtomicBool::new(false));
    let _ = tx.send(Update::Started {
        label: label.to_string(),
        total,
        est_secs,
        cancel: Arc::clone(&cancel),
    });

    let clock = SystemClock;
    let mut emit = |ev: ExecEvent| {
        let _ = match ev {
            ExecEvent::Progress { done, total, phase } => tx.send(Update::Progress {
                done,
                total,
                frame: match phase {
                    Phase::Frame { index, of } => Some((index, of)),
                    _ => None,
                },
            }),
            ExecEvent::Note(m) => tx.send(Update::Note(m)),
            // The CLI prints a line per finished frame; the TUI's progress bar
            // already shows it, so this is only worth logging.
            ExecEvent::FrameDone { index, of } => {
                tx.send(Update::Note(format!("frame {}/{of}", index + 1)))
            }
        };
    };
    let mut cx = ExecCtx {
        dev: &dev as &dyn Transport,
        cancel: &cancel,
        clock: &clock,
        emit: &mut emit,
    };

    match body(&mut cx) {
        Ok(()) => Ok(format!("{label}: done")),
        Err(e) => {
            if !matches!(e, DeviceError::Cancelled { .. }) {
                ready.store(false, Ordering::Relaxed);
            }
            Err(e
                .with_reconnect_hint(&path)
                .with_note(partial_note)
                .to_string())
        }
    }
}

fn open(ready: &Arc<AtomicBool>) -> Result<(std::path::PathBuf, Device), String> {
    let path = device::find_device().map_err(|e| {
        ready.store(false, Ordering::Relaxed);
        e.to_string()
    })?;
    let dev = Device::open(&path).map_err(|e| {
        ready.store(false, Ordering::Relaxed);
        e.to_string()
    })?;
    dev.drain().map_err(|e| {
        ready.store(false, Ordering::Relaxed);
        e.with_reconnect_hint(&path).to_string()
    })?;
    Ok((path, dev))
}

/// Shared flags, so the UI can hand the same pair to both threads.
pub struct Flags {
    pub busy: Arc<AtomicBool>,
    pub ready: Arc<AtomicBool>,
}

impl Default for Flags {
    fn default() -> Self {
        Self {
            busy: Arc::new(AtomicBool::new(false)),
            ready: Arc::new(AtomicBool::new(false)),
        }
    }
}
