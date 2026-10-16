import type {
  FlatChangeEvent,
  FlatFamily,
  FlatGroup,
  FlatGroupsRig,
  FlatSplit,
  FlatUnmeasuredSession,
} from '../../api/types';
import { GROUP_TONES } from './flatGroupIndex';
import { angleLabel, nightSpan, percent, plural } from './flatGroupLabels';

/** Flats within this many degrees share an angle, as the Angle filter reads it. */
const ANGLE_TOLERANCE = 2;

/** One side of a group: the split to the next group of its family, with
 *  the change it belongs to, or a change of the whole rig that came after
 *  the family's newest group. */
export interface GroupEdge {
  split?: FlatSplit;
  event?: FlatChangeEvent;
  /** The two sessions whose ratio shows it. */
  earlier: string;
  later: string;
}

/** A flat group as the Groups list and its detail show it. */
export interface GroupEntry {
  key: string;
  rig: FlatGroupsRig;
  family: FlatFamily;
  /** What the family is called: "G · 3.2°", with more when two share it. */
  familyLabel: string;
  group: FlatGroup;
  /** 1 for the family's oldest group. */
  number: number;
  /** Groups in the family. */
  groups: number;
  /** 1 to {@link GROUP_TONES}. */
  tone: number;
  /** The family's newest group: the one a light shot now would take. */
  current: boolean;
  /** Towards the next newer group, or a change after the newest. */
  newer?: GroupEdge;
  /** Towards the next older group. */
  older?: GroupEdge;
  /** Flat sessions of its filter and angle within its nights (or after
   *  them, for the current group) that are not measured. */
  unmeasured: FlatUnmeasuredSession[];
  /** In a project scope, a group listed that the project's lights do not
   *  take: a neighbour shown for the edge it shares, or one the scope holds
   *  only for a correction's map. */
  context: false | 'neighbour' | 'map';
}

/** One rig's filter at one angle, with its groups newest first. */
export interface FamilyEntry {
  key: string;
  rig: FlatGroupsRig;
  label: string;
  /** Newest first; each but the last has its `older` edge drawn below it. */
  groups: GroupEntry[];
}

export interface GroupBrowse {
  families: FamilyEntry[];
  /** Every group listed, in list order. */
  entries: GroupEntry[];
  /** The groups the keys move through: those in scope. */
  navigable: GroupEntry[];
  /** The group a key names, or the one now holding the session it names. */
  find: (key: string | null | undefined) => GroupEntry | undefined;
  /** The group of a flat session's rig, filter and angle around its
   *  night: the one spanning it, else the nearest. For a session no group
   *  holds, such as one whose flats were all kept out. */
  near: (session: { rig_uuid: string; filter: string | null; rotation: number | null; night: string | null }) =>
    GroupEntry | undefined;
}

export interface GroupBrowseOptions {
  /** The Filter filter, compared without case. */
  filter?: string;
  /** The Angle filter, in degrees. */
  rotation?: number;
  /** The Calibration page's sessions a project's lights use. When given,
   *  only groups holding one are listed, with each one's neighbours for
   *  context. */
  scopeSessions?: ReadonlySet<string>;
  /** The groups the project's own lights take, when known. A group the
   *  scope holds that is not one of them holds a correction's map. */
  projectGroups?: ReadonlySet<string>;
}

function angleGap(a: number, b: number): number {
  const difference = ((a - b) % 360 + 360) % 360;
  return Math.min(difference, 360 - difference);
}

const sameFilter = (a: string | null, b: string | null) =>
  (a ?? '').trim().toLowerCase() === (b ?? '').trim().toLowerCase();

/** The family an unmeasured session would join: the same filter at the
 *  nearest angle within tolerance. Families are read from the data, so a
 *  family whose angle drifts or that holds a hint of one still matches. */
function familyOf(rig: FlatGroupsRig, session: FlatUnmeasuredSession): FlatFamily | undefined {
  let best: FlatFamily | undefined;
  let bestGap = Number.POSITIVE_INFINITY;
  for (const family of rig.families) {
    if (!sameFilter(family.filter, session.filter)) continue;
    const gap = family.rotation === null || session.rotation === null
      ? (family.rotation === session.rotation ? 0 : Number.POSITIVE_INFINITY)
      : angleGap(family.rotation, session.rotation);
    if (gap <= ANGLE_TOLERANCE && gap < bestGap) {
      best = family;
      bestGap = gap;
    }
  }
  return best;
}

/** A family's name from its filter and angle. Two families of a rig with
 *  the same name (another frame size, gain or readout, or one angle the
 *  measurements split) are told apart by the night each begins. */
function familyLabels(rig: FlatGroupsRig): Map<FlatFamily, string> {
  const base = (family: FlatFamily) => `${family.filter ?? 'No filter'} · ${angleLabel(family.rotation)}`;
  const counts = new Map<string, number>();
  for (const family of rig.families) counts.set(base(family), (counts.get(base(family)) ?? 0) + 1);
  const labels = new Map<FlatFamily, string>();
  for (const family of rig.families) {
    const name = base(family);
    const since = family.groups.at(-1)?.first_night;
    labels.set(family, (counts.get(name) ?? 0) > 1 && since ? `${name} · from ${since}` : name);
  }
  return labels;
}

