//! Deciding what to upload, with nothing attached to a device or a terminal.
//!
//! Every command used to be one function that read a file, chose a frame rate,
//! wrote its explanation straight to stderr, opened the keyboard and sent
//! bytes. That shape works exactly once, for one caller. A TUI can reuse none
//! of it: the output path is `println!` and the device call blocks the thread
//! that is supposed to be drawing.
//!
//! So the deciding half lives here and returns its explanations as **data**.
//! The CLI prints them; the TUI will show them in a pane. Neither can drift
//! from the other, because there is one place that decides.
//!
//! It also makes the explanations testable. Two rounds of review on Milestone 4
//! asked for a test proving the frame-rate warnings actually reach the user,
//! and the honest answer at the time was that they could only be checked by
//! running the binary against real hardware -- the message text was built
//! inline inside a function that opened a device. Returned as data, asserting
//! them is an ordinary unit test.

use crate::protocol;
use image::ImageDecoder; // for `decoder.orientation()` / `set_limits()`
use std::num::NonZeroUsize;
use std::path::{Path, PathBuf};

/// Which command was reading the file, so the message can name the right one
/// and list the formats that command actually accepts.
///
/// This exists because one shared error type once told `set-gif` users that
/// the supported formats were "PNG and JPEG" -- correct for the other command,
/// useless advice here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKind {
    Picture,
    Gif,
}

impl MediaKind {
    /// How the file was being used, for "could not use X as a ...".
    fn subject(self) -> &'static str {
        match self {
            MediaKind::Picture => "a picture",
            MediaKind::Gif => "an animation",
        }
    }

    fn supported(self) -> &'static str {
        match self {
            MediaKind::Picture => "PNG and JPEG",
            MediaKind::Gif => "GIF",
        }
    }
}

/// A `set-picture` or `set-gif` failure that happened while reading or
/// converting the file, i.e. before any HID device was opened.
#[derive(Debug)]
pub struct MediaError {
    kind: MediaKind,
    path: PathBuf,
    detail: String,
}

impl std::fmt::Display for MediaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "could not use {} as {}: {}\n\
             (supported formats: {}. The keyboard was not contacted.)",
            self.path.display(),
            self.kind.subject(),
            self.detail,
            self.kind.supported()
        )
    }
}

