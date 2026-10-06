import { useQueries, useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { ProjectMosaic } from '../api/types';

const mosaicQuery = (dbId: string | null, projectId: number | null) => ({
  queryKey: ['db', dbId, 'project-mosaic', projectId] as const,
  queryFn: () => apiClient.getProjectMosaic(dbId!, projectId!),
  enabled: dbId !== null && projectId !== null,
  staleTime: 60_000,
});

/** The project's mosaic panels, `null` when its targets are not one. */
export function useProjectMosaic(dbId: string | null, projectId: number | null) {
  return useQuery(mosaicQuery(dbId, projectId));
}

/** The mosaic of each project, in order, for a switcher listing several. */
export function useProjectMosaics(projects: { db_id: string; id: number }[]): (ProjectMosaic | null)[] {
  return useQueries({ queries: projects.map(project => mosaicQuery(project.db_id, project.id)) })
    .map(result => result.data ?? null);
}

/** "M31 mosaic (4 panels)": the project's name, else what the panels'
 *  names share. */
export function mosaicLabel(mosaic: ProjectMosaic, fallbackName = ''): string {
  const name = mosaic.name || fallbackName;
  const count = `${mosaic.panels.length} panels`;
  return name ? `${name} mosaic (${count})` : `Mosaic (${count})`;
}
