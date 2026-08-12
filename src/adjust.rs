//! The vendor's "Screen Settings" adjustments, reimplemented.
//!
//! Four milestones assumed these controls were undecoded protocol. They are
//! not: **they send no HID at all**. Every one is a Fabric.js canvas filter the
//! vendor applies to the image in the browser, before the ordinary upload runs.
//! So there was nothing to reverse engineer here, only arithmetic to copy
//! exactly -- and copying it exactly is the whole job, because the numbers are
//! observable side by side with the vendor's tool.
//!
//! Every formula below was read out of the vendor bundle's `applyTo2d` -- the
//! CPU path, which is the canonical definition of each filter.
//!
//! ## Three rounding rules, not one
//!
//! This is where a naive port goes wrong, so it is worth stating up front:
//!
//! - Writing a float to a channel goes through `Uint8ClampedArray`, which
//!   clamps to 0..=255 and rounds **half to even**: 0.5 becomes 0, 1.5
//!   becomes 2, 2.5 becomes 2.
//! - JavaScript's `Math.round` is `floor(x + 0.5)` -- half toward positive
//!   infinity, so `Math.round(-0.5)` is `-0`.
//! - Rust's `f64::round` rounds half **away from zero**, and `as u8`
//!   saturates and truncates.
//!
//! All three differ. Everything here is `f64` end to end, and the only two
//! places a number narrows are [`to_u8`] and [`js_round`].
//!
//! ## Alpha is never written
//!
//! A deliberate difference from fabric, which convolves alpha in its sharpen
//! filter. For GIFs it makes no difference: frames are flattened onto black
//! before this runs, so alpha is 255 everywhere and a kernel summing to 1
//! returns 255. For pictures it matters a great deal -- that path deliberately
//! keeps partial alpha at full colour and turns only alpha 0 into black, and
//! convolving alpha would move which pixels the encoder blackens. A sharpen
//! filter has no business making that decision.

use image::RgbaImage;

/// One panel-sized frame's worth of adjustments.
///
/// Applied at 160x96, after any flattening and the resize, immediately before
/// the RGB565 encode. Not at full resolution: a 160-frame GIF from a 4K source
/// would be gigabytes of RGBA, and running six filters over that on every
/// keypress would stall the interface. Sharpening a nearest-neighbour
/// downscale is harsher than sharpening the original would have been -- that
/// is the trade, and the panel is 160x96 anyway.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Adjustments {
    /// -1.0 ..= 1.0
    pub brightness: f64,
    /// -1.0 ..= 1.0
    pub chroma: f64,
    /// -1.0 ..= 1.0
    pub saturation: f64,
    pub grayscale: bool,
    pub sharpen: bool,
    pub blur: bool,
}

impl Default for Adjustments {
    fn default() -> Self {
        Self::NONE
    }
}

impl Adjustments {
    pub const NONE: Self = Self {
        brightness: 0.0,
        chroma: 0.0,
        saturation: 0.0,
        grayscale: false,
        sharpen: false,
        blur: false,
    };

    /// Nothing to do.
    ///
    /// Checked before any work so an unadjusted upload produces byte-identical
    /// output to a build without this module -- not "the same thing through a
    /// no-op filter chain", which is how rounding drift creeps in.
    pub fn is_identity(&self) -> bool {
        *self == Self::NONE
    }