impl std::error::Error for MediaError {}
/// Reads an image file and converts it to the panel's 30720-byte RGB565
/// frame. Pure of any device access on purpose: a bad path or a corrupt file
/// must fail as an image problem, and must be testable without hardware.
pub fn load_and_encode_picture(path: &Path) -> Result<Vec<u8>, MediaError> {
    let fail = |detail: String| MediaError {
        kind: MediaKind::Picture,
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

/// One GIF's frames, already converted to panel-ready RGB565, plus what the
/// caller needs to explain what it did.
#[derive(Debug)]
pub struct GifFrames {
    frames: Vec<Vec<u8>>,
    /// Frames in the source file, before any subsampling.
    source_count: usize,
    /// What the source file's own delays imply about the frame rate, and --
    /// when they cannot be honoured -- the number needed to say why.
    rate: SourceRate,
}

/// The frame rate a GIF asks for, and whether the device can give it.
///
/// This is one enum rather than an `Option<u8>` plus a "delays vary" flag,
/// because that pair had a fourth state nobody handled: uniform delays that
/// imply a rate outside 1-60 looked exactly like the in-range case having
/// simply not been computed, so the upload silently dropped to 30 fps with no
/// warning at all. Every arm below now carries the number its message needs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SourceRate {
    /// Uniform delays that the device can store. Used as-is.
    Usable(u8),
    /// Uniform delays, but they ask for a rate outside 1-60. Carries the rate
    /// the file wanted, so the warning can name it.
    OutOfRange(f64),
    /// Delays differ between frames. The device animates at a single rate, so
    /// this cannot be reproduced exactly. Carries the mean delay in ms.
    Variable { mean_delay_ms: f64 },
    /// Every frame delay is zero -- "as fast as the viewer can manage", which
    /// is common and is not a rate at all.
    ///
    /// Its own arm because folding it into `Variable` made the warning say the
    /// delays "differ" when every one of them is identical. The fallback rate
    /// is the same; the sentence was simply false.
    Unspecified,
}

impl SourceRate {
    /// The rate to upload at when the user gave no `--fps`.
    fn or_default(self) -> u8 {
        match self {
            SourceRate::Usable(f) => f,
            _ => protocol::GIF_FPS_DEFAULT,
        }
    }

    /// Why the file's own rate was not used, if it was not. `None` means there
    /// is nothing to warn about.
    fn fallback_reason(self, chosen: u8) -> Option<String> {
        match self {
            SourceRate::Usable(_) => None,
            SourceRate::OutOfRange(wanted) => Some(format!(
                // Two decimals, not zero: a sub-1 fps rate printed as "{:.0}"
                // reads as "1 fps", which is the very confusion this arm exists
                // to prevent.
                "note: this GIF's delays ask for about {} fps, outside the {}-{} fps \
                 the keyboard can store. Using {chosen} fps.",
                if wanted < 1.0 {
                    format!("{wanted:.2}")
                } else {
                    format!("{wanted:.0}")
                },
                protocol::GIF_FPS_MIN,
                protocol::GIF_FPS_MAX
            )),
            SourceRate::Unspecified => Some(format!(
                "note: this GIF sets no frame delay, so it asks to play as fast as possible. \
                 The keyboard animates at a fixed rate. Using {chosen} fps."
            )),
            SourceRate::Variable { mean_delay_ms } => Some(format!(
                "note: this GIF's frames have different delays (averaging {mean_delay_ms:.0} ms, \
                 about {:.0} fps), but the keyboard animates at a single rate. Using {chosen} fps.",
                if mean_delay_ms > 0.0 {
                    1000.0 / mean_delay_ms
                } else {
                    0.0
                }
            )),
        }
    }
}

/// Decodes a GIF into panel-ready frames.
///
/// Frame construction is delegated to `image`'s `into_frames()`, which applies
/// GIF frame position, transparency and **disposal** and yields full-canvas
/// RGBA frames. That delegation is the whole point: real GIFs are optimised, so
/// most frames are a small sub-rectangle that only means anything once composed
/// onto what came before. Encoding raw sub-frames would upload garbage.
///
/// For the same reason, **every** frame is walked even when subsampling: a
/// skipped frame still mutates the canvas that later frames build on. Only the
/// selected frames are encoded and kept.
pub fn load_gif_frames(
    path: &Path,
    max_frames: Option<NonZeroUsize>,
) -> Result<GifFrames, MediaError> {
    let fail = |detail: String| MediaError {
        kind: MediaKind::Gif,
        path: path.to_path_buf(),
        detail,
    };

    // Pass 1: count the frames and reduce their delays as we go.
    //
    // Deliberately NOT collected into a Vec. Only four numbers are ever needed
    // -- the count, the first delay, whether they are all equal, and the total
    // -- and a per-frame Vec would grow with the source frame count, which no
    // limit here bounds. The 64 MiB decoder cap covers a single frame buffer,
    // not this. Four accumulators make "a huge GIF costs time, not memory"
    // true rather than nearly true.
    let mut source_count: usize = 0;
    let mut first_delay: u32 = 0;
    let mut all_equal = true;
    let mut total_delay: u64 = 0;
    for frame in gif_frames_iter(path)? {
        let frame = frame.map_err(|e| fail(format!("could not decode a frame: {e}")))?;
        let (num, den) = frame.delay().numer_denom_ms();
        let ms = if den == 0 { 0 } else { num / den };
        if source_count == 0 {
            first_delay = ms;
        } else if ms != first_delay {
            all_equal = false;
        }
        total_delay += ms as u64;
        source_count += 1;
    }

    if source_count == 0 {
        return Err(fail("it contains no frames".to_string()));
    }

    let keep = select_frame_indices(source_count, max_frames).map_err(fail)?;

    // Pass 2: walk every frame again, encode only the selected ones.
    let mut frames = Vec::with_capacity(keep.len());
    for (i, frame) in gif_frames_iter(path)?.enumerate() {
        let frame = frame.map_err(|e| fail(format!("could not decode a frame: {e}")))?;
        if !keep.contains(&i) {
            continue;
        }
        let mut canvas = image::DynamicImage::ImageRgba8(frame.into_buffer());
        flatten_onto_black(&mut canvas);
        let resized = canvas.resize_exact(
            protocol::PANEL_W,
            protocol::PANEL_H,
            image::imageops::FilterType::Nearest,
        );
        frames.push(protocol::rgb565_encode(resized.to_rgba8().as_raw()));
    }

    // A zero delay is "as fast as possible", which is not a rate the file is
    // actually asking for, so it is treated as variable rather than as 0 fps.
    let rate = if all_equal && first_delay == 0 {
        SourceRate::Unspecified
    } else if all_equal {
        // Range-check the exact rate, then round. Rounding first was a bug: a
        // uniform 1500 ms delay is 0.67 fps, which the device cannot store,
        // but it rounded to 1 and passed the check as if the file had asked
        // for 1 fps -- a 50% speed error, reported as an exact match.
        //
        // Rounding after the check is safe because GIF delays are whole
        // centiseconds, so the only reachable rates above 60 come from a 1 cs
        // delay (100 fps). Nothing lands just outside a bound and rounds back
        // inside it.
        let fps = 1000.0 / first_delay as f64;
        if fps >= protocol::GIF_FPS_MIN as f64 && fps <= protocol::GIF_FPS_MAX as f64 {
            SourceRate::Usable(fps.round() as u8)
        } else {
            SourceRate::OutOfRange(fps)
        }
    } else {
        SourceRate::Variable {
            mean_delay_ms: total_delay as f64 / source_count as f64,
        }
    };

    Ok(GifFrames {
        frames,
        source_count,
        rate,
    })
}

pub fn gif_frames_iter(path: &Path) -> Result<image::Frames<'static>, MediaError> {
    let fail = |detail: String| MediaError {
        kind: MediaKind::Gif,
        path: path.to_path_buf(),
        detail,
    };
    let file = std::io::BufReader::new(std::fs::File::open(path).map_err(|e| fail(e.to_string()))?);
    let mut decoder = image::codecs::gif::GifDecoder::new(file)
        .map_err(|e| fail(format!("it is not a readable GIF: {e}")))?;
    // Cap decode allocation so an absurd or hostile logical screen fails with a
    // message instead of eating memory. `image`'s own default ceiling is far
    // higher than anything a 160x96 panel could need.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(64 * 1024 * 1024);
    image::ImageDecoder::set_limits(&mut decoder, limits).map_err(|e| {
        fail(format!(
            "its dimensions exceed this tool's decode limit: {e}"
        ))
    })?;
    Ok(image::AnimationDecoder::into_frames(decoder))
}

