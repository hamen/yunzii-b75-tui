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

use crate::adjust::Adjustments;
use crate::device::{self, Device, DeviceError, ReportIdForm};
use crate::exec::{self, ExecCtx, ExecEvent, Phase, SystemClock, Transport};
use crate::plan;
use crate::protocol;
use crate::time;
use crate::tui::app::{DeviceState, Job, Pending, Update};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::Duration;

/// How often to look for the keyboard while it is not there.
const RESCAN: Duration = Duration::from_secs(2);

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
///
/// A job that panics does not take the worker with it, and does not leave the
/// interface waiting for an update that will never come. Detecting worker
/// death by watching for a closed channel does not work here -- the discovery
/// thread holds a sender for the life of the program, so the receiver never
/// disconnects. Catching the panic is both simpler and better: the next job
/// still runs.
pub fn spawn_worker(
    jobs: Receiver<Job>,
    tx: Sender<Update>,
    busy: Arc<AtomicBool>,
    ready: Arc<AtomicBool>,
) {
    std::thread::spawn(move || {
        for job in jobs {
            busy.store(true, Ordering::Relaxed);
            // Peeked BEFORE `job` moves into the `guarded` closure below: a
            // panic during a `Job::Replan` must route to `Update::Replan`,
            // not the generic `Update::Finished` every other job uses, or
            // `App`'s `replan_pending` flag is never cleared and the confirm
            // screen locks up for the rest of the session.
            let replan_generation = replan_generation_of(&job);
            let result = guarded(|| run_job(job, &tx, &ready));
            // Cleared even on a panic. Leaving it set would stop the discovery
            // thread from ever probing again.
            busy.store(false, Ordering::Relaxed);
            let update = match result {
                Ok(Some(finished)) => Some(Update::Finished(finished)),
                Ok(None) => None,
                Err(panic) => Some(panic_update(replan_generation, panic)),
            };
            if let Some(update) = update
                && tx.send(update).is_err()
            {
                return; // the interface is gone
            }
        }
    });
}

/// Routes a panic to the `Update` that undoes its effects correctly:
/// `Update::Replan` for a `Job::Replan` (so `replan_pending` clears and the
/// existing `Pending` stays shown, per `Update::Replan`'s own `Err` arm),
/// `Update::Finished` for everything else (unchanged from before).
///
/// Extracted so it's directly testable without forcing a real panic through
/// a real worker thread -- the same reasoning `guarded` itself is split out
/// for.
fn panic_update(replan_generation: Option<u64>, panic_msg: String) -> Update {
    let text = format!(
        "the job stopped unexpectedly ({panic_msg}) -- whatever was being sent may be incomplete"
    );
    match replan_generation {
        Some(generation) => Update::Replan {
            generation,
            result: Box::new(Err(text)),
        },
        None => Update::Finished(Err(text)),
    }
}

/// The generation carried by a `Job::Replan`, or `None` for any other job.
///
/// Extracted so the peek-before-move in `spawn_worker` is itself directly
/// testable, separate from `panic_update`'s routing decision.
fn replan_generation_of(job: &Job) -> Option<u64> {
    match job {
        Job::Replan { generation, .. } => Some(*generation),
        _ => None,
    }
}

/// Runs `f`, turning a panic into a message instead of a dead thread.
///
/// Extracted so it can be tested: there is no `Job` variant that panics, and
/// adding one purely for a test would be worse than testing the mechanism.
fn guarded<T>(f: impl FnOnce() -> T) -> Result<T, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).map_err(|e| {
        if let Some(s) = e.downcast_ref::<&str>() {
            (*s).to_string()
        } else if let Some(s) = e.downcast_ref::<String>() {
            s.clone()
        } else {
            "panic".to_string()
        }
    })
}

