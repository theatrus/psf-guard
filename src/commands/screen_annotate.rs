//! Diagnostic renderings for `screen-fits`: an annotated PNG per flagged
//! frame showing *why* it scored low, so verdicts can be verified visually.
//!
//! The stretched frame is downscaled and overlaid with the analysis grid;
//! cells are tinted by the signal that fired:
//!
//! - RED: dead cell (star density collapsed — occlusion)
//! - ORANGE: localized extinction (matched stars dimmed — small cloud),
//!   labeled with the cell's relative flux ratio
//! - MAGENTA: transient drop of the cell's star share vs its own history
//! - YELLOW frame: transient background rise (errant light)
//!
//! A caption strip carries the verdict, score, category, per-frame metric
//! values and the classifier's explanation, rendered with a built-in 5x7
//! bitmap font (no font-file dependency).

use anyhow::Result;
use image::{ImageBuffer, Rgb};
use std::path::Path;

use crate::image_analysis::FitsImage;
use seiza_stretch::{stretch_u16_to_u16, StretchParams};

/// Per-frame data the renderer needs beyond the FITS itself.
pub(crate) struct AnnotationData {
    pub grid_cols: usize,
    pub grid_rows: usize,
    /// Raw star counts per cell (dead-cell tint is derived from these the
    /// same way the metric is: count < 0.25 x median cell count).
    pub star_cell_counts: Vec<f64>,
    pub cell_relative_ratios: Vec<Option<f64>>,
    pub star_drop_cells: Vec<bool>,
    pub bg_rise_cells: Vec<bool>,
    pub bg_fall_cells: Vec<bool>,
    /// Static glow cells (within-frame plane residual above threshold).
    pub bg_glow_cells: Vec<bool>,
    pub caption_lines: Vec<String>,
}

const DOWNSCALE: usize = 4;
const CAPTION_HEIGHT: u32 = 88;
const RED: Rgb<u8> = Rgb([235, 60, 60]);
const ORANGE: Rgb<u8> = Rgb([245, 160, 40]);
const MAGENTA: Rgb<u8> = Rgb([225, 80, 225]);
const YELLOW: Rgb<u8> = Rgb([240, 230, 60]);
const BLUE: Rgb<u8> = Rgb([80, 150, 255]);
const CYAN: Rgb<u8> = Rgb([70, 210, 210]);
const GRID_GRAY: Rgb<u8> = Rgb([90, 90, 90]);
const TEXT_WHITE: Rgb<u8> = Rgb([230, 230, 230]);

