import type { DirectorVisibleSky, DirectorVisibleSkyRegion } from '../../api/directorTypes';
import type { LonLat } from '../../utils/skyProjection';
import { formatNight } from './skyModel';
import { countAt, type SkyCounts } from './skyShade';
import type { SkyRegions, SkyTerms } from './visibleSky';

/** How many nights one time of year shows: as many as Now counts. */
export const SEASON_NIGHTS = 31;

/**
 * A year's measured nights, one a week, unpacked: enough to work out any
 * night of the year here, by the rule the server counts the year with.
 */
export interface MeasuredSky {
  /** For each measured night, 1 where a node of the grid is in view. */
  grids: Uint8Array[];
  /** The nights from one measured night to the next. */
  every: number;
  /** The nights counted: from the first measured night to the last. */
  counted: number;
  columns: number;
  rows: number;
  /**
   * For a night `since` nights after a measured night, the columns to look
   * back on that measured night, and the columns to look on on the next.
   */
  turns: Array<{ back: number[]; on: number[] }>;
  /** Nights already worked out, by their place in the year. */
  nights: Map<number, Uint8Array>;
}

/** Unpack the measured nights of a year's answer; null when it has none. */
export function unpackMeasured(view: Pick<DirectorVisibleSky, 'measured' | 'nights' | 'every_nights' | 'grid' | 'nights_counted'>): MeasuredSky | null {
  const measured = view.measured;
  if (!measured) return null;
  try {
    const text = atob(measured.in_view);
    const { columns, rows, degrees } = view.grid;
    const nodes = columns * rows;
    const stride = measured.bytes_a_night;
    if (stride * 8 < nodes || text.length !== stride * view.nights.length || view.every_nights < 1) return null;
    const grids = view.nights.map((_, night) => {
      const grid = new Uint8Array(nodes);
      for (let index = 0; index < nodes; index += 1) grid[index] = (text.charCodeAt(night * stride + (index >> 3)) >> (index & 7)) & 1;
      return grid;
    });
    // The sky turns `turn_degrees_a_night` a night. A turn that falls
    // between two columns asks both.
    const steps = (nights: number) => {
      const turn = (nights * measured.turn_degrees_a_night) / degrees;
      return Math.floor(turn) === Math.ceil(turn) ? [Math.floor(turn)] : [Math.floor(turn), Math.ceil(turn)];
    };
    const turns = Array.from({ length: view.every_nights }, (_, since) => ({ back: steps(since), on: steps(view.every_nights - since) }));
    return { grids, every: view.every_nights, counted: view.nights_counted, columns, rows, turns, nights: new Map() };
  } catch {
    return null;
  }
}

/**
 * One night of the year, 1 where a node is in view. A measured night is
 * its own grid. A night between two measured nights has a node in view
 * only where both have it, each turned to the night: the measured night
 * before looked at back along its row, the one after looked at further on.
 */
export function nightGrid(sky: MeasuredSky, night: number): Uint8Array {
  const week = Math.floor(night / sky.every);
  const since = night - week * sky.every;
  if (since === 0) return sky.grids[week];
  const kept = sky.nights.get(night);
  if (kept) return kept;
  const { columns, rows } = sky;
  const [before, after] = [sky.grids[week], sky.grids[week + 1]];
  const { back, on } = sky.turns[since];
  const grid = new Uint8Array(columns * rows);
  for (let row = 0; row < rows; row += 1) {
    const base = row * columns;
    for (let col = 0; col < columns; col += 1) {
      let seen = 1;
      for (const step of back) seen &= before[base + ((col - step + columns) % columns)];
      for (const step of on) seen &= after[base + ((col + step) % columns)];
      grid[base + col] = seen;
    }
  }
  sky.nights.set(night, grid);
  return grid;
}

/** On how many of `nights` nights from night `first` of the year each
 *  node is in view. */
export function windowCounts(sky: MeasuredSky, first: number, nights: number): SkyCounts {
  const counts = new Uint16Array(sky.columns * sky.rows);
  for (let night = first; night < first + nights; night += 1) {
    const grid = nightGrid(sky, night);
    for (let index = 0; index < counts.length; index += 1) counts[index] += grid[index];
  }
  return { counts, nights, columns: sky.columns, rows: sky.rows };
}

/** The first night of each slider position: a week apart, and a last one
 *  whose nights end with the year. */
export function seasonStarts(sky: MeasuredSky, nights = SEASON_NIGHTS): number[] {
  const last = Math.max(0, sky.counted - nights);
  const starts: number[] = [];
  for (let first = 0; first < last; first += sky.every) starts.push(first);
  starts.push(last);
  return starts;
}

/**
 * The lines round the sky where the count is `level` or more, drawn
 * between the nodes of the grid: closed, each running with that sky on its
 * left seen with north up and right ascension rising to the right, as the
 * server's lines run. A line crosses between two nodes where the count,
 * taken to change evenly from one to the other, passes `level`; so these
 * lines are true to the grid's two degrees, not to the eighth of a degree
 * the server places its own lines to.
 */
