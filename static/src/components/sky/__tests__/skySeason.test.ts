import { describe, expect, it } from 'vitest';
import type { DirectorVisibleSky } from '../../../api/directorTypes';
import { countLines, insideLines, nightGrid, seasonDarknessNote, seasonDates, seasonFrom, seasonNights, seasonSky, seasonStarts, unpackMeasured, windowCounts } from '../skySeason';
import { formatNight } from '../skyModel';
import type { SkyCounts } from '../skyShade';

/** The small sky the server's tests hold too (`fill.rs`): six measured
 *  nights of three rows by twelve columns. */
const small = (night: number, index: number) => {
  const [row, col] = [Math.floor(index / 12), index % 12];
  return row === 0 ? false : row === 1 ? (col + 12 - ((3 * night) % 12)) % 12 < 7 : night !== 3 || col > 4;
};

/** Pack grids as the server does: a bit a node, lowest bit first, each
 *  night to whole bytes. */
function packed(nights: number, nodes: number, seen: (night: number, index: number) => boolean): string {
  const stride = Math.ceil(nodes / 8);
  const bytes = new Uint8Array(nights * stride);
  for (let night = 0; night < nights; night += 1) for (let index = 0; index < nodes; index += 1) if (seen(night, index)) bytes[night * stride + (index >> 3)] |= 1 << (index & 7);
  return btoa(String.fromCharCode(...bytes));
}

const nights = (count: number, every: number, dark: (night: number) => boolean = () => true) =>
  Array.from({ length: count }, (_, i) => ({ date: new Date(Date.parse('2026-09-24T00:00:00Z') + i * every * 86_400_000).toISOString().slice(0, 10), dusk_ms: dark(i) ? 1 : null, dawn_ms: dark(i) ? 2 : null }));

const year = (over: Partial<DirectorVisibleSky> = {}): DirectorVisibleSky => ({
  rig: { id: 'rig-1', name: 'Rig', revision: 1 },
  planning_site: null,
  site: { latitude_degrees: 34.2, longitude_degrees: -118.3, elevation_meters: 400 },
  location_from: 'rig',
  horizon: 'flat',
  horizon_from: 'none',
  limits: { minimum_altitude_degrees: 25, maximum_altitude_degrees: 90, horizon_offset_degrees: 0 },
  minimum_minutes: 30,
  rig_minimum_minutes: 30,
  step_minutes: 5,
  dark_below_degrees: -12,
  nights: nights(6, 7),
  span: 'year',
  every_nights: 7,
  nights_counted: 36,
  grid: { degrees: 2, columns: 12, rows: 3 },
  counts: '',
  measured: { in_view: packed(6, 36, small), bytes_a_night: 5, turn_degrees_a_night: 1 },
  every_night: { lines: [], sky_fraction: 0, declinations: null },
  any_night: { lines: [], sky_fraction: 0, declinations: null },
  ...over,
});