    /// Applies every active adjustment, in the fixed order.
    ///
    /// The order is fixed because the vendor's is not: fabric applies filters
    /// in array order and the vendor pushes each one as the user toggles it,
    /// so its result depends on which switch was clicked first. That is a bug,
    /// not a specification, and cannot be copied.
    ///
    /// Colour first so greyscale sees adjusted colours; the two spatial
    /// filters last, sharpen before blur -- sharpening a blur throws away the
    /// blur, while blurring a sharpen is a legitimate softening.
    pub fn apply(&self, img: &mut RgbaImage) {
        if self.is_identity() {
            return;
        }
        // A fully transparent pixel still carries RGB, and the encoder is
        // about to turn it black anyway. Left alone, the two spatial filters
        // would sample that hidden colour and smear it into visible
        // neighbours -- transparent red beside opaque black comes out
        // pink after a blur. Blacken it first, which is what the encoder
        // would have done, so the filters only ever see what will be shown.
        for px in img.pixels_mut() {
            if px[3] == 0 {
                px[0] = 0;
                px[1] = 0;
                px[2] = 0;
            }
        }
        if self.brightness != 0.0 {
            brightness(img, self.brightness);
        }
        if self.chroma != 0.0 {
            chroma(img, self.chroma);
        }
        if self.saturation != 0.0 {
            saturation(img, self.saturation);
        }
        if self.grayscale {
            grayscale(img);
        }
        if self.sharpen {
            sharpen(img);
        }
        if self.blur {
            blur(img);
        }
    }

    /// A short description of what is on, for `--dry-run` and the interface.
    pub fn summary(&self) -> Option<String> {
        if self.is_identity() {
            return None;
        }
        let mut parts = Vec::new();
        if self.brightness != 0.0 {
            parts.push(format!("brightness {:+.2}", self.brightness));
        }
        if self.chroma != 0.0 {
            parts.push(format!("chroma {:+.2}", self.chroma));
        }
        if self.saturation != 0.0 {
            parts.push(format!("saturation {:+.2}", self.saturation));
        }
        if self.grayscale {
            parts.push("grayscale".into());
        }
        if self.sharpen {
            parts.push("sharpen".into());
        }
        if self.blur {
            parts.push("blur".into());
        }
        Some(parts.join(", "))
    }
}

/// `Uint8ClampedArray` assignment: clamp to 0..=255, ties to even.
///
/// Not `as u8` (truncates) and not `f64::round` (ties away from zero). Both
/// give different pixels from the vendor for exactly the inputs a slider
/// produces.
fn to_u8(v: f64) -> u8 {
    if v.is_nan() {
        return 0;
    }
    if v <= 0.0 {
        return 0;
    }
    if v >= 255.0 {
        return 255;
    }
    // Ties to even, matching the ECMAScript conversion. Written as an
    // explicit comparison rather than a chain of ifs so it reads as the one
    // rule it is: round up above the tie, down below it, and at the tie pick
    // whichever of the two neighbours is even.
    let floor = v.floor();
    let frac = v - floor;
    let round_up = match frac.partial_cmp(&0.5) {
        Some(std::cmp::Ordering::Greater) => true,
        Some(std::cmp::Ordering::Less) => false,
        _ => (floor as i64) % 2 != 0,
    };
    (if round_up { floor + 1.0 } else { floor }) as u8
}

/// JavaScript `Math.round`: `floor(x + 0.5)`, so halves go toward +infinity.
fn js_round(v: f64) -> f64 {
    (v + 0.5).floor()
}

/// `filters.Brightness`: an integer offset on each colour channel.
fn brightness(img: &mut RgbaImage, v: f64) {
    let offset = js_round(v * 255.0);
    for px in img.pixels_mut() {
        for c in 0..3 {
            px[c] = to_u8(px[c] as f64 + offset);
        }
    }
}

/// The vendor's `ColorMatrix`. Its constant terms are zero and its alpha row
/// is the identity, so it reduces to one multiplier per channel.
fn chroma(img: &mut RgbaImage, v: f64) {
    let (kr, kg, kb) = (1.0 + v, 1.0 + v * 0.5, 1.0 - v);
    for px in img.pixels_mut() {
        px[0] = to_u8(px[0] as f64 * kr);
        px[1] = to_u8(px[1] as f64 * kg);
        px[2] = to_u8(px[2] as f64 * kb);
    }
}

