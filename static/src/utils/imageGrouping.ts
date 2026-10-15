import type { Image, ProjectMosaic } from '../api/types';
import type { GroupingMode } from '../types/grouping';
import type { ImageOrder } from './imageOrder';
import { FALLBACK_NIGHT_BOUNDARY_SECONDS, nightOf } from './observingNight';

export interface ImageGroup {
  /** Stable identity for React and URL state. The label may change as a live session grows. */
  key?: string;
  baseKey?: string;
  /** Keys older links named this group by. A link naming one still opens it. */
  aliases?: string[];
  /** The same for the group this exposure sub-group was split from. */
  baseAliases?: string[];
  filterName: string;
  images: Image[];
  /** In a mosaic's scope, the heading the first group of each panel
   *  carries: "r1c2 · M31 Panel 2". */
  panelHeading?: string;
}

/**
 * A frame as the grid groups it. A list merged from several databases stamps
 * each frame with its own (see `WithDb`); one database's grid leaves it off.
 * Target ids, project names and filter names repeat from one database to the
 * next, so every group key in a merged list carries the database.
 */
export type GroupedImage = Image & { db_id?: string };

export interface GroupingScope {
  /**
   * Where the catalog's nights split, in seconds after 00:00 UTC, as
   * `/stats/overall` reports it. A list spanning databases gives each its
   * own, by `db_id`. Noon UTC, the server's fallback, when absent.
   */
  nightBoundarySeconds?: number | ReadonlyMap<string, number>;
}

function nightBoundaryFor(image: GroupedImage, scope: GroupingScope): number {
  const boundary = scope.nightBoundarySeconds;
  if (typeof boundary === 'number') return boundary;
  return (image.db_id !== undefined ? boundary?.get(image.db_id) : undefined)
    ?? FALLBACK_NIGHT_BOUNDARY_SECONDS;
}

/**
 * A key in one database: the plain key a single database's grid has always
 * written, or, in a list whose frames carry their database, the same key
 * under it.
 */
function inDatabase(dbId: string | undefined, key: string): string {
  return dbId === undefined ? key : JSON.stringify(['db', dbId, key]);
}

/** A key made of ids, under the frame's database when it carries one. */
function idKey(dbId: string | undefined, parts: (string | number)[]): string {
  return JSON.stringify(dbId === undefined ? parts : ['db', dbId, ...parts]);
}

/** The UTC calendar date Date groups were keyed by before they meant nights. */
const utcDate = (seconds: number) => new Date(seconds * 1000).toISOString().split('T')[0];

