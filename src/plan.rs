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

use crate::adjust::Adjustments;
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

/// How a source image is fit into the panel's fixed 160x96.
///
/// The vendor's "Location" dropdown sends no HID at all -- a client-side
/// resize choice, the same class of finding as Milestone 6's sliders (see
/// PROTOCOL.md). Traced to the vendor's real GIF resize/placement function
/// (`Ut()` in the bundle); the picture path's own placement mechanism was
/// not separately confirmed to use the same geometry (see PROTOCOL.md), so
/// `Contain` here is vendor-inspired for GIFs and our own internally
/// consistent choice for pictures -- not vendor-exact for either in the
/// final resampled pixel values (browser-specific resampling isn't
/// reproducible in Rust regardless).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Placement {
    /// Scale to fit inside the panel, preserving aspect ratio, centered,
    /// padded with opaque black. The vendor's default ("In the middle").
    #[default]
    Contain,
    /// Stretch to exactly fill the panel; aspect ratio is not preserved.
    /// The vendor's "Cover up completely" -- despite the name, this is CSS
    /// `object-fit: fill` (plain stretch), not `cover` (crop-to-fill): the
    /// vendor's own resize function draws the whole source into the full
    /// destination rectangle with no cropping (see PROTOCOL.md).
    Fill,
}

impl std::fmt::Display for Placement {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Placement::Contain => "contain",
            Placement::Fill => "fill",
        })
    }
}

/// Blackens the RGB of every fully-transparent (`alpha == 0`) pixel before a
/// spatial resize, so hidden colour can't leak into a resized neighbour under
/// `Contain`'s `Lanczos3` filter (which treats R/G/B/A as independent
/// channels on straight, non-premultiplied alpha). Deliberately narrower than
/// Milestone 3's "partial transparency keeps its full colour" policy:
/// partial-alpha pixels are left untouched here, on purpose -- only fully
/// hidden (`alpha == 0`) colour is blackened, mirroring exactly what
/// `Adjustments::apply()` already does before its own spatial filters
/// (sharpen/blur), extended here to the resize step those didn't need to
/// touch.
fn blacken_transparent(img: &mut image::RgbaImage) {
    for px in img.pixels_mut() {
        if px[3] == 0 {
            px[0] = 0;
            px[1] = 0;
            px[2] = 0;
        }
    }
}

/// `Contain`'s geometry alone, extracted from `resize_to_panel` so it's
/// directly testable against hand-computed values without decoding an
/// image. All four of `dst_w`/`dst_h`/`dst_x`/`dst_y` are rounded
/// independently from unrounded intermediates via `js_round` (JS
/// `Math.round` semantics), matching the *shape* of the vendor's own four
/// independent `Math.round` calls -- see `resize_to_panel`'s own doc
/// comment for why the resolution they're computed at differs from the
/// vendor's.
fn contain_geometry(src_w: u32, src_h: u32) -> (u32, u32, u32, u32) {
    let (src_w, src_h) = (src_w as f64, src_h as f64);
    let scale = (protocol::PANEL_W as f64 / src_w).min(protocol::PANEL_H as f64 / src_h);
    let dst_w = crate::adjust::js_round(src_w * scale) as u32;
    let dst_h = crate::adjust::js_round(src_h * scale) as u32;
    let dst_x =
        crate::adjust::js_round((protocol::PANEL_W as f64 - src_w * scale) / 2.0).max(0.0) as u32;
    let dst_y =
        crate::adjust::js_round((protocol::PANEL_H as f64 - src_h * scale) / 2.0).max(0.0) as u32;
    (dst_x, dst_y, dst_w, dst_h)
}

