import type { DirectorPlanRow, DirectorTargetProgress } from '../../api/directorTypes';

export interface FrameCounts { desired: number; acquired: number; accepted: number; rejected: number }

export function sumFrames(targets: DirectorTargetProgress[]): FrameCounts {
  return targets.reduce<FrameCounts>((sum, target) => ({
    desired: sum.desired + target.desired,
    acquired: sum.acquired + target.acquired,
    accepted: sum.accepted + target.accepted,
    rejected: sum.rejected + target.rejected,
  }), { desired: 0, acquired: 0, accepted: 0, rejected: 0 });
}

const plural = (n: number, word: string) => `${n} ${word}${n === 1 ? '' : 's'}`;

/** Archived the way the Library archives a project: closed in Target
 *  Scheduler on every rig that shoots it. */
export function isArchivedPlan(row: DirectorPlanRow): boolean {
  return row.links.length > 0 && row.links.every(link => link.source_state === 3);
}

/** The stage in a word or two, for a compact row; the full line sits in
 *  the row's title. */
export function stageShort(row: DirectorPlanRow): string {
  if (row.activation) return 'Activated';
  if (row.plan && row.plan.objectives > 0) return 'Planned';
  if (row.framing?.source === 'catalog') return 'Framed in Target Scheduler';
  if (row.framing) return 'Framed';
  return row.links.length ? 'No target yet' : 'Not linked';
}

/** Where the plan stands, from activated back to unlinked. */
export function stage(row: DirectorPlanRow): string {
  if (row.activation) return `Activated rev ${row.activation.revision} on ${new Date(row.activation.applied_at_ms).toLocaleDateString()}, ${plural(row.activation.rigs, 'rig')}`;
  if (row.plan && row.plan.objectives > 0) return `Planned: ${plural(row.plan.objectives, 'objective')}, ${plural(row.plan.rigs, 'rig')}, not activated`;
  if (row.framing?.source === 'catalog') return `Framed in Target Scheduler: ${row.framing.target_name || 'target'}${row.framing.panels > 1 ? `, ${row.framing.panels} targets` : ''}; open to plan it in Director`;
  if (row.framing) return `Framed: ${row.framing.target_name || 'target'}, ${plural(row.framing.panels, 'panel')}`;
  return row.links.length ? 'Linked; its database has no target with coordinates yet' : 'Not linked to any database';
}

const listed = (words: string[]) => words.length <= 1 ? words.join('') : `${words.slice(0, -1).join(', ')} and ${words[words.length - 1]}`;

/** Why a plan needs activating, in the workspace's words, or null when its
 *  rigs have everything saved here. */
export function activationDueText(row: DirectorPlanRow): string | null {
  const due = row.activation_due;
  if (!due) return null;
  if (due.never) return 'Not activated yet';
  const rigName = (id: string) => row.links.find(link => link.rig.id === id)?.catalog_name ?? 'a rig';
  const reasons = [
    due.parts.length > 0 && `saved ${listed(due.parts)} not activated yet`,
    due.left_out.length > 0 && `${listed(due.left_out.map(rigName))} not activated yet`,
    due.differs.length > 0 && `${listed(due.differs.map(rigName))} ${due.differs.length === 1 ? 'differs' : 'differ'} from the plan in Target Scheduler`,
  ].filter((reason): reason is string => !!reason);
  return `Needs activating: ${reasons.join('; ')}`;
}