/** Use the project's server-assigned partition, never a filtered subset's durations. */
export function splitImageGroupsByExposure(groups: ImageGroup[]): ImageGroup[] {
  return groups.flatMap((group) => {
    if (!group.images.some((image) => image.exposure_group)) return [group];
    const multipleTargets = new Set(group.images.map((image) => image.target_id)).size > 1;
    const multipleFilters = new Set(group.images.map((image) => image.filter_name ?? '')).size > 1;
    const baseKey = imageGroupKey(group);
    const partitions = new Map<string, ImageGroup>();
    for (const image of group.images) {
      const exposure = image.exposure_group;
      // The base key carries the database in a merged list, so the ids
      // after it cannot meet another database's.
      const exposureKey = (base: string) => exposure
        ? JSON.stringify(['exposure', base, image.project_id, image.target_id, image.filter_name ?? '', exposure.key])
        : base;
      const key = exposureKey(baseKey);
      let partition = partitions.get(key);
      if (!partition) {
        const aliases = (group.aliases ?? []).map(exposureKey);
        partition = {
          key,
          baseKey,
          ...(aliases.length > 0 ? { aliases, baseAliases: group.aliases } : {}),
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
 * Date means the observing night, split where the catalog's nights split
 * (`scope.nightBoundarySeconds`), as calibration and the AstroBin export
 * count nights: an evening and the small hours after it share one date.
 *
 * The groups themselves keep their order either way. Date and Session
 * groups already start with the newest night; Filter and Project groups
 * stay A to Z, so they do not move about as a night's frames arrive. Only
 * Filter + Date, whose nights run oldest first under each filter, turns
 * them around for newest first, so the newest frame of each filter leads.
 * Exposure sub-groups are split in capture order and then turned around
 * inside, so they keep their places as frames arrive, and the flip only
 * mirrors each one's frames. The detail view's next and previous walk the
 * same list.
 *
 * Keys name projects by id, never by display name, and carry the database
 * when the frames do. Each group also answers to the keys older links used
 * for it (`aliases`): a project's display name, and the UTC dates its
 * frames fell on.
 */
export function groupGridImages(
  images: GroupedImage[],
  groupingMode: GroupingMode,
  order: ImageOrder = 'oldest',
  scope: GroupingScope = {},
): ImageGroup[] {
  const inOrder = (groups: ImageGroup[]) => order === 'newest'
    ? groups.map((group) => ({ ...group, images: [...group.images].reverse() }))
    : groups;
  if (groupingMode === 'session') {
    return inOrder(splitImageGroupsByExposure(groupImagesBySession(images)));
  }

  const groups = new Map<string, {
    label: string;
    filter: string;
    date: string;
    images: Image[];
    aliases: Set<string>;
  }>();
  for (const image of images) {
    const db = image.db_id;
    const project = image.project_display_name || 'Unknown Project';
    const filter = image.filter_name || 'No Filter';
    const date = image.acquired_date
      ? nightOf(image.acquired_date, nightBoundaryFor(image, scope))
      : 'Unknown Date';
    const oldDate = image.acquired_date ? utcDate(image.acquired_date) : 'Unknown Date';
    let label: string;
    let key: string;
    let oldKey: string;
    switch (groupingMode) {
      case 'date':
        label = date;
        key = inDatabase(db, label);
        oldKey = oldDate;
        break;
      case 'both':
        label = `${filter} - ${date}`;
        key = inDatabase(db, label);
        oldKey = `${filter} - ${oldDate}`;
        break;
      case 'project':
        label = project;
        key = idKey(db, ['project', image.project_id]);
        oldKey = project;
        break;
      case 'project+filter':
        label = `${project} - ${filter}`;
        key = idKey(db, ['project', image.project_id, filter]);
        oldKey = label;
        break;
      case 'project+date':
        label = `${project} - ${date}`;
        key = idKey(db, ['project', image.project_id, date]);
        oldKey = `${project} - ${oldDate}`;
        break;
      case 'project+date+filter':
        label = `${project} - ${date} - ${filter}`;
        key = idKey(db, ['project', image.project_id, date, filter]);
        oldKey = `${project} - ${oldDate} - ${filter}`;
        break;
      default:
        label = filter;
        key = inDatabase(db, label);
        oldKey = label;
    }
    let group = groups.get(key);
    if (!group) {
      group = { label, filter, date, images: [], aliases: new Set() };
      groups.set(key, group);
    }
    group.images.push(image);
    // Old links name one database's groups; a merged list has none.
    if (db === undefined && oldKey !== key) group.aliases.add(oldKey);
  }

  const sorted = Array.from(groups.entries()).map(([key, group]) => ({
    key,
    filterName: group.label,
    filter: group.filter,
    date: group.date,
    // West of Greenwich a night's small hours fall on the next UTC date,
    // which is the next night's own key. An old name that is a group's key
    // today names that group and no other.
    aliases: [...group.aliases].filter((alias) => !groups.has(alias)).sort(),
    // Capture order, stable for frames with the same time; newest first is
    // its mirror image.
    images: group.images.sort((a, b) => (a.acquired_date || 0) - (b.acquired_date || 0)),
  }));

  // Two groups can share a label (two projects of one name, one filter in
  // two databases); their keys keep the order fixed.
  const byKey = (a: { key: string }, b: { key: string }) => a.key.localeCompare(b.key);
  if (order === 'newest' && groupingMode === 'both') {
    sorted.sort((a, b) => a.filter.localeCompare(b.filter) || b.date.localeCompare(a.date) || byKey(a, b));
  } else if (groupingMode.includes('date')) {
    // Date modes: the newest night first.
    sorted.sort((a, b) => b.filterName.localeCompare(a.filterName) || byKey(a, b));
  } else {
    sorted.sort((a, b) => a.filterName.localeCompare(b.filterName) || byKey(a, b));
  }

  return inOrder(splitImageGroupsByExposure(
    sorted.map(({ key, filterName, aliases, images: groupImages }) => ({
      key,
      filterName,
      ...(aliases.length > 0 ? { aliases } : {}),
      images: groupImages,
    })),
  ));
}

/** Whether a grouping groups by night, and so needs the night boundary. */
export function isDateGrouping(groupingMode: GroupingMode): boolean {
  return groupingMode === 'both' || groupingMode.includes('date');
}

/**
 * The Images grid's groups in one order, under a mosaic's panels when the
 * scope is a mosaic. The grid and the detail view's next and previous both
 * group through here (see `useGridGroups`), so they walk the same groups.
 */
export function groupImagesForGrid(
  images: GroupedImage[],
  groupingMode: GroupingMode,
  order: ImageOrder,
  { mosaic, ...scope }: GroupingScope & { mosaic?: ProjectMosaic | null },
): ImageGroup[] {
  return mosaic
    ? groupImagesByPanel(images, mosaic, (inPanel) => groupGridImages(inPanel, groupingMode, order, scope))
    : groupGridImages(images, groupingMode, order, scope);
}

export const NO_EXPANDED_GROUPS = '__none__';

const SESSION_GAP_SECONDS = 60 * 60;

/**
 * Time runs of one target in one filter, split at an hour's gap. A target id
 * names one target in one database only, so a merged list's runs are told
 * apart by database too. One database's keys stay as they were:
 * `project|target|filter|start`.
 */
export function groupImagesBySession(images: GroupedImage[]): ImageGroup[] {
  const streams = new Map<string, GroupedImage[]>();
  for (const image of images) {
    const key = JSON.stringify([image.db_id ?? null, image.target_id, image.filter_name || 'No Filter']);
    const stream = streams.get(key);
    if (stream) stream.push(image);
    else streams.set(key, [image]);
  }

  const groups: Array<ImageGroup & { sortTime: number }> = [];
  for (const stream of streams.values()) {
    stream.sort((a, b) => (a.acquired_date || 0) - (b.acquired_date || 0));
    let current: GroupedImage[] = [];

    const flush = () => {
      if (current.length === 0) return;
      const first = current[0];
      const last = current[current.length - 1];
      const filter = first.filter_name || 'No Filter';
      const start = first.acquired_date ?? `image-${first.id}`;
      groups.push({
        key: first.db_id === undefined
          ? [first.project_display_name, first.target_id, filter, start].join('|')
          : idKey(first.db_id, ['session', first.target_id, filter, start]),
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

/**
 * The groups shown open: the ones the link names, or, when it names none
 * of these groups, the default ones. Keys that match no group are ignored,
 * so a link from another grouping, such as an All projects link from
 * before All projects grouped by project, still opens something.
 *
 * A link from before keys took ids and nights names a group by an alias.
 * Aliases are read only when the link names no group by today's keys, so a
 * group folded in today's link stays folded, even where an old name for
 * one group is spelled like another's key.
 */
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
    const namedBy = (withAliases: boolean) => imageGroups.filter((group) => {
      const key = imageGroupKey(group);
      const names = withAliases ? [key, ...(group.aliases ?? [])] : [key];
      const baseNames = group.baseKey === undefined ? []
        : withAliases ? [group.baseKey, ...(group.baseAliases ?? [])] : [group.baseKey];
      return names.some((name) => expandedGroups.has(name))
        || baseNames.some((name) => expandedGroups.has(name))
        || (group.baseKey === undefined && names.some((name) => previousBaseKeys.has(name)));
    }).map(imageGroupKey);
    let named = namedBy(false);
    if (named.length === 0) named = namedBy(true);
    if (named.length > 0 || imageGroups.length === 0) return new Set(named);
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
 *  panel's heading. Frames of targets outside the mosaic come last. A
 *  mosaic is one project in one database, and the inner keys carry the
 *  database when the frames do. */
export function groupImagesByPanel(
  images: GroupedImage[],
  mosaic: ProjectMosaic,
  groupWithin: (images: GroupedImage[]) => ImageGroup[],
): ImageGroup[] {
  const byTarget = new Map<number, GroupedImage[]>();
  for (const image of images) {
    const list = byTarget.get(image.target_id);
    if (list) list.push(image);
    else byTarget.set(image.target_id, [image]);
  }
  const sections: { id: string; heading: string; images: GroupedImage[] }[] = mosaic.panels.map(panel => ({
    id: panel.panel_id,
    heading: panel.target_name ? `${panel.panel_id} · ${panel.target_name}` : panel.panel_id,
    images: byTarget.get(panel.target_id) ?? [],
  }));
  const panelTargets = new Set(mosaic.panels.map(panel => panel.target_id));
  const others = images.filter(image => !panelTargets.has(image.target_id));
  if (others.length > 0) sections.push({ id: 'other', heading: 'Other targets', images: others });
  const inPanel = (id: string, key: string) => JSON.stringify(['panel', id, key]);
  return sections.flatMap(section => groupWithin(section.images).map((group, index) => ({
    ...group,
    key: inPanel(section.id, imageGroupKey(group)),
    ...(group.aliases ? { aliases: group.aliases.map(alias => inPanel(section.id, alias)) } : {}),
    ...(index === 0 ? { panelHeading: section.heading } : {}),
  })));
}