/** The short reason a split shows inline: "new dust 1.6%", "pattern 2.6%". */
export function splitShortReason(split: FlatSplit): string {
  const reason = split.reason;
  if (reason.startsWith('drifted')) return 'drifted apart';
  if (reason.startsWith('one night unlike')) return 'one odd night';
  const dust = /^(new dust|dust gone|dust moved)/.exec(reason);
  if (dust && split.dust_pct !== null) return `${dust[1]} ${percent(split.dust_pct)}`;
  if (split.broad_pct !== null && /light pattern/.test(reason)) return `pattern ${percent(split.broad_pct)}`;
  return reason.split(',')[0];
}

const edgeOf = (rig: FlatGroupsRig, split: FlatSplit): GroupEdge => ({
  split,
  event: split.event_key ? rig.events.find((event) => event.key === split.event_key) : undefined,
  earlier: split.before_response,
  later: split.after_response,
});

/**
 * Every flat group in view, by rig and family, newest first, with what the
 * Groups list marks on each: the current group, one partly measured, and
 * one followed by a change.
 */
export function browseGroups(rigs: FlatGroupsRig[], options: GroupBrowseOptions = {}): GroupBrowse {
  const families: FamilyEntry[] = [];
  const all: GroupEntry[] = [];
  for (const rig of rigs) {
    const labels = familyLabels(rig);
    const unmeasuredOf = new Map<FlatFamily, FlatUnmeasuredSession[]>();
    for (const session of rig.unmeasured) {
      const family = familyOf(rig, session);
      if (family) unmeasuredOf.set(family, [...(unmeasuredOf.get(family) ?? []), session]);
    }
    for (const family of rig.families) {
      if (options.filter && !sameFilter(family.filter, options.filter)) continue;
      if (options.rotation !== undefined
        && (family.rotation === null || angleGap(family.rotation, options.rotation) > ANGLE_TOLERANCE)) continue;
      const label = labels.get(family) ?? '';
      const entries = family.groups.map((group, position): GroupEntry => {
        const number = family.groups.length - position;
        const current = position === 0;
        let newer = position > 0 ? edgeOf(rig, family.splits[position - 1]) : undefined;
        if (current && rig.measured) {
          // A change of the whole rig after the family's newest flats
          // splits nothing in it, but a light shot now is past it.
          const last = Math.max(...group.sessions.map((session) => session.last));
          const after = rig.events
            .filter((event) => event.camera_side && event.start >= last)
            .sort((a, b) => a.start - b.start)[0];
          if (after) newer = { event: after, earlier: after.before_response, later: after.after_response };
        }
        const split = family.splits[position];
        const unmeasured = (unmeasuredOf.get(family) ?? []).filter((session) =>
          (session.night >= group.first_night && session.night <= group.last_night)
          || (current && session.night > group.last_night));
        return {
          key: group.key,
          rig,
          family,
          familyLabel: label,
          group,
          number,
          groups: family.groups.length,
          tone: ((number - 1) % GROUP_TONES) + 1,
          current,
          newer,
          older: split ? edgeOf(rig, split) : undefined,
          unmeasured,
          context: false,
        };
      });
      let kept = entries;
      if (options.scopeSessions) {
        const scope = options.scopeSessions;
        const holds = entries.map((entry) =>
          entry.group.sessions.some((session) => (session.sessions ?? []).some((id) => scope.has(id))));
        if (!holds.some(Boolean)) continue;
        // Its neighbours stay, dimmed: each is the other side of an edge.
        kept = entries.filter((_, index) => holds[index] || holds[index - 1] || holds[index + 1]);
        for (const entry of kept) {
          entry.context = !holds[entries.indexOf(entry)] ? 'neighbour'
            : options.projectGroups && !options.projectGroups.has(entry.key) ? 'map'
              : false;
        }
      }
      families.push({ key: `${rig.rig_uuid}:${family.key}`, rig, label, groups: kept });
      all.push(...kept);
    }
  }

  const find = (key: string | null | undefined) => {
    if (!key) return undefined;
    return all.find((entry) => entry.key === key)
      ?? all.find((entry) => entry.group.sessions.some((session) =>
        session.response_key === key || (session.sessions ?? []).includes(key)));
  };
  const near: GroupBrowse['near'] = (session) => {
    if (!session.night) return undefined;
    const night = session.night;
    const days = (a: string, b: string) => Math.abs(Date.parse(a) - Date.parse(b)) / 86_400_000;
    let best: GroupEntry | undefined;
    let bestDistance = Number.POSITIVE_INFINITY;
    for (const entry of all) {
      const { family, group } = entry;
      if (entry.rig.rig_uuid !== session.rig_uuid || !sameFilter(family.filter, session.filter)) continue;
      const angle = family.rotation === null || session.rotation === null
        ? (family.rotation === session.rotation ? 0 : Number.POSITIVE_INFINITY)
        : angleGap(family.rotation, session.rotation);
      if (angle > ANGLE_TOLERANCE) continue;
      const distance = night >= group.first_night && night <= group.last_night
        ? 0
        : Math.min(days(night, group.first_night), days(night, group.last_night));
      if (distance < bestDistance) {
        best = entry;
        bestDistance = distance;
      }
    }
    return best;
  };
  return { families, entries: all, navigable: all.filter((entry) => !entry.context), find, near };
}

/** A group in words: "Group 3 · G · 3.2° · 09-20 → 09-23 · 2 nights · 6 flats". */
export function groupName(entry: GroupEntry): string {
  const { group } = entry;
  return [
    `Group ${entry.number}`,
    entry.familyLabel,
    nightSpan(group.first_night, group.last_night),
    plural(group.sessions.length, 'night'),
    plural(group.frames, 'flat'),
  ].join(' · ');
}
