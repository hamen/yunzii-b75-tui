mod device;
mod exec;
mod plan;
mod protocol;
mod time;

use clap::{Parser, Subcommand, ValueEnum};
use device::{Device, DeviceError, ReportIdForm};
use plan::{MediaError, Note, Stream};
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Prints a plan's notes on the streams it asked for, in order.
///
/// The one place the CLI turns decisions into output. A TUI renders the same
/// `Note`s into a pane instead, which is the whole reason they are data.
fn print_notes(notes: &[Note]) {
    for note in notes {
        match note.stream {
            Stream::Stdout => println!("{}", note.text),
            Stream::Stderr => eprintln!("{}", note.text),
        }
    }
}

/// Anything a subcommand can fail with. `set-picture` and `set-gif` can fail
/// before they ever touch the device -- a missing or unreadable file is not a
/// device error and must not be reported as one.
#[derive(Debug)]
enum AppError {
    Device(DeviceError),
    Media(MediaError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Device(e) => write!(f, "{e}"),
            AppError::Media(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<DeviceError> for AppError {
    fn from(e: DeviceError) -> Self {
        AppError::Device(e)
    }
}

impl From<MediaError> for AppError {
    fn from(e: MediaError) -> Self {
        AppError::Media(e)
    }
}

#[derive(Parser)]
#[command(
    name = "yunzii-b75-tui",
    about = "Native control for the Yunzii B75 Pro Max keyboard's TFT screen"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

/// clap-facing page selector -- kept separate from `protocol::Page` so
/// `protocol.rs` stays CLI-agnostic (pure wire-format code, no clap
/// dependency); `PageArg::into()` maps 1:1 to `protocol::Page`.
///
/// `Gif` ships as of Milestone 4.
///
/// Its bytes (cmd15) have been decoded and correct since Milestone 2, which
/// nonetheless withheld the option because the page appeared not to switch.
/// Milestone 3 found the real cause -- the GIF under test had only ever been
/// saved in the vendor's mode 0, which stores frames somewhere that never
/// plays -- and Milestone 4 removes the remaining reason to withhold it by
/// giving this tool a way to put a GIF on that page in the first place.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum PageArg {
    Home,
    Picture,
    Gif,
}

impl From<PageArg> for protocol::Page {
    fn from(arg: PageArg) -> Self {
        match arg {
            PageArg::Home => protocol::Page::Home,
            PageArg::Picture => protocol::Page::Picture,
            PageArg::Gif => protocol::Page::Gif,
        }
    }
}

#[derive(Subcommand)]
enum Commands {
    /// Set the keyboard's clock and date to the current local time.
    SetTime {
        /// Use the "no leading report-ID byte" write form instead of the
        /// confirmed-correct default (leading 0x00 byte on write). Debug
        /// only -- this form is known not to work; the flag exists for
        /// re-running the discovery experiment if the device's behavior
        /// ever needs re-checking, not for normal use.
        #[arg(long)]
        debug_no_prefix: bool,
    },
    /// Switch the TFT screen to the given page: home, picture, or gif.
    SwitchPage { page: PageArg },
    /// Clear the currently-displayed picture. Whether this also affects a
    /// separately-stored GIF is still untested (see PROTOCOL.md).
    ClearPicture,
    /// Upload a PNG or JPEG to the TFT screen.
    ///
    /// The image is stretched to the panel's fixed 160x96 with
    /// nearest-neighbour sampling -- the same as the vendor's tool, which
    /// draws with image smoothing switched off. Aspect ratio is not
    /// preserved. Fully transparent pixels become black; partial
    /// transparency keeps its full colour rather than blending. EXIF
    /// orientation is applied, so photos straight off a phone are not
    /// uploaded sideways. Uploading also switches the panel to the picture
    /// page, so no separate switch-page is needed.
    SetPicture {
        /// Path to a PNG or JPEG file.
        path: PathBuf,
    },
    /// Upload an animated GIF to the TFT screen.
    ///
    /// Every frame is stretched to the panel's fixed 160x96 the same way
    /// set-picture does. GIF frame position, transparency and disposal are
    /// applied, so optimised GIFs work. The keyboard animates at one rate for
    /// the whole animation and stores at most 160 frames.
    ///
    /// An upload takes roughly a second per frame, because the device pauses
    /// three seconds every sixteenth frame.
    SetGif {
        /// Path to a GIF file.
        path: PathBuf,
        /// Frames per second, 1-60. Without it, the GIF's own rate is used
        /// when its frame delays are uniform AND inside 1-60. Otherwise the
        /// upload falls back to 30 fps and prints why: the delays vary, they
        /// are all zero, or they ask for a rate the keyboard cannot store.
        #[arg(long, value_parser = parse_fps)]
        fps: Option<u8>,
        /// Upload at most this many frames, 1-160, sampled evenly across the
        /// whole animation. Without it, a GIF longer than 160 frames is an
        /// error rather than being silently truncated.
        #[arg(long, value_parser = parse_max_frames)]
        max_frames: Option<NonZeroUsize>,
    },
}

/// Rejects a frame rate the device cannot store, at parse time, so it fails
/// before anything is decoded or sent.
fn parse_fps(s: &str) -> Result<u8, String> {
    let v: u8 = s
        .parse()
        .map_err(|_| format!("`{s}` is not a whole number"))?;
    if !(protocol::GIF_FPS_MIN..=protocol::GIF_FPS_MAX).contains(&v) {
        return Err(format!(
            "must be between {} and {}",
            protocol::GIF_FPS_MIN,
            protocol::GIF_FPS_MAX
        ));
    }
    Ok(v)
}

/// Rejects a frame count the device cannot store, at parse time.
///
/// Without this the value was checked only after the whole GIF had been
/// decoded, so `--max-frames 161` paid for a full decode before failing on
/// something clap could see immediately.
fn parse_max_frames(s: &str) -> Result<NonZeroUsize, String> {
    let v: usize = s
        .parse()
        .map_err(|_| format!("`{s}` is not a whole number"))?;
    let v = NonZeroUsize::new(v).ok_or_else(|| "must be at least 1".to_string())?;
    if v.get() > protocol::GIF_MAX_FRAMES {
        return Err(format!(
            "must be at most {} -- the keyboard cannot store more",
            protocol::GIF_MAX_FRAMES
        ));
    }
    Ok(v)
}

fn main() {
    let cli = Cli::parse();
    let result: Result<(), AppError> = match cli.command {
        Commands::SetTime { debug_no_prefix } => run_set_time(debug_no_prefix).map_err(Into::into),
        Commands::SwitchPage { page } => run_switch_page(page.into()).map_err(Into::into),
        Commands::ClearPicture => run_clear_picture().map_err(Into::into),
        Commands::SetPicture { path } => run_set_picture(&path),
        Commands::SetGif {
            path,
            fps,
            max_frames,
        } => run_set_gif(&path, fps, max_frames),
    };

    if let Err(e) = result {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run_set_time(debug_no_prefix: bool) -> Result<(), DeviceError> {
    let path = device::find_device()?;
    println!("found device: {}", path.display());

    let dev = Device::open(&path)?;
    dev.drain().map_err(|e| e.with_reconnect_hint(&path))?;

    let fields = time::snapshot_local();
    println!(
        "local time snapshot: {:02}:{:02}:{:02}  20{:02}-{:02}-{:02}  weekday={} (Mon=1..Sun=7)",
        fields.hour,
        fields.minute,
        fields.second,
        fields.year2digit,
        fields.month,
        fields.date,
        fields.weekday
    );

    let sequence = protocol::build_set_time_sequence(
        fields.hour,
        fields.minute,
        fields.second,
        fields.year2digit,
        fields.weekday,
        fields.month,
        fields.date,
    );
    println!(
        "built {} reports (2 command groups x 3 reports x 3 repeats)",
        sequence.len()
    );

    // Confirmed against real hardware (see PROTOCOL.md): a native hidraw
    // write() to this unnumbered-report interface needs a leading 0x00
    // "report number" byte prepended (65 bytes total on the wire); reads
    // come back as 64 bytes with no such prefix. `--debug-no-prefix` exists
    // only to re-run the (known-failing) alternative if ever needed.
    let form = if debug_no_prefix {
        ReportIdForm::NoPrefix
    } else {
        ReportIdForm::LeadingZeroOnWrite
    };

    dev.send_sequence(form, &sequence)
        .map_err(|e| e.with_reconnect_hint(&path))?;
    println!(
        "sent successfully using {form:?}. Check the keyboard's TFT screen for the correct time."
    );
    Ok(())
}

fn run_switch_page(page: protocol::Page) -> Result<(), DeviceError> {
    let path = device::find_device()?;
    println!("found device: {}", path.display());

    let dev = Device::open(&path)?;
    dev.drain().map_err(|e| e.with_reconnect_hint(&path))?;

    let sequence = protocol::build_page_switch_sequence(page);
    let label = match page {
        protocol::Page::Home => "home",
        protocol::Page::Picture => "picture",
        protocol::Page::Gif => "gif",
    };
    println!("built {} reports for {label}", sequence.len());

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &sequence)
        .map_err(|e| e.with_reconnect_hint(&path))?;
    println!("sent successfully. Check the keyboard's TFT screen for the {label} page.");
    Ok(())
}

fn run_clear_picture() -> Result<(), DeviceError> {
    let path = device::find_device()?;
    println!("found device: {}", path.display());

    let dev = Device::open(&path)?;
    dev.drain().map_err(|e| e.with_reconnect_hint(&path))?;

    let sequence = protocol::build_clear_picture_sequence();
    println!("built {} reports (16x info+finish repeat)", sequence.len());

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &sequence)
        .map_err(|e| e.with_reconnect_hint(&path))?;
    println!(
        "sent successfully. Check the keyboard's TFT screen -- the picture should be cleared."
    );
    Ok(())
}

fn run_set_gif(
    path: &Path,
    fps: Option<u8>,
    max_frames: Option<NonZeroUsize>,
) -> Result<(), AppError> {
    // Decode, composite and encode before touching the device: this can take a
    // while for a long GIF, and discovering a bad file after 500 writes would
    // leave a half-written animation on the panel for no reason.
    println!("reading {}...", path.display());
    let plan = plan::plan_gif_upload(path, fps, max_frames)?;
    print_notes(&plan.notes);

    // The plan is self-consistent before anything is sent. Same reasoning as
    // the picture path's report-count assertion: a planner that miscounted
    // would otherwise be discovered as a stalled upload on real hardware.
    debug_assert_eq!(
        plan.total_reports,
        protocol::gif_upload_report_count(plan.frames.len())
    );
    debug_assert!(plan.frames.len() <= plan.source_count);
    debug_assert!(plan.est_secs >= plan.frames.len());

    let dev_path = device::find_device().map_err(AppError::Device)?;
    println!("found device: {}", dev_path.display());
    let dev = Device::open(&dev_path).map_err(AppError::Device)?;
    dev.drain()
        .map_err(|e| AppError::Device(e.with_reconnect_hint(&dev_path)))?;

    send_gif_frames(
        &dev,
        &exec::SystemClock,
        &dev_path,
        &plan.frames,
        plan.rate,
        &mut |i, of| println!("  frame {}/{of}", i + 1),
    )?;

    println!(
        "sent successfully. The animation should now be playing on the keyboard's TFT screen."
    );
    Ok(())
}

/// The GIF upload choreography: reports **and** the pauses between them.
///
/// Behind `Transport` and `Clock` so a test can watch where the pauses fall --
/// see `exec.rs` for why that is not already covered. The body is the code that
/// used to sit inline in `run_set_gif`, in the same order, so a reviewer can
/// diff it line for line.
fn send_gif_frames(
    dev: &dyn exec::Transport,
    clock: &dyn exec::Clock,
    dev_path: &Path,
    frames: &[Vec<u8>],
    rate: u8,
    on_frame: &mut dyn FnMut(usize, usize),
) -> Result<(), AppError> {
    let frame_count = frames.len();

    // Deliberately NOT set-picture's message: nothing shows that clear-picture
    // clears a half-written GIF, and there is no clear-gif command yet.
    let upload_failed = |e: DeviceError| {
        AppError::Device(e.with_reconnect_hint(dev_path).with_note(
            "the animation on the keyboard may be incomplete -- re-run set-gif to overwrite it \
             (clear-picture is not known to clear a GIF)",
        ))
    };
    let send = |reports: &[[u8; 64]]| {
        dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, reports)
            .map_err(upload_failed)
    };
    let sleep = |ms: u64| clock.sleep(std::time::Duration::from_millis(ms));

    let mode = protocol::GIF_MODE_SAVE_TO_DEVICE;
    send(&protocol::build_gif_session_open(mode))?;
    sleep(protocol::GIF_SESSION_OPEN_DELAY_MS);

    for (i, pixels) in frames.iter().enumerate() {
        send(&[protocol::build_gif_frame_header(mode, i as u8)])?;
        sleep(if i % protocol::GIF_SLOW_DELAY_EVERY == 0 {
            protocol::GIF_FRAME_HEADER_SLOW_DELAY_MS
        } else {
            protocol::GIF_FRAME_HEADER_DELAY_MS
        });
        send(&[protocol::build_gif_declare_size()])?;
        for block in protocol::build_gif_frame_blocks(pixels) {
            send(&block)?;
            sleep(protocol::GIF_BLOCK_DELAY_MS);
        }
        on_frame(i, frame_count);
    }

    // Sent one at a time: the vendor sleeps 30 ms BETWEEN the two close
    // reports as well as after the second, and batching them would drop the
    // first of those gaps.
    for report in protocol::build_gif_session_close(mode, frame_count as u8, rate) {
        send(&[report])?;
        sleep(protocol::GIF_SESSION_CLOSE_DELAY_MS);
    }
    sleep(protocol::GIF_PRE_FINISH_DELAY_MS);
    send(&[protocol::build_finish()])?;
    Ok(())
}

fn run_set_picture(path: &Path) -> Result<(), AppError> {
    // Decode FIRST, before opening the device: a missing or corrupt file
    // should say so, not fail with "device not found" on a machine with no
    // keyboard plugged in.
    let plan = plan::plan_picture_upload(path)?;
    print_notes(&plan.notes);

    let dev_path = device::find_device().map_err(AppError::Device)?;
    println!("found device: {}", dev_path.display());

    let dev = Device::open(&dev_path).map_err(AppError::Device)?;
    dev.drain()
        .map_err(|e| AppError::Device(e.with_reconnect_hint(&dev_path)))?;

    let start = protocol::build_picture_upload_start();
    let body = protocol::build_picture_upload_body(&plan.pixels);
    debug_assert_eq!(plan.total_reports, 1 + body.len());
    println!(
        "sending {} reports (start, {} ms pause, declare-size, {} pixel packets, finish)",
        plan.total_reports,
        protocol::START_TO_DECLARE_DELAY_MS,
        body.len() - 2
    );

    send_picture_reports(&dev, &exec::SystemClock, &dev_path, &start, &body)?;

    println!("sent successfully. The picture should now be on the keyboard's TFT screen.");
    Ok(())
}

/// The picture upload choreography: two sends with a mandatory 300 ms between.
///
/// Same reasoning as `send_gif_frames` -- behind `Transport` and `Clock` so the
/// pause is observable. Body unchanged from what was inline in
/// `run_set_picture`.
fn send_picture_reports(
    dev: &dyn exec::Transport,
    clock: &dyn exec::Clock,
    dev_path: &Path,
    start: &[u8; 64],
    body: &[[u8; 64]],
) -> Result<(), AppError> {
    // An interrupted upload leaves a half-written frame on the panel, so say
    // so plainly rather than only reporting the underlying I/O error: 552
    // writes, each waiting for an ACK, is a long enough window to matter.
    let upload_failed = |e: DeviceError| {
        AppError::Device(e.with_reconnect_hint(dev_path).with_note(
            "the picture may be partially written -- re-run set-picture, or run clear-picture",
        ))
    };

    dev.send_sequence(
        ReportIdForm::LeadingZeroOnWrite,
        std::slice::from_ref(start),
    )
    .map_err(upload_failed)?;

    // The vendor pauses here, between the start report and declare-size --
    // not before the bulk data. See protocol::START_TO_DECLARE_DELAY_MS.
    clock.sleep(std::time::Duration::from_millis(
        protocol::START_TO_DECLARE_DELAY_MS,
    ));

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, body)
        .map_err(upload_failed)?;
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use crate::plan::*;

