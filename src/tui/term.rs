//! Getting the terminal back, whatever happens.
//!
//! This is the one part of a TUI whose failure punishes the user rather than
//! the program. Leave raw mode on and the shell stops echoing; leave the
//! alternate screen and their scrollback is gone. Either way they are left
//! blind-typing `reset`, and the program that did it has already exited.
//!
//! So restoration is a `Drop` guard *and* a panic hook, it is idempotent so
//! the two cannot fight, and a half-finished setup undoes whatever it managed
//! before reporting the error.

use std::io;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// The terminal operations, behind a trait so the tests can watch them.
///
/// "The hook is installed" is not a test of recovery. This is what lets a test
/// assert that a panic restores exactly once, and that a setup which fails
/// half way leaves nothing enabled.
pub trait Term {
    fn enable_raw(&self) -> io::Result<()>;
    fn disable_raw(&self) -> io::Result<()>;
    fn enter_alt(&self) -> io::Result<()>;
    fn leave_alt(&self) -> io::Result<()>;
}

/// The real terminal.
pub struct RealTerm;

impl Term for RealTerm {
    fn enable_raw(&self) -> io::Result<()> {
        ratatui::crossterm::terminal::enable_raw_mode()
    }
    fn disable_raw(&self) -> io::Result<()> {
        ratatui::crossterm::terminal::disable_raw_mode()
    }
    fn enter_alt(&self) -> io::Result<()> {
        ratatui::crossterm::execute!(
            io::stdout(),
            ratatui::crossterm::terminal::EnterAlternateScreen
        )
    }
    fn leave_alt(&self) -> io::Result<()> {
        ratatui::crossterm::execute!(
            io::stdout(),
            ratatui::crossterm::terminal::LeaveAlternateScreen
        )
    }
}

/// Restores on drop, once.
pub struct Guard<T: Term> {
    term: T,
    restored: Arc<AtomicBool>,
}

impl<T: Term> Guard<T> {
    /// Enters raw mode and the alternate screen, undoing a partial setup if
    /// the second step fails.
    pub fn new(term: T) -> io::Result<Self> {
        term.enable_raw()?;
        if let Err(e) = term.enter_alt() {
            // Raw mode is on and the alternate screen is not. Leaving it that
            // way would be worse than the original error.
            let _ = term.disable_raw();
            return Err(e);
        }
        Ok(Self {
            term,
            restored: Arc::new(AtomicBool::new(false)),
        })
    }

    /// The flag that says restoration already happened.
    ///
    /// Handed to the panic hook so the two share one answer. Two independent
    /// flags would both read "not yet" and both restore, and leaving the
    /// alternate screen twice pops one the user was already in.
    pub fn flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.restored)
    }

    /// Puts the terminal back. Safe to call more than once; the second call
    /// does nothing, which is what lets the panic hook and the drop guard both
    /// exist without fighting.
    pub fn restore(&self) {
        if self.restored.swap(true, Ordering::SeqCst) {
            return;
        }
        // Order matters: leave the alternate screen first, so anything printed
        // afterwards -- a panic message, a partial-write warning -- lands on
        // the normal screen where the user can actually read it.
        let _ = self.term.leave_alt();
        let _ = self.term.disable_raw();
    }
}

impl<T: Term> Drop for Guard<T> {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Installs a panic hook that restores the real terminal before printing.
///
/// Separate from the guard because a panic unwinds past `main`'s locals in an
/// order nobody should have to reason about; the hook runs first and the
/// guard's idempotence makes the later drop a no-op.
pub fn install_panic_hook(restored: Arc<AtomicBool>) {
    let ui_thread = std::thread::current().id();
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        if owns_the_terminal(ui_thread) && !restored.swap(true, Ordering::SeqCst) {
            let t = RealTerm;
            let _ = t.leave_alt();
            let _ = t.disable_raw();
        }
        previous(info);
    }));
}

