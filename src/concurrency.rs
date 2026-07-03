//! Shared worker-count policy and pool helper for the CPU-bound parallel
//! operations in psf-guard: the CLI `screen-fits` command, the server "Scan
//! Occlusion" task, and background image pre-generation.
//!
//! Each of these processes one FITS frame per worker; the heavy per-frame work
//! (`FitsImage::from_file` → star detection → `compute_spatial_metrics`, or a
//! stretch-to-PNG) is single-threaded internally, so the only lever is *how
//! many frames run at once*. Historically these paths hardcoded low caps (2
//! for the server scan, 4 for the CLI) or ran fully sequentially, leaving most
//! cores idle. This module scales the worker count to the machine while
//! staying inside three guardrails:
//!
//! 1. **Priority** — [`Priority::Interactive`] work (a user asked for it) gets
//!    the interactive core budget; [`Priority::Background`] work (pre-warming
//!    caches) gets a smaller budget and is expected to *yield* to interactive
//!    work entirely while it runs (the caller gates on that — see
//!    `AppState::interactive_job_active`).
//! 2. **Core budget** — a fraction of the logical cores per priority. The CLI
//!    uses all cores for its foreground scan; the server leaves headroom to
//!    keep serving the UI (`interactive_ratio`, default 0.5) and throttles
//!    background work harder (`background_ratio`, default 0.25).
//! 3. **Memory ceiling** — full-frame work holds several f64 buffers, so N
//!    in-flight frames on a big sensor can consume many GB. We cap workers at
//!    `budget_fraction * available_RAM / per_frame_peak` so a high-core box
//!    with a large sensor can't OOM.
//!
//! An explicit operator override (`--threads`) bypasses the ratio and is
//! trusted, clamped only to `[1, hard_max_workers]`.

use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};

/// Default fraction of cores interactive (user-triggered) work uses. Balanced:
/// fast scans while leaving half the machine to serve the UI.
pub const DEFAULT_INTERACTIVE_RATIO: f64 = 0.5;

/// Default fraction of cores background (cache pre-warming) work uses. Lower
/// than interactive so it stays out of the way; it is additionally expected to
/// pause entirely while an interactive job runs.
pub const DEFAULT_BACKGROUND_RATIO: f64 = 0.25;

/// Absolute backstop on workers regardless of cores / memory / override.
/// Guards against a pathological core count or a fat-fingered override; the
/// core ratio and memory ceiling normally bind well below this.
pub const DEFAULT_HARD_MAX_WORKERS: usize = 64;

/// Estimated peak resident bytes per image pixel while one frame is being
/// processed. The raw frame is `u16` (2 bytes/px), but HocusFocus detection
/// converts to `f64` and holds several full-frame working buffers at once
/// (`float_data`, its `structure_map` clone, wavelet/gaussian scratch). 32 is
/// a deliberately conservative envelope of that transient peak with margin;
/// the memory ceiling is a safety backstop, not a precise allocator.
pub const DEFAULT_PEAK_BYTES_PER_PIXEL: usize = 32;

/// Fraction of the probed system memory a pool may budget for in-flight
/// frames. Leaves the rest for the OS page cache, the server's other work,
/// and other processes. Applied to *available* RAM on Linux/Windows and
/// *total* RAM on macOS (see [`available_memory_bytes`]), so 0.5 is a safe
/// universal choice.
pub const DEFAULT_MEMORY_BUDGET_FRACTION: f64 = 0.5;

/// Fallback core count when the platform can't report parallelism.
const FALLBACK_CORES: usize = 4;

/// How much of the machine a piece of work is entitled to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// User asked for it and is waiting — use the interactive core budget.
    Interactive,
    /// Opportunistic cache pre-warming — use the smaller background budget and
    /// stay out of interactive work's way.
    Background,
}

