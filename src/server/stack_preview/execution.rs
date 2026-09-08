//! Reserve stack and calibration buffers before enabling frame preparation.
//!
//! These reference-sized estimates are not a hard RSS limit. Larger source
//! frames, decoder scratch space, allocator overhead and other jobs can exceed
//! them. Capture available RAM before loading the group's masters and reference
//! so their buffers are not charged twice against an already reduced reading.

use crate::calibration::CalibrationPlan;
use crate::concurrency::WorkerPolicy;
use seiza_stacking::{LiveStacker, PipelineOptions, PoolPipelineMemory, PoolPipelineReport};
use std::path::{Path, PathBuf};

const UNKNOWN_RAM_PIPELINE_BYTES: u64 = 1024 * 1024 * 1024;
const ADMISSION_BYTES_PER_SAMPLE: u64 = 40;
const MIB: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct ThreadBudget {
    pub compute_workers: usize,
    pub preparation_workers: usize,
    pub serial: bool,
}

impl ThreadBudget {
    pub fn from_total(total: usize) -> Self {
        let total = total.max(1);
        if total == 1 {
            return Self {
                compute_workers: 1,
                preparation_workers: 1,
                serial: true,
            };
        }
        // Container decoding uses CPU on the reader threads too. Two readers
        // can overlap a read with another frame's preparation without keeping
        // half the compute allowance idle while readers wait for the pool.
        let preparation_workers = (total / 2).clamp(1, 2);
        Self {
            compute_workers: total - preparation_workers,
            preparation_workers,
            serial: false,
        }
    }

    fn sequential(self) -> Self {
        Self {
            compute_workers: self.compute_workers.saturating_add(if self.serial {
                0
            } else {
                self.preparation_workers
            }),
            preparation_workers: 1,
            serial: true,
        }
    }
}

pub(super) fn run_pipeline(
    stacker: &mut LiveStacker,
    paths: &[PathBuf],
    options: &PipelineOptions,
    pool: &rayon::ThreadPool,
    threads: &ThreadBudget,
    on_frame: impl FnMut(
            &Path,
            seiza_stacking::Result<seiza_stacking::FrameDisposition>,
        ) -> seiza_stacking::Continue
        + Send,
) -> seiza_stacking::Result<PoolPipelineReport> {
    if threads.compute_workers == 0
        || threads.preparation_workers == 0
        || pool.current_num_threads() != threads.compute_workers
    {
        return Err(seiza_stacking::Error::Stack(
            "Stack compute pool does not match its thread budget".into(),
        ));
    }
    let options = PipelineOptions {
        workers: Some(
            options
                .workers
                .unwrap_or(threads.preparation_workers)
                .clamp(1, threads.preparation_workers),
        ),
        ..*options
    };
    if threads.serial {
        return stacker.push_fits_sequential_with_pool(
            paths,
            options.normalized_full_scale,
            pool,
            on_frame,
        );
    }
    if rayon::current_thread_index().is_some() {
        return Err(seiza_stacking::Error::Stack(
            "Parallel stack preparation requires a coordinator outside the Rayon pool".into(),
        ));
    }
    stacker.push_fits_pipelined_with_pool(paths, &options, pool, on_frame)
}

pub(super) struct PipelineBudget {
    pub options: PipelineOptions,
    pub threads: ThreadBudget,
    total_budget_bytes: Option<u64>,
    admission_baseline_bytes: u64,
    serial_peak_bytes: u64,
    resident_master_bytes: u64,
    master_clone_bytes: u64,
    integration_bytes: u64,
    worker_bytes: u64,
    sequential_bytes: u64,
    memory_limited: bool,
    workers: usize,
}