/// Render the annotated diagnostic PNG for one frame.
pub(crate) fn render_annotated_frame(
    fits: &FitsImage,
    data: &AnnotationData,
    out_path: &Path,
) -> Result<()> {
    let stats = fits.calculate_basic_statistics();
    let stretch_params = StretchParams::default();
    let stretched = stretch_u16_to_u16(&fits.data, &stats.to_stretch_statistics(), &stretch_params);

    let out_w = (fits.width / DOWNSCALE).max(1);
    let out_h = (fits.height / DOWNSCALE).max(1);
    let mut img = ImageBuffer::<Rgb<u8>, Vec<u8>>::new(out_w as u32, out_h as u32 + CAPTION_HEIGHT);

    // Downscaled stretched grayscale (point sampling is fine for diagnostics).
    for y in 0..out_h {
        for x in 0..out_w {
            let v = (stretched[(y * DOWNSCALE) * fits.width + x * DOWNSCALE] >> 8) as u8;
            img.put_pixel(x as u32, y as u32 + CAPTION_HEIGHT, Rgb([v, v, v]));
        }
    }

    let (cols, rows) = (data.grid_cols.max(1), data.grid_rows.max(1));
    let cell_w = out_w as f64 / cols as f64;
    let cell_h = out_h as f64 / rows as f64;

    // Dead cells from raw counts, mirroring star_grid_metrics including its
    // sparse branch: when most cells are empty the median is 0 and the dead
    // criterion becomes "no stars at all" (a mostly-occluded frame must
    // still tint red).
    let dead_cells: Vec<bool> = if data.star_cell_counts.len() == cols * rows {
        let mut sorted = data.star_cell_counts.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let median = sorted[sorted.len() / 2];
        let any_stars = sorted.last().copied().unwrap_or(0.0) > 0.0;
        data.star_cell_counts
            .iter()
            .map(|&c| {
                if median > 0.0 {
                    c < 0.25 * median
                } else {
                    any_stars && c == 0.0
                }
            })
            .collect()
    } else {
        vec![false; cols * rows]
    };

    let at = |v: &Vec<bool>, c: usize| v.get(c).copied().unwrap_or(false);
    for gy in 0..rows {
        for gx in 0..cols {
            let c = gy * cols + gx;
            let x0 = (gx as f64 * cell_w) as u32;
            let y0 = (gy as f64 * cell_h) as u32 + CAPTION_HEIGHT;
            let x1 = (((gx + 1) as f64 * cell_w) as u32).min(out_w as u32);
            let y1 = ((((gy + 1) as f64 * cell_h) as u32) + CAPTION_HEIGHT)
                .min(out_h as u32 + CAPTION_HEIGHT);

            let extinct = data
                .cell_relative_ratios
                .get(c)
                .copied()
                .flatten()
                .filter(|&r| r < 0.75);

            // Tint priority: dead (strongest evidence) > extinction >
            // star-share drop > static glow.
            let tint = if at(&dead_cells, c) {
                Some(RED)
            } else if extinct.is_some() {
                Some(ORANGE)
            } else if at(&data.star_drop_cells, c) {
                Some(MAGENTA)
            } else if at(&data.bg_glow_cells, c) {
                Some(CYAN)
            } else {
                None
            };
            if let Some(color) = tint {
                blend_rect(&mut img, x0, y0, x1, y1, color, 0.35);
            }
            if at(&data.bg_rise_cells, c) {
                draw_rect_border(&mut img, x0, y0, x1, y1, YELLOW, 3);
            }
            if at(&data.bg_fall_cells, c) {
                draw_rect_border(&mut img, x0, y0, x1, y1, BLUE, 3);
            }
            if let Some(r) = extinct {
                draw_text(&mut img, x0 + 6, y0 + 6, &format!("X{:.2}", r), ORANGE, 2);
            }

            // Grid lines.
            draw_rect_border(&mut img, x0, y0, x1, y1, GRID_GRAY, 1);
        }
    }

    // Caption strip.
    for y in 0..CAPTION_HEIGHT {
        for x in 0..out_w as u32 {
            img.put_pixel(x, y, Rgb([12, 12, 12]));
        }
    }
    for (i, line) in data.caption_lines.iter().take(4).enumerate() {
        draw_text(&mut img, 8, 6 + i as u32 * 20, line, TEXT_WHITE, 2);
    }
    // Legend on the last caption row.
    let legend_y = 6 + 3 * 20;
    let mut lx = 8u32;
    for (color, label) in [
        (RED, "DEAD"),
        (ORANGE, "EXTINCT"),
        (MAGENTA, "STARDROP"),
        (YELLOW, "BGRISE"),
        (BLUE, "BGFALL"),
        (CYAN, "GLOW"),
    ] {
        for dy in 0..12u32 {
            for dx in 0..12u32 {
                if lx + dx < img.width() {
                    img.put_pixel(lx + dx, legend_y + dy, color);
                }
            }
        }
        draw_text(&mut img, lx + 16, legend_y, label, TEXT_WHITE, 2);
        lx += 16 + (label.len() as u32) * 12 + 18;
    }

    img.save(out_path)?;
    Ok(())
}

fn blend_rect(
    img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    color: Rgb<u8>,
    alpha: f64,
) {
    for y in y0..y1.min(img.height()) {
        for x in x0..x1.min(img.width()) {
            let p = img.get_pixel(x, y);
            let blend = |a: u8, b: u8| ((a as f64) * (1.0 - alpha) + (b as f64) * alpha) as u8;
            img.put_pixel(
                x,
                y,
                Rgb([
                    blend(p[0], color[0]),
                    blend(p[1], color[1]),
                    blend(p[2], color[2]),
                ]),
            );
        }
    }
}