export function countLines(sky: SkyCounts, level: number): LonLat[][] {
  const { columns, rows, counts } = sky;
  const degrees = 360 / columns;
  const at = (col: number, row: number) => counts[row * columns + (col % columns)];
  // An edge of the grid by number: along a row from a node to the next
  // column, or up a column from a node to the next row.
  const along = (col: number, row: number) => 2 * (row * columns + (col % columns));
  const up = (col: number, row: number) => 2 * (row * columns + (col % columns)) + 1;
  // Each crossing is where one stretch of line ends and the next begins.
  const next = new Map<number, number>();
  for (let row = 0; row + 1 < rows; row += 1) {
    for (let col = 0; col < columns; col += 1) {
      const [sw, se, ne, nw] = [at(col, row), at(col + 1, row), at(col + 1, row + 1), at(col, row + 1)];
      const kind = (sw >= level ? 1 : 0) | (se >= level ? 2 : 0) | (ne >= level ? 4 : 0) | (nw >= level ? 8 : 0);
      if (kind === 0 || kind === 15) continue;
      const [south, north, west, east] = [along(col, row), along(col, row + 1), up(col, row), up(col + 1, row)];
      // Where two corners across from each other are in and the others
      // out, the middle decides whether the sky in view joins up.
      const joined = (sw + se + ne + nw) / 4 >= level;
      const stretches: Array<[number, number]> =
        kind === 1 ? [[south, west]]
        : kind === 2 ? [[east, south]]
        : kind === 4 ? [[north, east]]
        : kind === 8 ? [[west, north]]
        : kind === 3 ? [[east, west]]
        : kind === 6 ? [[north, south]]
        : kind === 12 ? [[west, east]]
        : kind === 9 ? [[south, north]]
        : kind === 14 ? [[west, south]]
        : kind === 13 ? [[south, east]]
        : kind === 11 ? [[east, north]]
        : kind === 7 ? [[north, west]]
        : kind === 5 ? (joined ? [[south, east], [north, west]] : [[south, west], [north, east]])
        : joined ? [[west, south], [east, north]] : [[east, south], [west, north]];
      for (const [from, to] of stretches) next.set(from, to);
    }
  }
  const place = (edge: number): LonLat => {
    const node = edge >> 1;
    const [col, row] = [node % columns, Math.floor(node / columns)];
    const first = at(col, row);
    const second = edge & 1 ? at(col, row + 1) : at(col + 1, row);
    const part = Math.min(1, Math.max(0, (level - first) / (second - first)));
    return edge & 1 ? [col * degrees, -90 + (row + part) * degrees] : [(col + part) * degrees, -90 + row * degrees];
  };
  const lines: LonLat[][] = [];
  const done = new Set<number>();
  for (const start of next.keys()) {
    if (done.has(start)) continue;
    const line: LonLat[] = [];
    let edge: number | undefined = start;
    while (edge !== undefined && !done.has(edge)) {
      done.add(edge);
      line.push(place(edge));
      edge = next.get(edge);
    }
    if (line.length > 1) {
      line.push(line[0]);
      lines.push(line);
    }
  }
  return lines;
}

/** The nodes whose count is `level` or more, as a sky of ones and noughts. */
function nodesAt(sky: SkyCounts, level: number): SkyCounts {
  return { ...sky, nights: 1, counts: sky.counts.map((count) => (count >= level ? 1 : 0)) };
}

/**
 * Whether a place is on the inside of the lines [`countLines`] draws round
 * the nodes that are 1, at half way: decided against the stretch of line
 * in the place's own cell of the grid, so it is the side the drawn line
 * puts the place on. A stretch cuts a corner off a cell half way along
 * each side, or runs straight across its middle.
 */
export function insideLines(nodes: SkyCounts, raDeg: number, decDeg: number): boolean {
  const { columns, rows, counts } = nodes;
  const across = ((((raDeg % 360) + 360) % 360) / 360) * columns;
  const up = Math.min(rows - 1, Math.max(0, ((decDeg + 90) / 180) * (rows - 1)));
  const col = Math.floor(across) % columns;
  const row = Math.min(rows - 2, Math.floor(up));
  const [x, y] = [across - Math.floor(across), up - row];
  const at = (c: number, r: number) => counts[r * columns + (c % columns)] >= 0.5;
  const [sw, se, ne, nw] = [at(col, row), at(col + 1, row), at(col + 1, row + 1), at(col, row + 1)];
  const inside = [sw, se, ne, nw].filter(Boolean).length;
  if (inside === 4) return true;
  if (inside === 0) return false;
  // How far the place is from each corner, along the sides.
  const from = [x + y, 1 - x + y, 2 - x - y, x + 1 - y];
  const corners = [sw, se, ne, nw];
  if (inside === 1) return from[corners.indexOf(true)] <= 0.5;
  if (inside === 3) return from[corners.indexOf(false)] >= 0.5;
  // Two corners across from each other: the line joins the sky in view
  // through the middle and cuts off the two corners outside.
  if (sw === ne) return corners.every((corner, index) => corner || from[index] >= 0.5);
  // Two corners on one side: the line runs straight across the middle.
  if (sw === se) return sw ? y <= 0.5 : y >= 0.5;
  return sw ? x <= 0.5 : x >= 0.5;
}