/// Tunables for the CPU-bound parallel operations, grouped so the whole policy
/// threads through the server (`ServerConfig` → `AppState`) and CLI as one
/// value instead of a fan-out of loose parameters. Only the two ratios are
/// surfaced in the on-disk TOML; the rest carry their compiled-in defaults but
/// live here so a future knob is a one-line addition rather than another
/// signature change.
#[derive(Debug, Clone, Copy)]
pub struct WorkerPolicy {
    /// Fraction of logical cores for interactive work (`0.0..=1.0`). The CLI
    /// uses `1.0` (all cores); the server default is
    /// [`DEFAULT_INTERACTIVE_RATIO`].
    pub interactive_ratio: f64,
    /// Fraction of logical cores for background work (`0.0..=1.0`), default
    /// [`DEFAULT_BACKGROUND_RATIO`].
    pub background_ratio: f64,
    /// Fraction of probed RAM to budget for in-flight frames.
    pub memory_budget_fraction: f64,
    /// Absolute cap on workers.
    pub hard_max_workers: usize,
    /// Estimated peak resident bytes per image pixel during processing.
    pub peak_bytes_per_pixel: usize,
}

impl Default for WorkerPolicy {
    fn default() -> Self {
        Self {
            interactive_ratio: DEFAULT_INTERACTIVE_RATIO,
            background_ratio: DEFAULT_BACKGROUND_RATIO,
            memory_budget_fraction: DEFAULT_MEMORY_BUDGET_FRACTION,
            hard_max_workers: DEFAULT_HARD_MAX_WORKERS,
            peak_bytes_per_pixel: DEFAULT_PEAK_BYTES_PER_PIXEL,
        }
    }
}

impl WorkerPolicy {
    /// A policy whose interactive tier uses all cores (the CLI default),
    /// memory-bounded as usual.
    pub fn all_cores() -> Self {
        Self {
            interactive_ratio: 1.0,
            ..Self::default()
        }
    }

