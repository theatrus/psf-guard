import type { DirectorPlanRow } from '../../api/directorTypes';

/** The plan's name, or a rig's name for its project or database. */
export function matchesSearch(row: DirectorPlanRow, search: string): boolean {
  const needle = search.trim().toLowerCase();
  if (!needle) return true;
  const haystack = [row.project.name, ...row.links.flatMap(link => [link.source_name ?? '', link.catalog_name])];
  return haystack.some(text => text.toLowerCase().includes(needle));
}
