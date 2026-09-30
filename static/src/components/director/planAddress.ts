import type { DirectorPlanRow } from '../../api/directorTypes';
import { withoutPlanningParams } from '../../hooks/useUrlState';

const guidsOf = (row: DirectorPlanRow) => new Set(row.links.map(link => link.source_project_guid.toLowerCase()));

/** How a URL names a plan, in order of preference:
 *  - the Target Scheduler GUID its rigs share, so a link means the same plan
 *    on any PSF Guard instance that holds those databases. Only when no
 *    other plan carries that GUID: a detached project keeps its GUID, so
 *    after a detach two plans can hold one;
 *  - Planning's own plan id otherwise (a plan with no database, rigs with
 *    different GUIDs after an attach, or a GUID another plan shares);
 *  - `slug:row`, a database's project row, for a project Planning has not
 *    taken in yet. */
export function planKey(row: DirectorPlanRow, rows: DirectorPlanRow[]): string {
  const guids = guidsOf(row);
  if (guids.size !== 1) return row.project.id;
  const [guid] = guids;
  const shared = rows.some(other => other.project.id !== row.project.id && guidsOf(other).has(guid));
  return shared ? row.project.id : guid;
}

export type ResolvedPlan =
  | { kind: 'plan'; row: DirectorPlanRow }
  | { kind: 'ambiguous'; rows: DirectorPlanRow[] }
  | { kind: 'source'; slug: string; projectId: number }
  | { kind: 'missing' };

const SOURCE = /^([^:]+):(\d+)$/;

/** The plan a key names, or the database row it names when no plan has it
 *  yet. A GUID that several plans carry is ambiguous; `prefer` (the plan
 *  that was open) settles it when it is one of them. */
export function resolvePlan(rows: DirectorPlanRow[], key: string | null, prefer: string | null = null): ResolvedPlan {
  if (!key) return { kind: 'missing' };
  const lower = key.toLowerCase();
  const byId = rows.find(row => row.project.id.toLowerCase() === lower);
  if (byId) return { kind: 'plan', row: byId };
  const byGuid = rows.filter(row => guidsOf(row).has(lower));
  if (byGuid.length === 1) return { kind: 'plan', row: byGuid[0] };
  if (byGuid.length > 1) {
    const kept = byGuid.find(row => row.project.id === prefer);
    return kept ? { kind: 'plan', row: kept } : { kind: 'ambiguous', rows: byGuid };
  }
  const source = SOURCE.exec(key);
  if (source && Number.isSafeInteger(Number(source[2]))) {
    const slug = source[1];
    const projectId = Number(source[2]);
    const row = rows.find(entry => entry.links.some(link => link.catalog_slug === slug && link.source_row_id === projectId));
    return row ? { kind: 'plan', row } : { kind: 'source', slug, projectId };
  }
  return { kind: 'missing' };
}

/** The workspace for a plan key, keeping the rest of the page's scope (the
 *  Library's filters, the review project) so its back link returns there. */
export function planHref(key: string, search: string | URLSearchParams): string {
  const next = withoutPlanningParams(search.toString());
  next.set('plan', key);
  return `/plan?${next}`;
}

/** Where an old `/director` link belongs now: its plan, its database row,
 *  or the Library. Its Show and search become the Library's in every case,
 *  so the workspace's back link keeps them. */
export function legacyPlanningHref(params: URLSearchParams): string {
  const scope = new URLSearchParams(params);
  const show = scope.get('directorShow');
  const search = scope.get('directorSearch');
  scope.delete('directorShow'); scope.delete('directorSearch');
  if (show) scope.set('show', show);
  if (search) scope.set('q', search);
  const project = params.get('directorProject');
  if (project) return planHref(project, scope);
  const source = params.get('directorSource');
  const row = params.get('project');
  if (source && row && /^\d+$/.test(row)) return planHref(`${source}:${row}`, scope);
  const query = withoutPlanningParams(scope.toString()).toString();
  return query ? `/?${query}` : '/';
}
