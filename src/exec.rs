//! Running an upload: reports, the pauses between them, and the ability to stop.
//!
//! An upload is not a list of reports. It is reports **and mandatory pauses**,
//! in an exact order: 300 ms between a picture's start report and its body,
//! 3000 ms after every sixteenth GIF frame header, 30 ms between the two
//! session-close reports, 500 ms before finish. Get a pause wrong and the
//! keyboard accepts every byte and stores a corrupt frame.
//!
//! `fixtures/*-upload.json` and `check-raw-consistency.js` cannot see any of
//! that: they compare *builder output* against captured logs, so they prove the
//! bytes and say nothing about what happens between them. Before Milestone 5
//! nothing in this repo tested the choreography at all.
//!
//! So the three things an upload does to the outside world -- talk to a device,
//! wait, and say what it is doing -- are all injected. In production they are
//! the real device, the real clock and a printing closure. In tests they are a
//! recorder, which turns "did the refactor change the timing?" into an
//! assertion instead of a hope.
//!
//! ## Why cancellation lives here and not in the device layer
//!
//! The 3-second pause after every sixteenth frame happens **between** device
//! calls, not inside one. A cancel flag checked only in a send loop would leave
//! Esc dead for up to three seconds -- which is most of what a GIF upload
//! spends its time doing. Only the layer that owns both the sends and the
//! sleeps can stop promptly, so that layer is this one.

use crate::device::{Device, DeviceError, ReportIdForm};
use crate::plan::{GifPlan, PicturePlan};
use crate::protocol;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// How often a cancellable sleep looks up from waiting. Short enough that Esc
/// feels immediate, long enough that a 3-second pause is 60 wake-ups.
const CANCEL_POLL_MS: u64 = 50;

/// What an upload is allowed to do to a device, one report at a time.
///
/// Per-report rather than per-sequence because cancellation and progress both
/// happen *between* reports, and a batch call cannot offer that.
pub trait Transport {
    fn drain(&self) -> Result<usize, DeviceError>;

    fn send_report(
        &self,
        form: ReportIdForm,
        report: &[u8; 64],
        notes: &mut dyn FnMut(String),
    ) -> Result<(), DeviceError>;
}

impl Transport for Device {
    fn drain(&self) -> Result<usize, DeviceError> {
        Device::drain(self)
    }

    fn send_report(
        &self,
        form: ReportIdForm,
        report: &[u8; 64],
        notes: &mut dyn FnMut(String),
    ) -> Result<(), DeviceError> {
        Device::send_and_await_acks(self, form, report, notes)
    }
}

/// Waiting, as a dependency.
pub trait Clock {
    /// Waits, looking up every `CANCEL_POLL_MS` to see whether it should stop.
    /// Returns `false` if it was cancelled.
    fn sleep_cancellable(&self, d: Duration, cancel: &AtomicBool) -> bool;
}

/// The real one.
pub struct SystemClock;

impl Clock for SystemClock {
    fn sleep_cancellable(&self, d: Duration, cancel: &AtomicBool) -> bool {
        let slice = Duration::from_millis(CANCEL_POLL_MS);
        let mut left = d;
        while !left.is_zero() {
            if cancel.load(Ordering::Relaxed) {
                return false;
            }
            let step = left.min(slice);
            std::thread::sleep(step);
            left -= step;
        }
        !cancel.load(Ordering::Relaxed)
    }
}

/// Which part of an upload is running, so a caller can say "frame 12 of 36"
/// rather than only a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    /// Opening or closing the GIF session; no frame is in flight.
    Session,
    Frame {
        index: usize,
        of: usize,
    },
    /// The picture body, which has no sub-structure worth naming.
    Picture,
}