/// `filters.Saturation`. Fabric negates the parameter, then pushes each
/// channel away from (or toward) the brightest one.
fn saturation(img: &mut RgbaImage, v: f64) {
    let w = -v;
    for px in img.pixels_mut() {
        let max = px[0].max(px[1]).max(px[2]) as f64;
        for c in 0..3 {
            let cur = px[c] as f64;
            if max != cur {
                px[c] = to_u8(cur + (max - cur) * w);
            }
        }
    }
}

/// `filters.Grayscale`, mode `average` -- fabric's default, and what the
/// vendor constructs.
fn grayscale(img: &mut RgbaImage) {
    for px in img.pixels_mut() {
        let avg = (px[0] as f64 + px[1] as f64 + px[2] as f64) / 3.0;
        let g = to_u8(avg);
        px[0] = g;
        px[1] = g;
        px[2] = g;
    }
}

/// The 3x3 sharpen kernel the vendor passes to `Convolute`.
const SHARPEN_KERNEL: [f64; 9] = [0.0, -1.0, 0.0, -1.0, 5.0, -1.0, 0.0, -1.0, 0.0];

/// `filters.Convolute` with the vendor's kernel.
///
/// Out-of-bounds samples are **skipped**, which is zero padding -- fabric's
/// loop is `!(Q<0||Q>=T||ee<0||ee>=D) && (...)`, so a missing neighbour
/// contributes nothing rather than repeating the edge pixel. Edges therefore
/// come out brighter than the interior, because the dropped taps are the
/// negative ones. That is the vendor's behaviour and this reproduces it.
fn sharpen(img: &mut RgbaImage) {
    let (w, h) = (img.width() as i64, img.height() as i64);
    let src = img.clone();
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f64; 3];
            for ky in 0..3i64 {
                for kx in 0..3i64 {
                    let sy = y + ky - 1;
                    let sx = x + kx - 1;
                    if sy < 0 || sy >= h || sx < 0 || sx >= w {
                        continue; // zero padding, exactly as fabric does
                    }
                    let k = SHARPEN_KERNEL[(ky * 3 + kx) as usize];
                    let p = src.get_pixel(sx as u32, sy as u32);
                    for (c, a) in acc.iter_mut().enumerate() {
                        *a += p[c] as f64 * k;
                    }
                }
            }
            let out = img.get_pixel_mut(x as u32, y as u32);
            for (c, a) in acc.iter().enumerate() {
                out[c] = to_u8(*a);
            }
        }
    }
}

/// A Gaussian kernel, sigma 1.0, sampled at -2..=2 and normalised to 1.0.
///
/// Literal rather than computed so the numbers are reviewable.
const GAUSS_5: [f64; 5] = [0.06136, 0.24477, 0.38774, 0.24477, 0.06136];

