import type { Image, ProjectMosaic } from '../api/types';
import type { GroupingMode } from '../types/grouping';
import type { ImageOrder } from './imageOrder';

export interface ImageGroup {
  /** Stable identity for React and URL state. The label may change as a live session grows. */
  key?: string;
  baseKey?: string;
  filterName: string;
  images: Image[];
  /** In a mosaic's scope, the heading the first group of each panel
   *  carries: "r1c2 · M31 Panel 2". */
  panelHeading?: string;
}

/** Use the project's server-assigned partition, never a filtered subset's durations. */
export function splitImageGroupsByExposure(groups: ImageGroup[]): ImageGroup[] {
  return groups.flatMap((group) => {
    if (!group.images.some((image) => image.exposure_group)) return [group];
    const multipleTargets = new Set(group.images.map((image) => image.target_id)).size > 1;
    const multipleFilters = new Set(group.images.map((image) => image.filter_name ?? '')).size > 1;
    const partitions = new Map<string, ImageGroup>();
    for (const image of group.images) {
      const exposure = image.exposure_group;
      const baseKey = imageGroupKey(group);
      const key = exposure
        ? JSON.stringify(['exposure', baseKey, image.project_id, image.target_id, image.filter_name ?? '', exposure.key])
        : baseKey;
      let partition = partitions.get(key);
      if (!partition) {
        partition = {
          key,
          baseKey,
          filterName: exposure ? [group.filterName,
            multipleTargets ? image.target_name : null,
            multipleFilters ? image.filter_name || 'No Filter' : null,
            exposure.label].filter(Boolean).join(' · ') : group.filterName,
          images: [],
        };
        partitions.set(key, partition);
      }
      partition.images.push(image);
    }
    return [...partitions.values()];
  });
}

/**
 * The Images grid's groups for one set of images in a grouping mode, each
 * group's frames in the order chosen: capture order, or newest first.
 *
 * The groups themselves keep their order either way. Date and Session
 * groups already start with the newest night; Filter and Project groups
 * stay A to Z, so they do not move about as a night's frames arrive. Only
 * Filter + Date, whose nights run oldest first under each filter, turns
 * them around for newest first, so the newest frame of each filter leads.
 * The detail view's next and previous walk the same list.
 */
export function groupGridImages(
  images: Image[],
  groupingMode: GroupingMode,
  order: ImageOrder = 'oldest',
): ImageGroup[] {
  if (groupingMode === 'session') {
    const sessions = groupImagesBySession(images);
    return splitImageGroupsByExposure(order === 'newest'
      ? sessions.map((group) => ({ ...group, images: [...group.images].reverse() }))
      : sessions);
  }

  const groups = new Map<string, { filter: string; date: string; images: Image[] }>();
  for (const image of images) {
    const project = image.project_display_name || 'Unknown Project';
    const filter = image.filter_name || 'No Filter';
    const date = image.acquired_date
      ? new Date(image.acquired_date * 1000).toISOString().split('T')[0]
      : 'Unknown Date';
    let key: string;
    switch (groupingMode) {
      case 'date': key = date; break;
      case 'both': key = `${filter} - ${date}`; break;
      case 'project': key = project; break;
      case 'project+filter': key = `${project} - ${filter}`; break;
      case 'project+date': key = `${project} - ${date}`; break;
      case 'project+date+filter': key = `${project} - ${date} - ${filter}`; break;
      default: key = filter;
    }
    const group = groups.get(key);
    if (group) group.images.push(image);
    else groups.set(key, { filter, date, images: [image] });
  }

  const sorted = Array.from(groups.entries()).map(([name, group]) => {
    // Capture order, stable for frames with the same time; newest first is
    // its mirror image.
    const chronological = group.images.sort((a, b) => (a.acquired_date || 0) - (b.acquired_date || 0));
    return {
      filterName: name,
      filter: group.filter,
      date: group.date,
      images: order === 'newest' ? chronological.reverse() : chronological,
    };
  });

  if (order === 'newest' && groupingMode === 'both') {
    sorted.sort((a, b) => a.filter.localeCompare(b.filter) || b.date.localeCompare(a.date));
  } else if (groupingMode.includes('date')) {
    // Date modes: the newest night first.
    sorted.sort((a, b) => b.filterName.localeCompare(a.filterName));
  } else {
    sorted.sort((a, b) => a.filterName.localeCompare(b.filterName));
  }

  return splitImageGroupsByExposure(
    sorted.map(({ filterName, images: groupImages }) => ({ filterName, images: groupImages })),
  );
}

export const NO_EXPANDED_GROUPS = '__none__';

const SESSION_GAP_SECONDS = 60 * 60;