/// Fits `img` into the panel per `placement`, returning panel-sized RGBA.
///
/// `Fill`: unchanged from before Milestone 7 -- `resize_exact` with
/// `FilterType::Nearest`. See the two call sites below for what this filter
/// choice matches (or doesn't) on each path; the claim differs by path and
/// belongs there, not here.
///
/// `Contain`: scale to fit inside the panel preserving aspect ratio,
/// centred, padded with opaque black -- computed once at panel resolution,
/// not the vendor's real 3x-then-1.5x-then-1x staged resolution (see
/// `Placement`'s own doc comment). Geometry itself lives in
/// `contain_geometry`, above. `FilterType::Lanczos3` is our own
/// high-quality choice, not a vendor-matched one.
fn resize_to_panel(img: &image::DynamicImage, placement: Placement) -> Vec<u8> {
    match placement {
        Placement::Fill => img
            .resize_exact(
                protocol::PANEL_W,
                protocol::PANEL_H,
                image::imageops::FilterType::Nearest,
            )
            .to_rgba8()
            .into_raw(),
        Placement::Contain => {
            let mut src = img.to_rgba8();
            blacken_transparent(&mut src);
            let (dst_x, dst_y, dst_w, dst_h) = contain_geometry(src.width(), src.height());

            let mut panel = image::RgbaImage::from_pixel(
                protocol::PANEL_W,
                protocol::PANEL_H,
                image::Rgba([0, 0, 0, 255]),
            );
            // A degenerate source (extreme aspect ratio) can round to zero on
            // one axis -- the vendor's own `drawImage` with a zero dimension
            // is a silent no-op onto its pre-filled black canvas; skip the
            // resize/copy the same way rather than let `image::imageops::resize`
            // panic on a zero target dimension.
            if dst_w > 0 && dst_h > 0 {
                let resized = image::imageops::resize(
                    &src,
                    dst_w,
                    dst_h,
                    image::imageops::FilterType::Lanczos3,
                );
                for (x, y, px) in resized.enumerate_pixels() {
                    let (px_x, px_y) = (x + dst_x, y + dst_y);
                    if px_x < protocol::PANEL_W && px_y < protocol::PANEL_H {
                        panel.put_pixel(px_x, px_y, *px);
                    }
                }
            }
            panel.into_raw()
        }
    }
}

/// Reads an image file and converts it to the panel's 30720-byte RGB565
/// frame. Pure of any device access on purpose: a bad path or a corrupt file
/// must fail as an image problem, and must be testable without hardware.
pub fn load_and_encode_picture(
    path: &Path,
    placement: Placement,
    adjustments: &Adjustments,
) -> Result<(Vec<u8>, Vec<u8>), MediaError> {
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

    // `Fill`'s nearest-neighbour matches the vendor's real picture-save
    // handler: it draws the fabric.js canvas onto the export canvas with
    // `imageSmoothingEnabled = false` (a real 2x downscale, 320x192 ->
    // 160x96, not a same-size copy -- see PROTOCOL.md). What that handler's
    // own FIRST stage (rendering the source onto its 320x192 canvas in the
    // first place, where placement itself is presumably decided) actually
    // does has not been traced -- this claim covers only the final export
    // step's filter, not the full pipeline.
    let panel = resize_to_panel(&img, placement);
    let pixels = adjust_and_encode(&panel, adjustments);
    debug_assert_eq!(pixels.len(), protocol::PICTURE_BYTES);
    Ok((pixels, panel))
}

