mod device;
mod protocol;
mod time;

use clap::{Parser, Subcommand, ValueEnum};
use device::{Device, DeviceError, ReportIdForm};
use image::ImageDecoder; // for `decoder.orientation()`
use std::path::{Path, PathBuf};

/// Anything a subcommand can fail with. `set-picture` can fail before it ever
/// touches the device -- a missing or unreadable image is not a device error
/// and must not be reported as one.
#[derive(Debug)]
enum AppError {
    Device(DeviceError),
    Picture(PictureError),
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AppError::Device(e) => write!(f, "{e}"),
            AppError::Picture(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for AppError {}

impl From<DeviceError> for AppError {
    fn from(e: DeviceError) -> Self {
        AppError::Device(e)
    }
}

impl From<PictureError> for AppError {
    fn from(e: PictureError) -> Self {
        AppError::Picture(e)
    }
}

/// A `set-picture` failure that happened while reading or converting the
/// image, i.e. before any HID device was opened.
#[derive(Debug)]
pub struct PictureError {
    path: PathBuf,
    detail: String,
}

impl std::fmt::Display for PictureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not use {} as a picture: {}\n\
             (supported formats: PNG and JPEG. The keyboard was not contacted.)",
            self.path.display(),
            self.detail
        )
    }
}

impl std::error::Error for PictureError {}

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
/// `Gif` is deliberately NOT a variant here, but for a different reason
/// than in Milestone 2. Back then it was withheld because cmd15 appeared not
/// to switch the panel. Milestone 3 found the real cause: the GIF had only
/// ever been saved in the vendor's mode 0 ("set as boot animation"), which
/// stores frames somewhere that never plays. Saved in mode 1 ("save to the
/// device"), the GIF displays and cmd15 works.
///
/// It stays unexposed because GIF upload is not implemented yet, so the
/// command would switch to a page this tool cannot put anything on. See
/// `protocol::Page` and `fields.json`'s `unresolved`.
#[derive(Debug, Clone, Copy, ValueEnum)]
enum PageArg {
    Home,
    Picture,
}

