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

/** Grid keys a link to some lights clears, so the grid shows exactly them. */
const GRID_FILTER_KEYS = ['target', 'mosaic', 'status', 'filter', 'flag', 'calib', 'dateStart', 'dateEnd', 'search', 'selected', 'current', 'expanded'];

/**
 * The Images grid for one project's lights that a calibration level or
 * finding code covers (see utils/calibrationFilter), in the given filters
 * or all of them.
 */
export function calibrationGridHref(
  search: string,
  dbId: string,
  projectId: number,
  calib: string,
  filterNames: readonly string[] = [],
): string {
  const params = withoutPlanningParams(search);
  for (const key of GRID_FILTER_KEYS) params.delete(key);
  params.set('db', dbId);
  params.set('project', String(projectId));
  params.set('calib', calib);
  // Each value encoded on its own, as the grid's list filters write them.
  if (filterNames.length > 0) params.set('filter', filterNames.map(encodeURIComponent).join(','));
  return `/grid?${params}`;
}
