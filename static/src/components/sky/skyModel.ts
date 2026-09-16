/**
 * The Sky page's data shaping: every database's coverage merged, then cut by
 * rig, filter, grade, and the night the time scrubber is parked on. Pure
 * functions so the map, the timeline, and the stat band all agree.
 */
import type { SkyCoverage, SkyNight, SkyTarget } from '../../api/types';
import type { WithDb } from '../../hooks/useDatabases';

export interface CoverageTarget extends SkyTarget {
  db_id: string;
  db_name: string;
  /** `${db_id}:${id}`, unique across databases. */
  key: string;
}

export interface CoverageNight extends SkyNight {
  db_id: string;
  /** The target's `key`. */
  target_key: string;
}

export interface MergedCoverage {
  targets: CoverageTarget[];
  nights: CoverageNight[];
  /** Every filter seen, in first-seen order across databases. */
  filters: string[];
  /** Every night with a capture, sorted. */
  nightKeys: string[];
  rigs: Array<{ db_id: string; db_name: string }>;
}

export interface SkyCut {
  /** Rigs to show; `null` shows all. */
  rigs: Set<string> | null;
  /** Filters to count; `null` counts all. */
  filters: Set<string> | null;
  acceptedOnly: boolean;
  /** Show only what had been captured by the end of this night; `null` shows everything. */
  asOfNight: string | null;
}

export const EVERYTHING: SkyCut = {
  rigs: null,
  filters: null,
  acceptedOnly: false,
  asOfNight: null,
};

export function mergeCoverage(rows: WithDb<SkyCoverage>[]): MergedCoverage {
  const targets: CoverageTarget[] = [];
  const nights: CoverageNight[] = [];
  const filters: string[] = [];
  const nightSet = new Set<string>();
  const rigs: Array<{ db_id: string; db_name: string }> = [];
  for (const row of rows) {
    rigs.push({ db_id: row.db_id, db_name: row.db_name });
    for (const filter of row.filters) {
      if (!filters.includes(filter)) filters.push(filter);
    }
    for (const target of row.targets) {
      targets.push({ ...target, db_id: row.db_id, db_name: row.db_name, key: `${row.db_id}:${target.id}` });
    }
    for (const night of row.nights) {
      nightSet.add(night.night);
      nights.push({ ...night, db_id: row.db_id, target_key: `${row.db_id}:${night.target_id}` });
    }
  }
  return { targets, nights, filters, nightKeys: [...nightSet].sort(), rigs };
}

function seconds(row: { seconds: number; accepted_seconds: number }, acceptedOnly: boolean): number {
  return acceptedOnly ? row.accepted_seconds : row.seconds;
}

function frames(row: { frames: number; accepted_frames: number }, acceptedOnly: boolean): number {
  return acceptedOnly ? row.accepted_frames : row.frames;
}

function nightAllowed(night: CoverageNight, cut: SkyCut): boolean {
  if (cut.rigs && !cut.rigs.has(night.db_id)) return false;
  if (cut.filters && !cut.filters.has(night.filter)) return false;
  if (cut.asOfNight && night.night > cut.asOfNight) return false;
  return true;
}

export interface ShownTarget {
  target: CoverageTarget;
  seconds: number;
  frames: number;
  nights: number;
  byFilter: Array<{ filter: string; seconds: number }>;
  firstNight: string | null;
  lastNight: string | null;
}

/** Targets with something to show under the cut, with their counted time. */
export function shownTargets(data: MergedCoverage, cut: SkyCut): ShownTarget[] {
  const tallies = new Map<
    string,
    { seconds: number; frames: number; nights: Set<string>; byFilter: Map<string, number>; first: string | null; last: string | null }
  >();
  for (const night of data.nights) {
    if (!nightAllowed(night, cut)) continue;
    const s = seconds(night, cut.acceptedOnly);
    const f = frames(night, cut.acceptedOnly);
    if (f === 0) continue;
    let tally = tallies.get(night.target_key);
    if (!tally) {
      tally = { seconds: 0, frames: 0, nights: new Set(), byFilter: new Map(), first: null, last: null };
      tallies.set(night.target_key, tally);
    }
    tally.seconds += s;
    tally.frames += f;
    tally.nights.add(night.night);
    tally.byFilter.set(night.filter, (tally.byFilter.get(night.filter) ?? 0) + s);
    if (!tally.first || night.night < tally.first) tally.first = night.night;
    if (!tally.last || night.night > tally.last) tally.last = night.night;
  }
  const shown: ShownTarget[] = [];
  for (const target of data.targets) {
    if (cut.rigs && !cut.rigs.has(target.db_id)) continue;
    const tally = tallies.get(target.key);
    if (!tally) continue;
    shown.push({
      target,
      seconds: tally.seconds,
      frames: tally.frames,
      nights: tally.nights.size,
      byFilter: [...tally.byFilter.entries()]
        .map(([filter, seconds]) => ({ filter, seconds }))
        .sort((a, b) => b.seconds - a.seconds),
      firstNight: tally.first,
      lastNight: tally.last,
    });
  }
  return shown;
}