describe('the nights of a year from its measured nights', () => {
  it('unpacks a bit a node and refuses what does not fit', () => {
    const sky = unpackMeasured(year())!;
    expect(sky.grids).toHaveLength(6);
    for (let night = 0; night < 6; night += 1) for (let index = 0; index < 36; index += 1) expect(sky.grids[night][index]).toBe(small(night, index) ? 1 : 0);
    expect([sky.every, sky.counted, sky.columns, sky.rows]).toEqual([7, 36, 12, 3]);
    // A degree a night on a two-degree grid: one night on looks no columns
    // and one column back, and three on; a turn on a column looks at one.
    expect(sky.turns[1]).toEqual({ back: [0, 1], on: [3] });
    expect(sky.turns[2]).toEqual({ back: [1], on: [2, 3] });
    expect(sky.turns[6]).toEqual({ back: [3], on: [0, 1] });
    expect(unpackMeasured(year({ measured: undefined }))).toBeNull();
    expect(unpackMeasured(year({ nights: nights(5, 7) }))).toBeNull();
    expect(unpackMeasured(year({ measured: { in_view: 'not base64!', bytes_a_night: 5, turn_degrees_a_night: 1 } }))).toBeNull();
  });

  it('adds up a window to the numbers the server adds it up to', () => {
    const sky = unpackMeasured(year())!;
    // The same numbers as `a_window_adds_up_the_nights_the_whole_count_is_made_of`.
    expect(Array.from(windowCounts(sky, 0, 10).counts)).toEqual([...Array(12).fill(0), 1, 3, 5, 8, 10, 10, 10, 7, 5, 3, 0, 0, ...Array(12).fill(10)]);
    expect(Array.from(windowCounts(sky, 14, 10).counts)).toEqual([...Array(12).fill(0), 10, 7, 5, 3, 0, 0, 1, 3, 5, 8, 10, 10, 2, 1, 2, 4, 6, 8, 10, 10, 10, 8, 6, 4]);
    expect(Array.from(windowCounts(sky, 0, 36).counts)).toEqual([...Array(12).fill(0), 16, 17, 19, 23, 22, 22, 23, 19, 17, 16, 14, 14, 28, 26, 25, 26, 28, 30, 32, 34, 36, 34, 32, 30]);
    // A measured night is its own grid, and a night between is kept once made.
    expect(nightGrid(sky, 7)).toBe(sky.grids[1]);
    expect(nightGrid(sky, 9)).toBe(nightGrid(sky, 9));
    // Another window gives other counts.
    expect(Array.from(windowCounts(sky, 7, 10).counts)).not.toEqual(Array.from(windowCounts(sky, 0, 10).counts));
  });

  it('places the slider a week apart, with a last place that ends with the year', () => {
    const starts = seasonStarts({ ...unpackMeasured(year())!, counted: 365 });
    expect(starts).toHaveLength(49);
    expect(starts.slice(0, 3)).toEqual([0, 7, 14]);
    expect(starts.slice(-2)).toEqual([329, 334]);
    expect(seasonStarts({ ...unpackMeasured(year())!, counted: 20 })).toEqual([0]);
  });
});

describe('lines drawn from the counts', () => {
  const grid = (count: (col: number, row: number) => number, columns = 36, rows = 19): SkyCounts => {
    const counts = new Uint16Array(columns * rows);
    for (let row = 0; row < rows; row += 1) for (let col = 0; col < columns; col += 1) counts[row * columns + col] = count(col, row);
    return { counts, nights: 10, columns, rows };
  };

  it('rings a polar cap once, with the sky in view on its left, where the count passes the level', () => {
    // Ten nights from +40° north, four at +30°, none south of that: 10° rows.
    const sky = grid((_, row) => (row >= 13 ? 10 : row === 12 ? 4 : 0));
    const every = countLines(sky, 9.5);
    expect(every).toHaveLength(1);
    // Closed, all round the sky.
    expect(every[0][0]).toEqual(every[0][every[0].length - 1]);
    expect(every[0]).toHaveLength(37);
    // Between +30° (4 nights) and +40° (10), 9.5 nights is 11/12 of the way up.
    for (const [, dec] of every[0]) expect(dec).toBeCloseTo(30 + (10 * 5.5) / 6, 9);
    // North is on the left of a line run toward rising right ascension.
    expect(every[0][1][0]).toBeCloseTo((every[0][0][0] + 10) % 360, 9);
    const any = countLines(sky, 0.5);
    expect(any).toHaveLength(1);
    for (const [, dec] of any[0]) expect(dec).toBeCloseTo(20 + 10 / 8, 9);
    // All of the sky, or none of it, has no line.
    expect(countLines(grid(() => 10), 9.5)).toEqual([]);
    expect(countLines(grid(() => 0), 0.5)).toEqual([]);
  });

  it('keeps a patch across 0h in one closed line, run the same way round', () => {
    const sky = grid((col, row) => ((col >= 34 || col <= 1) && row >= 8 && row <= 10 ? 10 : 0));
    const lines = countLines(sky, 5);
    expect(lines).toHaveLength(1);
    const line = lines[0];
    expect(line[0]).toEqual(line[line.length - 1]);
    // Anticlockwise with north up and right ascension rising to the right:
    // the area swept, with steps across 0h unwrapped, is positive.
    let area = 0;
    for (let i = 0; i + 1 < line.length; i += 1) {
      const step = ((line[i + 1][0] - line[i][0] + 540) % 360) - 180;
      area += step * (line[i][1] + line[i + 1][1]) / 2;
    }
    expect(area).toBeLessThan(0);
    expect(Math.abs(area)).toBeGreaterThan(30 * 20);
    // Two patches that touch at a corner: the middle between them decides.
    // At a level over half their count they are two lines, under it one.
    const corners = grid((col, row) => ((col === 5 && row === 5) || (col === 6 && row === 6) ? 10 : 0));
    expect(countLines(corners, 5.5)).toHaveLength(2);
    expect(countLines(corners, 4.5)).toHaveLength(1);
  });
});