impl PipelineBudget {
    pub fn summary(&self) -> String {
        let total = self.total_budget_bytes.map_or_else(
            || "available RAM unknown; serial peak cannot be checked".to_string(),
            |bytes| format!("{} MiB total budget", bytes / MIB),
        );
        let execution = if self.threads.serial {
            let reason = if self.memory_limited {
                "memory limit"
            } else {
                "CPU limit"
            };
            let unknown_limit = if self.total_budget_bytes.is_none() {
                "; 1024 MiB scratch allowance"
            } else {
                ""
            };
            format!(
                "serial preparation ({reason}), {} compute worker(s), {} MiB scratch estimate{unknown_limit}; no preparation queue",
                self.threads.compute_workers,
                self.sequential_bytes / MIB,
            )
        } else {
            format!(
                "{} MiB parallel admission baseline; {} MiB pipeline budget ({} MiB integration, {} MiB per preparation worker); {} preparation worker(s)",
                self.admission_baseline_bytes / MIB,
                self.options.max_in_flight_bytes as u64 / MIB,
                self.integration_bytes / MIB,
                self.worker_bytes / MIB,
                self.workers,
            )
        };
        format!(
            "{total}; {} MiB serial peak including masters; \
             {} MiB session masters, {} MiB master clones; \
             {execution}; reference-sized estimates, not an RSS limit",
            self.serial_peak_bytes / MIB,
            self.resident_master_bytes / MIB,
            self.master_clone_bytes / MIB,
        )
    }
}

