import type { NightPlanPriority, TsChanges, TsProject, TsProjectState } from '../../api/nightPlanTypes';

/** An edit of a rig's Target Scheduler rows: what each changed project and
 *  target is set to. */
export interface TsEdit {
  projects: Record<number, { priority?: NightPlanPriority; state?: TsProjectState }>;
  targets: Record<number, boolean>;
}

export const NO_TS_EDIT: TsEdit = { projects: {}, targets: {} };

export const PRIORITY_LABEL: Record<NightPlanPriority, string> = { low: 'Low', normal: 'Normal', high: 'High' };
export const STATE_LABEL: Record<TsProjectState, string> = { draft: 'Draft', active: 'Active', inactive: 'Inactive', closed: 'Closed' };

/** The edit as changes to send: only fields that differ from what was
 *  read, each with what the row held. */
export function tsChanges(saved: TsProject[], edit: TsEdit): TsChanges {
  const projects: TsChanges['projects'] = [];
  const targets: TsChanges['targets'] = [];
  for (const project of saved) {
    const edited = edit.projects[project.project_id];
    const priority = edited?.priority !== undefined && edited.priority !== project.priority ? edited.priority : undefined;
    const state = edited?.state !== undefined && edited.state !== project.state ? edited.state : undefined;
    if (priority !== undefined || state !== undefined) {
      projects.push({ project_id: project.project_id, ...(priority ? { priority } : {}), ...(state ? { state } : {}), was: { priority: project.priority, state: project.state } });
    }
    for (const target of project.targets) {
      const active = edit.targets[target.target_id];
      if (active !== undefined && active !== target.active) targets.push({ target_id: target.target_id, active, was_active: target.active });
    }
  }
  return { projects, targets };
}

export function isEmpty(changes: TsChanges): boolean {
  return changes.projects.length === 0 && changes.targets.length === 0;
}

/** The changes in words, one per row. */
export function describeTsChanges(saved: TsProject[], changes: TsChanges): string[] {
  const projects = new Map(saved.map(project => [project.project_id, project]));
  const targets = new Map(saved.flatMap(project => project.targets.map(target => [target.target_id, { ...target, project: project.name }] as const)));
  return [
    ...changes.projects.map(change => {
      const name = projects.get(change.project_id)?.name ?? `Project ${change.project_id}`;
      const parts = [
        change.state ? STATE_LABEL[change.state].toLowerCase() : null,
        change.priority ? `priority ${PRIORITY_LABEL[change.priority]}` : null,
      ].filter(Boolean);
      return `${name}: ${parts.join(', ')}`;
    }),
    ...changes.targets.map(change => {
      const target = targets.get(change.target_id);
      return `${target ? `${target.project} · ${target.name}` : `Target ${change.target_id}`}: ${change.active ? 'active' : 'not active'}`;
    }),
  ];
}
