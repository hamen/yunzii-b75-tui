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
    write_notes(&mut std::io::stdout(), &mut std::io::stderr(), notes)
        .expect("writing to the terminal");
}

/// The mapping itself, against two writers rather than the process's streams.
///
/// Split out only so it can be tested. Review asked twice for proof that the
/// notes reach the right stream, and "read the four-line function" is not
/// proof -- the same reasoning that put the note text in the planner instead
/// of inline in a device call.
fn write_notes(
    out: &mut dyn std::io::Write,
    err: &mut dyn std::io::Write,
    notes: &[Note],
) -> std::io::Result<()> {
    for note in notes {
        match note.stream {
            Stream::Stdout => writeln!(out, "{}", note.text)?,
            Stream::Stderr => writeln!(err, "{}", note.text)?,
        }
    }
    Ok(())
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

    dev.send_sequence(form, &sequence, &mut |m| eprintln!("{m}"))
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

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &sequence, &mut |m| {
        eprintln!("{m}")
    })
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

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &sequence, &mut |m| {
        eprintln!("{m}")
    })
    .map_err(|e| e.with_reconnect_hint(&path))?;
    println!(
        "sent successfully. Check the keyboard's TFT screen -- the picture should be cleared."
    );
    Ok(())
}

/// Runs an upload for the CLI: no cancellation source, notes to stderr.
///
/// The CLI has no key to press and PR A adds no signal handler, so the flag is
/// created here and never set. The mechanism exists for the TUI, which is the
/// first thing that can actually trip it.
fn run_upload(
    dev: &dyn exec::Transport,
    dev_path: &Path,
    body: impl FnOnce(&mut exec::ExecCtx) -> Result<(), DeviceError>,
    on_event: &mut dyn FnMut(exec::ExecEvent),
) -> Result<(), DeviceError> {
    let never = std::sync::atomic::AtomicBool::new(false);
    let clock = exec::SystemClock;
    let mut emit = |ev: exec::ExecEvent| {
        if let exec::ExecEvent::Note(ref m) = ev {
            eprintln!("{m}");
        }
        on_event(ev);
    };
    let mut cx = exec::ExecCtx {
        dev,
        cancel: &never,
        clock: &clock,
        emit: &mut emit,
    };
    body(&mut cx).map_err(|e| e.with_reconnect_hint(dev_path))
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

    // Deliberately NOT set-picture's message: nothing shows that clear-picture
    // clears a half-written GIF, and there is no clear-gif command yet.
    run_upload(&dev, &dev_path, |cx| exec::execute_gif(&plan, cx), &mut {
        let mut last_frame = usize::MAX;
        move |ev| {
            if let exec::ExecEvent::Progress {
                phase: exec::Phase::Frame { index, of },
                ..
            } = ev
                && index != last_frame
            {
                last_frame = index;
                println!("  frame {}/{of}", index + 1);
            }
        }
    })
    .map_err(|e| {
        AppError::Device(e.with_note(
            "the animation on the keyboard may be incomplete -- re-run set-gif to overwrite it \
             (clear-picture is not known to clear a GIF)",
        ))
    })?;

    println!(
        "sent successfully. The animation should now be playing on the keyboard's TFT screen."
    );
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

    // `- 2` excludes declare-size and finish, which are not pixel packets.
    let packets = plan.total_reports - 1 - 2;
    println!(
        "sending {} reports (start, {} ms pause, declare-size, {packets} pixel packets, finish)",
        plan.total_reports,
        protocol::START_TO_DECLARE_DELAY_MS,
    );

    // An interrupted upload leaves a half-written frame on the panel, so say
    // so plainly rather than only reporting the underlying I/O error: 552
    // writes, each waiting for an ACK, is a long enough window to matter.
    run_upload(
        &dev,
        &dev_path,
        |cx| exec::execute_picture(&plan, cx),
        &mut |_| {},
    )
    .map_err(|e| {
        AppError::Device(e.with_note(
            "the picture may be partially written -- re-run set-picture, or run clear-picture",
        ))
    })?;

    println!("sent successfully. The picture should now be on the keyboard's TFT screen.");
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use crate::plan::*;

    // --- Milestone 5: the CLI's own note plumbing ---

    /// The planner decides the stream; this proves the CLI honours it.
    ///
    /// Planner tests assert `Note` values. They cannot catch a `print_notes`
    /// that sent everything to stdout, which is exactly the wiring the
    /// hardware diff caught a reordering in.
    #[test]
    fn notes_reach_the_stream_the_planner_asked_for() {
        let notes = vec![
            Note {
                stream: Stream::Stderr,
                text: "a warning".into(),
            },
            Note {
                stream: Stream::Stdout,
                text: "a summary".into(),
            },
            Note {
                stream: Stream::Stderr,
                text: "another warning".into(),
            },
        ];

        let mut out = Vec::new();
        let mut err = Vec::new();
        write_notes(&mut out, &mut err, &notes).unwrap();

        assert_eq!(String::from_utf8(out).unwrap(), "a summary\n");
        assert_eq!(
            String::from_utf8(err).unwrap(),
            "a warning\nanother warning\n",
            "stderr notes keep their order relative to each other"
        );
    }

    /// End to end for a real file: the fallback warning goes to stderr, the
    /// summary to stdout, and nothing is lost between planner and writer.
    #[test]
    fn a_real_plans_notes_split_across_the_two_streams() {
        let plan = plan::plan_gif_upload(Path::new("fixtures/test-anim-too-fast.gif"), None, None)
            .unwrap();

        let mut out = Vec::new();
        let mut err = Vec::new();
        write_notes(&mut out, &mut err, &plan.notes).unwrap();
        let out = String::from_utf8(out).unwrap();
        let err = String::from_utf8(err).unwrap();

        assert!(err.contains("100 fps"), "warning on stderr; got {err:?}");
        assert!(
            out.contains("2 frame(s) at 30 fps"),
            "summary on stdout; got {out:?}"
        );
        assert!(
            !out.contains("100 fps"),
            "the warning must not be on stdout"
        );
        assert!(
            !err.contains("frame(s) at"),
            "the summary must not be on stderr"
        );
    }

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