/// Whether the panicking thread is the one drawing.
///
/// A panic hook is process-wide, so a panic on the **worker** thread runs it
/// too. The worker catches its own panics and carries on, so restoring there
/// would hand the terminal back while the interface is still drawing into it:
/// the alternate screen gone, raw mode off, and a UI merrily painting over the
/// user's shell. Only the thread that owns the terminal may give it back.
fn owns_the_terminal(ui_thread: std::thread::ThreadId) -> bool {
    std::thread::current().id() == ui_thread
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    enum Op {
        EnableRaw,
        DisableRaw,
        EnterAlt,
        LeaveAlt,
    }

    struct Fake {
        ops: RefCell<Vec<Op>>,
        fail_enter_alt: bool,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                ops: RefCell::new(Vec::new()),
                fail_enter_alt: false,
            }
        }
        fn failing_to_enter() -> Self {
            Self {
                fail_enter_alt: true,
                ..Self::new()
            }
        }
        fn ops(&self) -> Vec<Op> {
            self.ops.borrow().clone()
        }
    }

    impl Term for &Fake {
        fn enable_raw(&self) -> io::Result<()> {
            self.ops.borrow_mut().push(Op::EnableRaw);
            Ok(())
        }
        fn disable_raw(&self) -> io::Result<()> {
            self.ops.borrow_mut().push(Op::DisableRaw);
            Ok(())
        }
        fn enter_alt(&self) -> io::Result<()> {
            if self.fail_enter_alt {
                return Err(io::Error::other("no alternate screen"));
            }
            self.ops.borrow_mut().push(Op::EnterAlt);
            Ok(())
        }
        fn leave_alt(&self) -> io::Result<()> {
            self.ops.borrow_mut().push(Op::LeaveAlt);
            Ok(())
        }
    }

    #[test]
    fn a_normal_run_restores_exactly_once() {
        let fake = Fake::new();
        {
            let _guard = Guard::new(&fake).unwrap();
            assert_eq!(fake.ops(), vec![Op::EnableRaw, Op::EnterAlt]);
        }
        assert_eq!(
            fake.ops(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::LeaveAlt, Op::DisableRaw],
            "and it leaves the alternate screen before disabling raw mode"
        );
    }

    /// The case the panic hook creates: restored explicitly, then dropped.
    #[test]
    fn restoring_twice_does_it_once() {
        let fake = Fake::new();
        {
            let guard = Guard::new(&fake).unwrap();
            guard.restore();
            guard.restore();
        }
        let ops = fake.ops();
        assert_eq!(
            ops.iter().filter(|o| **o == Op::LeaveAlt).count(),
            1,
            "got {ops:?}"
        );
        assert_eq!(ops.iter().filter(|o| **o == Op::DisableRaw).count(), 1);
    }

    /// A setup that fails half way leaves nothing enabled.
    #[test]
    fn a_failed_setup_undoes_what_it_managed() {
        let fake = Fake::failing_to_enter();
        let err = match Guard::new(&fake) {
            Ok(_) => panic!("entering the alternate screen must fail here"),
            Err(e) => e,
        };
        assert_eq!(err.to_string(), "no alternate screen");
        assert_eq!(
            fake.ops(),
            vec![Op::EnableRaw, Op::DisableRaw],
            "raw mode must not be left on after a failed setup"
        );
    }

    /// Restoration happens even when the stack is being unwound.
    #[test]
    fn a_panic_still_restores() {
        let fake = Fake::new();
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _guard = Guard::new(&fake).unwrap();
            panic!("something went wrong");
        }));
        assert!(result.is_err());
        assert_eq!(
            fake.ops(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::LeaveAlt, Op::DisableRaw],
            "the drop guard runs during unwind"
        );
    }

    /// A panic on a background thread must not restore the terminal.
    ///
    /// The hook is process-wide, so it runs for every thread. The worker
    /// catches its own panics and keeps going, so a restore there would give
    /// the terminal back while the interface is still drawing into it.
    #[test]
    fn only_the_drawing_thread_may_restore() {
        let ui = std::thread::current().id();
        assert!(owns_the_terminal(ui), "the UI thread may");

        let elsewhere = std::thread::spawn(move || owns_the_terminal(ui))
            .join()
            .unwrap();
        assert!(!elsewhere, "a worker thread may not");
    }

    /// A hook sharing the guard's flag restores exactly once between them.
    ///
    /// The interesting failure is not "neither restores" but "both do":
    /// leaving the alternate screen twice pops one the user was already in,
    /// and their scrollback goes with it.
    #[test]
    fn the_hook_and_the_guard_do_not_both_restore() {
        let fake = Fake::new();
        let guard = Guard::new(&fake).unwrap();
        let flag = guard.flag();

        // What the hook does, without installing a process-wide hook in a
        // test that runs alongside others.
        assert!(!flag.swap(true, Ordering::SeqCst), "the hook wins the race");
        // ...so the guard's own restore must now do nothing.
        guard.restore();
        drop(guard);

        assert_eq!(
            fake.ops(),
            vec![Op::EnableRaw, Op::EnterAlt],
            "the guard must not restore again after the hook claimed it"
        );
    }

    /// And the other order: the guard restores, so a later hook does nothing.
    #[test]
    fn a_guard_that_restored_first_leaves_the_hook_nothing_to_do() {
        let fake = Fake::new();
        let flag = {
            let guard = Guard::new(&fake).unwrap();
            let flag = guard.flag();
            guard.restore();
            flag
        };
        assert!(
            flag.load(Ordering::SeqCst),
            "the flag is shared, so the hook sees it as already done"
        );
        assert_eq!(
            fake.ops(),
            vec![Op::EnableRaw, Op::EnterAlt, Op::LeaveAlt, Op::DisableRaw]
        );
    }
}