impl From<PageArg> for protocol::Page {
    fn from(arg: PageArg) -> Self {
        match arg {
            PageArg::Home => protocol::Page::Home,
            PageArg::Picture => protocol::Page::Picture,
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
    /// Switch the TFT screen to the given page (home or picture -- gif is
    /// decoded but deferred, see PROTOCOL.md).
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
}

fn main() {
    let cli = Cli::parse();
    let result: Result<(), AppError> = match cli.command {
        Commands::SetTime { debug_no_prefix } => run_set_time(debug_no_prefix).map_err(Into::into),
        Commands::SwitchPage { page } => run_switch_page(page.into()).map_err(Into::into),
        Commands::ClearPicture => run_clear_picture().map_err(Into::into),
        Commands::SetPicture { path } => run_set_picture(&path),
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

/// Reads an image file and converts it to the panel's 30720-byte RGB565
/// frame. Pure of any device access on purpose: a bad path or a corrupt file
/// must fail as an image problem, and must be testable without hardware.
fn load_and_encode_picture(path: &Path) -> Result<Vec<u8>, PictureError> {
    let fail = |detail: String| PictureError {
        path: path.to_path_buf(),
        detail,
    };

    let reader = image::ImageReader::open(path)
        .map_err(|e| fail(e.to_string()))?
        .with_guessed_format()
        .map_err(|e| fail(e.to_string()))?;

    let mut decoder = reader
        .into_decoder()
        .map_err(|e| fail(format!("could not decode it: {e}")))?;

    // Read the EXIF orientation before taking the pixels out of the decoder:
    // phone JPEGs commonly store the image rotated plus an orientation tag,
    // and ignoring the tag uploads them sideways. A file with no usable tag
    // is not an error -- most PNGs have none.
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let mut img = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| fail(format!("could not decode it: {e}")))?;
    img.apply_orientation(orientation);

    // Nearest-neighbour, to match the vendor's `imageSmoothingEnabled =
    // false`. An interpolating filter would produce different bytes than the
    // vendor's tool does for the same input file.
    let resized = img.resize_exact(
        protocol::PANEL_W,
        protocol::PANEL_H,
        image::imageops::FilterType::Nearest,
    );

    let pixels = protocol::rgb565_encode(resized.to_rgba8().as_raw());
    debug_assert_eq!(pixels.len(), protocol::PICTURE_BYTES);
    Ok(pixels)
}

fn run_set_picture(path: &Path) -> Result<(), AppError> {
    // Decode FIRST, before opening the device: a missing or corrupt file
    // should say so, not fail with "device not found" on a machine with no
    // keyboard plugged in.
    let pixels = load_and_encode_picture(path)?;
    println!(
        "encoded {} to {}x{} RGB565 ({} bytes)",
        path.display(),
        protocol::PANEL_W,
        protocol::PANEL_H,
        pixels.len()
    );

    let dev_path = device::find_device().map_err(AppError::Device)?;
    println!("found device: {}", dev_path.display());

    let dev = Device::open(&dev_path).map_err(AppError::Device)?;
    dev.drain()
        .map_err(|e| AppError::Device(e.with_reconnect_hint(&dev_path)))?;

    let start = protocol::build_picture_upload_start();
    let body = protocol::build_picture_upload_body(&pixels);
    let total = protocol::picture_upload_report_count();
    debug_assert_eq!(total, 1 + body.len());
    println!(
        "sending {total} reports (start, 300 ms pause, declare-size, {} pixel packets, finish)",
        body.len() - 2
    );

    // An interrupted upload leaves a half-written frame on the panel, so say
    // so plainly rather than only reporting the underlying I/O error: 552
    // writes, each waiting for an ACK, is a long enough window to matter.
    let upload_failed = |e: DeviceError| {
        AppError::Device(e.with_reconnect_hint(&dev_path).with_note(
            "the picture may be partially written -- re-run set-picture, or run clear-picture",
        ))
    };

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &[start])
        .map_err(upload_failed)?;

    // The vendor pauses here, between the start report and declare-size --
    // not before the bulk data. See protocol::START_TO_DECLARE_DELAY_MS.
    std::thread::sleep(std::time::Duration::from_millis(
        protocol::START_TO_DECLARE_DELAY_MS,
    ));

    dev.send_sequence(ReportIdForm::LeadingZeroOnWrite, &body)
        .map_err(upload_failed)?;

    println!("sent successfully. The picture should now be on the keyboard's TFT screen.");
    Ok(())
}

#[cfg(test)]
mod cli_tests {
    use super::*;

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

    // Round-3 cross-review (codex Blocker, PR #3): `gif` is deliberately not
    // a valid `switch-page` argument -- see `PageArg`'s doc comment. This
    // locks that deferral as a test, not just a comment that could drift.
    #[test]
    fn rejects_gif_as_a_switch_page_argument() {
        let result = Cli::try_parse_from(["yunzii-b75-tui", "switch-page", "gif"]);
        assert!(result.is_err(), "gif must be rejected, not a valid page");
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

    #[test]
    fn missing_file_fails_as_a_picture_error_not_a_device_error() {
        let err = load_and_encode_picture(Path::new("/nonexistent/nope.png")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("nope.png"), "should name the file: {msg}");
        assert!(
            msg.contains("keyboard was not contacted"),
            "should make clear the device was never touched: {msg}"
        );
    }

    #[test]
    fn zero_byte_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.png");
        std::fs::write(&path, b"").unwrap();
        assert!(load_and_encode_picture(&path).is_err());
    }

    #[test]
    fn corrupt_image_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.png");
        // A valid PNG signature followed by garbage: gets past format
        // sniffing, then fails in the decoder.
        let mut bytes = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        bytes.extend_from_slice(b"not actually a png body at all");
        std::fs::write(&path, &bytes).unwrap();
        assert!(load_and_encode_picture(&path).is_err());
    }

    #[test]
    fn unsupported_format_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"this is plain text, not an image").unwrap();
        assert!(load_and_encode_picture(&path).is_err());
    }

    #[test]
    fn real_png_encodes_to_exactly_one_panel_frame() {
        // The committed test image is already 160x96, so this also confirms
        // the resize path is a no-op at the exact panel size rather than
        // shifting pixels.
        let pixels = load_and_encode_picture(Path::new("fixtures/test-quadrants.png")).unwrap();
        assert_eq!(pixels.len(), protocol::PICTURE_BYTES);

        // Same bytes the vendor's own tool sent for this file: red top-left,
        // green top-right, blue bottom-left.
        let px = |x: usize, y: usize| {
            let o = (y * protocol::PANEL_W as usize + x) * 2;
            [pixels[o], pixels[o + 1]]
        };
        assert_eq!(px(5, 5), [0xf8, 0x00]);
        assert_eq!(px(120, 5), [0x07, 0xe0]);
        assert_eq!(px(5, 80), [0x00, 0x1f]);
    }

    // Cross-review round 1 (codex Should-fix, grok Should-fix 1, PR #4): EXIF
    // orientation was implemented but never proven to run. If `image` +
    // `zune-jpeg` silently returned NoTransforms for every file, the feature
    // and its README promise would be quietly false. This test would fail.
    //
    // fixtures/test-exif-rotated.jpg stores a 96x160 image with Orientation=6
    // ("rotate 90 CW to display"), whose UPRIGHT form is 160x96 with a white
    // square top-left and a black square bottom-right on mid grey.
    // Cross-review round 1 (grok Should-fix 2, PR #4): protocol.rs's
    // `picture_upload_body_matches_the_full_552_report_capture` rebuilds the
    // stream from pixels it read back OUT of the fixture, so it proves the
    // packaging (chunking, offsets, lengths, checksums) and nothing about the
    // encoder. The Node pipeline does check PNG -> reports against hardware,
    // but that left the RUST encoder unverified end to end.
    //
    // This closes it: source PNG -> load_and_encode_picture ->
    // build_picture_upload_body, compared against every byte of the fixture,
    // which check-raw-consistency.js separately pins to the real capture. So
    // the chain from "a file on disk" to "what the vendor's tool put on the
    // wire" is now unbroken on the Rust side too.
    #[test]
    fn encoding_the_source_png_reproduces_the_whole_captured_upload() {
        let data: serde_json::Value =
            serde_json::from_str(include_str!("../fixtures/picture-upload.json")).unwrap();
        let reports = data["reports"].as_array().unwrap();

        let parse = |hex: &str| -> [u8; 64] {
            let v: Vec<u8> = hex
                .split_whitespace()
                .map(|t| u8::from_str_radix(t, 16).unwrap())
                .collect();
            assert_eq!(v.len(), 64);
            v.try_into().unwrap()
        };

        let pixels = load_and_encode_picture(Path::new("fixtures/test-quadrants.png")).unwrap();
        let mut built = vec![protocol::build_picture_upload_start()];
        built.extend(protocol::build_picture_upload_body(&pixels));

        assert_eq!(built.len(), reports.len(), "report count");
        for (i, report) in built.iter().enumerate() {
            let expected = parse(reports[i]["payload_hex"].as_str().unwrap());
            assert_eq!(
                report,
                &expected,
                "report {i} ({}) differs",
                reports[i]["command_name"].as_str().unwrap()
            );
        }
    }

    #[test]
    fn exif_orientation_is_actually_applied_to_jpegs() {
        let pixels = load_and_encode_picture(Path::new("fixtures/test-exif-rotated.jpg")).unwrap();
        assert_eq!(pixels.len(), protocol::PICTURE_BYTES);

        let px = |x: usize, y: usize| {
            let o = (y * protocol::PANEL_W as usize + x) * 2;
            u16::from_be_bytes([pixels[o], pixels[o + 1]])
        };
        // A stored-orientation read would put these marks on the wrong edges,
        // and the un-rotated 96x160 source stretched to 160x96 would smear
        // them across the middle. Both fail these corners.
        let white = px(12, 8);
        let black = px(150, 88);
        assert!(
            white > 0xf000,
            "top-left should be white after rotation, got {white:#06x}"
        );
        assert!(
            black < 0x1000,
            "bottom-right should be black after rotation, got {black:#06x}"
        );
        // And the opposite corners must NOT be those marks.
        assert!(
            px(150, 8) < 0xf000,
            "top-right should not be the white mark"
        );
        assert!(
            px(12, 88) > 0x1000,
            "bottom-left should not be the black mark"
        );
    }

    // Cross-review round 1 (codex Blocker, PR #4): codex read the plan's
    // `out = src*a + 0*(1-a)` line and called partial-alpha handling a bug.
    // The plan sentence was my own guess; the device does something else.
    //
    // The vendor's PICTURE path encodes with:
    //   function te(J){ const ae=J.data; ... const xe=ae[he], ve=ae[he+1], re=ae[he+2]; ... }
    // which reads bytes 0-2 and never touches alpha, from a getImageData()
    // taken off a FRESH (transparent) canvas with no black fill. Drawing
    // source-over onto transparent preserves the un-premultiplied colour, so
    // a semi-transparent pixel contributes its FULL RGB, and only a fully
    // transparent pixel comes back as (0,0,0,0), i.e. black.
    //
    // (The `if (data[i+3] === 0)` black pre-pass codex may be thinking of is
    // real, but it is in the vendor's GIF path, not this one.)
    //
    // This test locks that behaviour so a future "fix" toward premultiplying
    // has to argue with the evidence rather than with a comment.
    #[test]
    fn partial_alpha_keeps_full_colour_and_only_alpha_zero_becomes_black() {
        let red_opaque = protocol::rgb565_encode(&[255, 0, 0, 255]);
        let red_half = protocol::rgb565_encode(&[255, 0, 0, 128]);
        let red_gone = protocol::rgb565_encode(&[255, 0, 0, 0]);

        assert_eq!(red_half, red_opaque, "alpha 128 must keep the full colour");
        assert_eq!(red_gone, vec![0x00, 0x00], "alpha 0 must become black");
        // If this ever premultiplied, alpha 128 would land near 0x7800.
        assert_ne!(red_half, vec![0x78, 0x00]);
    }

    #[test]
    fn a_differently_sized_image_is_still_resized_to_one_full_frame() {
        // Guards the resize step itself: whatever comes in, exactly one
        // panel frame comes out, or build_picture_upload_body would panic.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("wrong-size.png");
        image::RgbImage::from_pixel(37, 211, image::Rgb([255, 0, 0]))
            .save(&path)
            .unwrap();
        let pixels = load_and_encode_picture(&path).unwrap();
        assert_eq!(pixels.len(), protocol::PICTURE_BYTES);
        assert_eq!(&pixels[0..2], &[0xf8, 0x00]);
    }

    #[test]
    fn page_arg_maps_to_protocol_page() {
        assert_eq!(protocol::Page::from(PageArg::Home), protocol::Page::Home);
        assert_eq!(
            protocol::Page::from(PageArg::Picture),
            protocol::Page::Picture
        );
    }

    // Plan (Milestone 2, step 6) explicitly called for CLI dispatch tests
    // asserting home->11, picture->13 as inner cmd BYTES, not just that
    // PageArg maps to the right protocol::Page variant -- this closes that
    // gap by inspecting the actual wire byte the parsed CLI arg produces
    // (round-1 cross-review, cursor SF3, PR #3). `gif`->15 dropped from this
    // table in round 3: gif is no longer a valid CLI argument (see
    // `rejects_gif_as_a_switch_page_argument`), though `protocol.rs`'s own
    // fixture tests still cover cmd15's bytes directly.
    #[test]
    fn cli_page_name_maps_to_correct_inner_cmd_byte() {
        const CMD_BYTE_OFFSET: usize = 9; // payload = report[7..], cmd = payload[2]
        for (page_name, expected_cmd_byte) in [("home", 11u8), ("picture", 13)] {
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
