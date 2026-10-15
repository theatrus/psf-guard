import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CalibrationProjectScope, CalibrationRigSummary } from '../../api/types';

/**
 * What the Calibration page is narrowed to beyond its filters: one
 * project's calibration, the sessions, masters and flat groups its lights
 * use. A project shot by several rigs (a family, in the multi-rig plan)
 * will be another kind, with a segment per rig; the chip, the heading and
 * the lists read only what is here, so it adds a kind rather than a page.
 */
export interface CalibrationScope {
  kind: 'project';
  dbId: string;
  projectId: number;
  /** What the chip and the heading call it: "M 44". */
  name: string;
  /** What its lights use, once the server has worked it out. */
  use?: CalibrationProjectScope;
}

export const projectScopeKey = (dbId: string, project: number) =>
  ['db', dbId, 'calibrations', 'project-scope', project] as const;

/** One project's calibration scope, and the query that works it out. */
export function useProjectScope(dbId: string | null, projectId: number | undefined, name: string | undefined) {
  const query = useQuery({
    queryKey: projectScopeKey(dbId ?? '', projectId ?? 0),
    queryFn: () => apiClient.getCalibrationProjectScope(dbId!, projectId!),
    enabled: !!dbId && projectId !== undefined,
    staleTime: 60_000,
  });
  const scope: CalibrationScope | null = dbId && projectId !== undefined
    ? { kind: 'project', dbId, projectId, name: name ?? `Project ${projectId}`, use: query.data }
    : null;
  return { scope, query };
}

/** The rigs a scope spans, in words: the Rig filter's rig when one is
 *  chosen, the scope's one rig, or how many it spans. */
export function scopeRigsLabel(
  scope: CalibrationScope,
  rigs: CalibrationRigSummary[],
  chosenRig: string | undefined,
): string | undefined {
  const named = (uuid: string) => rigs.find((rig) => rig.rig_uuid === uuid)?.name;
  if (chosenRig) return named(chosenRig);
  const spans = scope.use?.rigs ?? [];
  if (spans.length === 1) return spans[0].name ?? named(spans[0].rig_uuid);
  if (spans.length > 1) return `${spans.length} rigs`;
  return undefined;
}

/** What the ✕ and the heading's link say they do. */
export const WIDEN_LABEL = 'Show the whole library';
