import type { Image } from '../api/types';
import type { GroupingMode } from '../types/grouping';

export interface ImageGroup {
  /** Stable identity for React and URL state. The label may change as a live session grows. */
  key?: string;
  baseKey?: string;
  filterName: string;
  images: Image[];
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
