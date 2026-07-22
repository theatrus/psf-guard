//! Golden regression tests.
//!
//! Every hash below was produced by an implementation verified bit-exact
//! against OpenCV 4.13 (x86-64, AVX2/FMA dispatch) across synthetic star
//! fields, noise images and structured blob shapes — see the PR that
//! removed the OpenCV dependency for the parity harness. These tests lock
//! that behavior in place without needing OpenCV installed.
//!
//! The fixture generator uses only exactly-rounded IEEE operations (no
//! transcendentals), so images are identical on every platform. The f32
//! results assume fused multiply-add hardware (any x86-64 with AVX2, all
//! aarch64); pre-FMA x86 CPUs take a documented fallback path and are not
//! covered by the f32 hashes.

use seiza_imgproc::border::BorderMode;
use seiza_imgproc::morphology::{KernelShape, MorphBorder, StructuringElement};

/// FNV-1a 64-bit.
fn fnv64(bytes: impl IntoIterator<Item = u8>) -> u64 {
    let mut h = 0xcbf29ce484222325u64;
    for b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

fn fnv_u8(data: &[u8]) -> u64 {
    fnv64(data.iter().copied())
}

fn fnv_f32(data: &[f32]) -> u64 {
    fnv64(data.iter().flat_map(|v| v.to_bits().to_le_bytes()))
}

struct Lcg(u64);

impl Lcg {
    fn next_u32(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 32) as u32
    }
    fn uniform(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * (self.next_u32() as f64 / u32::MAX as f64)
    }
}

/// Star field from exactly-rounded arithmetic only: gradient background,
/// rational-profile stars, LCG noise.
fn star_field(width: usize, height: usize, seed: u64) -> Vec<u8> {
    let mut rng = Lcg(seed);
    let mut img = vec![0f64; width * height];
    for y in 0..height {
        for x in 0..width {
            let bg = 15.0 + 20.0 * (x + y) as f64 / (width + height) as f64;
            img[y * width + x] = bg + rng.uniform(-6.0, 6.0);
        }
    }
    for _ in 0..60 {
        let cx = rng.uniform(3.0, width as f64 - 3.0);
        let cy = rng.uniform(3.0, height as f64 - 3.0);
        let s2 = rng.uniform(1.0, 6.0);
        let amp = rng.uniform(30.0, 220.0);
        let r = 8isize;
        for dy in -r..=r {
            for dx in -r..=r {
                let px = cx as isize + dx;
                let py = cy as isize + dy;
                if px >= 0 && py >= 0 && (px as usize) < width && (py as usize) < height {
                    let d2 =
                        (px as f64 - cx) * (px as f64 - cx) + (py as f64 - cy) * (py as f64 - cy);
                    // Rational star profile: exactly rounded everywhere.
                    img[py as usize * width + px as usize] += amp / (1.0 + d2 / s2);
                }
            }
        }
    }
    img.into_iter()
        .map(|v| v.round().clamp(0.0, 255.0) as u8)
        .collect()
}

fn blob_image(width: usize, height: usize, seed: u64) -> Vec<u8> {
    star_field(width, height, seed)
        .into_iter()
        .map(|v| if v > 60 { 255 } else { 0 })
        .collect()
}

const W: usize = 253; // odd, W % 8 = 5: exercises SIMD-tail column logic
const H: usize = 131;

#[test]
fn quantized_gaussian_kernels_match_opencv() {
    // Extracted from OpenCV 4.13 via impulse probes; all sum to 256.
    let cases: [(usize, f64, &[u16]); 5] = [
        (7, 1.0, &[1, 14, 62, 102, 62, 14, 1]),
        (13, 2.0, &[1, 2, 7, 16, 31, 45, 52, 45, 31, 16, 7, 2, 1]),
        (
            19,
            3.0,
            &[
                0, 1, 3, 4, 9, 14, 20, 28, 32, 34, 32, 28, 20, 14, 9, 4, 3, 1, 0,
            ],
        ),
        (9, 1.4, &[1, 8, 26, 56, 74, 56, 26, 8, 1]),
        (5, 1.4, &[28, 61, 78, 61, 28]),
    ];
    for (n, sigma, expected) in cases {
        // The quantizer is internal; verify through an impulse response.
        let mut img = vec![0u8; 64];
        img[32] = 255;
        let out =
            seiza_imgproc::blur::gaussian_blur_u8(&img, 64, 1, n, sigma, BorderMode::Reflect101);
        // out[x] = (S_v * 255 * kq[d] + 0x8000) >> 16 with S_v = 256.
        let r = n / 2;
        for (d, &kq) in expected.iter().enumerate() {
            let want = ((256u32 * 255 * kq as u32 + 0x8000) >> 16).min(255) as u8;
            assert_eq!(
                out[32 - r + d],
                want,
                "kernel tap {d} for ksize {n} sigma {sigma}"
            );
        }
    }
}

#[test]
fn golden_gaussian_u8() {
    let img = star_field(W, H, 1);
    for (ksize, sigma, want) in [
        (0usize, 1.0f64, 0x234b216c8e0937e0u64),
        (0, 2.0, 0x9a42115e58d382c9),
        (0, 3.0, 0x68059b6b4ef108a3),
        (5, 1.4, 0x70c1553d1a34252f),
    ] {
        let out =
            seiza_imgproc::blur::gaussian_blur_u8(&img, W, H, ksize, sigma, BorderMode::Reflect101);
        let h = fnv_u8(&out);
        if std::env::var("GOLDEN_PRINT").is_ok() {
            println!("gaussian_u8 k{ksize} s{sigma}: 0x{h:016x}");
        } else {
            assert_eq!(h, want, "gaussian u8 ksize={ksize} sigma={sigma}");
        }
    }
}

