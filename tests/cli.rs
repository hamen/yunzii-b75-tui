//! Command-level tests: the real binary, real argv, real stdout and stderr.
//!
//! Unit tests can assert that a planner returns the right `Note` on the right
//! `Stream`, and that `write_notes` honours it. Neither can catch the binary
//! wiring them up differently, and review was right to keep asking for this
//! layer after a console-output regression slipped through twice -- once
//! caught by a hardware diff, once only by a reviewer reading the code.
//!
//! **Nothing here may touch the keyboard.** `cargo test` runs in `bin/ci` on
//! every push, and a test suite that reprograms the user's hardware is a bug,
//! not a thorough test. The first draft of this file did exactly that: two
//! cases ran a real `set-gif` and took 7.7 seconds uploading a strobing test
//! pattern to the panel. `--dry-run` exists partly so they do not have to.
//!
//! Every case here either fails before a device is opened, is rejected by the
//! argument parser, or passes `--dry-run`.

use std::path::PathBuf;
use std::process::{Command, Output};

fn bin() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_yunzii-b75-tui"))
}

fn run(args: &[&str]) -> Output {
    Command::new(bin())
        .args(args)
        // The repo root, so `fixtures/...` resolves the same way it does for
        // a person running the binary from a checkout.
        .current_dir(env!("CARGO_MANIFEST_DIR"))
        .output()
        .expect("failed to run the binary")
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

#[test]
fn a_broken_gif_fails_on_stderr_and_says_the_keyboard_was_untouched() {
    let out = run(&["set-gif", "fixtures/test-quadrants.png"]);
    assert!(!out.status.success(), "a PNG is not a GIF");

    let err = stderr(&out);
    assert!(err.contains("as an animation"), "got: {err}");
    assert!(err.contains("supported formats: GIF."), "got: {err}");
    assert!(
        err.contains("The keyboard was not contacted."),
        "got: {err}"
    );
    assert!(
        !err.contains("PNG and JPEG"),
        "a set-gif failure must not advertise the other command's formats: {err}"
    );
    assert!(
        !stdout(&out).contains("could not use"),
        "errors belong on stderr"
    );
}

#[test]
fn a_missing_picture_is_an_image_error_not_a_device_error() {
    let out = run(&["set-picture", "fixtures/does-not-exist.png"]);
    assert!(!out.status.success());

    let err = stderr(&out);
    assert!(err.contains("as a picture"), "got: {err}");
    assert!(
        err.contains("The keyboard was not contacted."),
        "got: {err}"
    );
    assert!(
        !err.contains("no matching") && !err.contains("device not found"),
        "a missing file must not be reported as a missing keyboard: {err}"
    );
}

#[test]
fn out_of_range_options_are_rejected_by_the_argument_parser() {
    for (args, expected) in [
        (
            vec!["set-gif", "a.gif", "--max-frames", "161"],
            "max-frames",
        ),
        (vec!["set-gif", "a.gif", "--fps", "61"], "fps"),
        (vec!["switch-page", "nonsense"], "page"),
    ] {
        let out = run(&args);
        assert!(!out.status.success(), "{args:?} should be rejected");
        let err = stderr(&out).to_lowercase();
        assert!(
            err.contains(expected),
            "{args:?}: the error should name {expected}; got: {err}"
        );
        // Rejected at parse time means the file is never even opened.
        assert!(
            !err.contains("could not use"),
            "{args:?} must fail before the file is read: {err}"
        );
    }
}

/// The stream split, through the actual binary.
///
/// `plan.rs` asserts the `Note` values and `main.rs` asserts `write_notes`.
/// Only this can catch the two being wired together wrongly.
#[test]
fn the_rate_warning_goes_to_stderr_and_the_summary_to_stdout() {
    // A GIF whose delays ask for 100 fps: warns, then falls back to 30.
    // `--dry-run` stops before the device, so this asserts the output wiring
    // without uploading anything.
    let out = run(&["set-gif", "fixtures/test-anim-too-fast.gif", "--dry-run"]);
    assert!(out.status.success(), "a dry run succeeds: {}", stderr(&out));

    let o = stdout(&out);
    let e = stderr(&out);

    assert!(
        e.contains("100 fps") && e.contains("Using 30 fps"),
        "the fallback warning belongs on stderr; got: {e}"
    );
    assert!(
        o.contains("2 frame(s) at 30 fps"),
        "the summary belongs on stdout; got: {o}"
    );
    assert!(
        !o.contains("100 fps"),
        "the warning must not also appear on stdout: {o}"
    );
    assert!(
        !e.contains("frame(s) at 30 fps"),
        "the summary must not also appear on stderr: {e}"
    );
    assert!(
        o.contains("reading fixtures/test-anim-too-fast.gif"),
        "the progress line stays on stdout; got: {o}"
    );
}

/// An explicit rate silences the warning, end to end.
#[test]
fn an_explicit_rate_prints_no_warning_at_all() {
    let out = run(&[
        "set-gif",
        "fixtures/test-anim-too-fast.gif",
        "--fps",
        "24",
        "--dry-run",
    ]);
    let e = stderr(&out);
    assert!(
        !e.contains("Using") && !e.contains("outside the"),
        "no fallback note is due when the rate was chosen; got: {e}"
    );
    assert!(stdout(&out).contains("2 frame(s) at 24 fps"));
}

/// A dry run reports the plan and says plainly that nothing was sent.
#[test]
fn a_dry_run_reports_the_plan_without_contacting_the_keyboard() {
    let out = run(&["set-gif", "fixtures/test-anim-18frames.gif", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let o = stdout(&out);
    assert!(o.contains("18 frame(s)"), "got: {o}");
    assert!(o.contains("roughly 24s"), "the estimate is useful: {o}");
    assert!(
        o.contains("dry run: the keyboard was not contacted."),
        "got: {o}"
    );
    assert!(
        !o.contains("found device"),
        "a dry run must not look for a device: {o}"
    );
    assert!(!o.contains("sent successfully"), "nothing was sent: {o}");

    // The same for pictures.
    let out = run(&["set-picture", "fixtures/test-quadrants.png", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let o = stdout(&out);
    assert!(o.contains("30720 bytes"), "got: {o}");
    assert!(
        o.contains("dry run: the keyboard was not contacted."),
        "got: {o}"
    );
    assert!(!o.contains("found device"), "got: {o}");
}

/// A second stream-split case, with a different note on stderr.
///
/// One case can pass by accident. Subsampling is a different note from a
/// different branch of the planner, and it must land on stderr too, while its
/// summary goes to stdout.
#[test]
fn the_subsampling_note_also_splits_correctly() {
    let out = run(&[
        "set-gif",
        "fixtures/test-anim-18frames.gif",
        "--fps",
        "30",
        "--max-frames",
        "9",
        "--dry-run",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));

    let o = stdout(&out);
    let e = stderr(&out);

    assert!(
        e.contains("uploading 9 of 18 frames"),
        "the subsampling warning belongs on stderr; got: {e}"
    );
    assert!(
        e.contains("--fps 15"),
        "and it suggests the rate that keeps the duration; got: {e}"
    );
    assert!(
        o.contains("9 frame(s) at 30 fps"),
        "the summary belongs on stdout; got: {o}"
    );
    assert!(
        !o.contains("uploading 9 of 18"),
        "the warning must not also be on stdout: {o}"
    );
}

/// `--help` works without a device and names every shipped command.
#[test]
fn help_lists_the_commands() {
    let out = run(&["--help"]);
    assert!(out.status.success());
    let o = stdout(&out);
    for cmd in [
        "set-time",
        "switch-page",
        "clear-picture",
        "set-picture",
        "set-gif",
    ] {
        assert!(o.contains(cmd), "--help should mention {cmd}; got: {o}");
    }
}
