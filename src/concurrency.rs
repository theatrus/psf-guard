//! Shared worker-count policy and pool helper for the CPU-bound parallel
//! operations in psf-guard: the CLI `screen-fits` command, the server "Scan
//! Occlusion" task, and background image pre-generation.
//!
//! Each of these processes one FITS frame per worker; the heavy per-frame work
//! (`FitsImage::from_file` → star detection → `compute_spatial_metrics`, or a
//! stretch-to-PNG) is mostly single-threaded internally, so the main lever is
//! *how many frames run at once*. Historically these paths hardcoded low caps (2
//! for the server scan, 4 for the CLI) or ran fully sequentially, leaving most
//! cores idle. This module scales the worker count to the machine while
//! staying inside three guardrails:
//!
//! 1. **Priority** — [`Priority::Interactive`] work (a user asked for it) gets
//!    the interactive core budget; [`Priority::Background`] work (pre-warming
//!    caches) gets a smaller budget and *yields* to interactive work: the
//!    caller stops dispatching new background items the moment an interactive
//!    job appears and lets in-flight ones drain (see
//!    `AppState::interactive_job_active`).
//! 2. **Core budget** — a fraction of the logical cores per priority. The CLI
//!    uses all cores for its foreground scan; the server leaves headroom to
//!    keep serving the UI (`interactive_ratio`, default 0.5) and throttles
//!    background work harder (`background_ratio`, default 0.25).
//!    In the server each budget is shared: jobs lease their workers from
//!    [`WorkerBudgets`], so jobs running at once split it.
//! 3. **Memory ceiling** — full-frame work holds several f64 buffers, so N
//!    in-flight frames on a big sensor can consume many GB. We cap workers at
//!    `budget_fraction * available_RAM / per_frame_peak` so a high-core box
//!    with a large sensor can't OOM.
//!
//! An explicit operator override (`--threads`) bypasses the ratio and is
//! trusted, clamped only to `[1, hard_max_workers]`.
//!
//! What Seiza does split across threads (debayering, stretches, parts of
//! detection and stacking) runs in the Rayon pool of the thread that calls
//! it, and Rayon's global pool has a thread for every core. So a job runs its
//! Seiza calls inside a pool of the workers it leased, from [`ComputePool`],
//! and the job's work stays on that many threads. Interactive previews and
//! one-off requests share one pool the size of the interactive budget (see
//! `AppState::run_interactive`).

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
/// processed. The quality path holds the raw and stretched `u16` frames plus
/// full-frame detection and spatial-analysis buffers. 32 is a deliberately
/// conservative envelope of that transient peak with margin; the memory
/// ceiling is a safety backstop, not a precise allocator.
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

impl Priority {
    fn index(self) -> usize {
        match self {
            Priority::Interactive => 0,
            Priority::Background => 1,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Priority::Interactive => "interactive",
            Priority::Background => "background",
        }
    }
}

/// The server's two shared worker budgets, one per [`Priority`]. Each holds
/// as many workers as its share of the cores allows. A job leases the workers
/// it planned from the budget for its priority and gives them back when the
/// lease drops, so jobs running at once split that share instead of each
/// taking all of it: a stack build, a quality scan in every database and
/// preview pre-generation together stay inside the background share.
///
/// A lease never waits. It gets what is free, and always at least one
/// worker, so no job stalls behind a long one; a job started while its
/// budget is spent runs on one worker. A changed share applies to the next
/// lease.
#[derive(Debug, Default)]
pub struct WorkerBudgets {
    /// Workers leased out, per priority.
    in_use: std::sync::Mutex<[usize; 2]>,
}

/// Workers leased from [`WorkerBudgets`], returned when dropped.
#[derive(Debug)]
pub struct WorkerLease {
    budgets: std::sync::Arc<WorkerBudgets>,
    priority: Priority,
    /// How many workers the job may run.
    pub workers: usize,
    /// The priority's whole budget when the lease was taken.
    pub budget: usize,
}

impl WorkerBudgets {
    /// Lease up to `wanted` workers (the job's own plan, memory included) from
    /// the budget `policy` gives `priority`.
    pub fn lease(
        self: &std::sync::Arc<Self>,
        policy: &WorkerPolicy,
        priority: Priority,
        wanted: usize,
    ) -> WorkerLease {
        self.lease_from(priority_budget(policy, priority), priority, wanted)
    }

