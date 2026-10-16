import { useCallback } from 'react';
import type { Image, ProjectMosaic } from '../api/types';
import type { GroupingMode } from '../types/grouping';
import { groupImagesForGrid, isDateGrouping } from '../utils/imageGrouping';
import type { ImageOrder } from '../utils/imageOrder';
import { useNightBoundary } from './useNightBoundary';

/**
 * The Images grid's groups, as the grid shows them and the detail view's
 * next and previous walk them: one helper for both, so they cannot drift.
 * `groupsIn(order)` gives the groups in either order; `ready` stays false in
 * a date grouping until the catalog's night boundary is in.
 * `nightBoundarySeconds` is that boundary, for the night marks between cards.
 */
export function useGridGroups(
  dbId: string | null | undefined,
  images: Image[],
  groupingMode: GroupingMode,
  mosaic: ProjectMosaic | null,
) {
  const night = useNightBoundary(dbId);
  const nightBoundarySeconds = night.boundarySeconds;
  const groupsIn = useCallback(
    (order: ImageOrder) => groupImagesForGrid(images, groupingMode, order, { mosaic, nightBoundarySeconds }),
    [images, groupingMode, mosaic, nightBoundarySeconds],
  );
  return { groupsIn, nightBoundarySeconds, ready: !isDateGrouping(groupingMode) || night.ready };
}
