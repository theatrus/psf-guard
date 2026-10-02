import type { MasterBuildProgress, StackGroupStatus, StackMethod } from '../api/types';

/**
 * A channel's progress in frame reads, as the server weighs it: the live pass
 * reads each eligible frame once, and a final pass three more times per
 * admitted frame. Before the final pass starts its size is estimated from the
 * eligible frames. Returns 0–100.
 */
export function stackGroupPercent(
  group: Pick<StackGroupStatus, 'state' | 'eligible_frames' | 'processed_frames' | 'final_pass'>
    & Partial<Pick<StackGroupStatus, 'phase' | 'calibration_progress'>>,
  method: StackMethod | null | undefined
): number {
  if (group.state === 'ready') return 100;
  // While a master builds, the bar follows that master; its label names it.
  const master = group.phase === 'calibration' ? group.calibration_progress : null;
  if (master) return masterBuildPercent(master);
  const live = group.eligible_frames;
  if (live <= 0) return 0;
  const pass = group.final_pass;
  if (pass) {
    const total = pass.passes * pass.frames;
    const done = Math.min((pass.pass - 1) * pass.frames + pass.frame, total);
    return Math.min(100, ((live + done) / (live + total)) * 100);
  }
  const reintegrates = method?.final_pass !== 'draft' && live >= 3;
  const total = live + (reintegrates ? 3 * live : 0);
  return Math.min(100, (Math.min(group.processed_frames, live) / total) * 100);
}

const MASTER_NAMES: Record<MasterBuildProgress['kind'], string> = {
  bias: 'bias',
  dark: 'dark',
  dark_flat: 'dark flat',
  flat: 'flat',
};

/** Frame reads finished, of all a master build needs, as the server counts them. */
export function masterBuildReads(progress: MasterBuildProgress): [number, number] {
  const total = progress.frames * progress.passes;
  return [Math.min(total, (progress.pass - 1) * progress.frames + progress.frame - 1), total];
}

/** {@link masterBuildReads} as 0–100. */
export function masterBuildPercent(progress: MasterBuildProgress): number {
  const [done, total] = masterBuildReads(progress);
  return total > 0 ? (done / total) * 100 : 0;
}

/**
 * `master flat L · frame 12/40`, with the pass for a bias or dark, which
 * reads its frames twice. Matches the header queue's wording.
 */
export function masterBuildLabel(progress: MasterBuildProgress): string {
  const filter = (progress.kind === 'flat' || progress.kind === 'dark_flat') && progress.filter
    ? ` ${progress.filter}`
    : '';
  const pass = progress.passes > 1 ? ` · pass ${progress.pass}/${progress.passes}` : '';
  return `master ${MASTER_NAMES[progress.kind]}${filter}${pass} · frame ${progress.frame}/${progress.frames}`;
}
