import { withoutPlanningParams } from '../../hooks/useUrlState';

/**
 * The calibration page for one database, from wherever a person is. The
 * scope comes along; a project from another database does not.
 */
export function calibrationPageHref(search: string, dbId: string, projectId?: number): string {
  const params = withoutPlanningParams(search);
  if (params.get('db') !== dbId) {
    for (const key of ['project', 'target', 'mosaic']) params.delete(key);
  }
  params.set('db', dbId);
  if (projectId !== undefined) params.set('project', String(projectId));
  return `/calibration?${params}`;
}
