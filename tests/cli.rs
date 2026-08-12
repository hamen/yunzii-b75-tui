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

/// No subcommand, no terminal: exit rather than draw a UI nobody can see.
///
/// `Command::output()` gives the child pipes, not a terminal, so this is
/// exactly the situation a script creates. Before the TUI existed this was a
/// clap usage error; it must not become a program that hangs.
#[test]
fn without_a_terminal_the_bare_command_exits_instead_of_hanging() {
    let out = run(&[]);
    assert_eq!(
        out.status.code(),
        Some(2),
        "the old usage-error exit code is kept"
    );
    let err = stderr(&out);
    assert!(err.contains("not a terminal"), "got: {err}");
    assert!(err.contains("--help"), "and points somewhere useful: {err}");
}

/// Every subcommand still works with piped stdio, exactly as before.
#[test]
fn subcommands_are_unaffected_by_the_new_interactive_mode() {
    let out = run(&["set-gif", "fixtures/test-anim-2frames.gif", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("dry run"));
}

/// A GIF the device cannot hold is refused, and the message names the way out.
#[test]
fn an_over_long_gif_is_refused_with_the_flag_that_solves_it() {
    // 161 frames would be needed; the fixture is 18, so ask for a limit the
    // parser rejects and check the same guidance appears.
    let out = run(&[
        "set-gif",
        "fixtures/test-anim-18frames.gif",
        "--max-frames",
        "0",
    ]);
    assert!(!out.status.success());
    assert!(stderr(&out).to_lowercase().contains("max-frames"));
}

/// The adjustment flags are bounded at parse time, not clamped silently.
#[test]
fn adjustments_out_of_range_are_a_usage_error() {
    for flag in ["--brightness", "--chroma", "--saturation"] {
        for bad in ["1.5", "-2", "abc"] {
            let out = run(&["set-picture", "fixtures/test-quadrants.png", flag, bad]);
            assert!(!out.status.success(), "{flag} {bad} should be rejected");
            let err = stderr(&out).to_lowercase();
            assert!(
                err.contains("between -1.0 and 1.0") || err.contains("not a number"),
                "{flag} {bad}: got {err}"
            );
        }
        // The bounds themselves are fine.
        for good in ["-1", "0", "1", "0.25"] {
            let out = run(&[
                "set-picture",
                "fixtures/test-quadrants.png",
                flag,
                good,
                "--dry-run",
            ]);
            assert!(out.status.success(), "{flag} {good}: {}", stderr(&out));
        }
    }
}

/// A dry run says which adjustments are on, so the effect is visible without
/// an upload.
#[test]
fn a_dry_run_reports_the_active_adjustments() {
    let out = run(&[
        "set-gif",
        "fixtures/test-anim-2frames.gif",
        "--brightness",
        "-0.5",
        "--grayscale",
        "--dry-run",
    ]);
    assert!(out.status.success(), "{}", stderr(&out));
    let o = stdout(&out);
    assert!(o.contains("adjustments:"), "got: {o}");
    assert!(o.contains("brightness -0.50"), "got: {o}");
    assert!(o.contains("grayscale"), "got: {o}");
    assert!(!o.contains("chroma"), "only what is on: {o}");
}

/// With no adjustment flags, nothing is announced -- the untouched path stays
/// exactly as quiet as it was.
#[test]
fn without_adjustments_nothing_is_announced() {
    let out = run(&["set-picture", "fixtures/test-quadrants.png", "--dry-run"]);
    assert!(out.status.success());
    assert!(!stdout(&out).contains("adjustments:"), "{}", stdout(&out));
}

/// Both upload commands accept the same set.
#[test]
fn both_commands_take_the_same_adjustments() {
    for (cmd, file) in [
        ("set-picture", "fixtures/test-quadrants.png"),
        ("set-gif", "fixtures/test-anim-2frames.gif"),
    ] {
        let out = run(&[
            cmd,
            file,
            "--brightness",
            "0.1",
            "--chroma",
            "-0.1",
            "--saturation",
            "0.2",
            "--grayscale",
            "--sharpen",
            "--blur",
            "--dry-run",
        ]);
        assert!(out.status.success(), "{cmd}: {}", stderr(&out));
        assert!(stdout(&out).contains("adjustments:"), "{cmd}");
    }
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