    /// This policy with `interactive_ratio` replaced (clamped to `[0, 1]`).
    pub fn with_interactive_ratio(mut self, ratio: f64) -> Self {
        self.interactive_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// This policy with `background_ratio` replaced (clamped to `[0, 1]`).
    pub fn with_background_ratio(mut self, ratio: f64) -> Self {
        self.background_ratio = ratio.clamp(0.0, 1.0);
        self
    }

    /// The core ratio for the given priority.
    pub fn ratio_for(&self, priority: Priority) -> f64 {
        match priority {
            Priority::Interactive => self.interactive_ratio,
            Priority::Background => self.background_ratio,
        }
    }
}

/// The resolved worker count plus a human-readable explanation for logs.
#[derive(Debug, Clone)]
pub struct WorkerBudget {
    pub workers: usize,
    pub rationale: String,
}

/// Logical core count, or [`FALLBACK_CORES`] if the platform won't say.
pub fn logical_cores() -> usize {
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(FALLBACK_CORES)
}

/// Plan how many workers to run for a piece of work.
///
/// - `requested`: explicit override (CLI `--threads`). `Some(n)` wins outright
///   (clamped to `[1, hard_max_workers]`); the operator takes responsibility.
/// - `policy`: the tuning policy (per-priority core ratios, memory budget, caps).
/// - `priority`: which core budget applies.
/// - `frame_pixels`: pixel count of a representative frame, if known, used for
///   the memory ceiling. `None` skips the memory cap.
pub fn plan_workers(
    requested: Option<usize>,
    policy: &WorkerPolicy,
    priority: Priority,
    frame_pixels: Option<usize>,
) -> WorkerBudget {
    let cores = logical_cores();
    let available_bytes = available_memory_bytes();
    let (workers, rationale) = compute_worker_count(
        requested,
        cores,
        frame_pixels,
        available_bytes,
        policy,
        policy.ratio_for(priority),
    );
    WorkerBudget { workers, rationale }
}

/// Pure core of [`plan_workers`] — takes every input explicitly (including the
/// already-resolved `ratio`) so it is fully unit-testable without probing the
/// host.
fn compute_worker_count(
    requested: Option<usize>,
    cores: usize,
    frame_pixels: Option<usize>,
    available_bytes: Option<u64>,
    policy: &WorkerPolicy,
    ratio: f64,
) -> (usize, String) {
    let hard_max = policy.hard_max_workers.max(1);

    if let Some(n) = requested {
        let workers = n.clamp(1, hard_max);
        return (workers, format!("explicit override: {} worker(s)", workers));
    }

    let cores = cores.max(1);
    let ratio = ratio.clamp(0.0, 1.0);
    // Round to nearest, but never below 1 — a tiny ratio still makes progress.
    let scaled = (((cores as f64 * ratio).round() as usize).max(1)).min(hard_max);

    // Memory ceiling, when we can estimate both the frame size and the RAM.
    let per_pixel = policy.peak_bytes_per_pixel.max(1) as u64;
    let mem_cap = match (frame_pixels, available_bytes) {
        (Some(px), Some(avail)) if px > 0 => {
            let per_frame = (px as u64).saturating_mul(per_pixel).max(1);
            let budget = (avail as f64 * policy.memory_budget_fraction) as u64;
            Some((budget / per_frame).max(1) as usize)
        }
        _ => None,
    };

    let mut workers = scaled;
    let mut rationale = format!("{} of {} core(s) at ratio {:.2}", scaled, cores, ratio);
    if let Some(cap) = mem_cap {
        if cap < workers {
            rationale = format!(
                "{}, capped to {} by memory (~{} MB/frame)",
                rationale,
                cap,
                frame_pixels
                    .map(|px| (px as u64 * per_pixel) / (1024 * 1024))
                    .unwrap_or(0),
            );
            workers = cap;
        } else {
            rationale = format!("{} (memory allows {})", rationale, cap);
        }
    }

    (workers.max(1), rationale)
}

/// Run `f(i)` for every `i` in `0..len` across `workers` scoped threads using
/// atomic work-stealing, blocking until all items are processed. This is the
/// shared pool the CLI and server scans use; `f` is `Sync` (shared by all
/// workers) and must do its own synchronization for any shared output.
///
/// A panic in `f` propagates out of the scope (aborting under the release
/// profile's `panic = "abort"`); callers that must survive a bad frame should
/// catch inside `f`.
pub fn parallel_index<F>(len: usize, workers: usize, f: F)
where
    F: Fn(usize) + Sync,
{
    if len == 0 {
        return;
    }
    let workers = workers.clamp(1, len);
    let next = AtomicUsize::new(0);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= len {
                    break;
                }
                f(i);
            });
        }
    });
}

/// Best-effort system memory in bytes, or `None` when the platform can't be
/// probed (then the caller skips the memory ceiling).
///
/// - Linux: `MemAvailable` from `/proc/meminfo` (the kernel's estimate of
///   allocatable memory without swapping), falling back to `MemTotal`.
/// - macOS: `hw.memsize` (total physical RAM) via `sysctl`.
/// - Windows: `ullAvailPhys` (available physical RAM) via
///   `GlobalMemoryStatusEx`.
/// - Other: `None`.
pub fn available_memory_bytes() -> Option<u64> {
    #[cfg(target_os = "linux")]
    {
        let text = std::fs::read_to_string("/proc/meminfo").ok()?;
        let mut mem_total = None;
        for line in text.lines() {
            if let Some(rest) = line.strip_prefix("MemAvailable:") {
                if let Some(kb) = parse_meminfo_kb(rest) {
                    return Some(kb);
                }
            } else if let Some(rest) = line.strip_prefix("MemTotal:") {
                mem_total = parse_meminfo_kb(rest);
            }
        }
        return mem_total;
    }

    #[cfg(target_os = "macos")]
    {
        // hw.memsize: total physical memory in bytes.
        let mut size: u64 = 0;
        let mut len = std::mem::size_of::<u64>();
        let name = c"hw.memsize";
        // SAFETY: `name` is a valid NUL-terminated C string; `size`/`len`
        // point to properly sized, initialized storage that outlives the call.
        let rc = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                &mut size as *mut u64 as *mut libc::c_void,
                &mut len,
                std::ptr::null_mut(),
                0,
            )
        };
        return if rc == 0 && size > 0 {
            Some(size)
        } else {
            None
        };
    }

    #[cfg(target_os = "windows")]
    {
        use windows_sys::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
        // SAFETY: MEMORYSTATUSEX is a plain-old-data struct; zeroing it and
        // setting dwLength to its size is exactly what the API requires. The
        // call writes only within `status`, which outlives it.
        let mut status: MEMORYSTATUSEX = unsafe { std::mem::zeroed() };
        status.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
        let ok = unsafe { GlobalMemoryStatusEx(&mut status) };
        return if ok != 0 && status.ullAvailPhys > 0 {
            Some(status.ullAvailPhys)
        } else {
            None
        };
    }

    #[allow(unreachable_code)]
    None
}