/// One GIF's frames, already converted to panel-ready RGB565, plus what the
/// caller needs to explain what it did.
#[derive(Debug)]
pub struct GifFrames {
    frames: Vec<Vec<u8>>,
    /// The same frames before adjustment and before the RGB565 encode, panel
    /// sized. Kept so the interface can re-adjust from pristine pixels rather
    /// than stacking one adjustment on the last.
    panel_rgba: Vec<Vec<u8>>,
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
    placement: Placement,
    adjustments: &Adjustments,
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
    let mut panel_rgba = Vec::with_capacity(keep.len());
    for (i, frame) in gif_frames_iter(path)?.enumerate() {
        let frame = frame.map_err(|e| fail(format!("could not decode a frame: {e}")))?;
        if !keep.contains(&i) {
            continue;
        }
        let mut canvas = image::DynamicImage::ImageRgba8(frame.into_buffer());
        flatten_onto_black(&mut canvas);
        // `Fill`'s nearest-neighbour does NOT match the vendor here --
        // `Ut()`, the vendor's real GIF resize/placement function, uses
        // `imageSmoothingEnabled = true, imageSmoothingQuality = "high"`
        // for both its placement modes (see PROTOCOL.md). `FilterType::Nearest`
        // is kept anyway: this byte output already shipped and is exercised
        // by existing golden tests, and changing it is a separate,
        // higher-risk change orthogonal to adding the missing `Contain` mode.
        let panel = resize_to_panel(&canvas, placement);
        frames.push(adjust_and_encode(&panel, adjustments));
        panel_rgba.push(panel);
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
        panel_rgba,
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
    /// Panel-sized RGBA before adjustment; see `GifFrames::panel_rgba`.
    pub panel_rgba: Vec<u8>,
    /// How the source was fit into the panel to produce `panel_rgba`.
    /// Recorded for display/debugging, matching how `adjustments` is
    /// already carried. Changing placement always re-derives `panel_rgba`
    /// from the original source -- there is no cheap partial re-derivation
    /// the way `reencode()` gives adjustments, since the first resize
    /// already destroyed the information the other placement would need.
    pub placement: Placement,
    /// What `pixels` was encoded with. Changing it and calling `reencode`
    /// is how the interface applies a new setting.
    pub adjustments: Adjustments,
    pub total_reports: usize,
    pub notes: Vec<Note>,
}

/// A GIF upload, decided and ready to send.
#[derive(Debug)]
pub struct GifPlan {
    pub frames: Vec<Vec<u8>>,
    /// Panel-sized RGBA before adjustment, one per encoded frame.
    pub panel_rgba: Vec<Vec<u8>>,
    /// How the source was fit into the panel; see `PicturePlan::placement`.
    pub placement: Placement,
    /// What `frames` was encoded with; see `PicturePlan::adjustments`.
    pub adjustments: Adjustments,
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
pub fn plan_picture_upload(
    path: &Path,
    placement: Placement,
    adjustments: &Adjustments,
) -> Result<PicturePlan, MediaError> {
    let (pixels, panel_rgba) = load_and_encode_picture(path, placement, adjustments)?;
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
        panel_rgba,
        placement,
        adjustments: *adjustments,
        total_reports,
    })
}

/// The phrase a "too many frames" error is built from.
///
/// Public so a caller can recognise the case without matching on prose it does
/// not own. The TUI adds its own guidance when it sees this, and a reworded
/// message must not silently drop that.
pub const TOO_MANY_FRAMES: &str = "more frames than the keyboard can store";

impl PicturePlan {
    /// Re-encodes from the pristine panel pixels with the current settings.
    ///
    /// Cheap for a picture -- one frame -- but it lives here beside the GIF
    /// version so both front ends call the same thing.
    pub fn reencode(&mut self) {
        self.pixels = adjust_and_encode(&self.panel_rgba, &self.adjustments);
    }
}

impl GifPlan {
    /// Re-encodes every frame from the pristine panel pixels.
    ///
    /// Up to 160 frames through six filters, so this belongs on the worker
    /// thread, never on the one drawing the screen.
    pub fn reencode(&mut self) {
        for (out, src) in self.frames.iter_mut().zip(self.panel_rgba.iter()) {
            *out = adjust_and_encode(src, &self.adjustments);
        }
    }
}