/// Everything an upload tells the world.
///
/// One type rather than a progress number plus stray prints, because the
/// diagnostics that used to go to stderr from inside `device.rs` have to reach
/// a TUI's log pane instead of its screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecEvent {
    /// `done` counts reports **written and acknowledged**, out of
    /// `plan.total_reports`. Emitted after the ACK, so "12/36" means the device
    /// confirmed 12 -- not that 12 were pushed at the kernel.
    Progress {
        done: usize,
        total: usize,
        phase: Phase,
    },
    /// A drain warning or a `YUNZII_DEBUG` line. Never printed here; where it
    /// goes is the caller's decision.
    Note(String),
    /// Frame `index` is fully on the device: header, declare-size, every block
    /// and the pause after the last one.
    ///
    /// Separate from `Progress` because "which frame is in flight" and "which
    /// frames are finished" are different questions, and answering the second
    /// with the first is wrong in the way that matters: the CLI used to print
    /// the frame number after the whole frame had gone out, so a failure in
    /// the body meant the number was never printed. Driving that line from
    /// the first `Progress` of a frame reported it before the body was sent.
    /// Review caught it; a console diff could not, because the printed lines
    /// are identical and only their timing differs.
    FrameDone { index: usize, of: usize },
}

/// Everything a running upload needs that is not the bytes themselves.
pub struct ExecCtx<'a> {
    pub dev: &'a dyn Transport,
    pub cancel: &'a AtomicBool,
    pub clock: &'a dyn Clock,
    pub emit: &'a mut dyn FnMut(ExecEvent),
}

impl ExecCtx<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }
}

/// A cancellation with nothing attached yet. The caller adds the "what this
/// left behind" note, because only it knows which command was running.
fn cancelled() -> DeviceError {
    DeviceError::Cancelled { note: None }
}

/// Sends reports one at a time, draining first and stopping on request.
///
/// The drain-before-every-report behaviour is transcribed from
/// `Device::send_sequence`, which is what the choreography used before. The
/// difference is that the warning is an event instead of a line on stderr.
fn send_all(
    cx: &mut ExecCtx,
    reports: &[[u8; 64]],
    done: &mut usize,
    total: usize,
    phase: Phase,
) -> Result<(), DeviceError> {
    for (i, report) in reports.iter().enumerate() {
        // Between reports, never mid-report: a half-written report is a
        // protocol violation, while a half-written upload is only a bad
        // picture.
        if cx.cancelled() {
            return Err(cancelled());
        }

        let drained = cx.dev.drain()?;
        if drained > 0 {
            (cx.emit)(ExecEvent::Note(format!(
                "warning: drained {drained} unexpected report(s) before write #{i}"
            )));
        }

        let mut notes = Vec::new();
        let result = cx
            .dev
            .send_report(ReportIdForm::LeadingZeroOnWrite, report, &mut |m| {
                notes.push(m)
            });
        for note in notes {
            (cx.emit)(ExecEvent::Note(note));
        }
        result?;

        *done += 1;
        (cx.emit)(ExecEvent::Progress {
            done: *done,
            total,
            phase,
        });
    }
    Ok(())
}

/// An interruptible pause. An error rather than a bool at the call site --
/// there are eleven of them in a GIF upload, and `?` keeps them honest.
fn pause(cx: &mut ExecCtx, ms: u64) -> Result<(), DeviceError> {
    if cx
        .clock
        .sleep_cancellable(Duration::from_millis(ms), cx.cancel)
    {
        Ok(())
    } else {
        Err(cancelled())
    }
}

/// The picture upload: one report, the pause the vendor requires, then the body.
pub fn execute_picture(plan: &PicturePlan, cx: &mut ExecCtx) -> Result<(), DeviceError> {
    let start = protocol::build_picture_upload_start();
    let body = protocol::build_picture_upload_body(&plan.pixels);
    let total = plan.total_reports;
    let mut done = 0;

    send_all(cx, &[start], &mut done, total, Phase::Picture)?;

    // The vendor pauses here, between the start report and declare-size --
    // not before the bulk data. See protocol::START_TO_DECLARE_DELAY_MS.
    pause(cx, protocol::START_TO_DECLARE_DELAY_MS)?;

    send_all(cx, &body, &mut done, total, Phase::Picture)?;
    Ok(())
}