/** One region of a picture made here: its lines, its share of the sky and
 *  the declinations it spans, as the server gives them for its own. The
 *  line runs halfway between a node inside and a node outside. */
function region(sky: SkyCounts): DirectorVisibleSkyRegion {
  const lines = countLines(sky, 0.5);
  const degrees = 180 / (sky.rows - 1);
  let [inside, whole] = [0, 0];
  let [south, north] = [Infinity, -Infinity];
  for (let row = 0; row < sky.rows; row += 1) {
    const dec = -90 + row * degrees;
    const weight = Math.cos((dec * Math.PI) / 180);
    let seen = 0;
    for (let col = 0; col < sky.columns; col += 1) seen += sky.counts[row * sky.columns + col];
    inside += weight * seen;
    whole += weight * sky.columns;
    if (seen > 0) [south, north] = [Math.min(south, dec), Math.max(north, dec)];
  }
  for (const line of lines) for (const [, dec] of line) [south, north] = [Math.min(south, dec), Math.max(north, dec)];
  return { lines, sky_fraction: Math.round((inside / whole) * 10_000) / 10_000, declinations: south <= north ? [Math.round(south * 10) / 10, Math.round(north * 10) / 10] : null };
}

/**
 * One time of year, made from a year's answer: the nights from one night
 * of the year, counted and outlined here. It has what the year was drawn
 * under and its own two regions, and no more of a server's picture than
 * it knows: the dates of its nights, and dusk and dawn for the measured
 * nights among them only.
 */
export interface SeasonSky extends SkyTerms, SkyRegions {
  /** The first and last night counted, as `YYYY-MM-DD`. */
  first_night: string;
  last_night: string;
  nights_counted: number;
  /** The measured nights among those counted. */
  measured_nights: DirectorVisibleSky['nights'];
}

/** One time of year: its picture, its counts, and the nodes in view on
 *  every night and on any, which its lines are drawn round. */
export interface Season {
  view: SeasonSky;
  counts: SkyCounts;
  every: SkyCounts;
  any: SkyCounts;
}

/**
 * On how many of a season's nights a place is in view, read between the
 * nodes of the grid. Which side of each drawn line the place is on comes
 * first, and the count is kept on that side: every night inside the solid
 * line, none outside the dashed one, and at least one but not all between
 * them.
 */
export function seasonNights(season: Season, raDeg: number, decDeg: number): number {
  const nights = season.counts.nights;
  if (insideLines(season.every, raDeg, decDeg)) return nights;
  if (!insideLines(season.any, raDeg, decDeg)) return 0;
  return Math.min(nights - 1, Math.max(1, Math.round(countAt(season.counts, raDeg, decDeg))));
}

const DAY_MS = 86_400_000;
const dateAfter = (date: string, days: number) => new Date(Date.parse(`${date}T00:00:00Z`) + days * DAY_MS).toISOString().slice(0, 10);

export function seasonSky(year: DirectorVisibleSky, sky: MeasuredSky, first: number, nights = SEASON_NIGHTS): Season {
  const counts = windowCounts(sky, first, nights);
  const [every, any] = [nodesAt(counts, nights), nodesAt(counts, 1)];
  const start = year.nights[0]?.date ?? '1970-01-02';
  return {
    counts,
    every,
    any,
    view: {
      minimum_minutes: year.minimum_minutes,
      horizon: year.horizon,
      horizon_from: year.horizon_from,
      limits: year.limits,
      first_night: dateAfter(start, first),
      last_night: dateAfter(start, first + nights - 1),
      nights_counted: nights,
      measured_nights: year.nights.filter((_, week) => week * sky.every >= first && week * sky.every < first + nights),
      every_night: region(every),
      any_night: region(any),
    },
  };
}

/** A season's nights in a few words: for the slider's value. */
export function seasonFrom(season: SeasonSky): string {
  return `${season.nights_counted} nights from ${formatNight(season.first_night)}`;
}

/** A season's first and last night. */
export function seasonDates(season: SeasonSky): string {
  return `${formatNight(season.first_night)} to ${formatNight(season.last_night)}`;
}

/** A note when some of a season's measured nights never get dark. */
export function seasonDarknessNote(season: SeasonSky): string | null {
  const light = season.measured_nights.filter((night) => night.dusk_ms === null).length;
  return light === 0 ? null : `It does not get dark on ${light} of the ${season.measured_nights.length} nights measured.`;
}