    fn lease_from(
        self: &std::sync::Arc<Self>,
        budget: usize,
        priority: Priority,
        wanted: usize,
    ) -> WorkerLease {
        let mut in_use = self.in_use.lock().unwrap();
        let leased = &mut in_use[priority.index()];
        let workers = wanted.clamp(1, budget.saturating_sub(*leased).max(1));
        *leased += workers;
        WorkerLease {
            budgets: std::sync::Arc::clone(self),
            priority,
            workers,
            budget,
        }
    }

    /// Workers leased out at `priority` now.
    pub fn in_use(&self, priority: Priority) -> usize {
        self.in_use.lock().unwrap()[priority.index()]
    }
}

impl WorkerLease {
    /// For logs: how much of the shared budget this job holds.
    pub fn summary(&self) -> String {
        format!(
            "{} of the {} shared {} worker(s)",
            self.workers,
            self.budget,
            self.priority.label()
        )
    }
}

impl Drop for WorkerLease {
    fn drop(&mut self) {
        let mut in_use = self.budgets.in_use.lock().unwrap();
        let leased = &mut in_use[self.priority.index()];
        *leased = leased.saturating_sub(self.workers);
    }
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

/// Workers the whole of `priority`'s share of the cores allows, before any
/// memory ceiling: what the jobs of that priority split between them.
pub fn priority_budget(policy: &WorkerPolicy, priority: Priority) -> usize {
    compute_worker_count(
        None,
        logical_cores(),
        None,
        None,
        policy,
        policy.ratio_for(priority),
    )
    .0
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

/// Run `f(i)` for every `i` in `0..len` on a rayon pool of `workers` threads,
/// blocking until all are done. Unlike [`parallel_index`], rayon work an item
/// starts itself (Seiza's detectors and solvers) runs on the same threads, so
/// the job keeps to its share of the machine rather than spreading over
/// rayon's global pool. Falls back to [`parallel_index`] when the pool cannot
/// be built.
pub fn parallel_in_pool<F>(len: usize, workers: usize, f: F)
where
    F: Fn(usize) + Sync + Send,
{
    use rayon::prelude::*;
    if len == 0 {
        return;
    }
    let workers = workers.clamp(1, len);
    match ComputePool::take("parallel", workers) {
        // One item per split, so a slow frame never holds others behind it.
        Ok(pool) => pool.install(|| (0..len).into_par_iter().with_max_len(1).for_each(&f)),
        Err(error) => {
            tracing::warn!(
                "Could not build a {workers}-thread pool ({error}); using plain threads"
            );
            parallel_index(len, workers, f);
        }
    }
}

/// Most threads idle pools may hold between them, as a multiple of the
/// logical cores. Beyond it the oldest idle pool is dropped, which ends its
/// threads.
const IDLE_THREADS_PER_CORE: usize = 2;

/// Pools no job holds, oldest first, kept for the next job that wants one of
/// the same name and size.
#[derive(Default)]
struct IdlePools {
    pools: std::sync::Mutex<std::collections::VecDeque<(&'static str, rayon::ThreadPool)>>,
}

static IDLE_POOLS: IdlePools = IdlePools {
    pools: std::sync::Mutex::new(std::collections::VecDeque::new()),
};

impl IdlePools {
    fn lock(
        &self,
    ) -> std::sync::MutexGuard<'_, std::collections::VecDeque<(&'static str, rayon::ThreadPool)>>
    {
        self.pools
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }

    /// The idle pool of `name` and `threads` returned last, if one is kept.
    fn take(&self, name: &'static str, threads: usize) -> Option<rayon::ThreadPool> {
        let mut pools = self.lock();
        pools
            .iter()
            .rposition(|(kept, pool)| *kept == name && pool.current_num_threads() == threads)
            .and_then(|position| pools.remove(position))
            .map(|(_, pool)| pool)
    }

    /// Keep `pool`, then drop the oldest pools until those kept hold no more
    /// than `limit` threads.
    fn keep(&self, name: &'static str, pool: rayon::ThreadPool, limit: usize) {
        let mut dropped = Vec::new();
        {
            let mut pools = self.lock();
            pools.push_back((name, pool));
            let mut kept = pools
                .iter()
                .map(|(_, pool)| pool.current_num_threads())
                .sum::<usize>();
            while kept > limit
                && let Some((_, oldest)) = pools.pop_front()
            {
                kept -= oldest.current_num_threads();
                dropped.push(oldest);
            }
        }
        // Ended outside the lock.
        drop(dropped);
    }

    #[cfg(test)]
    fn threads(&self) -> usize {
        self.lock()
            .iter()
            .map(|(_, pool)| pool.current_num_threads())
            .sum()
    }
}

/// A Rayon pool of a job's leased workers, kept for the next job when this
/// one drops it rather than built for each.
///
/// Run the job's Seiza calls inside it with `install`: Seiza does its
/// parallel work in the pool of the calling thread, so the work stays on
/// these threads. A job has its pool to itself while it holds it.
pub struct ComputePool {
    name: &'static str,
    pool: Option<rayon::ThreadPool>,
    idle: &'static IdlePools,
    idle_limit: usize,
}

impl ComputePool {
    /// A pool of `threads` threads named `{name}-{index}`, reused when an
    /// idle one of that name and size is kept.
    pub fn take(name: &'static str, threads: usize) -> Result<Self, rayon::ThreadPoolBuildError> {
        Self::take_from(
            &IDLE_POOLS,
            logical_cores().saturating_mul(IDLE_THREADS_PER_CORE),
            name,
            threads,
        )
    }

    fn take_from(
        idle: &'static IdlePools,
        idle_limit: usize,
        name: &'static str,
        threads: usize,
    ) -> Result<Self, rayon::ThreadPoolBuildError> {
        let threads = threads.max(1);
        let pool = match idle.take(name, threads) {
            Some(pool) => pool,
            None => rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .thread_name(move |index| format!("{name}-{index}"))
                .build()?,
        };
        Ok(Self {
            name,
            pool: Some(pool),
            idle,
            idle_limit,
        })
    }
}

impl std::ops::Deref for ComputePool {
    type Target = rayon::ThreadPool;

    fn deref(&self) -> &rayon::ThreadPool {
        self.pool
            .as_ref()
            .expect("a pool is held until it is dropped")
    }
}

impl Drop for ComputePool {
    fn drop(&mut self) {
        if let Some(pool) = self.pool.take() {
            self.idle.keep(self.name, pool, self.idle_limit);
        }
    }
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

/// Read `NAXIS1 * NAXIS2` from a FITS primary header without loading the pixel
/// data, so a scan can size its worker pool to the sensor. `None` if the file
/// or the axes can't be read.
pub fn probe_frame_pixels(path: &Path) -> Option<usize> {
    let headers = crate::image_io::read_header(path).ok()?;
    let axis = |key: &str| -> Option<usize> {
        headers
            .iter()
            .find(|(k, _)| k == key)
            .and_then(|(_, v)| v.as_i64())
            .and_then(|n| usize::try_from(n).ok())
    };
    let w = axis("NAXIS1")?;
    let h = axis("NAXIS2")?;
    (w > 0 && h > 0).then_some(w * h)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jobs_of_one_priority_split_its_budget() {
        let budgets = std::sync::Arc::new(WorkerBudgets::default());
        let build = budgets.lease_from(5, Priority::Background, 4);
        let scan = budgets.lease_from(5, Priority::Background, 4);
        assert_eq!((build.workers, scan.workers), (4, 1));
        // Spent: the next job still runs, on one worker.
        let pregeneration = budgets.lease_from(5, Priority::Background, 3);
        assert_eq!(pregeneration.workers, 1);
        assert_eq!(budgets.in_use(Priority::Background), 6);
        // The other priority has its own budget.
        let preview = budgets.lease_from(10, Priority::Interactive, 8);
        assert_eq!(preview.workers, 8);

        drop(build);
        assert_eq!(budgets.in_use(Priority::Background), 2);
        let next = budgets.lease_from(5, Priority::Background, 9);
        assert_eq!(next.workers, 3);
        drop((scan, pregeneration, next, preview));
        assert_eq!(budgets.in_use(Priority::Background), 0);
        assert_eq!(budgets.in_use(Priority::Interactive), 0);
    }

    #[test]
    fn a_lease_never_takes_more_than_the_job_planned() {
        let budgets = std::sync::Arc::new(WorkerBudgets::default());
        let lease = budgets.lease_from(16, Priority::Interactive, 3);
        assert_eq!(lease.workers, 3);
        assert_eq!(lease.summary(), "3 of the 16 shared interactive worker(s)");
    }

    #[test]
    fn a_pooled_job_runs_its_items_side_by_side_and_keeps_nested_work_inside() {
        use std::sync::Mutex;
        let in_flight = AtomicUsize::new(0);
        let most = AtomicUsize::new(0);
        let nested = Mutex::new(Vec::new());
        let done = Mutex::new(Vec::new());
        parallel_in_pool(8, 3, |index| {
            let now = in_flight.fetch_add(1, Ordering::SeqCst) + 1;
            most.fetch_max(now, Ordering::SeqCst);
            // What a Seiza call inside the item would see.
            nested.lock().unwrap().push(rayon::current_num_threads());
            std::thread::sleep(std::time::Duration::from_millis(30));
            in_flight.fetch_sub(1, Ordering::SeqCst);
            done.lock().unwrap().push(index);
        });

        let mut done = done.into_inner().unwrap();
        done.sort_unstable();
        assert_eq!(done, (0..8).collect::<Vec<_>>());
        assert!(most.load(Ordering::SeqCst) > 1, "items ran one at a time");
        assert!(most.load(Ordering::SeqCst) <= 3);
        assert!(nested
            .into_inner()
            .unwrap()
            .iter()
            .all(|threads| *threads == 3));
    }

    /// The threads of `pool`, one id each.
    fn pool_threads(pool: &rayon::ThreadPool) -> Vec<std::thread::ThreadId> {
        let mut threads = pool.broadcast(|_| std::thread::current().id());
        threads.sort_by_key(|id| format!("{id:?}"));
        threads
    }

    /// Pools kept apart from the process's own, so tests running at once
    /// do not take or drop each other's.
    fn idle_pools() -> &'static IdlePools {
        Box::leak(Box::default())
    }

    #[test]
    fn a_compute_pool_is_kept_for_the_next_job_of_its_name_and_size() {
        let idle = idle_pools();
        let take = |name, threads| ComputePool::take_from(idle, 64, name, threads).unwrap();
        let first = take("reuse", 3);
        assert_eq!(first.current_num_threads(), 3);
        // Work inside sees the pool, which is all Seiza's own work uses.
        assert_eq!(first.install(rayon::current_num_threads), 3);
        let threads = pool_threads(&first);

        // A job running alongside gets a pool of its own.
        let second = take("reuse", 3);
        assert_ne!(pool_threads(&second), threads);
        drop(second);
        drop(first);

        // The next job takes the pool returned last, threads and all; one of
        // another size or name builds its own.
        let again = take("reuse", 3);
        assert_eq!(pool_threads(&again), threads);
        let other = take("reuse", 2);
        assert_eq!(other.current_num_threads(), 2);
        let renamed = take("renamed", 3);
        assert_ne!(pool_threads(&renamed), threads);
    }

    #[test]
    fn idle_pools_hold_a_bounded_number_of_threads() {
        let idle = idle_pools();
        let pools = (0..5)
            .map(|_| ComputePool::take_from(idle, 10, "bound", 4).unwrap())
            .collect::<Vec<_>>();
        let threads = pools
            .iter()
            .map(|pool| pool_threads(pool))
            .collect::<Vec<_>>();
        drop(pools);
        // Two four-thread pools fit in ten; the last two returned are kept.
        assert_eq!(idle.threads(), 8);
        let again = (0..2)
            .map(|_| ComputePool::take_from(idle, 10, "bound", 4).unwrap())
            .collect::<Vec<_>>();
        let mut kept = again
            .iter()
            .map(|pool| pool_threads(pool))
            .collect::<Vec<_>>();
        kept.sort_by_key(|threads| format!("{threads:?}"));
        let mut expected = threads[3..].to_vec();
        expected.sort_by_key(|threads| format!("{threads:?}"));
        assert_eq!(kept, expected);
    }

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