/// Our blur. **Not the vendor's**, and it cannot be.
///
/// Fabric's 2-D blur is `simpleBlur`: it draws the image onto two scratch
/// canvases twenty-one times per axis with `globalAlpha` compositing and a
/// `Math.random()` jitter on every pass. It is not reproducible outside a
/// browser and it is not deterministic inside one -- blurring the same image
/// twice gives two different results.
///
/// Same situation PROTOCOL.md already records for the vendor's GIF frame
/// resampling, and the same answer: do the honest thing and say so. This is a
/// separable Gaussian, clamped at the edges (zero padding would darken the
/// border, which on a 160x96 panel reads as a frame around the picture).
fn blur(img: &mut RgbaImage) {
    let (w, h) = (img.width() as i64, img.height() as i64);

    // Horizontal pass.
    let src = img.clone();
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f64; 3];
            for (i, k) in GAUSS_5.iter().enumerate() {
                let sx = (x + i as i64 - 2).clamp(0, w - 1);
                let p = src.get_pixel(sx as u32, y as u32);
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += p[c] as f64 * k;
                }
            }
            let out = img.get_pixel_mut(x as u32, y as u32);
            for (c, a) in acc.iter().enumerate() {
                out[c] = to_u8(*a);
            }
        }
    }

    // Vertical pass.
    let src = img.clone();
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0.0f64; 3];
            for (i, k) in GAUSS_5.iter().enumerate() {
                let sy = (y + i as i64 - 2).clamp(0, h - 1);
                let p = src.get_pixel(x as u32, sy as u32);
                for (c, a) in acc.iter_mut().enumerate() {
                    *a += p[c] as f64 * k;
                }
            }
            let out = img.get_pixel_mut(x as u32, y as u32);
            for (c, a) in acc.iter().enumerate() {
                out[c] = to_u8(*a);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    fn img(pixels: &[[u8; 4]], w: u32, h: u32) -> RgbaImage {
        let mut i = RgbaImage::new(w, h);
        for (n, px) in pixels.iter().enumerate() {
            i.put_pixel(n as u32 % w, n as u32 / w, Rgba(*px));
        }
        i
    }

    fn one(px: [u8; 4]) -> RgbaImage {
        img(&[px], 1, 1)
    }

    // --- The two conversions ---

    /// Ties to even, which is neither of Rust's rounding modes.
    #[test]
    fn to_u8_rounds_ties_to_even() {
        assert_eq!(to_u8(0.5), 0, "0.5 -> 0, not 1");
        assert_eq!(to_u8(1.5), 2);
        assert_eq!(to_u8(2.5), 2, "2.5 -> 2, not 3");
        assert_eq!(to_u8(3.5), 4);
        // Away from the ties it is ordinary rounding.
        assert_eq!(to_u8(1.4), 1);
        assert_eq!(to_u8(1.6), 2);
    }

    #[test]
    fn to_u8_saturates_at_both_ends() {
        assert_eq!(to_u8(-1.0), 0);
        assert_eq!(to_u8(-1000.0), 0);
        assert_eq!(to_u8(255.0), 255);
        assert_eq!(to_u8(1000.0), 255);
        assert_eq!(to_u8(f64::NAN), 0, "NaN becomes 0, as in JS");
    }

    /// `Math.round` is floor(x + 0.5), so it differs from Rust's `round` for
    /// negative halves -- which brightness hits at v = -0.5/255 boundaries.
    #[test]
    fn js_round_sends_halves_toward_positive_infinity() {
        assert_eq!(js_round(0.5), 1.0);
        assert_eq!(js_round(-0.5), 0.0, "Rust's round would give -1");
        assert_eq!(js_round(-1.5), -1.0, "Rust's round would give -2");
        assert_eq!(js_round(2.4), 2.0);
    }

    // --- Each filter against hand-computed values ---

    #[test]
    fn brightness_adds_a_rounded_offset() {
        // 0.5 * 255 = 127.5, floor(127.5 + 0.5) = 128.
        let mut i = one([10, 20, 30, 255]);
        brightness(&mut i, 0.5);
        assert_eq!(i.get_pixel(0, 0).0, [138, 148, 158, 255]);
    }

    #[test]
    fn brightness_saturates_rather_than_wrapping() {
        let mut i = one([200, 10, 0, 255]);
        brightness(&mut i, 1.0); // +255
        assert_eq!(i.get_pixel(0, 0).0, [255, 255, 255, 255]);

        let mut i = one([200, 10, 0, 255]);
        brightness(&mut i, -1.0); // -255
        assert_eq!(i.get_pixel(0, 0).0, [0, 0, 0, 255]);
    }

    #[test]
    fn chroma_warms_and_cools() {
        // v = 0.5: r * 1.5, g * 1.25, b * 0.5
        let mut i = one([100, 100, 100, 255]);
        chroma(&mut i, 0.5);
        assert_eq!(i.get_pixel(0, 0).0, [150, 125, 50, 255]);

        // v = -0.5 the other way: r * 0.5, g * 0.75, b * 1.5
        let mut i = one([100, 100, 100, 255]);
        chroma(&mut i, -0.5);
        assert_eq!(i.get_pixel(0, 0).0, [50, 75, 150, 255]);
    }

    #[test]
    fn saturation_pushes_channels_away_from_the_brightest() {
        // max = 200. w = -1. r: 100 + (200-100)*-1 = 0. b untouched (is max).
        let mut i = one([100, 150, 200, 255]);
        saturation(&mut i, 1.0);
        assert_eq!(i.get_pixel(0, 0).0, [0, 100, 200, 255]);

        // Negative pulls them toward the max instead.
        let mut i = one([100, 150, 200, 255]);
        saturation(&mut i, -1.0);
        assert_eq!(i.get_pixel(0, 0).0, [200, 200, 200, 255]);
    }

    #[test]
    fn grayscale_averages_the_three_channels() {
        let mut i = one([10, 20, 60, 255]);
        grayscale(&mut i); // (10+20+60)/3 = 30
        assert_eq!(i.get_pixel(0, 0).0, [30, 30, 30, 255]);
    }

    /// A flat field is unchanged in the interior, because the kernel sums to 1.
    #[test]
    fn sharpen_leaves_a_flat_interior_alone() {
        let mut i = RgbaImage::from_pixel(5, 5, Rgba([100, 100, 100, 255]));
        sharpen(&mut i);
        assert_eq!(
            i.get_pixel(2, 2).0,
            [100, 100, 100, 255],
            "centre of a flat field must not move"
        );
    }

    /// Edges are zero-padded, so the dropped taps are negative and the border
    /// comes out brighter. This is fabric's behaviour, not a bug in ours.
    #[test]
    fn sharpen_brightens_the_border_because_edges_are_zero_padded() {
        let mut i = RgbaImage::from_pixel(5, 5, Rgba([100, 100, 100, 255]));
        sharpen(&mut i);
        // Corner: centre 5*100, two in-bounds neighbours -1*100 each,
        // two out-of-bounds skipped -> 300.
        assert_eq!(i.get_pixel(0, 0).0[0], 255, "300 clamps to 255");

        // Same maths at a lower level, below the clamp: 5*50 - 2*50 = 150.
        let mut i = RgbaImage::from_pixel(5, 5, Rgba([50, 50, 50, 255]));
        sharpen(&mut i);
        assert_eq!(i.get_pixel(0, 0).0[0], 150);
        assert_eq!(i.get_pixel(2, 2).0[0], 50, "interior still flat");
    }

    #[test]
    fn sharpen_increases_local_contrast() {
        // A dark field with one bright pixel: the bright pixel gets brighter.
        let mut i = RgbaImage::from_pixel(5, 5, Rgba([10, 10, 10, 255]));
        i.put_pixel(2, 2, Rgba([100, 100, 100, 255]));
        sharpen(&mut i);
        // 5*100 - 4*10 = 460 -> clamped.
        assert_eq!(i.get_pixel(2, 2).0[0], 255);
        // Its neighbour loses: 5*10 - 100 - 3*10 = -80 -> 0.
        assert_eq!(i.get_pixel(2, 1).0[0], 0);
    }

    #[test]
    fn blur_leaves_a_flat_field_alone() {
        let mut i = RgbaImage::from_pixel(6, 6, Rgba([120, 120, 120, 255]));
        blur(&mut i);
        for px in i.pixels() {
            assert_eq!(px.0[0], 120, "a normalised kernel preserves a flat field");
        }
    }

    #[test]
    fn blur_softens_an_edge() {
        let mut i = RgbaImage::new(6, 1);
        for x in 0..6 {
            i.put_pixel(x, 0, Rgba([if x < 3 { 0 } else { 255 }, 0, 0, 255]));
        }
        blur(&mut i);
        let v: Vec<u8> = (0..6).map(|x| i.get_pixel(x, 0).0[0]).collect();
        assert!(v[2] > 0, "the dark side of the edge lifts: {v:?}");
        assert!(v[3] < 255, "and the light side falls: {v:?}");
        assert!(
            v[0] < v[2] && v[3] < v[5],
            "monotonic across the edge: {v:?}"
        );
    }

    // --- Alpha ---

    /// The rule that keeps Milestone 3's picture behaviour intact.
    #[test]
    fn no_adjustment_ever_writes_to_alpha() {
        let all = Adjustments {
            brightness: 0.4,
            chroma: -0.3,
            saturation: 0.7,
            grayscale: true,
            sharpen: true,
            blur: true,
        };
        let mut i = img(
            &[
                [10, 200, 30, 0],
                [40, 50, 60, 128],
                [70, 80, 90, 255],
                [1, 2, 3, 7],
            ],
            2,
            2,
        );
        let before: Vec<u8> = i.pixels().map(|p| p.0[3]).collect();
        all.apply(&mut i);
        let after: Vec<u8> = i.pixels().map(|p| p.0[3]).collect();
        assert_eq!(
            before, after,
            "alpha decides which pixels the encoder blackens; filters must not move it"
        );
    }

    // --- Composition ---

    #[test]
    fn identity_changes_nothing_and_does_no_work() {
        let a = Adjustments::NONE;
        assert!(a.is_identity());
        assert_eq!(a.summary(), None);

        let original = img(&[[10, 20, 30, 255], [200, 100, 50, 128]], 2, 1);
        let mut i = original.clone();
        a.apply(&mut i);
        assert_eq!(i.as_raw(), original.as_raw());
    }

    /// The documented order, pinned by its exact output.
    ///
    /// Not "the reverse order differs" -- that still passes when the chain is
    /// reordered wrongly. This is the number the documented chain produces.
    #[test]
    fn the_documented_order_produces_exactly_this() {
        let a = Adjustments {
            brightness: 0.2,
            saturation: 0.5,
            ..Adjustments::NONE
        };
        let mut i = one([100, 150, 200, 255]);
        a.apply(&mut i);

        // By hand, brightness then saturation:
        //   offset = floor(0.2*255 + 0.5) = 51 -> [151, 201, 251]
        //   max = 251, w = -0.5
        //   r: 151 + (251-151)*-0.5 = 101
        //   g: 201 + (251-201)*-0.5 = 176
        //   b: unchanged (is max)
        assert_eq!(i.get_pixel(0, 0).0, [101, 176, 251, 255]);

        // Brightness and saturation happen to commute exactly: adding a
        // constant shifts `max` by the same amount, so `(max - c)` -- the
        // only thing saturation reads -- is unchanged. Worth knowing, and
        // worth not relying on for an ordering test.
    }

    /// A pair that genuinely does not commute, so the fixed order is load
    /// bearing rather than decorative.
    #[test]
    fn colour_before_grayscale_is_a_different_picture_from_the_reverse() {
        let a = Adjustments {
            chroma: 0.5,
            grayscale: true,
            ..Adjustments::NONE
        };
        let mut documented = one([100, 150, 200, 255]);
        a.apply(&mut documented);

        // Chroma first: r 100*1.5 = 150, g 150*1.25 = 187.5 -> 188 (ties to
        // even, 187 is odd), b 200*0.5 = 100. Then grayscale:
        // (150 + 188 + 100) / 3 = 146.
        assert_eq!(documented.get_pixel(0, 0).0, [146, 146, 146, 255]);

        let mut reversed = one([100, 150, 200, 255]);
        grayscale(&mut reversed);
        chroma(&mut reversed, 0.5);
        // Grayscale first flattens to 150, then chroma re-colours it.
        assert_eq!(reversed.get_pixel(0, 0).0, [225, 188, 75, 255]);
        assert_ne!(documented.get_pixel(0, 0).0, reversed.get_pixel(0, 0).0);
    }

    #[test]
    fn summary_lists_only_what_is_on() {
        let a = Adjustments {
            brightness: -0.25,
            grayscale: true,
            ..Adjustments::NONE
        };
        let s = a.summary().unwrap();
        assert!(s.contains("brightness -0.25"), "got {s}");
        assert!(s.contains("grayscale"), "got {s}");
        assert!(!s.contains("chroma"), "got {s}");
        assert!(!s.contains("blur"), "got {s}");
    }

    /// The whole documented chain, on a flat field so the two spatial filters
    /// are computable by hand.
    ///
    /// A flat field is exactly what makes this checkable: both sharpen and
    /// blur use normalised kernels, so an interior pixel of a uniform image
    /// passes through them untouched, and the first four stages can be
    /// followed on paper.
    #[test]
    fn all_six_stages_in_order_produce_exactly_this() {
        let a = Adjustments {
            brightness: 0.2,
            chroma: 0.5,
            saturation: 0.5,
            grayscale: true,
            sharpen: true,
            blur: true,
        };
        let mut i = RgbaImage::from_pixel(7, 7, Rgba([100, 100, 100, 255]));
        a.apply(&mut i);

        // brightness: +floor(0.2*255 + 0.5) = +51        -> 151,151,151
        // chroma 0.5: r*1.5 = 226.5 -> 226 (tie, 226 even)
        //             g*1.25 = 188.75 -> 189
        //             b*0.5 = 75.5 -> 76 (tie, 75 odd)
        // saturation 0.5: w = -0.5, max = 226
        //             g: 189 + (226-189)*-0.5 = 170.5 -> 170 (tie, even)
        //             b: 76 + (226-76)*-0.5 = 1
        // grayscale: (226 + 170 + 1)/3 = 132.33 -> 132
        // sharpen, blur: normalised kernels over a flat field -> unchanged
        assert_eq!(i.get_pixel(3, 3).0, [132, 132, 132, 255]);
    }

    /// Chroma can push a channel past both ends at once.
    #[test]
    fn chroma_clamps_at_both_ends() {
        let mut i = one([200, 10, 200, 255]);
        chroma(&mut i, 1.0); // r*2 = 400, b*0 = 0
        assert_eq!(i.get_pixel(0, 0).0, [255, 15, 0, 255]);
    }

    /// Saturation overshoots below zero on the way out, and saturates the lot
    /// on the way in.
    #[test]
    fn saturation_clamps_at_both_ends() {
        // Pushing away from the max drives the darkest channel negative.
        let mut i = one([10, 10, 250, 255]);
        saturation(&mut i, 1.0); // 10 + (250-10)*-1 = -230
        assert_eq!(i.get_pixel(0, 0).0, [0, 0, 250, 255]);

        // Pulling toward it flattens everything onto the max.
        let mut i = one([0, 120, 255, 255]);
        saturation(&mut i, -1.0);
        assert_eq!(i.get_pixel(0, 0).0, [255, 255, 255, 255]);
    }

    /// Blur cannot overshoot: its kernel is normalised and every weight is
    /// positive, so a result always lies between the darkest and brightest
    /// input it saw. Asserted as staying strictly inside an extreme input
    /// rather than "<= 255", which a `u8` guarantees for free.
    #[test]
    fn blur_stays_between_the_inputs_it_saw() {
        let mut i = RgbaImage::new(6, 1);
        for x in 0..6 {
            i.put_pixel(x, 0, Rgba([if x % 2 == 0 { 0 } else { 255 }, 0, 0, 255]));
        }
        blur(&mut i);
        let v: Vec<u8> = (0..6).map(|x| i.get_pixel(x, 0).0[0]).collect();
        assert!(
            v.iter().any(|c| *c > 0 && *c < 255),
            "alternating black and white must average into the middle: {v:?}"
        );
    }

    /// The blur's exact output, so the weights, the edge clamp and the
    /// two-pass rounding are all pinned rather than described.
    ///
    /// A single bright pixel in a 5x1 row. The horizontal pass spreads it by
    /// the kernel; the vertical pass sees one row, clamps every sample to it,
    /// and the weights sum to one, so it returns what it was given.
    #[test]
    fn blur_produces_exactly_the_specified_kernel() {
        let mut i = RgbaImage::new(5, 1);
        for x in 0..5 {
            i.put_pixel(x, 0, Rgba([if x == 2 { 255 } else { 0 }, 0, 0, 255]));
        }
        blur(&mut i);

        // 255 * each weight, rounded: .06136 -> 15.65 -> 16,
        // .24477 -> 62.4 -> 62, .38774 -> 98.9 -> 99.
        let got: Vec<u8> = (0..5).map(|x| i.get_pixel(x, 0).0[0]).collect();
        assert_eq!(got, vec![16, 62, 99, 62, 16]);
    }

    /// Edge clamping, isolated: a bright pixel in the corner has its missing
    /// neighbours filled by repeating itself, so it keeps more of its value
    /// than the same pixel would in the middle.
    #[test]
    fn blur_clamps_at_the_edge_rather_than_darkening_it() {
        let mut edge = RgbaImage::new(5, 1);
        edge.put_pixel(0, 0, Rgba([255, 0, 0, 255]));
        for x in 1..5 {
            edge.put_pixel(x, 0, Rgba([0, 0, 0, 255]));
        }
        blur(&mut edge);

        // Two taps to its left are clamped back onto itself, so it keeps
        // three weights instead of one:
        // 255 * (.06136 + .24477 + .38774) = 176.94 -> 177.
        assert_eq!(edge.get_pixel(0, 0).0[0], 177);
        assert!(
            edge.get_pixel(0, 0).0[0] > 99,
            "zero padding would have given the 99 a centre pixel gets"
        );
    }

    /// Sharpen runs before blur, pinned on an image where the two do not
    /// commute.
    ///
    /// The six-stage test above uses a flat field, where both spatial kernels
    /// are identities -- so it cannot see this pair swapped. This can.
    #[test]
    fn sharpen_runs_before_blur() {
        let step = || {
            let mut i = RgbaImage::new(6, 6);
            for y in 0..6 {
                for x in 0..6 {
                    let v = if x < 3 { 20 } else { 200 };
                    i.put_pixel(x, y, Rgba([v, v, v, 255]));
                }
            }
            i
        };

        let mut documented = step();
        Adjustments {
            sharpen: true,
            blur: true,
            ..Adjustments::NONE
        }
        .apply(&mut documented);

        let mut by_hand = step();
        sharpen(&mut by_hand);
        blur(&mut by_hand);
        assert_eq!(
            documented.as_raw(),
            by_hand.as_raw(),
            "apply() must sharpen first"
        );

        let mut reversed = step();
        blur(&mut reversed);
        sharpen(&mut reversed);
        assert_ne!(
            documented.as_raw(),
            reversed.as_raw(),
            "the two orders must be distinguishable, or the test proves nothing"
        );
    }

    /// A transparent pixel must not leak its hidden colour into its visible
    /// neighbours.
    ///
    /// The encoder turns alpha 0 into black, so the colour underneath is
    /// never displayed -- but a spatial filter would happily sample it and
    /// spread it into pixels that *are* displayed. Transparent red beside
    /// opaque black would come out pink.
    #[test]
    fn a_transparent_pixel_does_not_bleed_into_its_neighbours() {
        let mut i = RgbaImage::new(3, 1);
        i.put_pixel(0, 0, Rgba([255, 0, 0, 0])); // invisible red
        i.put_pixel(1, 0, Rgba([0, 0, 0, 255])); // visible black
        i.put_pixel(2, 0, Rgba([0, 0, 0, 255]));

        Adjustments {
            blur: true,
            ..Adjustments::NONE
        }
        .apply(&mut i);

        assert_eq!(
            i.get_pixel(1, 0).0[0],
            0,
            "the hidden red must not reach a visible pixel"
        );
        // And the transparent pixel keeps its alpha, so the encoder still
        // knows to blacken it.
        assert_eq!(i.get_pixel(0, 0).0[3], 0);
    }
}
