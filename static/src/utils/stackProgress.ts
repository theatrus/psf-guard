import type { MasterBuildProgress, StackGroupStatus, StackMethod } from '../api/types';

/**
 * A channel's progress in frame reads, as the server weighs it: calibration
 * counts as one pass over the frames, the live pass reads each eligible frame
 * once, and a final pass three more times per admitted frame. Before the
 * final pass starts its size is estimated from the eligible frames. Returns
 * 0–100.
 */
export function stackGroupPercent(
  group: Pick<StackGroupStatus, 'state' | 'eligible_frames' | 'processed_frames' | 'final_pass'>
    & Partial<Pick<StackGroupStatus, 'phase' | 'calibration_progress'>>,
  method: StackMethod | null | undefined
): number {
  if (group.state === 'ready') return 100;
  const live = group.eligible_frames;
  if (live <= 0) return 0;
  // Settled once the channel is past it, whether masters were built, found
  // in the cache, or not wanted.
  const calibration = live;
  const master = group.calibration_progress;
  const calibrated = group.state === 'queued'
    ? 0
    : group.state === 'running' && group.phase === 'calibration'
      ? master ? masterBuildShare(master) * calibration : 0
      : calibration;
  const pass = group.final_pass;
  if (pass) {
    const total = pass.passes * pass.frames;
    const done = Math.min((pass.pass - 1) * pass.frames + pass.frame, total);
    return Math.min(100, ((calibrated + live + done) / (calibration + live + total)) * 100);
  }
  const reintegrates = method?.final_pass !== 'draft' && live >= 3;
  const total = calibration + live + (reintegrates ? 3 * live : 0);
  return Math.min(100, ((calibrated + Math.min(group.processed_frames, live)) / total) * 100);
}

const MASTER_NAMES: Record<MasterBuildProgress['kind'], string> = {
  bias: 'bias',
  dark: 'dark',
  dark_flat: 'dark flat',
  flat: 'flat',
};

/**
 * How far a channel's calibration has got, 0 to 1, without knowing how many
 * masters it will build: each build covers half of what is left, so the share
 * only grows. Matches the server.
 */
export function masterBuildShare(progress: MasterBuildProgress): number {
  const left = 0.5 ** (Math.max(progress.build ?? 1, 1) - 1);
  return 1 - left * (1 - 0.5 * Math.min(1, Math.max(0, progress.fraction)));
}

const STAGE_STEPS: Record<MasterBuildProgress['stage'], string> = {
  read: 'reading frame',
  reread: 'rereading kept frame',
  integrate: 'integrating frame',
  combine: 'combining tile',
};

/**
 * `master flat L · reading frame 12/40`, or `master dark · integrating frame
 * 3/8`. Matches the header queue's wording.
 */
export function masterBuildLabel(progress: MasterBuildProgress): string {
  const filter = (progress.kind === 'flat' || progress.kind === 'dark_flat') && progress.filter
    ? ` ${progress.filter}`
    : '';
  const step = Math.min(progress.done + 1, progress.total);
  return `master ${MASTER_NAMES[progress.kind]}${filter} · ${STAGE_STEPS[progress.stage]} ${step}/${progress.total}`;
}
