import type { Image } from '../api/types';
import type { GroupingMode } from '../types/grouping';

export interface ImageGroup {
  /** Stable identity for React and URL state. The label may change as a live session grows. */
  key?: string;
  filterName: string;
  images: Image[];
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
  if (expandedGroups.size > 0) return expandedGroups;

  const initialGroups = groupingMode === 'session' ? imageGroups.slice(0, 1) : imageGroups;
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