describe('a place against the lines', () => {
  // A grid of 10° cells whose nodes are 1 where listed.
  const nodes = (inside: Array<[number, number]>): SkyCounts => {
    const counts = new Uint16Array(36 * 19);
    for (const [col, row] of inside) counts[row * 36 + col] = 1;
    return { counts, nights: 1, columns: 36, rows: 19 };
  };
  // A place a share of the way across and up the cell whose south-west
  // node is column 5, row 5.
  const at = (x: number, y: number): [number, number] => [(5 + x) * 10, -90 + (5 + y) * 10];
  const corners: Record<string, [number, number]> = { sw: [5, 5], se: [6, 5], ne: [6, 6], nw: [5, 6] };
  const near: Record<string, (x: number, y: number) => [number, number]> = { sw: (x, y) => [x, y], se: (x, y) => [1 - x, y], ne: (x, y) => [1 - x, 1 - y], nw: (x, y) => [x, 1 - y] };

  it('is on the side the drawn line puts it on, in a cell with one corner out', () => {
    for (const out of Object.keys(corners)) {
      const sky = nodes(Object.entries(corners).filter(([name]) => name !== out).map(([, node]) => node));
      // The line cuts the corner off half way along each side. A place
      // 0.28 from the corner each way is past it, inside; taking the
      // count to change evenly across the cell would put it outside.
      const [x, y] = near[out](0.28, 0.28);
      expect(insideLines(sky, ...at(x, y)), `${out} out, 0.28`).toBe(true);
      const [cx, cy] = near[out](0.2, 0.2);
      expect(insideLines(sky, ...at(cx, cy)), `${out} out, 0.2`).toBe(false);
      // The line itself, drawn from the same nodes, runs between the two.
      const line = countLines(sky, 0.5)[0];
      const [ra, dec] = at(...near[out](0.25, 0.25));
      expect(line.some(([lineRa, lineDec], i) => i + 1 < line.length && Math.abs((lineRa + line[i + 1][0]) / 2 - ra) < 1e-9 && Math.abs((lineDec + line[i + 1][1]) / 2 - dec) < 1e-9), `${out} out, on the line`).toBe(true);
    }
  });

  it('is on the side the drawn line puts it on, in a cell with one corner in', () => {
    for (const name of Object.keys(corners)) {
      const sky = nodes([corners[name]]);
      expect(insideLines(sky, ...at(...near[name](0.2, 0.2))), `${name} in, 0.2`).toBe(true);
      expect(insideLines(sky, ...at(...near[name](0.28, 0.28))), `${name} in, 0.28`).toBe(false);
    }
  });

  it('follows a straight edge, and joins two corners across from each other through the middle', () => {
    const south = nodes([corners.sw, corners.se, [4, 5], [7, 5]]);
    expect(insideLines(south, ...at(0.3, 0.45))).toBe(true);
    expect(insideLines(south, ...at(0.3, 0.55))).toBe(false);
    const across = nodes([corners.sw, corners.ne]);
    expect(insideLines(across, ...at(0.5, 0.5))).toBe(true);
    expect(insideLines(across, ...at(0.8, 0.1))).toBe(false);
    expect(insideLines(across, ...at(0.1, 0.8))).toBe(false);
    // All in, and all out.
    expect(insideLines(nodes(Object.values(corners)), ...at(0.5, 0.5))).toBe(true);
    expect(insideLines(nodes([]), ...at(0.5, 0.5))).toBe(false);
  });
});

