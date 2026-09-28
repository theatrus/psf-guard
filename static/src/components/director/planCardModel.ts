import type { DirectorPlanRow, DirectorTargetProgress } from '../../api/directorTypes';

export interface FrameCounts { desired: number; acquired: number; accepted: number; rejected: number }

/** Frames accepted against desired, for a plan, a rig or one panel. */
export function frames(progress: { desired: number; accepted: number }): string {
  return `${progress.accepted}/${progress.desired} frames`;
}

export function sumFrames(targets: DirectorTargetProgress[]): FrameCounts {
  return targets.reduce<FrameCounts>((sum, target) => ({
    desired: sum.desired + target.desired,
    acquired: sum.acquired + target.acquired,
    accepted: sum.accepted + target.accepted,
    rejected: sum.rejected + target.rejected,
  }), { desired: 0, acquired: 0, accepted: 0, rejected: 0 });
}

/** Whole percent of desired frames accepted, capped at 100; 0 without a goal. */
export function percentDone(counts: { desired: number; accepted: number }): number {
  return counts.desired > 0 ? Math.min(100, Math.round((counts.accepted / counts.desired) * 100)) : 0;
}

/** How the acquired frames graded, as Overview draws it: accepted, rejected,
 *  and whatever is left pending. */
export function gradingSplit(counts: FrameCounts): { pending: number; acceptedPct: number; rejectedPct: number; pendingPct: number } {
  const pending = Math.max(0, counts.acquired - counts.accepted - counts.rejected);
  const total = counts.accepted + counts.rejected + pending;
  const pct = (n: number) => (total > 0 ? (n / total) * 100 : 0);
  return { pending, acceptedPct: pct(counts.accepted), rejectedPct: pct(counts.rejected), pendingPct: pct(pending) };
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** Where the plan stands, from activated back to unlinked. */
export function stage(row: DirectorPlanRow): string {
  if (row.activation) return `Activated rev ${row.activation.revision} on ${new Date(row.activation.applied_at_ms).toLocaleDateString()}, ${plural(row.activation.rigs, 'rig')}`;
  if (row.plan && row.plan.objectives > 0) return `Planned: ${plural(row.plan.objectives, 'objective')}, ${plural(row.plan.rigs, 'rig')}, not activated`;
  if (row.framing?.source === 'catalog') return `Framed in Target Scheduler: ${row.framing.target_name || 'target'}${row.framing.panels > 1 ? `, ${row.framing.panels} targets` : ''}; open to plan it in Director`;
  if (row.framing) return `Framed: ${row.framing.target_name || 'target'}, ${plural(row.framing.panels, 'panel')}`;
  return row.links.length ? 'Linked; its database has no target with coordinates yet' : 'Not linked to any database';
}