/// Applies adjustments to one panel-sized RGBA frame and encodes it.
///
/// The single place adjustment meets encoding, so the command line and the
/// interface cannot produce different pixels from the same numbers. The
/// interface re-runs exactly this on the pristine frame every time a slider
/// moves, which is what keeps "what the preview shows is what gets uploaded"
/// true.
pub fn adjust_and_encode(panel_rgba: &[u8], adjustments: &Adjustments) -> Vec<u8> {
    if adjustments.is_identity() {
        // Byte-for-byte what this did before adjustments existed.
        return protocol::rgb565_encode(panel_rgba);
    }
    let mut img =
        image::RgbaImage::from_raw(protocol::PANEL_W, protocol::PANEL_H, panel_rgba.to_vec())
            .expect("panel-sized buffer");
    adjustments.apply(&mut img);
    protocol::rgb565_encode(img.as_raw())
}

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
    placement: Placement,
    adjustments: &Adjustments,
) -> Result<GifPlan, MediaError> {
    let gif = load_gif_frames(path, max_frames, placement, adjustments)?;
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
        panel_rgba: gif.panel_rgba,
        placement,
        adjustments: *adjustments,
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
        let err = load_and_encode_picture(
            Path::new("/nonexistent/nope.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap_err();
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
        assert!(load_and_encode_picture(&path, Placement::Fill, &Adjustments::NONE).is_err());
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
        assert!(load_and_encode_picture(&path, Placement::Fill, &Adjustments::NONE).is_err());
    }

    #[test]
    fn unsupported_format_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"this is plain text, not an image").unwrap();
        assert!(load_and_encode_picture(&path, Placement::Fill, &Adjustments::NONE).is_err());
    }

    #[test]
    fn real_png_encodes_to_exactly_one_panel_frame() {
        // The committed test image is already 160x96, so this also confirms
        // the resize path is a no-op at the exact panel size rather than
        // shifting pixels.
        let (pixels, _) = load_and_encode_picture(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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

        let (pixels, _) = load_and_encode_picture(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        let (pixels, _) = load_and_encode_picture(
            Path::new("fixtures/test-exif-rotated.jpg"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        let (pixels, _) =
            load_and_encode_picture(&path, Placement::Fill, &Adjustments::NONE).unwrap();
        assert_eq!(pixels.len(), protocol::PICTURE_BYTES);
        assert_eq!(&pixels[0..2], &[0xf8, 0x00]);
    }

    // --- Milestone 7: placement ---

    #[test]
    fn contain_geometry_matches_the_hand_computed_formula() {
        // scale = min(160/161, 96/97) = 96/97; dst_w = js_round(161*96/97) =
        // 159; dst_h = js_round(97*96/97) = 96; dst_x = js_round((160 -
        // 159.34)/2) = 0; dst_y = js_round((96-96)/2) = 0.
        assert_eq!(contain_geometry(161, 97), (0, 0, 159, 96));
    }

    #[test]
    fn contain_on_a_degenerate_source_is_an_all_black_panel_no_panic() {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            10000,
            image::Rgba([255, 0, 0, 255]),
        ));
        let panel = resize_to_panel(&img, Placement::Contain);
        assert!(
            panel.chunks_exact(4).all(|px| px == [0, 0, 0, 255]),
            "a 1x10000 source rounds dst_w to 0 -- the panel must stay solid \
             black, matching the vendor's drawImage-with-zero-width no-op"
        );
    }

    #[test]
    fn an_exactly_panel_sized_source_encodes_identically_under_both_placements() {
        // Only an EXACT 160x96 source does no resizing at all under either
        // mode -- Fill uses Nearest and Contain uses Lanczos3, so a
        // same-aspect-ratio-but-different-size source would genuinely differ.
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            protocol::PANEL_W,
            protocol::PANEL_H,
            image::Rgba([12, 34, 56, 255]),
        ));
        let fill = adjust_and_encode(&resize_to_panel(&img, Placement::Fill), &Adjustments::NONE);
        let contain = adjust_and_encode(
            &resize_to_panel(&img, Placement::Contain),
            &Adjustments::NONE,
        );
        assert_eq!(fill, contain);
    }

    #[test]
    fn contain_padding_is_exactly_opaque_black() {
        // A 1x1 source scales UP to fill the limiting dimension (Contain
        // fits, it doesn't shrink-only) -- min(160,96) = 96, so it fills a
        // centred 96x96 square, leaving 32px black bars on each side. Use
        // `contain_geometry` itself to know exactly which pixels are
        // guaranteed padding, rather than assuming the source stays 1x1.
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 255, 255, 255]),
        ));
        let (dst_x, dst_y, dst_w, dst_h) = contain_geometry(1, 1);
        let panel = resize_to_panel(&img, Placement::Contain);
        for (i, px) in panel.chunks_exact(4).enumerate() {
            let x = (i as u32) % protocol::PANEL_W;
            let y = (i as u32) / protocol::PANEL_W;
            let inside = x >= dst_x && x < dst_x + dst_w && y >= dst_y && y < dst_y + dst_h;
            if !inside {
                assert_eq!(
                    px,
                    [0, 0, 0, 255],
                    "padding pixel ({x},{y}) is not opaque black"
                );
            }
        }
    }

    #[test]
    fn contain_padding_receives_adjustments_a_documented_divergence_from_the_vendor() {
        // The vendor bakes filters in BEFORE placement, so its padding stays
        // pure black; this repo resizes before adjusting (Milestone 6's
        // memory-driven order), so Contain's padding is an ordinary opaque
        // black pixel to the filter pipeline like any other -- a documented,
        // deliberate divergence, not a bug. Asserted directly rather than
        // assumed.
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            1,
            1,
            image::Rgba([255, 255, 255, 255]),
        ));
        let panel_rgba = resize_to_panel(&img, Placement::Contain);
        let brightened = adjust_and_encode(
            &panel_rgba,
            &Adjustments {
                brightness: 0.5,
                ..Adjustments::NONE
            },
        );
        let plain = adjust_and_encode(&panel_rgba, &Adjustments::NONE);
        // A corner pixel is guaranteed to be padding for a 1x1 source
        // Contain-fit into a 160x96 panel.
        let corner = |bytes: &[u8]| u16::from_be_bytes([bytes[0], bytes[1]]);
        assert_ne!(
            corner(&brightened),
            corner(&plain),
            "padding must be visibly affected by brightness, unlike the vendor"
        );
    }

    #[test]
    fn contain_preserves_alpha_in_the_image_region_padding_is_opaque() {
        // Scoped to pictures (the resize/placement helper directly) -- GIF
        // frames are already fully opaque by the time placement runs
        // (`flatten_onto_black`), so this alpha variation only exists here.
        for alpha in [0u8, 128, 255] {
            let mut src = image::RgbaImage::new(1, 1);
            src.put_pixel(0, 0, image::Rgba([200, 50, 25, alpha]));
            let img = image::DynamicImage::ImageRgba8(src);
            let panel = resize_to_panel(&img, Placement::Contain);
            // The single source pixel Contain-fits to exactly fill one axis
            // (1x1 into 160x96: scale = min(160,96) = 96, dst_w=dst_h=96,
            // centred) -- sample its centre, guaranteed inside the source
            // region, not the padding.
            let cx = protocol::PANEL_W / 2;
            let cy = protocol::PANEL_H / 2;
            let o = ((cy * protocol::PANEL_W + cx) * 4) as usize;
            assert_eq!(
                panel[o + 3],
                alpha,
                "alpha {alpha} should survive the resize unpremultiplied in the \
                 image region"
            );
        }
    }

    #[test]
    fn a_transparent_pixel_does_not_bleed_into_a_resized_neighbour_under_contain() {
        // Mirrors adjust.rs's own `a_transparent_pixel_does_not_bleed_into_its_neighbours`,
        // but for the resize step Milestone 7 adds rather than the spatial
        // filters Milestone 6 already covered.
        let mut src = image::RgbaImage::new(4, 1);
        src.put_pixel(0, 0, image::Rgba([255, 0, 0, 0])); // invisible red
        src.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        src.put_pixel(2, 0, image::Rgba([0, 0, 0, 255]));
        src.put_pixel(3, 0, image::Rgba([0, 0, 0, 255]));
        let img = image::DynamicImage::ImageRgba8(src);
        let panel = resize_to_panel(&img, Placement::Contain);
        // Every resulting pixel's red channel must stay 0 -- the hidden red
        // must not leak into the resized, visible black neighbours.
        assert!(
            panel.chunks_exact(4).all(|px| px[0] == 0),
            "hidden red bled into a resized neighbour"
        );
    }

    #[test]
    fn plan_picture_upload_carries_its_placement() {
        let plan = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Contain,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(plan.placement, Placement::Contain);
    }

    #[test]
    fn plan_gif_upload_carries_its_placement() {
        let plan = plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            None,
            None,
            Placement::Contain,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(plan.placement, Placement::Contain);
    }

    #[test]
    fn every_gif_frame_respects_the_selected_placement() {
        // Mirrors Milestone 6's `every_gif_frame_is_adjusted` test shape --
        // every frame, not just the first, must go through `resize_to_panel`
        // with the chosen placement.
        let contain = plan_gif_upload(
            Path::new("fixtures/test-anim-disposal.gif"),
            None,
            None,
            Placement::Contain,
            &Adjustments::NONE,
        )
        .unwrap();
        let fill = plan_gif_upload(
            Path::new("fixtures/test-anim-disposal.gif"),
            None,
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(contain.frames.len(), fill.frames.len());
        for (i, (c, f)) in contain.frames.iter().zip(fill.frames.iter()).enumerate() {
            assert_ne!(
                c, f,
                "frame {i}: a 64x48 source into a 160x96 panel must differ \
                 between Contain and Fill, or this frame wasn't placed at all"
            );
        }
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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-disposal.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
            Placement::Fill,
            &Adjustments::NONE,
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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-variable-delay.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-too-fast.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();

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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-too-slow.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();

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
            let gif = load_gif_frames(
                Path::new(fixture),
                None,
                Placement::Fill,
                &Adjustments::NONE,
            )
            .unwrap();

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

        let err = load_gif_frames(&path, None, Placement::Fill, &Adjustments::NONE)
            .expect_err("no frames must not succeed");
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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-zero-delay.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        let gif = load_gif_frames(
            Path::new("fixtures/test-anim-2frames.gif"),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        assert!(load_gif_frames(&txt, None, Placement::Fill, &Adjustments::NONE).is_err());

        let empty = dir.path().join("empty.gif");
        std::fs::write(&empty, b"").unwrap();
        assert!(load_gif_frames(&empty, None, Placement::Fill, &Adjustments::NONE).is_err());

        // A PNG is a valid image but not a GIF.
        assert!(
            load_gif_frames(
                Path::new("fixtures/test-quadrants.png"),
                None,
                Placement::Fill,
                &Adjustments::NONE
            )
            .is_err()
        );

        let err = load_gif_frames(&txt, None, Placement::Fill, &Adjustments::NONE)
            .unwrap_err()
            .to_string();
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
        let plan = plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            None,
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
            let plan = plan_gif_upload(
                Path::new(fixture),
                None,
                None,
                Placement::Fill,
                &Adjustments::NONE,
            )
            .unwrap();
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
            let plan = plan_gif_upload(
                Path::new(fixture),
                Some(24),
                None,
                Placement::Fill,
                &Adjustments::NONE,
            )
            .unwrap();
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
            Placement::Fill,
            &Adjustments::NONE,
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
        let plan = plan_gif_upload(
            Path::new("fixtures/test-anim-18frames.gif"),
            Some(10),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(plan.total_reports, protocol::gif_upload_report_count(18));
        // Two slow pauses (frames 0 and 16) at 3 s, plus a second per frame.
        assert_eq!(plan.est_secs, 2 * 3 + 18);
    }

    /// The picture planner reports the same two lines the CLI always printed,
    /// both on stdout.
    #[test]
    fn the_picture_planner_describes_the_upload_on_stdout() {
        let plan = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
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
        let err = plan_gif_upload(
            Path::new("fixtures/test-quadrants.png"),
            None,
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .expect_err("a PNG is not a GIF");
        let msg = err.to_string();
        assert!(msg.contains("as an animation"), "got: {msg}");
        assert!(
            msg.contains("The keyboard was not contacted."),
            "got: {msg}"
        );
    }

    // --- Milestone 6: adjustments on the encoder path ---

    /// An unadjusted upload never enters the filter path at all.
    ///
    /// The whole milestone is worthless if adding the feature moves a pixel
    /// for someone who never touches it. `is_identity` short-circuits before
    /// any filter runs, so the bytes are the plain encode of the panel --
    /// literally the same call the code made before adjustments existed.
    ///
    /// The end-to-end proof that this still matches real hardware is
    /// `encoding_the_source_png_reproduces_the_whole_captured_upload`, which
    /// compares against the captured upload and is unchanged by this
    /// milestone.
    #[test]
    fn no_adjustments_means_the_plain_encode() {
        let plan = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(plan.pixels.len(), protocol::PICTURE_BYTES);
        assert_eq!(
            plan.pixels,
            protocol::rgb565_encode(&plan.panel_rgba),
            "identity must bypass the filters, not run a no-op chain through them"
        );
    }

    /// The panel pixels are carried alongside, unadjusted, so the interface
    /// can re-adjust from pristine data.
    #[test]
    fn plans_carry_the_unadjusted_panel_pixels() {
        let plan = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(
            plan.panel_rgba.len(),
            protocol::PANEL_W as usize * protocol::PANEL_H as usize * 4
        );

        let gif = plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            Some(10),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        assert_eq!(gif.panel_rgba.len(), gif.frames.len());
    }

    /// Re-encoding pristine pixels with the same adjustments is the same
    /// answer, which is what lets the interface preview and upload separately.
    #[test]
    fn adjust_and_encode_is_the_one_place_pixels_are_made() {
        let adj = Adjustments {
            brightness: 0.3,
            grayscale: true,
            ..Adjustments::NONE
        };
        let planned = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &adj,
        )
        .unwrap();
        let unadjusted = plan_picture_upload(
            Path::new("fixtures/test-quadrants.png"),
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();

        assert_ne!(
            planned.pixels, unadjusted.pixels,
            "the adjustment did something"
        );
        assert_eq!(
            planned.pixels,
            adjust_and_encode(&unadjusted.panel_rgba, &adj),
            "and re-running it on the pristine pixels reproduces it exactly"
        );
    }

    /// Adjustments reach every encoded GIF frame, not just the first.
    #[test]
    fn every_gif_frame_is_adjusted() {
        let adj = Adjustments {
            grayscale: true,
            ..Adjustments::NONE
        };
        let plain = plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            Some(10),
            None,
            Placement::Fill,
            &Adjustments::NONE,
        )
        .unwrap();
        let grey = plan_gif_upload(
            Path::new("fixtures/test-anim-2frames.gif"),
            Some(10),
            None,
            Placement::Fill,
            &adj,
        )
        .unwrap();

        assert_eq!(grey.frames.len(), 2);
        for i in 0..2 {
            assert_ne!(
                grey.frames[i], plain.frames[i],
                "frame {i} was not adjusted"
            );
            assert_eq!(
                grey.frames[i],
                adjust_and_encode(&plain.panel_rgba[i], &adj),
                "frame {i} does not match a direct re-encode"
            );
        }
    }
}