/// The GIF upload, pause by pause.
///
/// Transcribed from what used to sit inline in `run_set_gif`; the order of
/// sends and sleeps is unchanged, and the choreography tests exist to prove it.
pub fn execute_gif(plan: &GifPlan, cx: &mut ExecCtx) -> Result<(), DeviceError> {
    let total = plan.total_reports;
    let frame_count = plan.frames.len();
    let mode = plan.mode;
    let mut done = 0;

    send_all(
        cx,
        &protocol::build_gif_session_open(mode),
        &mut done,
        total,
        Phase::Session,
    )?;
    pause(cx, protocol::GIF_SESSION_OPEN_DELAY_MS)?;

    for (i, pixels) in plan.frames.iter().enumerate() {
        let phase = Phase::Frame {
            index: i,
            of: frame_count,
        };
        send_all(
            cx,
            &[protocol::build_gif_frame_header(mode, i as u8)],
            &mut done,
            total,
            phase,
        )?;
        pause(
            cx,
            if i % protocol::GIF_SLOW_DELAY_EVERY == 0 {
                protocol::GIF_FRAME_HEADER_SLOW_DELAY_MS
            } else {
                protocol::GIF_FRAME_HEADER_DELAY_MS
            },
        )?;
        send_all(
            cx,
            &[protocol::build_gif_declare_size()],
            &mut done,
            total,
            phase,
        )?;
        for block in protocol::build_gif_frame_blocks(pixels) {
            send_all(cx, &block, &mut done, total, phase)?;
            pause(cx, protocol::GIF_BLOCK_DELAY_MS)?;
        }
        // Exactly where the CLI's `println!("  frame i/of")` used to sit.
        (cx.emit)(ExecEvent::FrameDone {
            index: i,
            of: frame_count,
        });
    }

    // Sent one at a time: the vendor sleeps 30 ms BETWEEN the two close
    // reports as well as after the second, and batching them would drop the
    // first of those gaps.
    for report in protocol::build_gif_session_close(mode, frame_count as u8, plan.rate) {
        send_all(cx, &[report], &mut done, total, Phase::Session)?;
        pause(cx, protocol::GIF_SESSION_CLOSE_DELAY_MS)?;
    }
    pause(cx, protocol::GIF_PRE_FINISH_DELAY_MS)?;
    send_all(
        cx,
        &[protocol::build_finish()],
        &mut done,
        total,
        Phase::Session,
    )?;
    Ok(())
}

/// One thing that happened, in order.
///
/// Consecutive reports are coalesced into a count so the pause layout stays
/// readable. The bytes are kept separately and asserted in full, so a swapped
/// or duplicated report cannot hide behind a matching count -- a gap the first
/// version of this recorder had, and which review caught.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `n` reports sent back to back with nothing between them.
    Reports(usize),
    /// A pause, in milliseconds.
    Slept(u64),
}

#[cfg(test)]
pub use recorder::Recorder;

#[cfg(test)]
mod recorder {
    use super::*;
    use std::cell::RefCell;

    /// A `Transport` and a `Clock` that write down what they were asked to do.
    pub struct Recorder<'a> {
        steps: RefCell<Vec<Step>>,
        reports: RefCell<Vec<[u8; 64]>>,
        /// Trips this flag once `n` reports have gone out, standing in for a
        /// user pressing Esc part-way through.
        cancel_after: Option<(usize, &'a AtomicBool)>,
        /// What `drain()` claims to have read. Non-zero stands in for a device
        /// that had stale reports queued.
        drain_returns: usize,
        /// Diagnostics the transport hands back per report, standing in for
        /// the `YUNZII_DEBUG` lines a real `Device` produces.
        note_per_report: Option<String>,
    }

    impl<'a> Recorder<'a> {
        pub fn new() -> Self {
            Self {
                steps: RefCell::new(Vec::new()),
                reports: RefCell::new(Vec::new()),
                cancel_after: None,
                drain_returns: 0,
                note_per_report: None,
            }
        }

        /// A transport that reports a diagnostic on every write, the way
        /// `Device` does when `YUNZII_DEBUG` is set.
        pub fn chatty(note: &str) -> Self {
            Self {
                note_per_report: Some(note.to_string()),
                ..Self::new()
            }
        }

