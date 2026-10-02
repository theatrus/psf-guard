import type { StackGroupStatus, StackMethod } from '../api/types';

/**
 * A channel's progress in frame reads, as the server weighs it: the live pass
 * reads each eligible frame once, and a final pass three more times per
 * admitted frame. Before the final pass starts its size is estimated from the
 * eligible frames. Returns 0–100.
 */
export function stackGroupPercent(
  group: Pick<StackGroupStatus, 'state' | 'eligible_frames' | 'processed_frames' | 'final_pass'>,
  method: StackMethod | null | undefined
): number {
  if (group.state === 'ready') return 100;
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