pub(super) fn plan_pipeline(
    available_bytes: Option<u64>,
    policy: &WorkerPolicy,
    output_pixels: usize,
    output_samples: usize,
    master_plan: &CalibrationPlan,
    threads: &ThreadBudget,
) -> Result<PipelineBudget, String> {
    if output_pixels == 0 || output_samples < output_pixels {
        return Err("Stack pipeline requires a non-empty reference image".into());
    }
    if output_samples
        .checked_mul(std::mem::size_of::<f32>())
        .is_none()
    {
        return Err("Stack reference exceeds the supported address space".into());
    }
    if threads.preparation_workers == 0 || threads.compute_workers == 0 {
        return Err("Stack pipeline requires non-zero thread allowances".into());
    }
    if !policy.memory_budget_fraction.is_finite()
        || policy.memory_budget_fraction <= 0.0
        || policy.memory_budget_fraction > 1.0
    {
        return Err(
            "Stack memory budget fraction must be greater than zero and at most one".into(),
        );
    }

    let admission_baseline_bytes =
        (output_samples as u64).saturating_mul(ADMISSION_BYTES_PER_SAMPLE);
    let (resident_master_bytes, largest_master_bytes) =
        master_plan
            .sessions
            .iter()
            .fold((0_u64, 0_u64), |(total, largest), session| {
                let bytes = session.masters.image_buffer_bytes() as u64;
                (total.saturating_add(bytes), largest.max(bytes))
            });
    // The plan keeps every session. Swapping a deep-cloned active master set
    // can briefly retain both the previous and incoming stacker copies.
    let master_clone_bytes = largest_master_bytes.saturating_mul(2);
    let master_reserve_bytes = resident_master_bytes.saturating_add(master_clone_bytes);
    let reserved_bytes = admission_baseline_bytes.saturating_add(master_reserve_bytes);
    // Initialization, checkpoints and final rejection run without the frame
    // preparation queue. Check their peak separately, not as simultaneous work.
    let serial_peak_bytes = (output_samples as u64)
        .saturating_mul(super::STACK_BYTES_PER_OUTPUT_SAMPLE)
        .saturating_add(master_reserve_bytes);
    if serial_peak_bytes == u64::MAX {
        return Err("Stack memory estimate exceeds the supported address space".into());
    }
    let total_budget_bytes =
        available_bytes.map(|available| (available as f64 * policy.memory_budget_fraction) as u64);
    if let Some(total) = total_budget_bytes
        && serial_peak_bytes > total
    {
        return Err(format!(
            "Insufficient stack memory budget for serial processing: {} MiB needed for \
             initialization, checkpoints or final rejection including masters, {} MiB available \
             (reference-sized estimate)",
            serial_peak_bytes.div_ceil(MIB),
            total / MIB,
        ));
    }
    let pipeline_bytes = total_budget_bytes.map_or(UNKNOWN_RAM_PIPELINE_BYTES, |total| {
        total.saturating_sub(reserved_bytes)
    });
    let max_in_flight_bytes = usize::try_from(pipeline_bytes).unwrap_or(usize::MAX);

    // Seiza's budget includes the frame currently being integrated; do not
    // subtract that from the budget twice.
    let memory = PoolPipelineMemory::for_reference(output_pixels, output_samples);
    let worker_bytes = memory.worker_bytes as u64;
    let integration_bytes = memory.integration_bytes as u64;
    let sequential_bytes = memory.sequential_bytes as u64;
    let affordable = (max_in_flight_bytes as u64).saturating_sub(integration_bytes) / worker_bytes;
    if total_budget_bytes.is_none()
        && threads.serial
        && sequential_bytes > UNKNOWN_RAM_PIPELINE_BYTES
    {
        return Err(format!(
            "Available RAM is unknown: serial preparation needs {} MiB of scratch space, \
             exceeding the bounded {} MiB allowance (reference-sized estimate)",
            sequential_bytes.div_ceil(MIB),
            UNKNOWN_RAM_PIPELINE_BYTES / MIB,
        ));
    }
    if affordable == 0 && total_budget_bytes.is_none() && !threads.serial {
        return Err(format!(
            "Available RAM is unknown: the bounded {} MiB preparation allowance cannot fit \
             one preparation worker and integration frame ({} MiB estimated); \
             memory-limited serial fallback requires a known RAM budget",
            max_in_flight_bytes as u64 / MIB,
            worker_bytes.saturating_add(integration_bytes).div_ceil(MIB),
        ));
    }
    let memory_limited = affordable == 0 && !threads.serial;
    let mut threads = if threads.serial || memory_limited {
        threads.sequential()
    } else {
        *threads
    };
    let workers = if threads.serial {
        1
    } else {
        threads
            .preparation_workers
            .min(policy.hard_max_workers.max(1))
            .min(seiza_stacking::MAXIMUM_WORKERS)
            .min(usize::try_from(affordable).unwrap_or(usize::MAX))
    };
    if !threads.serial {
        // A memory-limited reader count leaves CPU slots for the shared pool.
        let total = threads
            .compute_workers
            .saturating_add(threads.preparation_workers);
        threads.preparation_workers = workers;
        threads.compute_workers = total.saturating_sub(workers).max(1);
    }
    Ok(PipelineBudget {
        options: PipelineOptions {
            max_in_flight_bytes: if threads.serial {
                0
            } else {
                max_in_flight_bytes
            },
            normalized_full_scale: Some(crate::image_io::NORMALIZED_FULL_SCALE),
            workers: Some(workers),
        },
        threads,
        total_budget_bytes,
        admission_baseline_bytes,
        serial_peak_bytes,
        resident_master_bytes,
        master_clone_bytes,
        integration_bytes,
        worker_bytes,
        sequential_bytes,
        memory_limited,
        workers,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calibration::{AppliedCalibration, CalibrationSession};
    use seiza_stacking::{
        CalibrationMasters, Continue, FrameDisposition, LinearImage, PipelineExecution,
        StackOptions,
    };

    fn pipeline_fixture() -> (tempfile::TempDir, LiveStacker, Vec<PathBuf>) {
        let (width, height) = (192, 160);
        // Match Seiza's pipeline fixture: a nondegenerate star field and a
        // measured background spread, so the default normalization is valid.
        let stars = (0..24)
            .map(|index| {
                let x = ((index * 7919) % 1000) as f32 / 1000.0 * (width as f32 - 24.0) + 12.0;
                let y = ((index * 6271) % 1000) as f32 / 1000.0 * (height as f32 - 24.0) + 12.0;
                (x, y, 6000.0 + ((index * 37) % 41) as f32 * 300.0)
            })
            .collect::<Vec<_>>();
        let directory = tempfile::tempdir().unwrap();
        let mut paths = (0..3)
            .map(|frame| {
                let shift_x = ((frame * 13) % 5) as f32 - 2.0;
                let shift_y = ((frame * 7) % 3) as f32 - 1.0;
                let data = (0..width * height)
                    .map(|index| {
                        let (x, y) = (index % width, index / width);
                        let noise = ((x * 17 + y * 31 + frame * 11) % 23) as f32 * 1.5;
                        let mut value = 1000.0 + noise;
                        for &(star_x, star_y, brightness) in &stars {
                            let dx = x as f32 - (star_x + shift_x);
                            let dy = y as f32 - (star_y + shift_y);
                            let radius_squared = dx.mul_add(dx, dy * dy);
                            if radius_squared < 40.0 {
                                value += brightness * (-radius_squared / 3.2).exp();
                            }
                        }
                        value.clamp(0.0, 65535.0) as u16 as f32
                    })
                    .collect::<Vec<_>>();
                let path = directory.path().join(format!("source-{frame}.fits"));
                seiza_fits::write_f32_image(
                    &path,
                    width,
                    height,
                    seiza_fits::F32ImageData::Mono(&data),
                    &[
                        seiza_fits::WriteHeaderCard::new(
                            "IMAGETYP",
                            seiza_fits::HeaderValue::String("LIGHT".into()),
                        ),
                        seiza_fits::WriteHeaderCard::new(
                            "EXPTIME",
                            seiza_fits::HeaderValue::Float(60.0),
                        ),
                    ],
                )
                .unwrap();
                path
            })
            .collect::<Vec<_>>();
        let reference = crate::image_io::open_linear_frame(paths.remove(0)).unwrap();
        let stacker = LiveStacker::new(
            reference,
            CalibrationMasters::default(),
            StackOptions::default(),
        )
        .unwrap();
        (directory, stacker, paths)
    }

    fn compute_pool(threads: &ThreadBudget) -> rayon::ThreadPool {
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads.compute_workers)
            .build()
            .unwrap()
    }

    fn policy() -> WorkerPolicy {
        WorkerPolicy {
            memory_budget_fraction: 1.0,
            ..WorkerPolicy::default()
        }
    }

    fn raw_plan() -> CalibrationPlan {
        CalibrationPlan::without_calibration(10)
    }

    fn allowance(total: usize) -> ThreadBudget {
        ThreadBudget::from_total(total)
    }

    fn session(pixels: usize) -> CalibrationSession {
        CalibrationSession {
            masters: CalibrationMasters::new(
                Some(LinearImage::new(pixels, 1, 1, vec![10.0; pixels]).unwrap()),
                None,
                None,
            )
            .unwrap(),
            applied: AppliedCalibration::default(),
        }
    }

    #[test]
    fn zero_dimensions_workers_and_invalid_policy_are_refused() {
        assert!(plan_pipeline(None, &policy(), 0, 0, &raw_plan(), &allowance(4)).is_err());
        assert!(plan_pipeline(None, &policy(), 10, 9, &raw_plan(), &allowance(4)).is_err());
        let invalid = ThreadBudget {
            compute_workers: 0,
            ..allowance(4)
        };
        assert!(plan_pipeline(None, &policy(), 10, 10, &raw_plan(), &invalid).is_err());
        for fraction in [f64::NAN, f64::INFINITY, -0.1, 0.0, 1.1] {
            let policy = WorkerPolicy {
                memory_budget_fraction: fraction,
                ..policy()
            };
            assert!(plan_pipeline(None, &policy, 10, 10, &raw_plan(), &allowance(4)).is_err());
        }
    }

    #[test]
    fn tiny_budget_does_not_force_an_unaffordable_worker() {
        let baseline = 100 * ADMISSION_BYTES_PER_SAMPLE;
        let one_worker = 100 * 80;
        let integrating = 100 * 4;
        let serial_peak = 100 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE;
        for available in [0, baseline, serial_peak - 1] {
            assert!(plan_pipeline(
                Some(available),
                &policy(),
                100,
                100,
                &raw_plan(),
                &allowance(4)
            )
            .is_err());
        }
        let plan = plan_pipeline(
            Some(baseline + one_worker + integrating),
            &policy(),
            100,
            100,
            &raw_plan(),
            &allowance(4),
        )
        .unwrap();
        assert_eq!(plan.options.workers, Some(1));
        assert_eq!(
            plan.options.max_in_flight_bytes as u64,
            one_worker + integrating
        );
    }

    #[test]
    fn rgb_samples_reduce_worker_capacity_without_triple_charging_detector_pixels() {
        let budget = 30_000;
        let mono = plan_pipeline(
            Some(budget),
            &policy(),
            100,
            100,
            &raw_plan(),
            &allowance(8),
        )
        .unwrap();
        let rgb = plan_pipeline(
            Some(budget),
            &policy(),
            100,
            300,
            &raw_plan(),
            &allowance(8),
        )
        .unwrap();
        assert_eq!(mono.worker_bytes, 8_000);
        assert_eq!(rgb.worker_bytes, 11_200);
        assert_eq!(rgb.integration_bytes, 1_200);
        assert_eq!(mono.options.workers, Some(2));
        assert_eq!(rgb.options.workers, Some(1));
        assert_eq!(rgb.threads.preparation_workers, 1);
        assert_eq!(rgb.threads.compute_workers, 7);
    }

    #[test]
    fn reserves_all_sessions_and_two_largest_active_master_copies() {
        let mut masters = raw_plan();
        masters.sessions = vec![session(100), session(200), session(50)];
        let budget =
            plan_pipeline(Some(100_000), &policy(), 100, 100, &masters, &allowance(4)).unwrap();
        assert_eq!(budget.resident_master_bytes, 1_400);
        assert_eq!(budget.master_clone_bytes, 1_600);
        assert_eq!(
            budget.options.max_in_flight_bytes as u64,
            100_000 - budget.admission_baseline_bytes - 1_400 - 1_600,
        );
    }

    #[test]
    fn unknown_ram_uses_a_bounded_fallback_and_preserves_compute_limit() {
        let budget = plan_pipeline(None, &policy(), 100, 100, &raw_plan(), &allowance(3)).unwrap();
        assert_eq!(
            budget.options.max_in_flight_bytes as u64,
            UNKNOWN_RAM_PIPELINE_BYTES
        );
        assert_eq!(budget.options.workers, Some(1));
        assert_eq!(
            budget.options.normalized_full_scale,
            Some(crate::image_io::NORMALIZED_FULL_SCALE),
        );
        assert!(budget.summary().contains("available RAM unknown"));
        assert!(budget.summary().contains("serial peak cannot be checked"));
        assert!(budget.summary().contains("not an RSS limit"));
    }

    #[test]
    fn separate_phases_fit_without_reserving_their_sum() {
        let (pixels, samples) = (100, 300);
        let admission = samples as u64 * ADMISSION_BYTES_PER_SAMPLE;
        let serial_peak = samples as u64 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE;
        let pipeline = PoolPipelineMemory::for_reference(pixels, samples).in_flight_bytes(1) as u64;
        let total = serial_peak.max(admission + pipeline);
        assert!(total < serial_peak + pipeline);
        let budget = plan_pipeline(
            Some(total),
            &policy(),
            pixels,
            samples,
            &raw_plan(),
            &allowance(4),
        )
        .unwrap();
        assert_eq!(budget.admission_baseline_bytes, admission);
        assert_eq!(budget.serial_peak_bytes, serial_peak);
        assert_eq!(budget.options.max_in_flight_bytes as u64, total - admission);
        assert_eq!(budget.options.workers, Some(1));
    }

    #[test]
    fn serial_peak_is_checked_even_when_a_preparation_worker_would_fit() {
        let (pixels, samples) = (100, 300);
        let admission = samples as u64 * ADMISSION_BYTES_PER_SAMPLE;
        let pipeline = PoolPipelineMemory::for_reference(pixels, samples).in_flight_bytes(1) as u64;
        let total = admission + pipeline;
        assert!(total < samples as u64 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE);
        let error = plan_pipeline(
            Some(total),
            &policy(),
            pixels,
            samples,
            &raw_plan(),
            &allowance(4),
        )
        .err()
        .unwrap();
        assert!(error.contains("serial processing"));
    }

    #[test]
    fn serial_preparation_is_selected_when_only_the_serial_peak_fits() {
        let total = 100 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE;
        let budget =
            plan_pipeline(Some(total), &policy(), 100, 100, &raw_plan(), &allowance(8)).unwrap();
        assert!(budget.threads.serial);
        assert!(budget.memory_limited);
        assert_eq!(budget.threads.compute_workers, 8);
        assert_eq!(budget.options.max_in_flight_bytes, 0);
        assert!(budget.summary().contains("memory limit"));
    }

    #[test]
    fn full_sensor_serial_stack_remains_available_without_a_preparation_queue() {
        let pixels = 61_000_000;
        let total = 6034 * MIB;
        let budget = plan_pipeline(
            Some(total),
            &policy(),
            pixels,
            pixels,
            &raw_plan(),
            &allowance(8),
        )
        .unwrap();
        assert!(budget.threads.serial);
        assert!(budget.memory_limited);
        assert_eq!(budget.threads.compute_workers, 8);
        assert_eq!(budget.options.max_in_flight_bytes, 0);
        assert!(budget.serial_peak_bytes <= total);
    }

    #[test]
    fn unknown_ram_refuses_an_unbounded_memory_fallback() {
        let error = plan_pipeline(
            None,
            &policy(),
            61_000_000,
            61_000_000,
            &raw_plan(),
            &allowance(8),
        )
        .err()
        .unwrap();
        assert!(error.contains("Available RAM is unknown"));
        assert!(error.contains("bounded 1024 MiB preparation allowance"));
        assert!(error.contains("serial fallback requires a known RAM budget"));
    }

    #[test]
    fn one_cpu_unknown_ram_checks_serial_scratch_not_a_nonexistent_queue() {
        let one_pixel = PoolPipelineMemory::for_reference(1, 1);
        let pixels = UNKNOWN_RAM_PIPELINE_BYTES as usize / one_pixel.in_flight_bytes(1) + 1;
        let memory = PoolPipelineMemory::for_reference(pixels, pixels);
        assert!(memory.in_flight_bytes(1) as u64 > UNKNOWN_RAM_PIPELINE_BYTES);
        assert!(memory.sequential_bytes as u64 <= UNKNOWN_RAM_PIPELINE_BYTES);
        let budget =
            plan_pipeline(None, &policy(), pixels, pixels, &raw_plan(), &allowance(1)).unwrap();
        assert!(budget.threads.serial);
        assert!(!budget.memory_limited);
        assert_eq!(budget.options.max_in_flight_bytes, 0);
        assert!(budget.summary().contains("1024 MiB scratch allowance"));

        let too_large = UNKNOWN_RAM_PIPELINE_BYTES as usize / one_pixel.sequential_bytes + 1;
        let error = plan_pipeline(
            None,
            &policy(),
            too_large,
            too_large,
            &raw_plan(),
            &allowance(1),
        )
        .err()
        .unwrap();
        assert!(error.contains("serial preparation needs"));
        assert!(error.contains("bounded 1024 MiB allowance"));
    }

    #[test]
    fn serial_peak_includes_all_session_masters_and_clone_reserve() {
        let mut masters = raw_plan();
        masters.sessions = vec![session(100), session(200), session(50)];
        let bare_serial_peak = 300 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE;
        let total = bare_serial_peak + 1_400 + 1_600;
        let budget =
            plan_pipeline(Some(total), &policy(), 100, 300, &masters, &allowance(4)).unwrap();
        assert_eq!(budget.serial_peak_bytes, total);
        let error = plan_pipeline(
            Some(total - 1),
            &policy(),
            100,
            300,
            &masters,
            &allowance(4),
        )
        .err()
        .unwrap();
        assert!(error.contains("serial processing"));
    }

    #[test]
    fn memory_fraction_and_worker_caps_are_applied() {
        let configured = WorkerPolicy {
            memory_budget_fraction: 0.5,
            hard_max_workers: 2,
            ..policy()
        };
        let budget = plan_pipeline(
            Some(200_000),
            &configured,
            100,
            100,
            &raw_plan(),
            &allowance(16),
        )
        .unwrap();
        assert_eq!(budget.total_budget_bytes, Some(100_000));
        assert_eq!(budget.options.workers, Some(2));
        let capped =
            plan_pipeline(None, &policy(), 1, 1, &raw_plan(), &allowance(usize::MAX)).unwrap();
        assert_eq!(capped.options.workers, Some(2));
    }

    #[test]
    fn thread_budget_counts_decoders_and_compute_within_the_total() {
        for total in [0, 1] {
            assert_eq!(
                ThreadBudget::from_total(total),
                ThreadBudget {
                    compute_workers: 1,
                    preparation_workers: 1,
                    serial: true,
                },
            );
        }
        for (total, readers) in [(2, 1), (3, 1), (4, 2), (7, 2), (16, 2), (usize::MAX, 2)] {
            let budget = ThreadBudget::from_total(total);
            assert!(!budget.serial);
            assert_eq!(budget.compute_workers + budget.preparation_workers, total);
            assert_eq!(budget.preparation_workers, readers);
            assert_eq!(budget.compute_workers, total - readers);
        }
    }

    #[test]
    fn parallel_wrapper_refuses_an_accidental_rayon_coordinator() {
        let threads = ThreadBudget::from_total(4);
        let pool = compute_pool(&threads);
        let (_directory, mut stacker, paths) = pool.install(pipeline_fixture);
        let result = pool.install(|| {
            run_pipeline(
                &mut stacker,
                &paths,
                &PipelineOptions::default(),
                &pool,
                &threads,
                |_, _| panic!("a nested parallel coordinator must not start the batch"),
            )
        });
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("coordinator outside the Rayon pool"),);
        assert!(stacker.input_paths().is_empty());
    }

    #[test]
    fn one_cpu_budget_uses_explicit_sequential_processing() {
        let threads = ThreadBudget::from_total(1);
        let pool = compute_pool(&threads);
        let (_directory, mut stacker, paths) = pool.install(pipeline_fixture);
        let mut seen = Vec::new();
        let coordinator = std::thread::current().id();
        let report = run_pipeline(
            &mut stacker,
            &paths,
            &PipelineOptions::default(),
            &pool,
            &threads,
            |path, outcome| {
                assert_eq!(std::thread::current().id(), coordinator);
                assert!(rayon::current_thread_index().is_none());
                let outcome = outcome.unwrap();
                assert!(
                    matches!(&outcome, FrameDisposition::Accepted(_)),
                    "{outcome:?}"
                );
                seen.push(path.to_path_buf());
                Continue::Yes
            },
        )
        .unwrap();
        assert_eq!(seen, paths);
        assert_eq!(report.execution, PipelineExecution::SequentialRequested);
        assert_eq!(report.workers, 1);
        assert_eq!(report.frames.integrated, 2);
    }

    #[test]
    fn parallel_wrapper_caps_readers_and_preserves_callback_order() {
        let threads = ThreadBudget::from_total(4);
        let pool = compute_pool(&threads);
        let (_directory, mut stacker, paths) = pool.install(pipeline_fixture);
        let options = PipelineOptions {
            workers: Some(64),
            ..PipelineOptions::default()
        };
        let coordinator = std::thread::current().id();
        let mut seen = Vec::new();
        let report = run_pipeline(
            &mut stacker,
            &paths,
            &options,
            &pool,
            &threads,
            |path, outcome| {
                assert_eq!(std::thread::current().id(), coordinator);
                assert!(rayon::current_thread_index().is_none());
                let outcome = outcome.unwrap();
                assert!(
                    matches!(&outcome, FrameDisposition::Accepted(_)),
                    "{outcome:?}"
                );
                seen.push(path.to_path_buf());
                Continue::Yes
            },
        )
        .unwrap();
        assert_eq!(seen, paths);
        assert_eq!(report.execution, PipelineExecution::Overlapped);
        assert_eq!(report.workers, threads.preparation_workers);
        assert_eq!(report.frames.integrated, 2);
    }

    #[test]
    fn memory_limited_wrapper_integrates_on_the_full_phase_pool() {
        let initial = allowance(4);
        let pixels = 192 * 160;
        let total = pixels as u64 * super::super::STACK_BYTES_PER_OUTPUT_SAMPLE;
        let budget = plan_pipeline(
            Some(total),
            &policy(),
            pixels,
            pixels,
            &raw_plan(),
            &initial,
        )
        .unwrap();
        assert!(budget.threads.serial);
        assert_eq!(budget.threads.compute_workers, 4);
        let pool = compute_pool(&budget.threads);
        let (_directory, mut stacker, paths) = pool.install(pipeline_fixture);
        let coordinator = std::thread::current().id();
        let mut seen = Vec::new();
        let report = run_pipeline(
            &mut stacker,
            &paths,
            &budget.options,
            &pool,
            &budget.threads,
            |path, outcome| {
                assert_eq!(std::thread::current().id(), coordinator);
                let outcome = outcome.unwrap();
                assert!(
                    matches!(&outcome, FrameDisposition::Accepted(_)),
                    "{outcome:?}"
                );
                seen.push(path.to_path_buf());
                Continue::Yes
            },
        )
        .unwrap();
        assert_eq!(seen, paths);
        assert_eq!(report.execution, PipelineExecution::SequentialRequested);
        assert_eq!(report.frames.integrated, 2);
        assert_eq!(report.workers, 1);
    }

    #[test]
    fn overflowing_image_estimates_saturate_and_fail_closed() {
        assert!(plan_pipeline(
            Some(u64::MAX),
            &policy(),
            usize::MAX,
            usize::MAX,
            &raw_plan(),
            &allowance(usize::MAX),
        )
        .is_err());
    }
}