describe('one time of year', () => {
  it('counts 31 nights, outlines them and keeps the measured nights among them', () => {
    // 53 measured nights on the real grid: in view north of the equator
    // always, and south to −30° on the first ten measured nights only.
    const seen = (night: number, index: number) => {
      const dec = -90 + Math.floor(index / 180) * 2;
      return dec >= 0 || (dec >= -30 && night < 10);
    };
    const view = year({ nights: nights(53, 7, (night) => night !== 2), nights_counted: 365, grid: { degrees: 2, columns: 180, rows: 91 }, measured: { in_view: packed(53, 180 * 91, seen), bytes_a_night: 2048, turn_degrees_a_night: 1 } });
    const sky = unpackMeasured(view)!;
    const first = seasonSky(view, sky, 0);
    // A season has its dates and what the year was drawn under, and no
    // night-by-night dusk to mistake for a measured one.
    expect(first.view.first_night).toBe('2026-09-24');
    expect(first.view.last_night).toBe('2026-10-24');
    expect(first.view.minimum_minutes).toBe(30);
    expect('nights' in first.view).toBe(false);
    expect(seasonFrom(first.view)).toBe(`31 nights from ${formatNight('2026-09-24')}`);
    expect(seasonDates(first.view)).toBe(`${formatNight('2026-09-24')} to ${formatNight('2026-10-24')}`);
    expect(seasonDarknessNote(first.view)).toBe('It does not get dark on 1 of the 5 nights measured.');
    expect(first.view.nights_counted).toBe(31);
    // Nights 0, 7, 14, 21 and 28 are measured, and the third never gets dark.
    expect(first.view.measured_nights.map((night) => night.date)).toEqual(['2026-09-24', '2026-10-01', '2026-10-08', '2026-10-15', '2026-10-22']);
    expect(first.view.measured_nights[2].dusk_ms).toBeNull();
    expect(first.counts.nights).toBe(31);
    // In the first month everything north of −30° is in view every night.
    // Each line runs halfway between the last row in view and the first
    // row out of it.
    expect(first.view.every_night.declinations).toEqual([-31, 90]);
    expect(first.view.any_night?.declinations).toEqual([-31, 90]);
    expect(first.view.every_night.sky_fraction).toBeCloseTo(0.75, 1);
    // From night 49 the southern band is in view on the first nights only:
    // the solid line is at the equator and the dashed one further south.
    const later = seasonSky(view, sky, 49);
    expect(later.view.first_night).toBe('2026-11-12');
    expect(seasonDarknessNote(later.view)).toBeNull();
    expect(later.view.every_night.declinations).toEqual([-1, 90]);
    expect(later.view.any_night?.declinations).toEqual([-31, 90]);
    // A place is counted on its side of each line: every night north of
    // the solid one, none south of the dashed one, some nights between.
    expect(seasonNights(later, 100, 20)).toBe(31);
    expect(seasonNights(later, 100, -0.9)).toBe(31);
    // North of the solid line a place has every night, whatever the count
    // between its nodes comes to; just south of it, that count.
    expect(seasonNights(later, 100, -1.01)).toBe(23);
    expect(seasonNights(later, 100, -10)).toBe(15);
    expect(seasonNights(later, 100, -30.9)).toBe(8);
    expect(seasonNights(later, 100, -31.5)).toBe(0);
    expect(seasonNights(later, 100, -80)).toBe(0);
    expect(later.view.every_night.sky_fraction).toBeCloseTo(0.5, 1);
    expect(later.counts.counts[30 * 180]).toBe(15);
    // Late in the year the band is gone.
    const last = seasonSky(view, sky, 334);
    expect(last.view.last_night).toBe('2027-09-23');
    expect(last.view.any_night?.declinations).toEqual([-1, 90]);
  });
});
