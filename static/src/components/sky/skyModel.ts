/**
 * The Sky page's data shaping: every database's coverage merged, then cut by
 * rig, filter, grade, and the night the time scrubber is parked on. Pure
 * functions so the map, the timeline, and the stat band all agree.
 */
import type { SkyCoverage, SkyNight, SkyTarget } from '../../api/types';
import type { WithDb } from '../../hooks/useDatabases';
import { footprintOutline, wrap180 } from '../../utils/skyProjection';

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
  /** Show only what was captured from this night on; `null` starts at the first. */
  fromNight: string | null;
  /** Show only what had been captured by the end of this night; `null` shows everything. */
  asOfNight: string | null;
}

export const EVERYTHING: SkyCut = {
  rigs: null,
  filters: null,
  acceptedOnly: false,
  fromNight: null,
  asOfNight: null,
};

/** The nights the selected rigs and filters captured on, sorted: the span
 * the time slider runs over. The time cuts themselves are not applied. */
export function nightKeysFor(data: MergedCoverage, cut: SkyCut): string[] {
  const keys = new Set<string>();
  for (const night of data.nights) {
    if (!nightAllowed(night, { ...cut, fromNight: null, asOfNight: null })) continue;
    if (cut.acceptedOnly ? night.accepted_frames === 0 : night.frames === 0) continue;
    keys.add(night.night);
  }
  return [...keys].sort();
}

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
  if (cut.fromNight && night.night < cut.fromNight) return false;
  if (cut.asOfNight && night.night > cut.asOfNight) return false;
  return true;
}

export interface ShownTarget {
  target: CoverageTarget;
  seconds: number;
  frames: number;
  nights: number;
  byFilter: Array<{ filter: string; seconds: number }>;
  /** Seconds per night the target was shot, in night order. */
  perNight: Array<[string, number]>;
  firstNight: string | null;
  lastNight: string | null;
}