#[test]
fn golden_median_otsu_canny_morph() {
    let img = star_field(W, H, 2);
    let blobs = blob_image(W, H, 3);

    let median = seiza_imgproc::blur::median_blur3_u8(&img, W, H);
    let otsu = seiza_imgproc::threshold::otsu_binary(&img, W, H);
    let canny = seiza_imgproc::canny::canny(&img, W, H, 10, 80);
    let se_rect = StructuringElement::new(KernelShape::Rect, 3);
    let dil = seiza_imgproc::morphology::dilate(&blobs, W, H, &se_rect, MorphBorder::Ignore);
    let se_ell = StructuringElement::new(KernelShape::Ellipse, 3);
    let ero = seiza_imgproc::morphology::erode(&blobs, W, H, &se_ell, MorphBorder::Reflect);

    let results = [
        ("median", fnv_u8(&median), 0x356eea4d48a9819du64),
        ("otsu", fnv_u8(&otsu), 0x956459fde222935a),
        ("canny", fnv_u8(&canny), 0x605a3f589693cc98),
        ("dilate", fnv_u8(&dil), 0x2c609a3701c655dd),
        ("erode", fnv_u8(&ero), 0x361c1e1fc6bc81c1),
    ];
    for (name, h, want) in results {
        if std::env::var("GOLDEN_PRINT").is_ok() {
            println!("{name}: 0x{h:016x}");
        } else {
            assert_eq!(h, want, "{name}");
        }
    }
}

#[test]
fn golden_contour_metrics() {
    let blobs = blob_image(W, H, 4);
    let contours = seiza_imgproc::contours::find_external_contours(&blobs, W, H);
    // Hash the derived metrics rather than point lists.
    let mut acc: Vec<u8> = Vec::new();
    acc.extend((contours.len() as u64).to_le_bytes());
    for c in &contours {
        let bb = seiza_imgproc::contours::bounding_rect(c);
        let m = seiza_imgproc::contours::contour_moments(c);
        let hull = seiza_imgproc::contours::convex_hull(c);
        for v in [bb.0, bb.1, bb.2, bb.3] {
            acc.extend(v.to_le_bytes());
        }
        for v in [
            seiza_imgproc::contours::contour_area(c),
            seiza_imgproc::contours::arc_length_closed(c),
            seiza_imgproc::contours::contour_area(&hull),
            m.m00,
            m.m10,
            m.m01,
        ] {
            acc.extend(v.to_bits().to_le_bytes());
        }
    }
    let h = fnv64(acc);
    if std::env::var("GOLDEN_PRINT").is_ok() {
        println!("contours: 0x{h:016x} ({} contours)", contours.len());
    } else {
        assert_eq!(h, 0x2ef24b59548cf663, "contour metrics");
    }
}

#[test]
fn golden_f32_blur_and_structures() {
    if !fma_available() {
        eprintln!("skipping f32 goldens: no FMA hardware");
        return;
    }
    let img_u8 = star_field(W, H, 5);
    let img: Vec<f32> = img_u8.iter().map(|&v| v as f32 * 17.0).collect();
    for (ksize, sigma, want) in [
        (3usize, 0.8f64, 0x16470d9d63a0ee1fu64),
        (5, 1.6, 0x39875d7212974245),
        (9, 3.2, 0x7ebf61258bc5ee98),
    ] {
        let out =
            seiza_imgproc::blur::gaussian_blur_f32(&img, W, H, ksize, sigma, BorderMode::Reflect);
        let h = fnv_f32(&out);
        if std::env::var("GOLDEN_PRINT").is_ok() {
            println!("gaussian_f32 k{ksize}: 0x{h:016x}");
        } else {
            assert_eq!(h, want, "gaussian f32 ksize={ksize} sigma={sigma}");
        }
    }

    let data: Vec<f64> = img_u8.iter().map(|&v| v as f64 * 257.0).collect();
    let filtered =
        seiza_imgproc::wavelets::StructureRemover::new(4).remove_structures_filtered(&data, W, H);
    let bits: Vec<f32> = filtered.iter().map(|&v| v as f32).collect();
    let h = fnv_f32(&bits);
    if std::env::var("GOLDEN_PRINT").is_ok() {
        println!("structure_filtered: 0x{h:016x}");
    } else {
        assert_eq!(h, 0xb26aacb02e167f57, "structure removal filtered");
    }

    let atrous =
        seiza_imgproc::wavelets::StructureRemover::new(4).remove_structures_atrous(&data, W, H);
    let bits: Vec<u8> = atrous
        .iter()
        .flat_map(|v| v.to_bits().to_le_bytes())
        .collect();
    let h = fnv_u8(&bits);
    if std::env::var("GOLDEN_PRINT").is_ok() {
        println!("structure_atrous: 0x{h:016x}");
    } else {
        assert_eq!(h, 0xa5acd2912c7da2d9, "structure removal atrous");
    }
}

#[test]
fn golden_dt_filter() {
    let img_u8 = star_field(W, H, 6);
    let img: Vec<f32> = img_u8.iter().map(|&v| v as f32).collect();
    let out = seiza_imgproc::dtfilter::dt_filter_nc(&img, &img, W, H, 8.0, 30.0, 3);
    let h = fnv_f32(&out);
    if std::env::var("GOLDEN_PRINT").is_ok() {
        println!("dt_filter: 0x{h:016x}");
    } else {
        assert_eq!(h, 0xab57bf1887840456, "dt filter");
    }
}

fn fma_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        true
    }
}
