import type { ProjectOverview } from '../api/types';
import type { WithDb } from '../hooks/useDatabases';

/** Projects the Library shows as one: the same Target Scheduler GUID in
 *  several databases is one plan shot by several rigs. A project without a
 *  GUID, or alone with its GUID, is a family of one. Order follows the list
 *  given: a family sits where its first member does. */
export interface ProjectFamily {
  key: string;
  members: WithDb<ProjectOverview>[];
  /** The name the members share, the first member's when they differ. */
  name: string;
  accepted: number;
  desired: number;
  totalImages: number;
  /** The newest capture across the members, in seconds, if any has one. */
  latest: number | null;
}

export function projectFamilies(projects: WithDb<ProjectOverview>[]): ProjectFamily[] {
  const order: string[] = [];
  const byKey = new Map<string, WithDb<ProjectOverview>[]>();
  for (const project of projects) {
    const key = project.guid ? `guid:${project.guid.toLowerCase()}` : `one:${project.db_id}:${project.id}`;
    if (!byKey.has(key)) { byKey.set(key, []); order.push(key); }
    byKey.get(key)!.push(project);
  }
  return order.map(key => {
    const members = byKey.get(key)!;
    const latest = members.map(m => m.date_range.latest ?? 0).reduce((a, b) => Math.max(a, b), 0);
    return {
      key,
      members,
      name: members[0].display_name,
      accepted: members.reduce((sum, m) => sum + m.accepted_images, 0),
      desired: members.reduce((sum, m) => sum + m.total_desired, 0),
      totalImages: members.reduce((sum, m) => sum + m.total_images, 0),
      latest: latest > 0 ? latest : null,
    };
  });
}

/** Percent of the desired frames accepted, whole numbers; null without a goal. */
export function percentDone(accepted: number, desired: number): number | null {
  return desired > 0 ? Math.round((accepted / desired) * 100) : null;
}

/** "3 d ago", "2 h ago", "just now"; null without a time. */
export function ago(seconds: number | null | undefined, nowMs: number): string | null {
  if (!seconds) return null;
  const delta = Math.max(0, nowMs / 1000 - seconds);
  if (delta < 3600) return 'just now';
  if (delta < 86_400) return `${Math.round(delta / 3600)} h ago`;
  if (delta < 86_400 * 60) return `${Math.round(delta / 86_400)} d ago`;
  return `${Math.round(delta / (86_400 * 30))} mo ago`;
}

/** Target Scheduler's project states as the Library names them. */
export function stateLabel(state: number): string {
  switch (state) {
    case 0: return 'Draft';
    case 1: return 'Active';
    case 2: return 'Inactive';
    case 3: return 'Closed';
    default: return `State ${state}`;
  }
}