/// Composites onto opaque black.
///
/// The GIF path is not the picture path here. The vendor's GIF pipeline fills
/// its canvas black (`fillStyle = "black"; fillRect(...)`) before drawing, so
/// partial alpha blends toward black; the picture pipeline draws onto an
/// unfilled canvas and discards alpha, keeping full colour. GIF transparency is
/// a single transparent *index*, so alpha out of `into_frames()` is 0 or 255
/// and this blend is a no-op for well-formed files -- but doing it properly is
/// what makes that a fact about the input rather than a bug we did not hit.
pub fn flatten_onto_black(img: &mut image::DynamicImage) {
    use image::GenericImageView;
    let (w, h) = img.dimensions();
    let mut out = image::RgbaImage::new(w, h);
    let src = img.to_rgba8();
    for (dst, px) in out.pixels_mut().zip(src.pixels()) {
        let a = px.0[3] as u32;
        *dst = image::Rgba([
            ((px.0[0] as u32 * a) / 255) as u8,
            ((px.0[1] as u32 * a) / 255) as u8,
            ((px.0[2] as u32 * a) / 255) as u8,
            255,
        ]);
    }
    *img = image::DynamicImage::ImageRgba8(out);
}

/// Which source frames to upload.
///
/// With no `--max-frames`, a GIF longer than the device limit is an error
/// rather than a silent truncation. With `--max-frames M`, frames are dropped
/// uniformly across the whole animation, so it still reads as the same motion,
/// just coarser.
pub fn select_frame_indices(
    n: usize,
    max_frames: Option<NonZeroUsize>,
) -> Result<Vec<usize>, String> {
    let limit = match max_frames {
        None => {
            if n > protocol::GIF_MAX_FRAMES {
                return Err(format!(
                    "it has {n} frames, {TOO_MANY_FRAMES} ({}). \
                     Re-run with --max-frames {} to upload a uniformly sampled subset, \
                     or shorten the GIF first.",
                    protocol::GIF_MAX_FRAMES,
                    protocol::GIF_MAX_FRAMES
                ));
            }
            return Ok((0..n).collect());
        }
        Some(m) => m.get(),
    };

    if limit > protocol::GIF_MAX_FRAMES {
        return Err(format!(
            "--max-frames {limit} is above the {} the keyboard can store",
            protocol::GIF_MAX_FRAMES
        ));
    }
    if n <= limit {
        return Ok((0..n).collect());
    }
    if limit == 1 {
        return Ok(vec![0]);
    }
    Ok((0..limit)
        .map(|i| ((i * (n - 1)) as f64 / (limit - 1) as f64).round() as usize)
        .collect())
}

/// Where a note belongs when a command-line caller prints it.
///
/// Carried rather than decided at the print site, because the split between
/// the two streams is a real interface -- scripts redirect them separately --
/// and burying it in whichever `println!`/`eprintln!` happened to be typed is
/// how it drifts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    Stdout,
    Stderr,
}

/// Why a note exists, so a caller can tell which ones still apply.
///
/// The TUI needs this: choosing a rate by hand is the equivalent of `--fps`
/// and suppresses the fallback warning, so the confirm screen has to know
/// which note *was* that warning. Without it the interface shows "using 30
/// fps" next to a rate the user just set to 24, and the interface is lying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoteKind {
    /// The file's own rate could not be used, so one was chosen for it.
    RateFallback,
    /// Anything else: the summary, the subsampling warning.
    Info,
}

/// Something the user should be told before the upload starts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Note {
    pub stream: Stream,
    pub kind: NoteKind,
    pub text: String,
}

impl Note {
    fn out(text: impl Into<String>) -> Self {
        Self {
            stream: Stream::Stdout,
            kind: NoteKind::Info,
            text: text.into(),
        }
    }

    fn err(text: impl Into<String>) -> Self {
        Self {
            stream: Stream::Stderr,
            kind: NoteKind::Info,
            text: text.into(),
        }
    }

    fn rate_fallback(text: impl Into<String>) -> Self {
        Self {
            stream: Stream::Stderr,
            kind: NoteKind::RateFallback,
            text: text.into(),
        }
    }
}

/// A picture upload, decided and ready to send.
#[derive(Debug)]
pub struct PicturePlan {
    pub pixels: Vec<u8>,
    pub total_reports: usize,
    pub notes: Vec<Note>,
}

/// A GIF upload, decided and ready to send.
#[derive(Debug)]
pub struct GifPlan {
    pub frames: Vec<Vec<u8>>,
    pub rate: u8,
    /// Which of the vendor's three GIF modes to save into. Carried rather than
    /// hardcoded in the executor because it is a property of what was decided,
    /// and because modes 0 and 2 exist and are still unexercised.
    pub mode: u8,
    /// Frames in the source file, before any subsampling.
    pub source_count: usize,
    pub total_reports: usize,
    /// Rough upload time. `None` for pictures and never guessed: the CLI has
    /// never estimated picture time, and inventing a number would be a guess
    /// presented as data.
    pub est_secs: usize,
    pub notes: Vec<Note>,
}