/** Targets with something to show under the cut, with their counted time. */
export function shownTargets(data: MergedCoverage, cut: SkyCut): ShownTarget[] {
  const tallies = new Map<
    string,
    {
      seconds: number;
      frames: number;
      nights: Map<string, number>;
      byFilter: Map<string, number>;
      first: string | null;
      last: string | null;
    }
  >();
  for (const night of data.nights) {
    if (!nightAllowed(night, cut)) continue;
    const s = seconds(night, cut.acceptedOnly);
    const f = frames(night, cut.acceptedOnly);
    if (f === 0) continue;
    let tally = tallies.get(night.target_key);
    if (!tally) {
      tally = { seconds: 0, frames: 0, nights: new Map(), byFilter: new Map(), first: null, last: null };
      tallies.set(night.target_key, tally);
    }
    tally.seconds += s;
    tally.frames += f;
    tally.nights.set(night.night, (tally.nights.get(night.night) ?? 0) + s);
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
      perNight: [...tally.nights.entries()].sort(([a], [b]) => (a < b ? -1 : a > b ? 1 : 0)),
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
  /** The lane that sums every rig, shown first when there is more than one. */
  aggregate?: boolean;
}

export const ALL_RIGS_LANE = 'all-rigs';

/** One timeline lane per rig, with the time counted per night. The time
 * scrubber is not applied here: the strip always shows the whole span. */
export function timelineLanes(data: MergedCoverage, cut: SkyCut): Lane[] {
  const lanes = new Map<string, Lane>();
  for (const rig of data.rigs) {
    if (cut.rigs && !cut.rigs.has(rig.db_id)) continue;
    lanes.set(rig.db_id, { db_id: rig.db_id, db_name: rig.db_name, seconds: 0, perNight: new Map() });
  }
  for (const night of data.nights) {
    if (!nightAllowed(night, { ...cut, fromNight: null, asOfNight: null })) continue;
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
  const rigLanes = [...lanes.values()];
  if (rigLanes.length < 2) {
    return rigLanes;
  }
  const total: Lane = { db_id: ALL_RIGS_LANE, db_name: 'All rigs', seconds: 0, perNight: new Map(), aggregate: true };
  for (const lane of rigLanes) {
    total.seconds += lane.seconds;
    for (const [night, slot] of lane.perNight) {
      let sum = total.perNight.get(night);
      if (!sum) {
        sum = { seconds: 0, byFilter: new Map() };
        total.perNight.set(night, sum);
      }
      sum.seconds += slot.seconds;
      for (const [filter, seconds] of slot.byFilter) {
        sum.byFilter.set(filter, (sum.byFilter.get(filter) ?? 0) + seconds);
      }
    }
  }
  return [total, ...rigLanes];
}

export interface SkyStats {
  hours: number;
  frames: number;
  targets: number;
  nights: number;
  rigs: number;
  /** Area of sky the shown fields cover, square degrees, overlaps counted once. */
  areaDeg2: number;
  /** The shown fields' areas added together, square degrees, overlaps and all. */
  fieldsDeg2: number;
  /** Pixels the covered sky resolves into, each patch at the finest scale that reached it. */
  pixels: number;
  firstNight: string | null;
  lastNight: string | null;
  /** The most one rig captured in one night. */
  longestNight: { night: string; hours: number; rig: string } | null;
  topTarget: { name: string; hours: number } | null;
}

export function skyStats(shown: ShownTarget[], lanes: Lane[], cut: SkyCut): SkyStats {
  let seconds = 0;
  let frames = 0;
  const nights = new Set<string>();
  const rigs = new Set<string>();
  let first: string | null = null;
  let last: string | null = null;
  let top: { name: string; hours: number } | null = null;
  for (const item of shown) {
    seconds += item.seconds;
    frames += item.frames;
    rigs.add(item.target.db_id);
    if (item.firstNight && (!first || item.firstNight < first)) first = item.firstNight;
    if (item.lastNight && (!last || item.lastNight > last)) last = item.lastNight;
    if (!top || item.seconds > top.hours * 3600) {
      top = { name: item.target.name, hours: item.seconds / 3600 };
    }
  }
  let longest: { night: string; hours: number; rig: string } | null = null;
  for (const lane of lanes) {
    if (lane.aggregate) continue;
    for (const [night, slot] of lane.perNight) {
      if (cut.fromNight && night < cut.fromNight) continue;
      if (cut.asOfNight && night > cut.asOfNight) continue;
      nights.add(night);
      if (!longest || slot.seconds > longest.hours * 3600) {
        longest = { night, hours: slot.seconds / 3600, rig: lane.db_name };
      }
    }
  }
  return {
    hours: seconds / 3600,
    frames,
    targets: shown.length,
    nights: nights.size,
    rigs: rigs.size,
    ...coveredSky(shown),
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

/** Side of the sky cells the covered area is counted on, degrees. */
const AREA_CELL_DEG = 0.05;

export interface CoveredSky {
  areaDeg2: number;
  fieldsDeg2: number;
  pixels: number;
}

/**
 * How much sky the shown fields cover. `areaDeg2` counts overlaps once: each
 * field is laid onto a grid of small cells and every cell whose centre falls
 * inside any field is counted, weighted by the cosine of its declination,
 * so two rigs on one target, or the panels of a mosaic, add up to the sky
 * they share. `fieldsDeg2` adds the fields up as they are. `pixels` turns
 * each covered cell into pixels at the finest plate scale of any field that
 * reached it, so a long focal length earns more per square degree.
 */
export function coveredSky(shown: ShownTarget[]): CoveredSky {
  // cell key → [area in square degrees, finest arcseconds per pixel seen]
  const cells = new Map<number, [number, number]>();
  const columns = Math.round(360 / AREA_CELL_DEG);
  let fieldsDeg2 = 0;
  for (const { target } of shown) {
    if (!target.footprint || target.ra_deg == null || target.dec_deg == null) continue;
    fieldsDeg2 += target.footprint.width_deg * target.footprint.height_deg;
    const scale = target.footprint.pixel_scale_arcsec ?? Number.POSITIVE_INFINITY;
    const corners = footprintOutline(target.ra_deg, target.dec_deg, target.footprint, 1);
    if (corners.length < 3) continue;
    // Work in a flat patch about the field: RA offsets shrunk by cos(dec),
    // so the point-in-polygon test sees the field's true shape.
    const dec0 = target.dec_deg;
    const cosDec0 = Math.max(0.05, Math.cos((dec0 * Math.PI) / 180));
    const polygon = corners.map(([ra, dec]) => [wrap180(ra - target.ra_deg!) * cosDec0, dec - dec0]);
    const xs = polygon.map(([x]) => x);
    const ys = polygon.map(([, y]) => y);
    const minX = Math.min(...xs);
    const maxX = Math.max(...xs);
    const minY = Math.min(...ys);
    const maxY = Math.max(...ys);
    const raStep = AREA_CELL_DEG;
    const decStep = AREA_CELL_DEG;
    const decLo = Math.max(-90, dec0 + minY);
    const decHi = Math.min(90, dec0 + maxY);
    const raSpan = (maxX - minX) / cosDec0;
    const raLo = target.ra_deg + minX / cosDec0;
    const iyLo = Math.floor((decLo + 90) / decStep);
    const iyHi = Math.floor((decHi + 90) / decStep);
    const ixLo = Math.floor(raLo / raStep);
    const ixHi = Math.floor((raLo + raSpan) / raStep);
    for (let iy = iyLo; iy <= iyHi; iy += 1) {
      const dec = -90 + (iy + 0.5) * decStep;
      const y = dec - dec0;
      const cosDec = Math.cos((dec * Math.PI) / 180);
      for (let ix = ixLo; ix <= ixHi; ix += 1) {
        const ra = (ix + 0.5) * raStep;
        const x = wrap180(ra - target.ra_deg) * cosDec0;
        if (!pointInPolygon(x, y, polygon)) continue;
        const key = ((ix % columns) + columns) % columns + iy * columns;
        const cell = cells.get(key);
        if (!cell) cells.set(key, [raStep * decStep * cosDec, scale]);
        else if (scale < cell[1]) cell[1] = scale;
      }
    }
  }
  let areaDeg2 = 0;
  let pixels = 0;
  for (const [area, scale] of cells.values()) {
    areaDeg2 += area;
    if (Number.isFinite(scale) && scale > 0) pixels += (area * 3600 * 3600) / (scale * scale);
  }
  return { areaDeg2, fieldsDeg2, pixels };
}

/** Pixels as `1.2 Gpx` or `640 Mpx`. */
export function formatPixels(pixels: number): string {
  if (!(pixels > 0)) return '—';
  if (pixels >= 1e9) return `${(pixels / 1e9).toFixed(pixels >= 1e10 ? 0 : 1)} Gpx`;
  if (pixels >= 1e6) return `${Math.round(pixels / 1e6)} Mpx`;
  return `${Math.round(pixels / 1e3)} kpx`;
}

function pointInPolygon(x: number, y: number, polygon: number[][]): boolean {
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const [xi, yi] = polygon[i];
    const [xj, yj] = polygon[j];
    if (yi > y !== yj > y && x < ((xj - xi) * (y - yi)) / (yj - yi) + xi) inside = !inside;
  }
  return inside;
}
