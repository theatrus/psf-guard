//! Builds the render crate's display transform from ICC profiles, with
//! Little CMS as the color engine.

use lcms2::{Intent, PixelFormat, Profile, Transform};
use psf_studio_render::display::DisplayCurves;
use std::path::Path;

pub const SYSTEM_PROFILES: &str = "/System/Library/ColorSync/Profiles";

/// Converts colors from one profile's encoded RGB to another's, unclamped.
pub fn convert(source: &Profile, target: &Profile, colors: &[[f32; 3]]) -> Vec<[f32; 3]> {
    let transform = Transform::new(
        source,
        PixelFormat::RGB_FLT,
        target,
        PixelFormat::RGB_FLT,
        Intent::RelativeColorimetric,
    )
    .expect("an RGB to RGB transform");
    let mut out = vec![[0f32; 3]; colors.len()];
    transform.transform_pixels(colors, &mut out);
    out
}

const CURVE_WIDTH: u32 = 4096;

/// What a profile does to each channel alone, measured through the engine:
/// the XYZ of each primary at full drive, and each channel's curve from
/// encoded value to linear light.
struct Measured {
    /// XYZ of the red, green and blue primaries.
    primaries: [[f64; 3]; 3],
    curves: Vec<[f64; 3]>,
}

fn measure(profile: &Profile) -> Measured {
    let transform: Transform<[f32; 3], [f32; 3]> = Transform::new(
        profile,
        PixelFormat::RGB_FLT,
        &Profile::new_xyz(),
        PixelFormat::XYZ_FLT,
        Intent::RelativeColorimetric,
    )
    .expect("an RGB to XYZ transform");
    let xyz = |colors: &[[f32; 3]]| {
        let mut out = vec![[0f32; 3]; colors.len()];
        transform.transform_pixels(colors, &mut out);
        out
    };
    let black = xyz(&[[0.0; 3]])[0];
    let full = xyz(&[[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let primaries: [[f64; 3]; 3] = std::array::from_fn(|channel| {
        std::array::from_fn(|axis| (full[channel][axis] - black[axis]) as f64)
    });
    let step = (CURVE_WIDTH - 1) as f32;
    let mut curves = vec![[0f64; 3]; CURVE_WIDTH as usize];
    for channel in 0..3 {
        let ramp: Vec<[f32; 3]> = (0..CURVE_WIDTH)
            .map(|index| {
                let mut color = [0.0; 3];
                color[channel] = index as f32 / step;
                color
            })
            .collect();
        // A channel alone moves along its primary, so its share of the
        // primary's luminance is its linear value.
        for (sample, measured) in curves.iter_mut().zip(xyz(&ramp)) {
            sample[channel] = ((measured[1] - black[1]) as f64 / primaries[channel][1]).max(0.0);
        }
    }
    Measured { primaries, curves }
}

fn invert(m: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    let cofactor = |r: usize, c: usize| {
        let (r1, r2, c1, c2) = ((r + 1) % 3, (r + 2) % 3, (c + 1) % 3, (c + 2) % 3);
        m[r1][c1] * m[r2][c2] - m[r1][c2] * m[r2][c1]
    };
    let determinant = (0..3).map(|c| m[0][c] * cofactor(0, c)).sum::<f64>();
    std::array::from_fn(|r| std::array::from_fn(|c| cofactor(c, r) / determinant))
}

/// Builds the curve, matrix, curve form of source to target.
pub fn curves(source: &Profile, target: &Profile) -> DisplayCurves {
    let (from, to) = (measure(source), measure(target));
    // `primaries[channel]` is a column of the RGB to XYZ matrix.
    let columns = |primaries: [[f64; 3]; 3]| -> [[f64; 3]; 3] {
        std::array::from_fn(|row| std::array::from_fn(|column| primaries[column][row]))
    };
    let (source_to_xyz, xyz_to_target) = (columns(from.primaries), invert(columns(to.primaries)));
    let to_target: [[f32; 3]; 3] = std::array::from_fn(|column| {
        std::array::from_fn(|row| {
            (0..3)
                .map(|k| xyz_to_target[row][k] * source_to_xyz[k][column])
                .sum::<f64>() as f32
        })
    });
    // Invert the target's curves: for each linear value, the encoded value
    // that produces it.
    let step = (CURVE_WIDTH - 1) as f64;
    let mut encode = vec![[0f32; 3]; CURVE_WIDTH as usize];
    for channel in 0..3 {
        let mut at = 0usize;
        for (index, sample) in encode.iter_mut().enumerate() {
            let linear = (index as f64 / step).powi(2);
            while at + 2 < CURVE_WIDTH as usize && to.curves[at + 1][channel] < linear {
                at += 1;
            }
            let (low, high) = (to.curves[at][channel], to.curves[at + 1][channel]);
            let fraction = if high > low {
                ((linear - low) / (high - low)).clamp(0.0, 1.0)
            } else {
                0.0
            };
            sample[channel] = ((at as f64 + fraction) / step) as f32;
        }
    }
    DisplayCurves {
        width: CURVE_WIDTH,
        decode: from
            .curves
            .iter()
            .map(|sample| sample.map(|value| value as f32))
            .collect(),
        encode,
        to_target,
    }
}

/// The curve, matrix, curve form between two profile files.
pub fn curves_between(source: &Path, target: &Path) -> Result<DisplayCurves, String> {
    let open = |path: &Path| {
        Profile::new_file(path).map_err(|error| format!("{}: {error}", path.display()))
    };
    Ok(curves(&open(source)?, &open(target)?))
}