fn draw_rect_border(
    img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x0: u32,
    y0: u32,
    x1: u32,
    y1: u32,
    color: Rgb<u8>,
    thickness: u32,
) {
    let (w, h) = (img.width(), img.height());
    for t in 0..thickness {
        for x in x0..x1.min(w) {
            if y0 + t < h {
                img.put_pixel(x, y0 + t, color);
            }
            if y1 > t + 1 && y1 - 1 - t < h {
                img.put_pixel(x, y1 - 1 - t, color);
            }
        }
        for y in y0..y1.min(h) {
            if x0 + t < w {
                img.put_pixel(x0 + t, y, color);
            }
            if x1 > t + 1 && x1 - 1 - t < w {
                img.put_pixel(x1 - 1 - t, y, color);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Minimal built-in 5x7 bitmap font (uppercase, digits, basic punctuation) so
// captions need no font-file dependency.
// ---------------------------------------------------------------------------

fn glyph(ch: char) -> [u8; 7] {
    // Each byte is one row, low 5 bits used (MSB-left).
    match ch.to_ascii_uppercase() {
        'A' => [0x0E, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'B' => [0x1E, 0x11, 0x11, 0x1E, 0x11, 0x11, 0x1E],
        'C' => [0x0E, 0x11, 0x10, 0x10, 0x10, 0x11, 0x0E],
        'D' => [0x1E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x1E],
        'E' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x1F],
        'F' => [0x1F, 0x10, 0x10, 0x1E, 0x10, 0x10, 0x10],
        'G' => [0x0E, 0x11, 0x10, 0x17, 0x11, 0x11, 0x0F],
        'H' => [0x11, 0x11, 0x11, 0x1F, 0x11, 0x11, 0x11],
        'I' => [0x0E, 0x04, 0x04, 0x04, 0x04, 0x04, 0x0E],
        'J' => [0x07, 0x02, 0x02, 0x02, 0x02, 0x12, 0x0C],
        'K' => [0x11, 0x12, 0x14, 0x18, 0x14, 0x12, 0x11],
        'L' => [0x10, 0x10, 0x10, 0x10, 0x10, 0x10, 0x1F],
        'M' => [0x11, 0x1B, 0x15, 0x15, 0x11, 0x11, 0x11],
        'N' => [0x11, 0x19, 0x15, 0x13, 0x11, 0x11, 0x11],
        'O' => [0x0E, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'P' => [0x1E, 0x11, 0x11, 0x1E, 0x10, 0x10, 0x10],
        'Q' => [0x0E, 0x11, 0x11, 0x11, 0x15, 0x12, 0x0D],
        'R' => [0x1E, 0x11, 0x11, 0x1E, 0x14, 0x12, 0x11],
        'S' => [0x0F, 0x10, 0x10, 0x0E, 0x01, 0x01, 0x1E],
        'T' => [0x1F, 0x04, 0x04, 0x04, 0x04, 0x04, 0x04],
        'U' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x11, 0x0E],
        'V' => [0x11, 0x11, 0x11, 0x11, 0x11, 0x0A, 0x04],
        'W' => [0x11, 0x11, 0x11, 0x15, 0x15, 0x1B, 0x11],
        'X' => [0x11, 0x0A, 0x04, 0x04, 0x04, 0x0A, 0x11],
        'Y' => [0x11, 0x11, 0x0A, 0x04, 0x04, 0x04, 0x04],
        'Z' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x10, 0x1F],
        '0' => [0x0E, 0x11, 0x13, 0x15, 0x19, 0x11, 0x0E],
        '1' => [0x04, 0x0C, 0x04, 0x04, 0x04, 0x04, 0x0E],
        '2' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x08, 0x1F],
        '3' => [0x1E, 0x01, 0x01, 0x0E, 0x01, 0x01, 0x1E],
        '4' => [0x02, 0x06, 0x0A, 0x12, 0x1F, 0x02, 0x02],
        '5' => [0x1F, 0x10, 0x1E, 0x01, 0x01, 0x11, 0x0E],
        '6' => [0x06, 0x08, 0x10, 0x1E, 0x11, 0x11, 0x0E],
        '7' => [0x1F, 0x01, 0x02, 0x04, 0x08, 0x08, 0x08],
        '8' => [0x0E, 0x11, 0x11, 0x0E, 0x11, 0x11, 0x0E],
        '9' => [0x0E, 0x11, 0x11, 0x0F, 0x01, 0x02, 0x0C],
        '.' => [0x00, 0x00, 0x00, 0x00, 0x00, 0x0C, 0x0C],
        ',' => [0x00, 0x00, 0x00, 0x00, 0x0C, 0x04, 0x08],
        ':' => [0x00, 0x0C, 0x0C, 0x00, 0x0C, 0x0C, 0x00],
        '%' => [0x18, 0x19, 0x02, 0x04, 0x08, 0x13, 0x03],
        '-' => [0x00, 0x00, 0x00, 0x1F, 0x00, 0x00, 0x00],
        '=' => [0x00, 0x00, 0x1F, 0x00, 0x1F, 0x00, 0x00],
        '/' => [0x01, 0x01, 0x02, 0x04, 0x08, 0x10, 0x10],
        '(' => [0x02, 0x04, 0x08, 0x08, 0x08, 0x04, 0x02],
        ')' => [0x08, 0x04, 0x02, 0x02, 0x02, 0x04, 0x08],
        '?' => [0x0E, 0x11, 0x01, 0x02, 0x04, 0x00, 0x04],
        '+' => [0x00, 0x04, 0x04, 0x1F, 0x04, 0x04, 0x00],
        '\'' => [0x04, 0x04, 0x08, 0x00, 0x00, 0x00, 0x00],
        _ => [0x00; 7], // space / unsupported
    }
}

/// Draw ASCII text with the built-in font at integer `scale`.
fn draw_text(
    img: &mut ImageBuffer<Rgb<u8>, Vec<u8>>,
    x: u32,
    y: u32,
    text: &str,
    color: Rgb<u8>,
    scale: u32,
) {
    let mut cx = x;
    for ch in text.chars() {
        let rows = glyph(ch);
        for (ry, row) in rows.iter().enumerate() {
            for rx in 0..5u32 {
                if row & (0x10 >> rx) != 0 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cx + rx * scale + sx;
                            let py = y + ry as u32 * scale + sy;
                            if px < img.width() && py < img.height() {
                                img.put_pixel(px, py, color);
                            }
                        }
                    }
                }
            }
        }
        cx += 6 * scale;
        if cx >= img.width() {
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_annotated_png() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("frame.reject.png");
        let fits = FitsImage {
            width: 800,
            height: 600,
            data: vec![500u16; 800 * 600],
            raw_min: 0.0,
            raw_scale: 1.0,
            bzero: 0.0,
        };
        let mut star_cells = vec![100.0; 48];
        star_cells[0] = 0.0; // dead cell -> red tint
        let mut ratios = vec![Some(1.0); 48];
        ratios[5] = Some(0.5); // extinction cell -> orange tint + label
        let data = AnnotationData {
            grid_cols: 8,
            grid_rows: 6,
            star_cell_counts: star_cells,
            cell_relative_ratios: ratios,
            star_drop_cells: vec![false; 48],
            bg_rise_cells: {
                let mut v = vec![false; 48];
                v[10] = true; // yellow border
                v
            },
            bg_fall_cells: {
                let mut v = vec![false; 48];
                v[20] = true; // blue border
                v
            },
            bg_glow_cells: {
                let mut v = vec![false; 48];
                v[30] = true; // cyan tint
                v
            },
            caption_lines: vec![
                "REJECT CLOUDS SCORE=0.31".to_string(),
                "STARS=993 TRANSP=0.82 EXT=22%".to_string(),
                "44% of frame grid cells have no stars.".to_string(),
            ],
        };
        render_annotated_frame(&fits, &data, &out).unwrap();
        let meta = std::fs::metadata(&out).unwrap();
        assert!(meta.len() > 1000, "png should have content");
        // Decodes back with the caption strip added to the height.
        let img = image::open(&out).unwrap();
        assert_eq!(img.height(), 600 / DOWNSCALE as u32 + CAPTION_HEIGHT);
    }
}