        /// A device with stale reports queued before every write.
        pub fn draining(n: usize) -> Self {
            Self {
                drain_returns: n,
                ..Self::new()
            }
        }

        pub fn cancelling_after(n: usize, flag: &'a AtomicBool) -> Self {
            Self {
                cancel_after: Some((n, flag)),
                ..Self::new()
            }
        }

        /// The pause layout.
        pub fn steps(&self) -> Vec<Step> {
            self.steps.borrow().clone()
        }

        /// Every report, in order and in full.
        pub fn reports(&self) -> Vec<[u8; 64]> {
            self.reports.borrow().clone()
        }

        pub fn report_count(&self) -> usize {
            self.reports.borrow().len()
        }
    }

    impl Transport for Recorder<'_> {
        fn drain(&self) -> Result<usize, DeviceError> {
            // A real device drains nothing mid-upload, and zero-read drains
            // are not recorded, so the default matches.
            Ok(self.drain_returns)
        }

        fn send_report(
            &self,
            _form: ReportIdForm,
            report: &[u8; 64],
            notes: &mut dyn FnMut(String),
        ) -> Result<(), DeviceError> {
            if let Some(n) = &self.note_per_report {
                notes(n.clone());
            }
            self.reports.borrow_mut().push(*report);
            {
                let mut steps = self.steps.borrow_mut();
                match steps.last_mut() {
                    Some(Step::Reports(prev)) => *prev += 1,
                    _ => steps.push(Step::Reports(1)),
                }
            }
            if let Some((n, flag)) = self.cancel_after
                && self.reports.borrow().len() >= n
            {
                flag.store(true, Ordering::Relaxed);
            }
            Ok(())
        }
    }

    impl Clock for Recorder<'_> {
        fn sleep_cancellable(&self, d: Duration, cancel: &AtomicBool) -> bool {
            // Records the pause it was asked for and returns instantly: a test
            // asserting a 3-second pause should not take three seconds.
            self.steps
                .borrow_mut()
                .push(Step::Slept(d.as_millis() as u64));
            !cancel.load(Ordering::Relaxed)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan;
    use std::path::Path;

    /// Runs an executor against a recorder with no cancellation, collecting
    /// nothing. The tests that care about events collect them explicitly.
    fn run(
        rec: &Recorder,
        body: &mut dyn FnMut(&mut ExecCtx) -> Result<(), DeviceError>,
    ) -> Result<(), DeviceError> {
        let never = AtomicBool::new(false);
        let mut emit = |_: ExecEvent| {};
        let mut cx = ExecCtx {
            dev: rec,
            cancel: &never,
            clock: rec,
            emit: &mut emit,
        };
        body(&mut cx)
    }

    /// Runs and keeps every event, for the progress and note assertions.
    fn run_collecting(
        rec: &Recorder,
        cancel: &AtomicBool,
        body: &mut dyn FnMut(&mut ExecCtx) -> Result<(), DeviceError>,
    ) -> (Result<(), DeviceError>, Vec<ExecEvent>) {
        let mut events = Vec::new();
        let mut emit = |e: ExecEvent| events.push(e);
        let mut cx = ExecCtx {
            dev: rec,
            cancel,
            clock: rec,
            emit: &mut emit,
        };
        let r = body(&mut cx);
        (r, events)
    }

    // --- The choreography ---
    //
    // These are the only tests in the repo that can see a pause. The capture
    // fixtures pin every byte and none of the timing, so a refactor that
    // dropped the 300 ms before a picture body, or moved the 3-second pause to
    // the wrong frame, would pass everything else in `bin/ci`.
    //
    // The durations below are written as LITERALS, deliberately, and must not
    // be re-derived from `protocol::*`. An earlier version used the constants
    // on both sides, so changing one from 3000 to 2000 changed the expectation
    // with it and the test still passed -- review caught that. These numbers
    // come from the vendor capture, they are what the firmware requires, and
    // this is where they are stated independently of the code that uses them.
    // Changing a delay should mean deliberately changing it here too.

    /// The picture upload, end to end: one report, the 300 ms the vendor
    /// requires, then the 551-report body. Nothing else, in that order.
    #[test]
    fn picture_upload_choreography() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let body = protocol::build_picture_upload_body(&plan.pixels);

        let rec = Recorder::new();
        run(&rec, &mut |cx| execute_picture(&plan, cx)).unwrap();

        assert_eq!(
            rec.steps(),
            vec![
                Step::Reports(1),          // start
                Step::Slept(300),          // the vendor's pause, as a number
                Step::Reports(body.len()), // declare + pixels + finish
            ],
            "the picture pause must sit between the start report and the body"
        );
        assert_eq!(
            rec.report_count(),
            protocol::picture_upload_report_count(),
            "552 reports, matching fixtures/picture-upload.json"
        );
    }

    /// The GIF upload for a 2-frame animation, pause by pause.
    ///
    /// Written out in full rather than summarised: this sequence is the thing
    /// being protected, and a reader should be able to check it against
    /// PROTOCOL.md without running anything.
    #[test]
    fn gif_upload_choreography() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        assert_eq!(plan.frames.len(), 2);

        let rec = Recorder::new();
        run(&rec, &mut |cx| execute_gif(&plan, cx)).unwrap();

        let blocks = protocol::GIF_BLOCKS_PER_FRAME;
        let per_block = protocol::GIF_PACKETS_PER_BLOCK;

        let mut want = vec![
            // Both session-open reports go out together, then ONE pause.
            // There is no gap between report 18 and report 19.
            Step::Reports(2),
            Step::Slept(30),
        ];
        for i in 0..2 {
            want.push(Step::Reports(1)); // frame header
            want.push(Step::Slept(if i % 16 == 0 { 3000 } else { 30 }));
            // Declare-size and the first block run together: there is no
            // pause between them, so the trace shows them as one run of
            // 1 + 19 reports. Worth seeing rather than hiding -- it is the
            // only place in the upload where two different kinds of report
            // are sent back to back.
            want.push(Step::Reports(1 + per_block));
            want.push(Step::Slept(30));
            for _ in 1..blocks {
                want.push(Step::Reports(per_block));
                want.push(Step::Slept(30));
            }
        }
        // Close reports one at a time, so the gap BETWEEN them survives.
        want.push(Step::Reports(1));
        want.push(Step::Slept(30));
        want.push(Step::Reports(1));
        want.push(Step::Slept(30));
        want.push(Step::Slept(500));
        want.push(Step::Reports(1)); // finish

        assert_eq!(rec.steps(), want);
        assert_eq!(
            rec.report_count(),
            protocol::gif_upload_report_count(2),
            "1149 reports for 2 frames, matching fixtures/gif-upload.json"
        );
    }

    /// The three-second pause lands on frames 0 and 16 and nowhere else.
    ///
    /// Two frames cannot show this: index 0 is the only slow one they contain,
    /// so `i % 16 == 0` and `i == 0` are indistinguishable. Eighteen frames
    /// tell them apart, which is the whole reason that fixture exists.
    #[test]
    fn the_long_pause_falls_on_every_sixteenth_frame_only() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-18frames.gif"), Some(10), None)
                .unwrap();
        assert_eq!(plan.frames.len(), 18);

        let rec = Recorder::new();
        run(&rec, &mut |cx| execute_gif(&plan, cx)).unwrap();

        // Header pauses are the only ones that are either 3000 or 30 and sit
        // immediately after a single-report step, so pick them out by walking
        // the trace rather than by filtering on duration -- block pauses are
        // also 30 ms.
        let steps = rec.steps();
        let declare_plus_first_block = 1 + protocol::GIF_PACKETS_PER_BLOCK;
        let mut header_pauses = Vec::new();
        for w in steps.windows(3) {
            // A frame header is the only lone report followed by a pause and
            // then the declare-size-plus-first-block run.
            if let [Step::Reports(1), Step::Slept(ms), Step::Reports(n)] = w
                && *n == declare_plus_first_block
            {
                header_pauses.push(*ms);
            }
        }

        assert_eq!(
            header_pauses.len(),
            18,
            "one header pause per frame, got {header_pauses:?}"
        );
        let slow: Vec<usize> = header_pauses
            .iter()
            .enumerate()
            .filter(|(_, ms)| **ms == 3000)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            slow,
            vec![0, 16],
            "the 3 s pause belongs to frames 0 and 16 only"
        );
    }

    // --- Bytes, not just counts ---

    /// Every report, in order, compared against an independently-built list.
    ///
    /// The first version of this recorder stored only counts, and review was
    /// right that a swapped, duplicated or replaced report would slip through
    /// a matching count. The capture fixtures do not close that gap either:
    /// they check `fixtures/gif-upload.json` against a hardware log, not
    /// against what the executor emits.
    #[test]
    fn the_gif_executor_emits_exactly_the_expected_reports() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let rec = Recorder::new();
        run(&rec, &mut |cx| execute_gif(&plan, cx)).unwrap();

        let mode = plan.mode;
        let mut want: Vec<[u8; 64]> = Vec::new();
        want.extend(protocol::build_gif_session_open(mode));
        for (i, pixels) in plan.frames.iter().enumerate() {
            want.push(protocol::build_gif_frame_header(mode, i as u8));
            want.push(protocol::build_gif_declare_size());
            for block in protocol::build_gif_frame_blocks(pixels) {
                want.extend(block);
            }
        }
        want.extend(protocol::build_gif_session_close(
            mode,
            plan.frames.len() as u8,
            plan.rate,
        ));
        want.push(protocol::build_finish());

        let got = rec.reports();
        assert_eq!(got.len(), want.len(), "report count");
        // Report-by-report, so a failure names the index instead of dumping
        // 1149 arrays.
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(g, w, "report #{i} differs");
        }
    }

    #[test]
    fn the_picture_executor_emits_exactly_the_expected_reports() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let rec = Recorder::new();
        run(&rec, &mut |cx| execute_picture(&plan, cx)).unwrap();

        let mut want = vec![protocol::build_picture_upload_start()];
        want.extend(protocol::build_picture_upload_body(&plan.pixels));

        let got = rec.reports();
        assert_eq!(got.len(), want.len());
        for (i, (g, w)) in got.iter().zip(want.iter()).enumerate() {
            assert_eq!(g, w, "report #{i} differs");
        }
    }

    // --- Progress ---

    /// Progress counts acknowledged reports, reaches the planned total exactly,
    /// and never runs backwards.
    #[test]
    fn progress_counts_every_report_and_ends_on_the_planned_total() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let never = AtomicBool::new(false);
        let rec = Recorder::new();
        let (result, events) = run_collecting(&rec, &never, &mut |cx| execute_gif(&plan, cx));
        result.unwrap();

        let progress: Vec<(usize, usize)> = events
            .iter()
            .filter_map(|e| match e {
                ExecEvent::Progress { done, total, .. } => Some((*done, *total)),
                _ => None,
            })
            .collect();

        assert_eq!(progress.len(), plan.total_reports, "one event per report");
        assert_eq!(progress.first(), Some(&(1, plan.total_reports)));
        assert_eq!(
            progress.last(),
            Some(&(plan.total_reports, plan.total_reports)),
            "the last event must reach the total the planner promised"
        );
        assert!(
            progress.windows(2).all(|w| w[1].0 == w[0].0 + 1),
            "progress must advance by exactly one report at a time"
        );
    }

    /// The phase says which frame is in flight, so a caller can print
    /// "frame 12/36" and not only a percentage.
    #[test]
    fn progress_names_the_frame_being_sent() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let never = AtomicBool::new(false);
        let rec = Recorder::new();
        let (_, events) = run_collecting(&rec, &never, &mut |cx| execute_gif(&plan, cx));

        let frames: Vec<usize> = events
            .iter()
            .filter_map(|e| match e {
                ExecEvent::Progress {
                    phase: Phase::Frame { index, of },
                    ..
                } => {
                    assert_eq!(*of, 2);
                    Some(*index)
                }
                _ => None,
            })
            .collect();
        assert!(frames.contains(&0) && frames.contains(&1));
        assert!(
            frames.windows(2).all(|w| w[1] >= w[0]),
            "frames are sent in order"
        );

        // The session reports at each end are not attributed to a frame.
        let first = events.iter().find_map(|e| match e {
            ExecEvent::Progress { phase, .. } => Some(*phase),
            _ => None,
        });
        assert_eq!(first, Some(Phase::Session));
    }

    // --- Cancellation ---

    /// Cancelling stops between reports, not mid-report, and reports itself as
    /// a cancellation rather than as an I/O fault.
    #[test]
    fn cancelling_stops_promptly_and_is_not_an_io_error() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let cancel = AtomicBool::new(false);
        let rec = Recorder::cancelling_after(50, &cancel);

        let (result, _) = run_collecting(&rec, &cancel, &mut |cx| execute_gif(&plan, cx));

        let err = result.expect_err("a cancelled upload must not report success");
        assert!(
            matches!(err, DeviceError::Cancelled { .. }),
            "pressing Esc is not a fault; got {err:?}"
        );
        assert!(
            rec.report_count() < plan.total_reports,
            "it must actually have stopped early: sent {} of {}",
            rec.report_count(),
            plan.total_reports
        );
        assert!(
            rec.report_count() <= 51,
            "it must stop at the next report boundary, not run on: {}",
            rec.report_count()
        );
    }

    /// A cancelled upload still says what it left on the keyboard.
    ///
    /// `with_note` used to rewrite only the `Io` variant, so adding a
    /// `Cancelled` variant would have silently dropped the warning that the
    /// animation is half-written. Two reviewers flagged that on the plan.
    #[test]
    fn a_cancelled_upload_still_warns_about_what_it_left_behind() {
        let err = DeviceError::Cancelled { note: None }
            .with_note("the animation on the keyboard may be incomplete");
        let msg = err.to_string();
        assert!(msg.starts_with("cancelled"), "got: {msg}");
        assert!(msg.contains("may be incomplete"), "got: {msg}");

        // And it composes, rather than replacing an existing note.
        let twice = DeviceError::Cancelled {
            note: Some("first".into()),
        }
        .with_note("second");
        assert!(twice.to_string().contains("first -- second"));
    }

    /// The pause is interruptible: a cancel raised during a 3-second wait ends
    /// the upload instead of being ignored until the wait finishes.
    ///
    /// This is the case that made cancellation belong in the executor rather
    /// than in a send loop -- most of a GIF upload's wall-clock time is spent
    /// in these pauses.
    #[test]
    fn a_cancel_during_a_pause_ends_the_upload() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let cancel = AtomicBool::new(false);
        // Two session-open reports go out, then the 30 ms pause: trip the flag
        // during it by arming after exactly those two.
        let rec = Recorder::cancelling_after(2, &cancel);

        let (result, _) = run_collecting(&rec, &cancel, &mut |cx| execute_gif(&plan, cx));

        assert!(matches!(
            result.expect_err("must stop"),
            DeviceError::Cancelled { .. }
        ));
        assert_eq!(
            rec.report_count(),
            2,
            "nothing may be sent after the interrupted pause"
        );
    }

    /// The real clock wakes up often enough for Esc to feel immediate.
    #[test]
    fn the_system_clock_returns_early_when_cancelled() {
        let cancel = AtomicBool::new(true);
        let start = std::time::Instant::now();
        let completed = SystemClock.sleep_cancellable(Duration::from_secs(3), &cancel);
        assert!(!completed, "an already-cancelled sleep must report so");
        assert!(
            start.elapsed() < Duration::from_millis(500),
            "a 3 s pause must not run to completion once cancelled: took {:?}",
            start.elapsed()
        );

        // And it still waits when nobody cancels.
        let never = AtomicBool::new(false);
        let start = std::time::Instant::now();
        assert!(SystemClock.sleep_cancellable(Duration::from_millis(120), &never));
        assert!(start.elapsed() >= Duration::from_millis(100));
    }

    /// The frame line is reported when the frame is *finished*, not when its
    /// header is acknowledged.
    ///
    /// This is the regression codex found in round 1 and the reason
    /// `FrameDone` exists. A console diff cannot catch it: the printed lines
    /// are identical either way, and only their position relative to the
    /// sends differs. So the assertion is on that position.
    #[test]
    fn a_frame_is_reported_only_after_its_whole_body_is_sent() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let never = AtomicBool::new(false);
        let rec = Recorder::new();
        let (result, events) = run_collecting(&rec, &never, &mut |cx| execute_gif(&plan, cx));
        result.unwrap();

        // Reports sent by the time each FrameDone was emitted.
        let mut sent = 0usize;
        let mut done_at = Vec::new();
        for ev in &events {
            match ev {
                ExecEvent::Progress { done, .. } => sent = *done,
                ExecEvent::FrameDone { index, of } => {
                    assert_eq!(*of, 2);
                    done_at.push((*index, sent));
                }
                _ => {}
            }
        }

        assert_eq!(done_at.len(), 2, "one per frame");
        let per_frame = 1 + 1 + protocol::GIF_BLOCKS_PER_FRAME * protocol::GIF_PACKETS_PER_BLOCK;
        // 2 session-open reports precede frame 0.
        assert_eq!(
            done_at[0],
            (0, 2 + per_frame),
            "frame 0 is finished only once its header, declare-size and all \
             {} blocks have gone out",
            protocol::GIF_BLOCKS_PER_FRAME
        );
        assert_eq!(done_at[1], (1, 2 + 2 * per_frame));
    }

    /// A GIF that fails part-way must not have reported the frame it was on.
    #[test]
    fn an_interrupted_frame_is_never_reported_as_done() {
        let plan =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        let cancel = AtomicBool::new(false);
        // Stop inside frame 0's body: past its header, well before its blocks
        // are finished.
        let rec = Recorder::cancelling_after(10, &cancel);
        let (result, events) = run_collecting(&rec, &cancel, &mut |cx| execute_gif(&plan, cx));

        assert!(matches!(
            result.expect_err("must stop"),
            DeviceError::Cancelled { .. }
        ));
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, ExecEvent::FrameDone { .. })),
            "no frame finished, so none may be reported"
        );
    }

    /// A drain that reads something becomes a note the caller can show.
    ///
    /// This used to be an `eprintln!` inside `device.rs`. Nothing proved it
    /// still reaches anyone after the move.
    #[test]
    fn a_nonzero_drain_becomes_a_note() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let never = AtomicBool::new(false);
        let rec = Recorder::draining(3);
        let (result, events) = run_collecting(&rec, &never, &mut |cx| execute_picture(&plan, cx));
        result.unwrap();

        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ExecEvent::Note(m) => Some(m.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes.len(),
            plan.total_reports,
            "one warning per report, since this device always has stale data"
        );
        assert!(
            notes[0].contains("drained 3 unexpected report(s)"),
            "got {:?}",
            notes[0]
        );
        assert!(notes[0].contains("before write #0"), "got {:?}", notes[0]);
    }

    /// Diagnostics the transport produces reach the caller as events.
    ///
    /// A real `Device` writes `YUNZII_DEBUG` lines through this same callback.
    /// It used to `eprintln!` them, which would land on top of a TUI's screen;
    /// nothing proved the replacement path actually carries them.
    #[test]
    fn transport_diagnostics_become_notes_rather_than_output() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let never = AtomicBool::new(false);
        let rec = Recorder::chatty("DEBUG send: de ad be ef");
        let (result, events) = run_collecting(&rec, &never, &mut |cx| execute_picture(&plan, cx));
        result.unwrap();

        let notes: Vec<&str> = events
            .iter()
            .filter_map(|e| match e {
                ExecEvent::Note(m) => Some(m.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            notes.len(),
            plan.total_reports,
            "one diagnostic per report reaches the caller"
        );
        assert!(
            notes.iter().all(|n| n.contains("DEBUG send:")),
            "got {:?}",
            notes.first()
        );
    }
}