/// Parse a `/proc/meminfo` value line tail like ` 16384000 kB` into bytes.
#[cfg(any(target_os = "linux", test))]
fn parse_meminfo_kb(rest: &str) -> Option<u64> {
    let kb: u64 = rest.split_whitespace().next()?.parse().ok()?;
    Some(kb.saturating_mul(1024))
}

/// Peak-memory estimate (bytes) for analyzing one frame of `pixels` pixels,
/// using the default per-pixel envelope. Exposed so callers can size a shared
/// budget if they ever pipeline frames.
pub fn estimated_frame_peak_bytes(pixels: usize) -> usize {
    pixels.saturating_mul(DEFAULT_PEAK_BYTES_PER_PIXEL)
}

/// Read `NAXIS1 * NAXIS2` from a FITS primary header without loading the pixel
/// data, so a scan can size its worker pool to the sensor. `None` if the file
/// or the axes can't be read.
pub fn probe_frame_pixels(path: &Path) -> Option<usize> {
    use fitrs::Fits;

    let fits = Fits::open(path).ok()?;
    let hdu = fits.get(0)?;
    // fitrs Debug-renders header values as `IntegerNumber(9576)` etc. Same
    // pattern the rest of the codebase uses to pull numbers from headers.
    let re =
        regex::Regex::new(r"(?:FloatingPoint|Integer|RealFloatingNumber|IntegerNumber)\(([^)]+)\)")
            .ok()?;
    let axis = |key: &str| -> Option<usize> {
        let v = hdu.value(key)?;
        let s = format!("{:?}", v);
        let caps = re.captures(&s)?;
        caps[1].trim().parse::<f64>().ok().map(|n| n as usize)
    };
    let w = axis("NAXIS1")?;
    let h = axis("NAXIS2")?;
    (w > 0 && h > 0).then_some(w * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pol() -> WorkerPolicy {
        WorkerPolicy::default()
    }

    #[test]
    fn explicit_override_wins_and_is_clamped() {
        // Override ignores ratio, cores and memory entirely.
        let (w, _) = compute_worker_count(
            Some(6),
            4,
            Some(50_000_000),
            Some(1_000_000_000),
            &pol(),
            0.5,
        );
        assert_eq!(w, 6);
        // Clamped to >= 1.
        let (w, _) = compute_worker_count(Some(0), 8, None, None, &pol(), 1.0);
        assert_eq!(w, 1);
        // Clamped to hard_max_workers.
        let (w, _) = compute_worker_count(Some(9999), 8, None, None, &pol(), 1.0);
        assert_eq!(w, DEFAULT_HARD_MAX_WORKERS);
    }

    #[test]
    fn scales_by_core_ratio_when_no_override() {
        // Half of 16 cores.
        let (w, _) = compute_worker_count(None, 16, None, None, &pol(), 0.5);
        assert_eq!(w, 8);
        // All cores.
        let (w, _) = compute_worker_count(None, 18, None, None, &pol(), 1.0);
        assert_eq!(w, 18);
        // Rounds to nearest: 0.5 * 4 = 2.
        let (w, _) = compute_worker_count(None, 4, None, None, &pol(), 0.5);
        assert_eq!(w, 2);
        // Never below 1 even with a tiny ratio.
        let (w, _) = compute_worker_count(None, 4, None, None, &pol(), 0.01);
        assert_eq!(w, 1);
    }

    #[test]
    fn priority_selects_ratio() {
        let policy = WorkerPolicy::default();
        assert_eq!(
            policy.ratio_for(Priority::Interactive),
            DEFAULT_INTERACTIVE_RATIO
        );
        assert_eq!(
            policy.ratio_for(Priority::Background),
            DEFAULT_BACKGROUND_RATIO
        );
        // Background gets fewer cores than interactive at the same core count.
        let interactive = plan_from(&policy, Priority::Interactive, 16);
        let background = plan_from(&policy, Priority::Background, 16);
        assert_eq!(interactive, 8);
        assert_eq!(background, 4);
        assert!(background < interactive);
    }

    // Helper: resolve worker count for a policy/priority at a fixed core count,
    // no override, no memory cap.
    fn plan_from(policy: &WorkerPolicy, priority: Priority, cores: usize) -> usize {
        compute_worker_count(None, cores, None, None, policy, policy.ratio_for(priority)).0
    }

    #[test]
    fn memory_ceiling_caps_workers() {
        // 50 MP frame -> 50e6 * 32 = 1.6 GB peak/frame.
        let frame_px = 50_000_000usize;
        // 8 GB available, budget fraction 0.5 -> 4 GB / 1.6 GB = 2 workers,
        // even though the ratio would allow 16.
        let (w, reason) = compute_worker_count(
            None,
            32,
            Some(frame_px),
            Some(8 * 1024 * 1024 * 1024),
            &pol(),
            1.0,
        );
        assert_eq!(w, 2, "reason: {reason}");
        assert!(reason.contains("memory"));
    }

    #[test]
    fn memory_ceiling_does_not_raise_below_core_budget() {
        // Plenty of RAM: the core budget (4) binds, not memory.
        let (w, reason) = compute_worker_count(
            None,
            8,
            Some(20_000_000),
            Some(256 * 1024 * 1024 * 1024),
            &pol(),
            0.5,
        );
        assert_eq!(w, 4);
        assert!(reason.contains("memory allows"));
    }

    #[test]
    fn no_memory_probe_falls_back_to_core_budget() {
        let (w, _) = compute_worker_count(None, 12, Some(50_000_000), None, &pol(), 0.5);
        assert_eq!(w, 6);
        let (w, _) =
            compute_worker_count(None, 12, None, Some(64 * 1024 * 1024 * 1024), &pol(), 0.5);
        assert_eq!(w, 6);
    }

    #[test]
    fn parallel_index_covers_every_item_once() {
        use std::sync::atomic::AtomicU64;
        let len = 1000;
        let counters: Vec<AtomicU64> = (0..len).map(|_| AtomicU64::new(0)).collect();
        parallel_index(len, 8, |i| {
            counters[i].fetch_add(1, Ordering::Relaxed);
        });
        assert!(counters.iter().all(|c| c.load(Ordering::Relaxed) == 1));
        // Empty input is a no-op.
        parallel_index(0, 4, |_| panic!("must not be called"));
    }

    #[test]
    fn parse_meminfo_line() {
        assert_eq!(parse_meminfo_kb(" 16384000 kB").unwrap(), 16384000 * 1024);
        assert_eq!(parse_meminfo_kb("       512 kB").unwrap(), 512 * 1024);
        assert!(parse_meminfo_kb("  not-a-number kB").is_none());
    }

    #[test]
    fn memory_probe_is_plausible_on_this_platform() {
        // On Linux/macOS the probe should return a sane, nonzero value; other
        // platforms legitimately return None.
        if let Some(bytes) = available_memory_bytes() {
            assert!(bytes >= 128 * 1024 * 1024, "implausibly small: {bytes}");
        }
    }
}
