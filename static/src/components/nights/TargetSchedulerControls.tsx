import { useState } from 'react';
import { useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { NightPlanPriority, RigNights, TsProject } from '../../api/nightPlanTypes';
import { httpStatus } from '../../utils/httpStatus';
import { useDraftSection } from '../director/pageDraftsState';
import { describeTsChanges, isEmpty, PRIORITY_LABEL, STATE_LABEL, tsChanges, type TsEdit } from './tsControlsModel';

/** Everything a Target Scheduler change moves: the night plans wherever
 *  they are drawn, and the plans' estimates. */
const CHANGED_QUERIES = [['nightPlans'], ['nightPlanDetail'], ['nightPlansTonight'], ['directorProjection'], ['directorFeasibility']] as const;

interface Props {
  rig: RigNights;
  /** The rig's projects as its database holds them. */
  saved: TsProject[];
  canEdit: boolean;
  /** The open edit, which the schedule above plans; `null` when none is. */
  edit: TsEdit | null;
  onEdit: (edit: TsEdit | null) => void;
}

const ORDER: Record<TsProject['state'], number> = { active: 0, inactive: 1, draft: 2, closed: 3 };

/**
 * What a Target Scheduler rig's night is planned from, and the settings to
 * change it: each project's priority and whether it is active, and whether
 * each target is. The schedule above plans an edit before it is saved.
 * Saved changes are Target Scheduler's own settings: they last until
 * changed back, for every night, not this one only.
 */
export default function TargetSchedulerControls({ rig, saved, canEdit, edit, onEdit }: Props) {
  const client = useQueryClient();
  const [refused, setRefused] = useState<string | null>(null);
  const changes = tsChanges(saved, edit ?? { projects: {}, targets: {} });
  const where = rig.ts_save.files.length > 1
    ? `the rig's database and Target Scheduler's own file (${rig.ts_save.files[1]})`
    : "the rig's database, the file Target Scheduler reads";
  useDraftSection('target-scheduler', {
    label: 'Target Scheduler settings',
    order: 2,
    unsaved: edit !== null && !isEmpty(changes),
    changes: describeTsChanges(saved, changes),
    save: async () => {
      try {
        await apiClient.saveTargetSchedulerChanges(rig.rig.id, changes);
        setRefused(null);
        onEdit(null);
        for (const queryKey of CHANGED_QUERIES) void client.invalidateQueries({ queryKey: [...queryKey] });
        return true;
      } catch (error) {
        const message = error instanceof Error ? error.message : 'the server refused them.';
        // A row changed since it was read: read the rig again rather than
        // save over it.
        if (httpStatus(error) === 409) {
          setRefused(`Target Scheduler's settings were not saved: ${message}`);
          onEdit(null);
          for (const queryKey of CHANGED_QUERIES) void client.invalidateQueries({ queryKey: [...queryKey] });
        }
        return message;
      }
    },
    discard: () => onEdit(null),
  });
  const projects = [...saved].sort((a, b) => ORDER[a.state] - ORDER[b.state] || a.name.localeCompare(b.name));
  const setProject = (project: TsProject, change: { priority?: NightPlanPriority; active?: boolean }) => {
    if (!edit) return;
    const current = edit.projects[project.project_id] ?? {};
    const state = change.active === undefined ? current.state : change.active ? 'active' : project.state === 'active' ? 'inactive' : project.state;
    onEdit({ ...edit, projects: { ...edit.projects, [project.project_id]: { ...current, ...(change.priority ? { priority: change.priority } : {}), ...(state ? { state } : {}) } } });
  };
  const setTarget = (target: number, active: boolean) => edit && onEdit({ ...edit, targets: { ...edit.targets, [target]: active } });
  return <section className="nights-ts-controls" aria-label="Target Scheduler settings">
    <div className="nights-mark-row">
      <h4>Target Scheduler settings</h4>
      {canEdit && !edit && !rig.ts_save.refused && saved.length > 0 && <button type="button" onClick={() => { setRefused(null); onEdit({ projects: {}, targets: {} }); }}>Change settings</button>}
    </div>
    <p className="director-muted">Target Scheduler picks this rig's targets from its database. {rig.ts_save.refused
      ? rig.ts_save.refused
      : `Changes here are saved to ${where}. They are Target Scheduler's own settings, so they last until changed back, for every night.`}</p>
    {refused && !edit && <p className="director-error" role="alert">{refused}</p>}
    {saved.length === 0 && <p className="director-muted">The rig's database has no projects for its N.I.N.A. profile.</p>}
    {saved.length > 0 && <div className="director-table-scroll"><table className="nights-plan-table">
      <thead><tr><th>Project or target</th><th>Active</th><th>Priority</th></tr></thead>
      <tbody>{projects.flatMap(project => {
        const edited = edit?.projects[project.project_id];
        const state = edited?.state ?? project.state;
        const priority = edited?.priority ?? project.priority;
        return [
          <tr key={`p${project.project_id}`} className="nights-ts-project">
            <th scope="row">{project.name}{state !== 'active' && state !== 'inactive' ? ` (${STATE_LABEL[state].toLowerCase()})` : ''}</th>
            <td><input type="checkbox" aria-label={`${project.name} active`} checked={state === 'active'} disabled={!edit}
              onChange={event => setProject(project, { active: event.target.checked })} /></td>
            <td><select aria-label={`${project.name} priority`} value={priority} disabled={!edit}
              onChange={event => setProject(project, { priority: event.target.value as NightPlanPriority })}>
              {(Object.keys(PRIORITY_LABEL) as NightPlanPriority[]).map(option => <option key={option} value={option}>{PRIORITY_LABEL[option]}</option>)}
            </select></td>
          </tr>,
          ...project.targets.map(target => <tr key={`t${target.target_id}`} className="nights-ts-target">
            <td>{target.name}</td>
            <td><input type="checkbox" aria-label={`${project.name} · ${target.name} active`} checked={edit?.targets[target.target_id] ?? target.active} disabled={!edit}
              onChange={event => setTarget(target.target_id, event.target.checked)} /></td>
            <td />
          </tr>),
        ];
      })}</tbody>
    </table></div>}
    {edit && <p className="director-muted">The schedule above shows these changes before they are saved.{isEmpty(changes) && ' '}{isEmpty(changes) && <button type="button" onClick={() => onEdit(null)}>Stop changing</button>}</p>}
  </section>;
}