    #[test]
    fn parses_set_time() {
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "set-time"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::SetTime {
                debug_no_prefix: false
            }
        ));
    }

    #[test]
    fn parses_switch_page_home() {
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", "home"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::SwitchPage {
                page: PageArg::Home
            }
        ));
    }

    #[test]
    fn parses_switch_page_picture() {
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", "picture"]).unwrap();
        assert!(matches!(
            cli.command,
            Commands::SwitchPage {
                page: PageArg::Picture
            }
        ));
    }

    #[test]
    fn rejects_invalid_page_name() {
        let result = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", "nonsense"]);
        assert!(result.is_err());
    }

    #[test]
    fn parses_clear_picture() {
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "clear-picture"]).unwrap();
        assert!(matches!(cli.command, Commands::ClearPicture));
    }

    // Round-2 cross-review (cursor SF2, PR #3): the switch-page cmd-byte
    // test below covers home/picture/gif, but clear-picture -> cmd14 had no
    // symmetric check. Closes that gap the same way.
    #[test]
    fn clear_picture_dispatch_produces_cmd14() {
        const CMD_BYTE_OFFSET: usize = 9;
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "clear-picture"]).unwrap();
        assert!(matches!(cli.command, Commands::ClearPicture));
        let sequence = protocol::build_clear_picture_sequence();
        assert_eq!(
            sequence[0][CMD_BYTE_OFFSET], 14,
            "expected inner cmd byte 14"
        );
    }

    // --- Milestone 3: set-picture ---

    #[test]
    fn parses_set_picture_with_a_path() {
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "set-picture", "logo.png"]).unwrap();
        let Commands::SetPicture { path } = cli.command else {
            panic!("expected SetPicture");
        };
        assert_eq!(path, PathBuf::from("logo.png"));
    }

    #[test]
    fn set_picture_requires_a_path() {
        assert!(Cli::try_parse_from(["yunzii-b75-tui", "set-picture"]).is_err());
    }

    // The image is decoded before the device is opened, so every one of
    // these failure paths is reachable on a machine with no keyboard
    // attached -- which is the point: otherwise they would all fail with
    // "device not found" first and none of them would be tested.

    // --- Milestone 5: the upload choreography ---
    //
    // These are the only tests in the repo that can see a pause. The capture
    // fixtures pin every byte and none of the timing, so a refactor that
    // dropped the 300 ms before a picture body, or moved the 3-second pause to
    // the wrong frame, would pass everything else in `bin/ci`.

    use crate::exec::{Recorder, Step};

    /// The picture upload, end to end: one report, the 300 ms the vendor
    /// requires, then the 551-report body. Nothing else, in that order.
    #[test]
    fn picture_upload_choreography() {
        let plan = plan::plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        let start = protocol::build_picture_upload_start();
        let body = protocol::build_picture_upload_body(&plan.pixels);

        let rec = Recorder::new();
        send_picture_reports(&rec, &rec, Path::new("/dev/null"), &start, &body).unwrap();

        assert_eq!(
            rec.steps(),
            vec![
                Step::Reports(1),                                 // start
                Step::Slept(protocol::START_TO_DECLARE_DELAY_MS), // 300 ms
                Step::Reports(body.len()),                        // declare + pixels + finish
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
        let gif =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), Some(10), None)
                .unwrap();
        assert_eq!(gif.frames.len(), 2);

        let rec = Recorder::new();
        send_gif_frames(
            &rec,
            &rec,
            Path::new("/dev/null"),
            &gif.frames,
            10,
            &mut |_, _| {},
        )
        .unwrap();

        let blocks = protocol::GIF_BLOCKS_PER_FRAME;
        let per_block = protocol::GIF_PACKETS_PER_BLOCK;

        let mut want = vec![
            // Both session-open reports go out together, then ONE pause.
            // There is no gap between report 18 and report 19.
            Step::Reports(2),
            Step::Slept(protocol::GIF_SESSION_OPEN_DELAY_MS),
        ];
        for i in 0..2 {
            want.push(Step::Reports(1)); // frame header
            want.push(Step::Slept(if i % protocol::GIF_SLOW_DELAY_EVERY == 0 {
                protocol::GIF_FRAME_HEADER_SLOW_DELAY_MS
            } else {
                protocol::GIF_FRAME_HEADER_DELAY_MS
            }));
            // Declare-size and the first block run together: there is no
            // pause between them, so the trace shows them as one run of
            // 1 + 19 reports. Worth seeing rather than hiding -- it is the
            // only place in the upload where two different kinds of report
            // are sent back to back.
            want.push(Step::Reports(1 + per_block));
            want.push(Step::Slept(protocol::GIF_BLOCK_DELAY_MS));
            for _ in 1..blocks {
                want.push(Step::Reports(per_block));
                want.push(Step::Slept(protocol::GIF_BLOCK_DELAY_MS));
            }
        }
        // Close reports one at a time, so the gap BETWEEN them survives.
        want.push(Step::Reports(1));
        want.push(Step::Slept(protocol::GIF_SESSION_CLOSE_DELAY_MS));
        want.push(Step::Reports(1));
        want.push(Step::Slept(protocol::GIF_SESSION_CLOSE_DELAY_MS));
        want.push(Step::Slept(protocol::GIF_PRE_FINISH_DELAY_MS));
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
        let gif =
            plan::plan_gif_upload(Path::new("fixtures/test-anim-18frames.gif"), Some(10), None)
                .unwrap();
        assert_eq!(gif.frames.len(), 18);

        let rec = Recorder::new();
        send_gif_frames(
            &rec,
            &rec,
            Path::new("/dev/null"),
            &gif.frames,
            10,
            &mut |_, _| {},
        )
        .unwrap();

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
            .filter(|(_, ms)| **ms == protocol::GIF_FRAME_HEADER_SLOW_DELAY_MS)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(
            slow,
            vec![0, 16],
            "the 3 s pause belongs to frames 0 and 16 only"
        );
    }

    // --- Milestone 4: set-gif ---

    /// The device stores at most 160 frames, so clap should refuse a larger
    /// value rather than decoding the whole GIF and failing afterwards.
    #[test]
    fn max_frames_is_bounded_at_parse_time() {
        assert!(parse_max_frames("0").is_err(), "0 frames is meaningless");
        assert!(parse_max_frames("161").is_err(), "above the device limit");
        assert!(parse_max_frames("nope").is_err(), "not a number");
        assert_eq!(
            parse_max_frames("160").unwrap().get(),
            protocol::GIF_MAX_FRAMES,
            "the limit itself is allowed"
        );
        assert_eq!(parse_max_frames("1").unwrap().get(), 1);

        // The bound must track the protocol constant, not a copy of it.
        let over = (protocol::GIF_MAX_FRAMES + 1).to_string();
        let msg = parse_max_frames(&over).unwrap_err();
        assert!(
            msg.contains(&protocol::GIF_MAX_FRAMES.to_string()),
            "the error should say the limit; got: {msg}"
        );
    }

    /// The bound must be enforced by clap itself, not only by the parser
    /// function -- a value parser that is never wired up rejects nothing.
    #[test]
    fn clap_rejects_out_of_range_max_frames_end_to_end() {
        let ok = Cli::try_parse_from(["yunzii-b75-tui", "set-gif", "a.gif", "--max-frames", "160"]);
        assert!(ok.is_ok(), "160 is the device limit and must be accepted");

        for bad in ["161", "0", "99999"] {
            let parsed =
                Cli::try_parse_from(["yunzii-b75-tui", "set-gif", "a.gif", "--max-frames", bad]);
            let msg = match parsed {
                Ok(_) => panic!("clap must reject --max-frames {bad}"),
                Err(e) => e.to_string(),
            };
            assert!(
                msg.contains("max-frames"),
                "the error should name the option; got: {msg}"
            );
        }

        // The same for --fps, so both parsers are proven to be wired in.
        assert!(
            Cli::try_parse_from(["yunzii-b75-tui", "set-gif", "a.gif", "--fps", "61"]).is_err(),
            "61 fps is above the device limit"
        );
        assert!(
            Cli::try_parse_from(["yunzii-b75-tui", "set-gif", "a.gif", "--fps", "60"]).is_ok(),
            "60 fps is the limit itself"
        );
    }

    #[test]
    fn max_frames_above_the_device_limit_is_rejected() {
        let err = select_frame_indices(500, Some(NonZeroUsize::new(161).unwrap())).unwrap_err();
        assert!(err.contains("161") && err.contains("160"), "{err}");
    }

    #[test]
    fn fps_outside_the_devices_range_is_rejected_at_parse_time() {
        assert!(parse_fps("0").is_err());
        assert!(parse_fps("61").is_err());
        assert!(parse_fps("not-a-number").is_err());
        assert_eq!(parse_fps("1").unwrap(), 1);
        assert_eq!(parse_fps("60").unwrap(), 60);
        assert_eq!(parse_fps("30").unwrap(), 30);
    }

    #[test]
    fn parses_set_gif_with_options() {
        let cli = Cli::try_parse_from([
            "yunzii-b75-tui",
            "set-gif",
            "a.gif",
            "--fps",
            "12",
            "--max-frames",
            "40",
        ])
        .unwrap();
        let Commands::SetGif {
            path,
            fps,
            max_frames,
        } = cli.command
        else {
            panic!("expected SetGif");
        };
        assert_eq!(path, PathBuf::from("a.gif"));
        assert_eq!(fps, Some(12));
        assert_eq!(max_frames.map(|m| m.get()), Some(40));

        // --max-frames 0 is unrepresentable rather than a runtime check.
        assert!(
            Cli::try_parse_from(["yunzii-b75-tui", "set-gif", "a.gif", "--max-frames", "0"])
                .is_err()
        );
        assert!(Cli::try_parse_from(["yunzii-b75-tui", "set-gif"]).is_err());
    }

    // Milestone 4 ships `switch-page gif`; Milestone 3's test asserted the
    // opposite, so it is replaced rather than deleted.
    #[test]
    fn switch_page_gif_is_accepted_and_maps_to_cmd15() {
        const CMD_BYTE_OFFSET: usize = 9;
        let cli = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", "gif"]).unwrap();
        let Commands::SwitchPage { page } = cli.command else {
            panic!("expected SwitchPage");
        };
        assert!(matches!(page, PageArg::Gif));
        let sequence = protocol::build_page_switch_sequence(page.into());
        assert_eq!(sequence[0][CMD_BYTE_OFFSET], 15);
    }

    #[test]
    fn page_arg_maps_to_protocol_page() {
        assert_eq!(protocol::Page::from(PageArg::Home), protocol::Page::Home);
        assert_eq!(
            protocol::Page::from(PageArg::Picture),
            protocol::Page::Picture
        );
    }

    // CLI dispatch asserted as inner cmd BYTES, not just that PageArg maps to
    // the right protocol::Page variant (Milestone 2 plan step 6; PR #3 round-1
    // cross-review, cursor SF3). `gif`->15 is back in the table as of
    // Milestone 4, which ships that page.
    #[test]
    fn cli_page_name_maps_to_correct_inner_cmd_byte() {
        const CMD_BYTE_OFFSET: usize = 9; // payload = report[7..], cmd = payload[2]
        for (page_name, expected_cmd_byte) in [("home", 11u8), ("picture", 13), ("gif", 15)] {
            let cli = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", page_name]).unwrap();
            let Commands::SwitchPage { page } = cli.command else {
                panic!("expected SwitchPage");
            };
            let sequence = protocol::build_page_switch_sequence(page.into());
            assert_eq!(
                sequence[0][CMD_BYTE_OFFSET], expected_cmd_byte,
                "{page_name}: expected inner cmd byte {expected_cmd_byte}"
            );
        }
    }
}