export interface LaneNight {
  seconds: number;
  byFilter: Map<string, number>;
}

export interface Lane {
  db_id: string;
  db_name: string;
  seconds: number;
  perNight: Map<string, LaneNight>;
}

/** One timeline lane per rig, with the time counted per night. The time
 * scrubber is not applied here: the strip always shows the whole span. */
export function timelineLanes(data: MergedCoverage, cut: SkyCut): Lane[] {
  const lanes = new Map<string, Lane>();
  for (const rig of data.rigs) {
    if (cut.rigs && !cut.rigs.has(rig.db_id)) continue;
    lanes.set(rig.db_id, { db_id: rig.db_id, db_name: rig.db_name, seconds: 0, perNight: new Map() });
  }
  for (const night of data.nights) {
    if (!nightAllowed(night, { ...cut, asOfNight: null })) continue;
    const lane = lanes.get(night.db_id);
    if (!lane) continue;
    const s = seconds(night, cut.acceptedOnly);
    if (!(s > 0)) continue;
    lane.seconds += s;
    let slot = lane.perNight.get(night.night);
    if (!slot) {
      slot = { seconds: 0, byFilter: new Map() };
      lane.perNight.set(night.night, slot);
    }
    slot.seconds += s;
    slot.byFilter.set(night.filter, (slot.byFilter.get(night.filter) ?? 0) + s);
  }
  return [...lanes.values()];
}

export interface SkyStats {
  hours: number;
  frames: number;
  targets: number;
  nights: number;
  rigs: number;
  /** Sum of the shown fields' areas, square degrees. */
  areaDeg2: number;
  firstNight: string | null;
  lastNight: string | null;
  longestNight: { night: string; hours: number } | null;
  topTarget: { name: string; hours: number } | null;
}

export function skyStats(shown: ShownTarget[], lanes: Lane[], cut: SkyCut): SkyStats {
  let seconds = 0;
  let frames = 0;
  let areaDeg2 = 0;
  const nights = new Set<string>();
  const rigs = new Set<string>();
  let first: string | null = null;
  let last: string | null = null;
  let top: { name: string; hours: number } | null = null;
  for (const item of shown) {
    seconds += item.seconds;
    frames += item.frames;
    rigs.add(item.target.db_id);
    if (item.target.footprint) {
      areaDeg2 += item.target.footprint.width_deg * item.target.footprint.height_deg;
    }
    if (item.firstNight && (!first || item.firstNight < first)) first = item.firstNight;
    if (item.lastNight && (!last || item.lastNight > last)) last = item.lastNight;
    if (!top || item.seconds > top.hours * 3600) {
      top = { name: item.target.name, hours: item.seconds / 3600 };
    }
  }
  const perNight = new Map<string, number>();
  for (const lane of lanes) {
    for (const [night, slot] of lane.perNight) {
      if (cut.asOfNight && night > cut.asOfNight) continue;
      nights.add(night);
      perNight.set(night, (perNight.get(night) ?? 0) + slot.seconds);
    }
  }
  let longest: { night: string; hours: number } | null = null;
  for (const [night, s] of perNight) {
    if (!longest || s > longest.hours * 3600) longest = { night, hours: s / 3600 };
  }
  return {
    hours: seconds / 3600,
    frames,
    targets: shown.length,
    nights: nights.size,
    rigs: rigs.size,
    areaDeg2,
    firstNight: first,
    lastNight: last,
    longestNight: longest,
    topTarget: top,
  };
}

/** `2026-09-08` as `Sep 8, 2026`. */
export function formatNight(night: string | null): string {
  if (!night) return '—';
  const [y, m, d] = night.split('-').map(Number);
  if (!y || !m || !d) return night;
  return new Date(Date.UTC(y, m - 1, d)).toLocaleDateString(undefined, {
    year: 'numeric',
    month: 'short',
    day: 'numeric',
    timeZone: 'UTC',
  });
}

/** Hours with one decimal under ten, whole above. */
export function formatHours(hours: number): string {
  if (hours < 10) return `${hours.toFixed(1)} h`;
  return `${Math.round(hours)} h`;
}
