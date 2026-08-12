//! Turning a panel frame into terminal cells.
//!
//! The point of a preview is to answer "what will this look like on the
//! keyboard" before spending forty-five seconds finding out. So it renders the
//! frame the device will actually receive -- 160x96 RGB565, after the stretch
//! -- not the source file. If the stretch mangles your image, the preview
//! mangles it the same way, which is the useful behaviour.
//!
//! Deliberately free of ratatui types: bytes and a target size in, colours
//! out. That keeps it testable without a terminal, and the widget that draws
//! it stays trivial.

use crate::protocol;

/// One pixel, as the terminal wants it.
pub type Rgb = (u8, u8, u8);

/// A rendered preview: rows of cells, each carrying the two pixels it stacks.
///
/// Half-blocks are what make a terminal preview worth looking at. A cell is
/// about twice as tall as it is wide, so drawing one pixel per cell squashes
/// the image; drawing `▀` with the foreground set to the upper pixel and the
/// background to the lower gets two pixels into one cell and the aspect ratio
/// comes out right.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preview {
    pub rows: Vec<Vec<(Rgb, Rgb)>>,
}

#[cfg(test)]
impl Preview {
    fn width(&self) -> usize {
        self.rows.first().map_or(0, |r| r.len())
    }

    fn height(&self) -> usize {
        self.rows.len()
    }
}

/// The largest preview that fits in `max_w` x `max_h` cells while keeping the
/// panel's shape.
///
/// A full-size preview needs **160 columns by 48 rows** -- wider than most
/// terminals, which is why this exists rather than a fixed size. One cell is
/// one source column and two source rows, so preserving the panel's 160:96
/// means `width == rows * 10 / 3`.
///
/// Returns `None` when the space is too small to show anything honest. The
/// caller shows the metadata instead; a two-pixel-wide smear is worse than a
/// sentence.
pub fn fit(max_w: usize, max_h: usize) -> Option<(usize, usize)> {
    const MIN_H: usize = 4;

    if max_w == 0 || max_h == 0 {
        return None;
    }
    // h limited by both the height available and the width available.
    let h = max_h.min(max_w * 3 / 10);
    if h < MIN_H {
        return None;
    }
    let w = h * 10 / 3;
    if w == 0 { None } else { Some((w, h)) }
}

/// Renders `pixels` -- one panel frame, RGB565 big-endian -- into `w` x `h`
/// cells by nearest-neighbour sampling.
///
/// Nearest neighbour on purpose, and not for speed: it is what `set-picture`
/// and `set-gif` use to reach the panel in the first place, so a smoothed
/// preview would show something the keyboard will never display.
pub fn render(pixels: &[u8], w: usize, h: usize) -> Preview {
    let src_w = protocol::PANEL_W as usize;
    let src_h = protocol::PANEL_H as usize;

    let sample = |x: usize, y: usize| -> Rgb {
        // Map the cell grid onto the panel by centre, so a 1-cell preview
        // takes the middle pixel rather than the corner.
        let sx = (x * src_w + src_w / 2) / w.max(1);
        let sy = (y * src_h + src_h / 2) / (h * 2).max(1);
        let sx = sx.min(src_w - 1);
        let sy = sy.min(src_h - 1);
        let o = (sy * src_w + sx) * 2;
        if o + 1 >= pixels.len() {
            return (0, 0, 0);
        }
        rgb565_to_rgb(u16::from_be_bytes([pixels[o], pixels[o + 1]]))
    };

    let rows = (0..h)
        .map(|cy| {
            (0..w)
                .map(|cx| (sample(cx, cy * 2), sample(cx, cy * 2 + 1)))
                .collect()
        })
        .collect();

    Preview { rows }
}