/// `None` means the job reported its own completion (preview does).
fn run_job(
    job: Job,
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
) -> Option<Result<String, String>> {
    match job {
        Job::Preview {
            path,
            for_gif,
            generation,
        } => {
            let built = build_preview(&path, for_gif);
            let _ = tx.send(Update::Preview {
                generation,
                result: Box::new(built),
            });
            None
        }
        Job::Replan {
            path,
            for_gif,
            placement,
            adjustments,
            rate_override,
            row,
            generation,
        } => {
            let built = build_pending(&path, for_gif, placement, adjustments, rate_override, row);
            let _ = tx.send(Update::Replan {
                generation,
                result: Box::new(built),
            });
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
        Job::ClearGif => Some(simple(tx, ready, "cleared the GIF", |dev, notes| {
            dev.send_sequence(
                ReportIdForm::LeadingZeroOnWrite,
                &protocol::build_clear_gif_sequence(),
                notes,
            )
        })),
        Job::UploadPicture(mut plan) => {
            // The interface only ever re-encoded the frame it was showing;
            // everything else is caught up here, off the drawing thread.
            plan.reencode();
            Some(upload(
                tx,
                ready,
                "Uploading picture",
                plan.total_reports,
                None,
                |cx| exec::execute_picture(&plan, cx),
                "the picture may be partially written -- re-run it, or clear the picture",
            ))
        }
        Job::UploadGif(mut plan) => {
            plan.reencode();
            Some(upload(
                tx,
                ready,
                "Uploading GIF",
                plan.total_reports,
                Some(plan.est_secs),
                |cx| exec::execute_gif(&plan, cx),
                "the animation on the keyboard may be incomplete -- re-run set-gif to overwrite it",
            ))
        }
    }
}

/// Builds a `Pending` from a source file. Shared by `Job::Preview` (the
/// first load, always `Placement::default()`/`Adjustments::NONE`/no
/// rate override/row 0) and `Job::Replan` (a placement toggle, carrying
/// forward whatever the user had already chosen). `rate_override`/`row` are
/// carried through unchanged into the resulting `Pending` -- `rate_override`
/// is applied onto `plan.rate` only at upload time (`confirm_key`'s Enter
/// handler), not here, matching how it already worked before Milestone 7.
fn build_pending(
    path: &std::path::Path,
    for_gif: bool,
    placement: plan::Placement,
    adjustments: Adjustments,
    rate_override: Option<u8>,
    row: usize,
) -> Result<Pending, String> {
    if for_gif {
        // No `max_frames`: sampling a long animation down is a decision with a
        // suggested rate attached, and this interface has nowhere sensible to
        // put that conversation yet. Refusing with a pointer is honest.
        let plan =
            plan::plan_gif_upload(path, None, None, placement, &adjustments).map_err(|e| {
                let mut m = e.to_string();
                // Matched against a constant the planner owns, not against its
                // prose: a reworded message must not silently drop the guidance.
                if m.contains(plan::TOO_MANY_FRAMES) {
                    m.push_str(
                        "\n(the interface cannot sample frames yet -- use the command line: \
                         `yunzii-b75-tui set-gif <file> --max-frames 160`)",
                    );
                }
                m
            })?;
        Ok(Pending::Gif {
            path: path.to_path_buf(),
            plan,
            rate_override,
            adjustments,
            row,
        })
    } else {
        let plan =
            plan::plan_picture_upload(path, placement, &adjustments).map_err(|e| e.to_string())?;
        Ok(Pending::Picture {
            path: path.to_path_buf(),
            plan,
            adjustments,
            row,
        })
    }
}

fn build_preview(path: &std::path::Path, for_gif: bool) -> Result<Pending, String> {
    build_pending(
        path,
        for_gif,
        plan::Placement::default(),
        Adjustments::NONE,
        None,
        0,
    )
}

/// The short commands: open, send, close. No progress worth reporting.
fn simple(
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
    done_msg: &str,
    body: impl FnOnce(&Device, &mut dyn FnMut(String)) -> Result<(), DeviceError>,
) -> Result<String, String> {
    let (path, dev) = open_reporting(tx, ready)?;
    let mut notes = |m: String| {
        let _ = tx.send(Update::Note(m));
    };
    body(&dev, &mut notes)
        .map(|()| done_msg.to_string())
        .map_err(|e| {
            lost(tx, ready);
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
    let (path, dev) = open_reporting(tx, ready)?;

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
            // The CLI prints a line per finished frame. Here the gauge already
            // says "frame 12/36", so repeating it in the log would push
            // everything else out of a scrollback that holds 200 lines -- a
            // 160-frame upload would be the only thing in it.
            ExecEvent::FrameDone { .. } => Ok(()),
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
            // A cancellation says nothing about the device; anything else does,
            // and the interface must stop claiming a keyboard immediately
            // rather than at the next probe two seconds later.
            if !matches!(e, DeviceError::Cancelled { .. }) {
                lost(tx, ready);
            }
            Err(e
                .with_reconnect_hint(&path)
                .with_note(partial_note)
                .to_string())
        }
    }
}

/// Marks the device gone and tells the interface at once.
///
/// Clearing `ready` alone only restarts discovery; without the update the
/// header keeps its green dot, and its actions keep looking available, until
/// the next probe two seconds later. Two seconds of a UI claiming a keyboard
/// that just failed is two seconds of it lying.
fn lost(tx: &Sender<Update>, ready: &Arc<AtomicBool>) {
    ready.store(false, Ordering::Relaxed);
    let _ = tx.send(Update::Device(DeviceState::NotFound));
}

fn open_reporting(
    tx: &Sender<Update>,
    ready: &Arc<AtomicBool>,
) -> Result<(std::path::PathBuf, Device), String> {
    let path = device::find_device().map_err(|e| {
        lost(tx, ready);
        e.to_string()
    })?;
    let dev = Device::open(&path).map_err(|e| {
        lost(tx, ready);
        e.to_string()
    })?;
    dev.drain().map_err(|e| {
        lost(tx, ready);
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    /// A panicking job is reported, not swallowed, and does not kill the
    /// worker or leave `busy` stuck.
    ///
    /// Stuck `busy` is the quiet half of this bug: the discovery thread checks
    /// it before probing, so a worker that died mid-job would also stop the
    /// keyboard from ever being found again.
    #[test]
    fn a_panicking_job_is_reported_and_the_worker_survives() {
        let out = guarded(|| -> u8 { panic!("decoder exploded") });
        assert_eq!(out.unwrap_err(), "decoder exploded");

        // And the ordinary path is untouched.
        assert_eq!(guarded(|| 7).unwrap(), 7);
    }

    /// The worker keeps taking jobs after one fails, and always answers.
    #[test]
    fn the_worker_answers_every_job_even_without_a_device() {
        let (job_tx, job_rx) = mpsc::channel();
        let (tx, rx) = mpsc::channel();
        let flags = Flags::default();
        spawn_worker(job_rx, tx, flags.busy.clone(), flags.ready.clone());

        // Two jobs that cannot succeed here: no plan, no device. Both must
        // come back rather than hanging.
        job_tx
            .send(Job::Preview {
                path: std::path::PathBuf::from("/nonexistent.gif"),
                for_gif: true,
                generation: 1,
            })
            .unwrap();

        let first = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker must answer");
        match first {
            Update::Preview { generation, result } => {
                assert_eq!(generation, 1, "the reply carries the request it answers");
                assert!(result.is_err(), "a missing file cannot preview");
            }
            other => panic!("expected a preview reply, got {other:?}"),
        }

        job_tx
            .send(Job::Preview {
                path: std::path::PathBuf::from("/also-missing.png"),
                for_gif: false,
                generation: 2,
            })
            .unwrap();
        let second = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("the worker is still alive after a failure");
        assert!(matches!(second, Update::Preview { generation: 2, .. }));

        // `busy` is cleared just AFTER the reply is sent, so this waits for it
        // rather than sampling once -- a flaky test is worse than no test.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while flags.busy.load(Ordering::Relaxed) && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !flags.busy.load(Ordering::Relaxed),
            "busy must be cleared, or discovery never probes again"
        );
    }

    /// A real preview goes through the planner and produces something the
    /// confirm screen can draw.
    #[test]
    fn a_preview_carries_the_plan_it_will_upload() {
        let built = build_preview(std::path::Path::new("fixtures/test-anim-2frames.gif"), true)
            .expect("a valid GIF");
        match built {
            Pending::Gif { plan, .. } => {
                assert_eq!(plan.frames.len(), 2);
                assert_eq!(plan.frames[0].len(), crate::protocol::PICTURE_BYTES);
            }
            other => panic!("expected a GIF, got {other:?}"),
        }
    }

    // --- Milestone 7: Job::Replan / panic routing ---

    /// A panic during `Job::Replan` must route to `Update::Replan`, not the
    /// generic `Update::Finished` every other job uses -- otherwise
    /// `App::replan_pending` is never cleared and the confirm screen locks
    /// up for the rest of the session. Tested directly, without forcing a
    /// real panic through a real worker thread.
    #[test]
    fn panic_update_routes_a_replan_panic_to_update_replan() {
        let update = panic_update(Some(42), "boom".to_string());
        match update {
            Update::Replan { generation, result } => {
                assert_eq!(generation, 42);
                let err = result.unwrap_err();
                assert!(err.contains("boom"), "got {err:?}");
            }
            other => panic!("expected Update::Replan, got {other:?}"),
        }
    }

    #[test]
    fn panic_update_routes_every_other_panic_to_update_finished() {
        let update = panic_update(None, "boom".to_string());
        match update {
            Update::Finished(Err(msg)) => assert!(msg.contains("boom")),
            other => panic!("expected Update::Finished(Err), got {other:?}"),
        }
    }

    /// The peek that has to happen BEFORE `job` moves into the `guarded`
    /// closure -- tested on its own so a mistake there can't hide behind
    /// `panic_update`'s own (separately correct) routing logic.
    #[test]
    fn replan_generation_of_reads_only_job_replan() {
        assert_eq!(
            replan_generation_of(&Job::Replan {
                path: std::path::PathBuf::from("x.png"),
                for_gif: false,
                placement: plan::Placement::Contain,
                adjustments: Adjustments::NONE,
                rate_override: None,
                row: 0,
                generation: 7,
            }),
            Some(7)
        );
        assert_eq!(replan_generation_of(&Job::ClearPicture), None);
    }

    /// `build_pending` (what `Job::Replan`'s handler calls) must carry
    /// forward `row`/`adjustments`/`rate_override` exactly, not just
    /// produce SOME new `Pending` -- this is the test round 15 added because
    /// the App-level tests only prove `App` applies a given `Pending`
    /// correctly, not that the worker built the right one in the first
    /// place.
    #[test]
    fn build_pending_carries_row_adjustments_and_rate_override_through_a_replan() {
        let adjustments = Adjustments {
            brightness: 0.3,
            ..Adjustments::NONE
        };
        let built = build_pending(
            std::path::Path::new("fixtures/test-anim-2frames.gif"),
            true,
            plan::Placement::Fill,
            adjustments,
            Some(45),
            3,
        )
        .expect("a valid GIF");
        match built {
            Pending::Gif {
                plan,
                rate_override,
                adjustments: got_adj,
                row,
                ..
            } => {
                assert_eq!(plan.placement, plan::Placement::Fill);
                assert_eq!(got_adj, adjustments);
                assert_eq!(rate_override, Some(45));
                assert_eq!(row, 3);
            }
            other => panic!("expected a GIF, got {other:?}"),
        }
    }

    /// Probing without assuming a keyboard is attached: whatever it returns
    /// must be a state the interface knows how to describe.
    #[test]
    fn probing_always_yields_a_describable_state() {
        let state = probe();
        assert!(!state.summary().is_empty());
    }

    /// A job that cannot reach a device tells the interface the device is
    /// gone, not just its own discovery flag.
    ///
    /// Clearing `ready` alone only restarts probing; without the update the
    /// header keeps its green dot and its actions keep looking available until
    /// the next probe. That gap was the bug: the test for it used to send the
    /// missing update by hand, so it proved nothing about the worker.
    #[test]
    fn a_job_that_cannot_open_the_device_reports_it_lost() {
        let (tx, rx) = mpsc::channel();
        let flags = Flags::default();
        flags.ready.store(true, Ordering::Relaxed);

        // `open_reporting` is the shared entry every device job goes through.
        // On this machine it either succeeds (a keyboard is attached) or
        // fails; only the failure is interesting, and it must announce itself.
        if open_reporting(&tx, &flags.ready).is_err() {
            let update = rx
                .try_recv()
                .expect("a failure to open must be announced, not just recorded");
            assert!(matches!(update, Update::Device(_)), "got {update:?}");
            assert!(!flags.ready.load(Ordering::Relaxed));
        } else {
            // A keyboard is attached, so the failure path cannot be reached
            // here. `lost()` is the whole of it, and it is asserted directly.
            let (tx2, rx2) = mpsc::channel();
            let ready = Arc::new(AtomicBool::new(true));
            lost(&tx2, &ready);
            assert!(!ready.load(Ordering::Relaxed));
            assert!(matches!(
                rx2.try_recv().unwrap(),
                Update::Device(DeviceState::NotFound)
            ));
        }
    }
}
