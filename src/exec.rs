//! The seam that makes an upload observable.
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
//! So the two things an upload does to the outside world -- talk to a device,
//! and wait -- become traits. In production they are the real device and the
//! real clock. In tests they are a recorder, which turns "did the refactor
//! change the timing?" into an assertion instead of a hope.

use crate::device::{Device, DeviceError, ReportIdForm};
use std::time::Duration;

/// What an upload is allowed to do to a device.
///
/// Deliberately the two calls the existing runners already make, at the same
/// granularity, so introducing this trait moves no logic.
pub trait Transport {
    fn send_sequence(&self, form: ReportIdForm, reports: &[[u8; 64]]) -> Result<(), DeviceError>;
}

impl Transport for Device {
    fn send_sequence(&self, form: ReportIdForm, reports: &[[u8; 64]]) -> Result<(), DeviceError> {
        Device::send_sequence(self, form, reports)
    }
}

/// Waiting, as a dependency.
///
/// The pauses are firmware requirements, not politeness, so a test must be able
/// to observe them without actually waiting 45 seconds for a GIF.
pub trait Clock {
    fn sleep(&self, d: Duration);
}

/// The real one.
pub struct SystemClock;

impl Clock for SystemClock {
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// One thing that happened, in order.
///
/// Consecutive reports are coalesced into a count rather than kept
/// individually, on purpose. The exact bytes are already pinned by
/// `fixtures/picture-upload.json` and `fixtures/gif-upload.json` against real
/// captures; repeating them here would make a 1149-entry golden file whose
/// diffs nobody reads, and would bury the one thing this trace exists to show.
/// What is missing elsewhere -- and therefore what this records -- is **where
/// the pauses fall**.
#[cfg(test)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// `n` reports sent back to back with nothing between them.
    Reports(usize),
    /// A pause, in milliseconds.
    Slept(u64),
}

// There is deliberately no `Drained` step yet. `send_sequence` drains before
// every report, so recording drains would add ~1150 near-empty entries to a
// GIF trace and bury the pauses this exists to show. When the executor starts
// surfacing drain warnings as events, the step and the trait method arrive
// together with a test that needs them -- not before.

#[cfg(test)]
pub use recorder::Recorder;

#[cfg(test)]
mod recorder {
    use super::*;
    use std::cell::RefCell;

    /// A `Transport` and a `Clock` that write down what they were asked to do.
    pub struct Recorder {
        steps: RefCell<Vec<Step>>,
    }

    impl Recorder {
        pub fn new() -> Self {
            Self {
                steps: RefCell::new(Vec::new()),
            }
        }

        /// The trace so far, with consecutive `Reports` runs merged.
        pub fn steps(&self) -> Vec<Step> {
            self.steps.borrow().clone()
        }

        /// Total reports sent.
        pub fn report_count(&self) -> usize {
            self.steps
                .borrow()
                .iter()
                .map(|s| match s {
                    Step::Reports(n) => *n,
                    _ => 0,
                })
                .sum()
        }

        fn push_reports(&self, n: usize) {
            let mut steps = self.steps.borrow_mut();
            match steps.last_mut() {
                Some(Step::Reports(prev)) => *prev += n,
                _ => steps.push(Step::Reports(n)),
            }
        }
    }

    impl Transport for Recorder {
        fn send_sequence(
            &self,
            _form: ReportIdForm,
            reports: &[[u8; 64]],
        ) -> Result<(), DeviceError> {
            self.push_reports(reports.len());
            Ok(())
        }
    }

    impl Clock for Recorder {
        fn sleep(&self, d: Duration) {
            self.steps
                .borrow_mut()
                .push(Step::Slept(d.as_millis() as u64));
        }
    }
}