/// Expands a 5/6/5 pixel back to 8 bits per channel.
///
/// The low bits are filled by repeating the high ones (`r << 3 | r >> 2`)
/// rather than zero-filling, so full white stays 255 instead of drifting to
/// 248 and making every preview look slightly grey.
fn rgb565_to_rgb(v: u16) -> Rgb {
    let r = ((v >> 11) & 0x1f) as u8;
    let g = ((v >> 5) & 0x3f) as u8;
    let b = (v & 0x1f) as u8;
    (
        (r << 3) | (r >> 2),
        (g << 2) | (g >> 4),
        (b << 3) | (b >> 2),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A panel frame filled with one colour.
    fn solid(v: u16) -> Vec<u8> {
        v.to_be_bytes()
            .iter()
            .cycle()
            .take(protocol::PICTURE_BYTES)
            .copied()
            .collect()
    }

    #[test]
    fn full_white_stays_white() {
        assert_eq!(rgb565_to_rgb(0xffff), (255, 255, 255));
        assert_eq!(rgb565_to_rgb(0x0000), (0, 0, 0));
        // Pure red/green/blue keep their channel at full.
        assert_eq!(rgb565_to_rgb(0xf800), (255, 0, 0));
        assert_eq!(rgb565_to_rgb(0x07e0), (0, 255, 0));
        assert_eq!(rgb565_to_rgb(0x001f), (0, 0, 255));
    }

    #[test]
    fn the_fit_keeps_the_panels_shape() {
        // Plenty of room: capped by height.
        assert_eq!(fit(200, 48), Some((160, 48)));
        // Narrow: capped by width. 60 columns -> 18 rows -> 60 columns.
        assert_eq!(fit(60, 48), Some((60, 18)));
        // Short.
        assert_eq!(fit(200, 10), Some((33, 10)));

        for (w, h) in [(200, 48), (60, 48), (200, 10), (40, 12)]
            .into_iter()
            .filter_map(|(a, b)| fit(a, b))
        {
            // Rendered aspect: w cells wide by h*2 pixels tall, against 160x96.
            let ratio = w as f64 / (h * 2) as f64;
            assert!(
                (ratio - 160.0 / 96.0).abs() < 0.15,
                "{w}x{h} distorts the panel: {ratio}"
            );
        }
    }

    #[test]
    fn a_space_too_small_gets_no_preview_rather_than_a_smear() {
        assert_eq!(fit(0, 40), None);
        assert_eq!(fit(40, 0), None);
        assert_eq!(fit(10, 40), None, "3 rows is not a picture");
        assert_eq!(fit(200, 3), None);
    }

    #[test]
    fn a_solid_frame_renders_as_that_colour_everywhere() {
        let p = render(&solid(0xf800), 20, 6);
        assert_eq!(p.width(), 20);
        assert_eq!(p.height(), 6);
        for row in &p.rows {
            for (upper, lower) in row {
                assert_eq!(*upper, (255, 0, 0));
                assert_eq!(*lower, (255, 0, 0));
            }
        }
    }

    /// Top and bottom halves land in the right cells, and each cell really is
    /// carrying two different pixels.
    #[test]
    fn the_two_halves_of_a_cell_are_two_different_rows() {
        let mut pixels = vec![0u8; protocol::PICTURE_BYTES];
        let w = protocol::PANEL_W as usize;
        for y in 0..protocol::PANEL_H as usize {
            // Alternate red and blue by source row.
            let v: u16 = if y % 2 == 0 { 0xf800 } else { 0x001f };
            for x in 0..w {
                let o = (y * w + x) * 2;
                pixels[o..o + 2].copy_from_slice(&v.to_be_bytes());
            }
        }

        // At full height every cell straddles one even and one odd row.
        let p = render(&pixels, 160, 48);
        for row in &p.rows {
            for (upper, lower) in row {
                assert_ne!(upper, lower, "a cell must show two distinct pixels");
            }
        }
    }

    /// Nearest neighbour, not smoothing: a hard edge stays hard.
    #[test]
    fn a_hard_edge_is_not_blurred() {
        let mut pixels = vec![0u8; protocol::PICTURE_BYTES];
        let w = protocol::PANEL_W as usize;
        for y in 0..protocol::PANEL_H as usize {
            for x in 0..w {
                let v: u16 = if x < w / 2 { 0xffff } else { 0x0000 };
                let o = (y * w + x) * 2;
                pixels[o..o + 2].copy_from_slice(&v.to_be_bytes());
            }
        }

        let p = render(&pixels, 40, 12);
        for row in &p.rows {
            for (i, (upper, _)) in row.iter().enumerate() {
                let expected = if i < 20 { (255, 255, 255) } else { (0, 0, 0) };
                assert_eq!(
                    *upper, expected,
                    "column {i} should be a clean edge, not a blend"
                );
            }
        }
    }

    /// Degenerate sizes must not panic or index out of bounds.
    #[test]
    fn tiny_and_empty_targets_are_safe() {
        let frame = solid(0x07e0);
        for (w, h) in [(1, 1), (1, 40), (40, 1), (0, 0), (0, 5), (5, 0)] {
            let p = render(&frame, w, h);
            assert_eq!(p.height(), h);
            if h > 0 {
                assert_eq!(p.width(), w);
            }
        }
    }

    /// A short buffer renders black rather than panicking. Defensive: the
    /// planner never produces one, but a preview must never take down the UI.
    #[test]
    fn a_truncated_frame_renders_black_instead_of_panicking() {
        let p = render(&[0xff, 0xff], 8, 4);
        assert_eq!(p.rows[3][7], ((0, 0, 0), (0, 0, 0)));
    }
}