export function groupImagesBySession(images: Image[]): ImageGroup[] {
  const streams = new Map<string, Image[]>();
  for (const image of images) {
    const key = [
      image.project_display_name,
      image.target_id,
      image.filter_name || 'No Filter',
    ].join('|');
    const stream = streams.get(key);
    if (stream) stream.push(image);
    else streams.set(key, [image]);
  }

  const groups: Array<ImageGroup & { sortTime: number }> = [];
  for (const [streamKey, stream] of streams) {
    stream.sort((a, b) => (a.acquired_date || 0) - (b.acquired_date || 0));
    let current: Image[] = [];

    const flush = () => {
      if (current.length === 0) return;
      const first = current[0];
      const last = current[current.length - 1];
      groups.push({
        key: `${streamKey}|${first.acquired_date ?? `image-${first.id}`}`,
        filterName: sessionLabel(first, last),
        images: current,
        sortTime: first.acquired_date || 0,
      });
      current = [];
    };

    for (const image of stream) {
      const prior = current[current.length - 1];
      const gap = prior?.acquired_date != null && image.acquired_date != null
        ? image.acquired_date - prior.acquired_date
        : 0;
      if (current.length > 0 && gap > SESSION_GAP_SECONDS) flush();
      current.push(image);
    }
    flush();
  }

  return groups
    .sort((a, b) => b.sortTime - a.sortTime)
    .map(({ key, filterName, images: sessionImages }) => ({
      key,
      filterName,
      images: sessionImages,
    }));
}

export function imageGroupKey(group: ImageGroup): string {
  return group.key ?? group.filterName;
}

export function resolveExpandedGroups(
  imageGroups: ImageGroup[],
  groupingMode: GroupingMode,
  expandedGroups: ReadonlySet<string>,
): ReadonlySet<string> {
  if (expandedGroups.has(NO_EXPANDED_GROUPS)) return new Set();
  if (expandedGroups.size > 0) {
    const previousBaseKeys = new Set<string>();
    for (const key of expandedGroups) {
      try {
        const parts = JSON.parse(key);
        if (Array.isArray(parts) && parts[0] === 'exposure' && typeof parts[1] === 'string') {
          previousBaseKeys.add(parts[1]);
        }
      } catch { /* Existing filter/session keys are plain strings. */ }
    }
    return new Set(imageGroups.filter((group) => {
      const key = imageGroupKey(group);
      return expandedGroups.has(key)
        || (group.baseKey !== undefined && expandedGroups.has(group.baseKey))
        || (group.baseKey === undefined && previousBaseKeys.has(key));
    }).map(imageGroupKey));
  }

  const newestBase = imageGroups[0]?.baseKey;
  const initialGroups = groupingMode !== 'session' ? imageGroups
    : newestBase ? imageGroups.filter((group) => group.baseKey === newestBase) : imageGroups.slice(0, 1);
  return new Set(initialGroups.map(imageGroupKey));
}

function sessionLabel(first: Image, last: Image): string {
  const target = first.target_name || 'Unknown Target';
  const filter = first.filter_name || 'No Filter';
  if (!first.acquired_date) return `${target} · ${filter} · Unknown time`;

  const start = new Date(first.acquired_date * 1000);
  const end = new Date((last.acquired_date ?? first.acquired_date) * 1000);
  const formatDate = (value: Date) => value.toLocaleDateString([], {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
  });
  const time = (value: Date) => value.toLocaleTimeString([], {
    hour: '2-digit',
    minute: '2-digit',
  });
  const sameDay = start.getFullYear() === end.getFullYear()
    && start.getMonth() === end.getMonth()
    && start.getDate() === end.getDate();
  const endLabel = sameDay ? time(end) : `${formatDate(end)}, ${time(end)}`;
  return `${target} · ${filter} · ${formatDate(start)}, ${time(start)}–${endLabel}`;
}

/** A mosaic's images under its panels, row by row from the top: each
 *  panel's frames grouped by `groupWithin`, its first group carrying the
 *  panel's heading. Frames of targets outside the mosaic come last. */
export function groupImagesByPanel(
  images: Image[],
  mosaic: ProjectMosaic,
  groupWithin: (images: Image[]) => ImageGroup[],
): ImageGroup[] {
  const byTarget = new Map<number, Image[]>();
  for (const image of images) {
    const list = byTarget.get(image.target_id);
    if (list) list.push(image);
    else byTarget.set(image.target_id, [image]);
  }
  const sections: { id: string; heading: string; images: Image[] }[] = mosaic.panels.map(panel => ({
    id: panel.panel_id,
    heading: panel.target_name ? `${panel.panel_id} · ${panel.target_name}` : panel.panel_id,
    images: byTarget.get(panel.target_id) ?? [],
  }));
  const panelTargets = new Set(mosaic.panels.map(panel => panel.target_id));
  const others = images.filter(image => !panelTargets.has(image.target_id));
  if (others.length > 0) sections.push({ id: 'other', heading: 'Other targets', images: others });
  return sections.flatMap(section => groupWithin(section.images).map((group, index) => ({
    ...group,
    key: JSON.stringify(['panel', section.id, imageGroupKey(group)]),
    ...(index === 0 ? { panelHeading: section.heading } : {}),
  })));
}