/// Reads a PNG or JPEG and decides everything about its upload.
pub fn plan_picture_upload(path: &Path) -> Result<PicturePlan, MediaError> {
    let pixels = load_and_encode_picture(path)?;
    let total_reports = protocol::picture_upload_report_count();
    Ok(PicturePlan {
        // Only what the planner alone knows. The CLI's "sending N reports"
        // line is deliberately NOT here: it is printed after "found device",
        // and moving it into the notes -- which are printed before the device
        // is looked for -- reordered the output. A hardware diff against the
        // previous commit caught it. Anything a caller can derive from
        // `total_reports` is the caller's to phrase and to place.
        notes: vec![Note::out(format!(
            "encoded {} to {}x{} RGB565 ({} bytes)",
            path.display(),
            protocol::PANEL_W,
            protocol::PANEL_H,
            pixels.len()
        ))],
        pixels,
        total_reports,
    })
}

/// The phrase a "too many frames" error is built from.
///
/// Public so a caller can recognise the case without matching on prose it does
/// not own. The TUI adds its own guidance when it sees this, and a reworded
/// message must not silently drop that.
pub const TOO_MANY_FRAMES: &str = "more frames than the keyboard can store";

/// Reads a GIF and decides everything about its upload: which frames, at what
/// rate, and what the user needs to be told about both.
///
/// `fps` is taken here rather than applied later because note gating depends
/// on it -- an explicit rate means the user already knows the file's own rate
/// is not being used, so the fallback note is suppressed.
pub fn plan_gif_upload(
    path: &Path,
    fps: Option<u8>,
    max_frames: Option<NonZeroUsize>,
) -> Result<GifPlan, MediaError> {
    let gif = load_gif_frames(path, max_frames)?;
    let frame_count = gif.frames.len();
    let rate = match fps {
        Some(f) => f,
        None => gif.rate.or_default(),
    };

    let mut notes = Vec::new();

    // Only when the rate was not chosen explicitly: if the user asked for a
    // rate, they already know it is not the file's.
    if let Some(why) = fps
        .is_none()
        .then(|| gif.rate.fallback_reason(rate))
        .flatten()
    {
        notes.push(Note::rate_fallback(why));
    }

    if frame_count < gif.source_count {
        let suggested = (rate as f64 * frame_count as f64 / gif.source_count as f64).round();
        let suggested = suggested.clamp(protocol::GIF_FPS_MIN as f64, protocol::GIF_FPS_MAX as f64);
        notes.push(Note::err(format!(
            "note: uploading {frame_count} of {} frames, sampled evenly. At {rate} fps the \
             animation plays {:.0}% as long as the original; --fps {suggested:.0} is closest to \
             the original duration.",
            gif.source_count,
            100.0 * frame_count as f64 / gif.source_count as f64
        )));
    }

    let total_reports = protocol::gif_upload_report_count(frame_count);
    // Dominated by the 3 s pause every 16th frame plus ~30 blocks/frame.
    let slow_pauses = frame_count.div_ceil(protocol::GIF_SLOW_DELAY_EVERY);
    let est_secs = slow_pauses * 3 + frame_count;
    notes.push(Note::out(format!(
        "{frame_count} frame(s) at {rate} fps -> {total_reports} reports, roughly {est_secs}s"
    )));

    Ok(GifPlan {
        frames: gif.frames,
        rate,
        mode: protocol::GIF_MODE_SAVE_TO_DEVICE,
        source_count: gif.source_count,
        total_reports,
        est_secs,
        notes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    // These moved here from `main.rs` with Milestone 5, along with the code
    // they exercise. They were never CLI tests: they read fixtures and assert
    // pixel values and message text, and several reach for internals that stay
    // private now that they live beside them.

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

    fn gif_px(pixels: &[u8], x: usize, y: usize) -> u16 {
        let o = (y * protocol::PANEL_W as usize + x) * 2;
        u16::from_be_bytes([pixels[o], pixels[o + 1]])
    }

    /// The test that justifies delegating frame construction to `image`
    /// instead of encoding raw frames.
    ///
    /// `fixtures/test-anim-disposal.gif` is 64x48. Frame 0 is the full canvas:
    /// grey, with a RED mark at the top-left. Frame 1 is a **16x12
    /// sub-rectangle at (48,36)** carrying a BLUE mark, with disposal "do not
    /// dispose" -- so frame 1, once composed, must still show the red mark AND
    /// show the blue one.
    ///
    /// An implementation that encoded raw sub-frames would produce a 16x12
    /// buffer for frame 1 and fail the length assertion, or -- worse, if it
    /// resized that buffer to the panel -- upload a full screen of blue.
    ///
    /// **What this does not prove**: that disposal is honoured. "Do not
    /// dispose" means keep the previous canvas, which is also what a decoder
    /// that ignored disposal entirely would do. The name used to claim
    /// otherwise. `gif_frames_honour_restore_to_background_disposal` below is
    /// the test that can actually fail if disposal is ignored.
    #[test]
    fn gif_sub_rectangle_frames_are_composed_onto_the_previous_canvas() {
        let gif = load_gif_frames(Path::new("fixtures/test-anim-disposal.gif"), None).unwrap();
        assert_eq!(gif.frames.len(), 2);
        for f in &gif.frames {
            assert_eq!(
                f.len(),
                protocol::PICTURE_BYTES,
                "every frame is one full panel frame"
            );
        }

        // Panel coordinates of the two marks after the 64x48 -> 160x96 stretch.
        let red_at = (10, 10); // frame 0's mark, top-left
        let blue_at = (140, 84); // frame 1's mark, bottom-right

        let f0_red = gif_px(&gif.frames[0], red_at.0, red_at.1);
        let f1_red = gif_px(&gif.frames[1], red_at.0, red_at.1);
        let f0_blue = gif_px(&gif.frames[0], blue_at.0, blue_at.1);
        let f1_blue = gif_px(&gif.frames[1], blue_at.0, blue_at.1);

        // Red channel high, others low -> the top 5 bits dominate.
        assert!(
            f0_red > 0xf000,
            "frame 0 should be red there, got {f0_red:#06x}"
        );
        assert!(
            f1_red > 0xf000,
            "frame 1 must STILL be red there -- the sub-rectangle was composed onto \
             the previous canvas rather than replacing it; got {f1_red:#06x}"
        );
        // The blue mark exists only in frame 1; frame 0 is the grey background
        // there (rgb(128,128,128) -> 0x8410).
        assert_eq!(
            f0_blue, 0x8410,
            "frame 0 should still be background grey there, got {f0_blue:#06x}"
        );
        assert!(
            (0x0010..=0x001f).contains(&f1_blue),
            "frame 1 should be blue there, got {f1_blue:#06x}"
        );
    }

    /// Disposal being applied, in a way that fails if it is ignored.
    ///
    /// `fixtures/test-anim-disposal-background.gif` is 64x48. Frame 0 is the
    /// full canvas -- grey with a RED mark at the top-left -- and its disposal
    /// is **restore to background**, so the canvas must be cleared before
    /// frame 1 is drawn. Frame 1 is a small GREEN rectangle somewhere else
    /// entirely.
    ///
    /// So in frame 1 the red mark must be **gone**. A decoder that ignored
    /// disposal would leave it there and fail this test -- which is exactly
    /// what the "do not dispose" fixture above cannot detect.
    #[test]
    fn gif_frames_honour_restore_to_background_disposal() {
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-disposal-background.gif"),
            None,
        )
        .unwrap();
        assert_eq!(gif.frames.len(), 2);

        // Panel coordinates after the 64x48 -> 160x96 stretch.
        let red_at = (30, 25); // inside frame 0's red mark
        let green_at = (140, 84); // inside frame 1's green rectangle

        assert_eq!(
            gif_px(&gif.frames[0], red_at.0, red_at.1),
            0xf800,
            "frame 0 should be red there"
        );
        // Black, not the fixture's grey background index. "Restore to
        // background" is implemented as "clear to transparent" by `image` --
        // the same choice browsers make, since the GIF spec's background
        // colour is widely ignored in favour of transparency -- and
        // `flatten_onto_black` then composites that transparency onto black.
        // So 0x0000 is the correct post-pipeline value, and 0x8410 (the
        // palette grey) would mean the background *index* had been painted
        // instead. Either way the red mark must be gone; that is the point.
        let cleared = gif_px(&gif.frames[1], red_at.0, red_at.1);

        // The invariant first, so a failure says which thing broke. If this
        // one fails, disposal was genuinely ignored.
        assert_ne!(
            cleared, 0xf800,
            "frame 0's red mark survived into frame 1 -- disposal was ignored"
        );
        // Then the exact value, pinning today's pipeline. If ONLY this one
        // fails, disposal still works and `image` changed how it clears;
        // update the expectation, do not weaken the assertion above.
        assert_eq!(
            cleared, 0x0000,
            "expected transparent-cleared-to-black; got {cleared:#06x} \
             (0x8410 would mean the palette background index was painted instead)"
        );

        assert_eq!(
            gif_px(&gif.frames[0], green_at.0, green_at.1),
            0x8410,
            "frame 0 is background grey where frame 1's rectangle will go"
        );
        assert_eq!(
            gif_px(&gif.frames[1], green_at.0, green_at.1),
            0x07e0,
            "frame 1 should be green there"
        );
    }

    /// Frames with different delays cannot be reproduced by a device that
    /// animates at one rate, so the fallback must say so and name the average.
    #[test]
    fn variable_delays_fall_back_with_a_warning_that_names_the_average() {
        let gif =
            load_gif_frames(Path::new("fixtures/test-anim-variable-delay.gif"), None).unwrap();
        assert_eq!(gif.source_count, 3);

        // 100, 400, 100 ms -> mean 200 ms.
        match gif.rate {
            SourceRate::Variable { mean_delay_ms } => {
                assert!(
                    (mean_delay_ms - 200.0).abs() < 1.0,
                    "mean delay should be ~200 ms, got {mean_delay_ms}"
                );
            }
            other => panic!("expected variable delays, got {other:?}"),
        }

        let chosen = gif.rate.or_default();
        assert_eq!(chosen, protocol::GIF_FPS_DEFAULT);

        let why = gif
            .rate
            .fallback_reason(chosen)
            .expect("a fallback must be explained");
        assert!(why.contains("different delays"), "got: {why}");
        assert!(why.contains("200 ms"), "must name the average; got: {why}");
        assert!(
            why.contains("30 fps"),
            "must name the rate used; got: {why}"
        );
    }

    /// Uniform delays asking for more than 60 fps used to be indistinguishable
    /// from "no rate computed", so the upload silently dropped to 30 fps with
    /// nothing printed. The warning is the point of this test.
    #[test]
    fn uniform_but_out_of_range_delays_warn_instead_of_clamping_silently() {
        let gif = load_gif_frames(Path::new("fixtures/test-anim-too-fast.gif"), None).unwrap();

        // 10 ms delays -> 100 fps, above the device's 60.
        match gif.rate {
            SourceRate::OutOfRange(wanted) => {
                assert!(
                    (wanted - 100.0).abs() < 1.0,
                    "should have asked for ~100 fps, got {wanted}"
                );
            }
            other => panic!("expected an out-of-range rate, got {other:?}"),
        }

        let chosen = gif.rate.or_default();
        assert_eq!(chosen, protocol::GIF_FPS_DEFAULT);

        let why = gif
            .rate
            .fallback_reason(chosen)
            .expect("clamping must never be silent");
        assert!(
            why.contains("100 fps"),
            "must name what was asked; got: {why}"
        );
        assert!(
            why.contains("30 fps"),
            "must name what was used; got: {why}"
        );
    }

    /// A GIF failure must not tell the user to supply a PNG.
    #[test]
    fn media_errors_name_the_command_that_failed() {
        let picture = MediaError {
            kind: MediaKind::Picture,
            path: PathBuf::from("/tmp/x.bmp"),
            detail: "unsupported".to_string(),
        }
        .to_string();
        assert!(picture.contains("as a picture"), "got: {picture}");
        assert!(picture.contains("PNG and JPEG"), "got: {picture}");
        assert!(!picture.contains("GIF."), "got: {picture}");

        let gif = MediaError {
            kind: MediaKind::Gif,
            path: PathBuf::from("/tmp/x.png"),
            detail: "not a readable GIF".to_string(),
        }
        .to_string();
        assert!(gif.contains("as an animation"), "got: {gif}");
        assert!(gif.contains("supported formats: GIF."), "got: {gif}");
        assert!(
            !gif.contains("PNG and JPEG"),
            "a set-gif failure must not advertise PNG; got: {gif}"
        );

        // Both keep the part that saves a user from checking the keyboard.
        for m in [picture, gif] {
            assert!(m.contains("The keyboard was not contacted."), "got: {m}");
        }
    }

    /// Delays slower than 1 fps must not round up into the valid range.
    ///
    /// This is the bug this fixture exists for: `(1000.0 / 1500.0).round()` is
    /// 1, so a 0.67 fps GIF passed the range check as though the file had
    /// asked for exactly 1 fps -- a 50% speed error reported as an exact
    /// match. Range-checking before rounding is what makes it `OutOfRange`.
    #[test]
    fn delays_below_one_fps_are_out_of_range_not_rounded_up() {
        let gif = load_gif_frames(Path::new("fixtures/test-anim-too-slow.gif"), None).unwrap();

        match gif.rate {
            SourceRate::OutOfRange(wanted) => {
                assert!(
                    (wanted - 0.666_666).abs() < 0.01,
                    "should have asked for ~0.67 fps, got {wanted}"
                );
            }
            SourceRate::Usable(f) => panic!(
                "0.67 fps was rounded up to {f} and accepted -- the range check \
                 is happening after the rounding again"
            ),
            other => panic!("expected an out-of-range rate, got {other:?}"),
        }

        let why = gif
            .rate
            .fallback_reason(gif.rate.or_default())
            .expect("below the floor must be explained");
        assert!(
            why.contains("0.67"),
            "a sub-1 rate must not be printed as \"1\"; got: {why}"
        );
    }

    /// An explicit `--fps` means the user already knows the file's own rate is
    /// not being used, so the fallback note must stay quiet. This is the other
    /// half of the warning contract.
    #[test]
    fn an_explicit_rate_suppresses_the_fallback_note_contract_only() {
        for fixture in [
            "fixtures/test-anim-variable-delay.gif",
            "fixtures/test-anim-too-fast.gif",
            "fixtures/test-anim-too-slow.gif",
        ] {
            let gif = load_gif_frames(Path::new(fixture), None).unwrap();

            // With no --fps the note is printed...
            assert!(
                gif.rate.fallback_reason(gif.rate.or_default()).is_some(),
                "{fixture} should warn when the rate is not chosen explicitly"
            );

            // ...and `run_set_gif` gates that call on `fps.is_none()`, which is
            // the condition mirrored here.
            let explicit: Option<u8> = Some(24);
            let note = explicit
                .is_none()
                .then(|| gif.rate.fallback_reason(24))
                .flatten();
            assert!(
                note.is_none(),
                "{fixture} must stay quiet when --fps was given; got: {note:?}"
            );
        }
    }

    /// A GIF that decodes to no frames must be a clean error, not a panic.
    ///
    /// The rate calculation indexes `delays[0]`, so an empty delay list would
    /// panic if the zero-frame guard above it were ever removed.
    #[test]
    fn a_gif_with_no_frames_is_a_clean_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("empty.gif");
        // A valid GIF89a header and trailer, with no image blocks at all.
        std::fs::write(
            &path,
            [
                b'G', b'I', b'F', b'8', b'9', b'a', 0x01, 0x00, 0x01, 0x00, 0x00, 0x00, 0x00, 0x3b,
            ],
        )
        .unwrap();

        let err = load_gif_frames(&path, None).expect_err("no frames must not succeed");
        let msg = err.to_string();
        assert!(
            msg.contains("as an animation"),
            "must be reported as a GIF problem; got: {msg}"
        );
        assert!(
            msg.contains("The keyboard was not contacted."),
            "got: {msg}"
        );
    }

    /// Every delay zero is uniform, not variable, and the note must not claim
    /// the delays differ when all of them are identical.
    #[test]
    fn zero_delays_are_reported_as_unspecified_not_as_differing() {
        let gif = load_gif_frames(Path::new("fixtures/test-anim-zero-delay.gif"), None).unwrap();
        assert_eq!(gif.rate, SourceRate::Unspecified);
        assert_eq!(gif.rate.or_default(), protocol::GIF_FPS_DEFAULT);

        let why = gif
            .rate
            .fallback_reason(protocol::GIF_FPS_DEFAULT)
            .expect("a zero-delay GIF still falls back, so it must say so");
        assert!(
            !why.contains("different delays"),
            "every delay is 0 -- they are identical, not different; got: {why}"
        );
        assert!(why.contains("as fast as possible"), "got: {why}");
        assert!(
            why.contains("30 fps"),
            "must name the rate used; got: {why}"
        );
    }

    #[test]
    fn gif_frames_encode_to_full_panel_frames_and_report_source_count() {
        let gif = load_gif_frames(Path::new("fixtures/test-anim-2frames.gif"), None).unwrap();
        assert_eq!(gif.frames.len(), 2);
        assert_eq!(gif.source_count, 2);
        // 100 ms delays -> 10 fps, inside 1-60, so it becomes the default.
        assert_eq!(gif.rate, SourceRate::Usable(10));
        assert_eq!(gif.rate.or_default(), 10);
        assert_eq!(gif.rate.fallback_reason(10), None, "nothing to warn about");
    }

    #[test]
    fn non_gif_and_broken_files_are_rejected_before_any_device_access() {
        let dir = tempfile::tempdir().unwrap();

        let txt = dir.path().join("notes.txt");
        std::fs::write(&txt, b"not a gif").unwrap();
        assert!(load_gif_frames(&txt, None).is_err());

        let empty = dir.path().join("empty.gif");
        std::fs::write(&empty, b"").unwrap();
        assert!(load_gif_frames(&empty, None).is_err());

        // A PNG is a valid image but not a GIF.
        assert!(load_gif_frames(Path::new("fixtures/test-quadrants.png"), None).is_err());

        let err = load_gif_frames(&txt, None).unwrap_err().to_string();
        assert!(
            err.contains("keyboard was not contacted"),
            "must be clear the device was never opened: {err}"
        );
    }

    #[test]
    fn frame_selection_keeps_everything_when_it_fits() {
        assert_eq!(select_frame_indices(3, None).unwrap(), vec![0, 1, 2]);
        assert_eq!(
            select_frame_indices(3, Some(NonZeroUsize::new(160).unwrap())).unwrap(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn over_limit_gif_errors_without_max_frames_but_samples_with_it() {
        // The distinction the Option type exists to preserve: absent means
        // refuse, an explicit 160 means sample.
        let err = select_frame_indices(200, None).unwrap_err();
        assert!(err.contains("200"), "names the frame count: {err}");
        assert!(err.contains("--max-frames"), "names the way forward: {err}");

        let kept = select_frame_indices(200, Some(NonZeroUsize::new(160).unwrap())).unwrap();
        assert_eq!(kept.len(), 160);
        assert_eq!(kept[0], 0);
        assert_eq!(
            *kept.last().unwrap(),
            199,
            "the sample spans the whole animation"
        );
        assert!(kept.windows(2).all(|w| w[0] < w[1]), "strictly increasing");
    }

    #[test]
    fn frame_selection_is_uniform_and_handles_the_single_frame_edge() {
        // 10 frames down to 5: evenly spread, both ends included.
        assert_eq!(
            select_frame_indices(10, Some(NonZeroUsize::new(5).unwrap())).unwrap(),
            vec![0, 2, 5, 7, 9]
        );
        // M == 1 would divide by zero in the general formula.
        assert_eq!(
            select_frame_indices(10, Some(NonZeroUsize::new(1).unwrap())).unwrap(),
            vec![0]
        );
    }

    // --- Milestone 5: the notes are data, so they can finally be asserted ---
    //
    // Two rounds of review on Milestone 4 asked for a test that the frame-rate
    // warnings actually reach the user. The honest answer then was that they
    // could only be checked by running the binary against real hardware,
    // because the text was built inline in a function that opened a device.
    // These are that test.

    fn note_texts(notes: &[Note]) -> Vec<&str> {
        notes.iter().map(|n| n.text.as_str()).collect()
    }

    /// A plain GIF whose own rate is usable says exactly one thing, on stdout.
    #[test]
    fn a_usable_rate_produces_only_the_summary_note() {
        let plan =
            plan_gif_upload(Path::new("fixtures/test-anim-2frames.gif"), None, None).unwrap();
        assert_eq!(plan.rate, 10, "the file's own 10 fps is usable");
        assert_eq!(plan.notes.len(), 1, "no warning is due: {:?}", plan.notes);
        assert_eq!(plan.notes[0].stream, Stream::Stdout);
        assert!(plan.notes[0].text.contains("2 frame(s) at 10 fps"));
        assert!(plan.notes[0].text.contains("1149 reports"));
    }

    /// Every fallback warns, on stderr, before the stdout summary.
    ///
    /// The order matters and is the thing separate stdout/stderr captures
    /// cannot show: a warning printed *after* the summary reads as if it
    /// applied to something else.
    #[test]
    fn every_fallback_warns_on_stderr_before_the_summary() {
        let cases = [
            ("fixtures/test-anim-variable-delay.gif", "different delays"),
            ("fixtures/test-anim-too-fast.gif", "100 fps"),
            ("fixtures/test-anim-too-slow.gif", "0.67 fps"),
            ("fixtures/test-anim-zero-delay.gif", "as fast as possible"),
        ];
        for (fixture, expected) in cases {
            let plan = plan_gif_upload(Path::new(fixture), None, None).unwrap();
            assert_eq!(
                plan.notes.len(),
                2,
                "{fixture} should warn and summarise: {:?}",
                note_texts(&plan.notes)
            );
            assert_eq!(
                plan.notes[0].stream,
                Stream::Stderr,
                "{fixture}: the warning belongs on stderr"
            );
            assert!(
                plan.notes[0].text.contains(expected),
                "{fixture}: got {:?}",
                plan.notes[0].text
            );
            assert_eq!(
                plan.notes[1].stream,
                Stream::Stdout,
                "{fixture}: the summary belongs on stdout"
            );
            assert_eq!(plan.rate, protocol::GIF_FPS_DEFAULT);
        }
    }

    /// An explicit rate suppresses the warning -- through the real API this
    /// time, not by re-implementing the `fps.is_none()` gate in the test.
    #[test]
    fn an_explicit_rate_suppresses_the_warning_end_to_end() {
        for fixture in [
            "fixtures/test-anim-variable-delay.gif",
            "fixtures/test-anim-too-fast.gif",
            "fixtures/test-anim-too-slow.gif",
            "fixtures/test-anim-zero-delay.gif",
        ] {
            let plan = plan_gif_upload(Path::new(fixture), Some(24), None).unwrap();
            assert_eq!(plan.rate, 24);
            assert_eq!(
                plan.notes.len(),
                1,
                "{fixture} must stay quiet when a rate was given: {:?}",
                note_texts(&plan.notes)
            );
            assert_eq!(plan.notes[0].stream, Stream::Stdout);
        }
    }

    /// Subsampling is the fifth note, and it names the rate that keeps the
    /// original duration.
    #[test]
    fn subsampling_warns_and_suggests_a_rate() {
        let plan = plan_gif_upload(
            Path::new("fixtures/test-anim-18frames.gif"),
            Some(30),
            Some(NonZeroUsize::new(9).unwrap()),
        )
        .unwrap();

        assert_eq!(plan.frames.len(), 9);
        assert_eq!(plan.source_count, 18);
        // An explicit rate silences the rate warning, so the only stderr note
        // left is the subsampling one -- which proves they are independent.
        let stderr: Vec<&str> = plan
            .notes
            .iter()
            .filter(|n| n.stream == Stream::Stderr)
            .map(|n| n.text.as_str())
            .collect();
        assert_eq!(stderr.len(), 1, "got {stderr:?}");
        assert!(stderr[0].contains("uploading 9 of 18 frames"));
        assert!(
            stderr[0].contains("--fps 15"),
            "half the frames at half the rate keeps the duration; got {:?}",
            stderr[0]
        );
    }

    /// The estimate and the report count come from the planner, so the CLI's
    /// message and a future progress bar cannot disagree.
    #[test]
    fn the_planner_owns_the_report_count_and_the_estimate() {
        let plan =
            plan_gif_upload(Path::new("fixtures/test-anim-18frames.gif"), Some(10), None).unwrap();
        assert_eq!(plan.total_reports, protocol::gif_upload_report_count(18));
        // Two slow pauses (frames 0 and 16) at 3 s, plus a second per frame.
        assert_eq!(plan.est_secs, 2 * 3 + 18);
    }

    /// The picture planner reports the same two lines the CLI always printed,
    /// both on stdout.
    #[test]
    fn the_picture_planner_describes_the_upload_on_stdout() {
        let plan = plan_picture_upload(Path::new("fixtures/test-quadrants.png")).unwrap();
        assert_eq!(plan.pixels.len(), protocol::PICTURE_BYTES);
        assert_eq!(plan.total_reports, protocol::picture_upload_report_count());
        assert_eq!(
            plan.notes.len(),
            1,
            "only the encode result -- the report-count line is the CLI's, so \
             that it keeps its place after \"found device\": {:?}",
            note_texts(&plan.notes)
        );
        assert_eq!(plan.notes[0].stream, Stream::Stdout);
        assert!(plan.notes[0].text.contains("160x96 RGB565 (30720 bytes)"));
    }

    /// A bad file fails in the planner, before anything device-shaped exists.
    #[test]
    fn planning_a_broken_file_fails_without_touching_a_device() {
        let err = plan_gif_upload(Path::new("fixtures/test-quadrants.png"), None, None)
            .expect_err("a PNG is not a GIF");
        let msg = err.to_string();
        assert!(msg.contains("as an animation"), "got: {msg}");
        assert!(
            msg.contains("The keyboard was not contacted."),
            "got: {msg}"
        );
    }
}
