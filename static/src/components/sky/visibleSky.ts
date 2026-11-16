import { isAxiosError } from 'axios';
import { projectedPath, projectedPoint, type LonLat, type PathScale, type SkyView } from '../../utils/skyProjection';
import type { DirectorVisibleSky } from '../../api/directorTypes';
import { formatNightDate } from '../nights/nightsModel';
import { formatNight } from './skyModel';

const DAY_MS = 86_400_000;

/**
 * Which night is under way at a longitude at an instant: a count that
 * steps at each local mean noon there, as the server's nights do. Two
 * instants with the same count are in the same night.
 */
export function nightCount(atMs: number, longitudeDegrees: number): number {
  return Math.floor((atMs + (longitudeDegrees / 15) * 3_600_000 - DAY_MS / 2) / DAY_MS);
}

export function formatMinutes(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

/** Which horizon the lines were drawn against, and where it was set. */
export function horizonText(view: DirectorVisibleSky): string {
  const minimum = `${Number(view.limits.minimum_altitude_degrees.toFixed(1))}°`;
  if (view.horizon === 'flat') return `flat horizon at ${minimum}`;
  const from = view.horizon_from === 'site' ? "the site's" : "the rig's";
  const offset = view.limits.horizon_offset_degrees > 0 ? ` raised ${Number(view.limits.horizon_offset_degrees.toFixed(1))}°` : '';
  return `${from} custom horizon${offset}, at least ${minimum}`;
}

/**
 * Which nights were counted, in a few words. `fromTonight` is for nights
 * asked for without a date: the server starts those from the night under
 * way at the rig's site, or from the next once its dark has ended, which
 * either way is the night a person there calls tonight.
 */
export function nightsText(view: DirectorVisibleSky, fromTonight: boolean): string {
  const first = view.nights[0]?.date;
  const last = view.nights[view.nights.length - 1]?.date;
  if (!first || !last) return '';
  const from = fromTonight ? 'from tonight, ' : '';
  if (view.span === 'year') return `one night a week for a year ${from}${formatNight(first)}${fromTonight ? ',' : ''} to ${formatNight(last)}`;
  if (view.nights.length === 1) return fromTonight ? `tonight, ${formatNightDate(first)}` : formatNightDate(first);
  return `${view.nights.length} nights ${from}${formatNightDate(first)}${fromTonight ? ',' : ''} to ${formatNightDate(last)}`;
}

/** The legend: the rig, the minimum time, the nights and the horizon. */
export function visibleSkyLegend(view: DirectorVisibleSky, rigName: string, fromTonight = true): string {
  return [`${rigName}: in view ${formatMinutes(view.minimum_minutes)} or more`, nightsText(view, fromTonight), horizonText(view)].filter(Boolean).join(' · ');
}

const percent = (fraction: number) => `${Math.round(fraction * 100)}%`;
const degrees = (value: number) => `${value < 0 ? '−' : '+'}${Math.round(Math.abs(value))}°`;

/** One region in words: its share of the sky and the declinations it spans. */
function regionText(region: DirectorVisibleSky['every_night']): string {
  if (!region.declinations) return 'none of the sky';
  const [south, north] = region.declinations;
  return `${percent(region.sky_fraction)} of the sky, declination ${degrees(south)} to ${degrees(north)}`;
}

/** What the lines enclose, for the legend and for readers who cannot see
 *  the map. */
export function visibleSkyShares(view: DirectorVisibleSky): string {
  if (!view.any_night) return `In view: ${regionText(view.every_night)}.`;
  return `In view every night: ${regionText(view.every_night)}. On at least one night: ${regionText(view.any_night)}.`;
}

/** The map in words: what it shows, for whom, and the numbers. */
export function visibleSkySummary(view: DirectorVisibleSky, rigName: string): string {
  return `Map of the sky ${visibleSkyLegend(view, rigName)}. ${visibleSkyShares(view)}`;
}

/** Where a place stands, from the nights it is in view out of those counted. */
export function standing(nights: number, counted: number): 'every' | 'some' | 'none' {
  return nights >= counted ? 'every' : nights > 0 ? 'some' : 'none';
}

/** A note when some of the nights never get dark. */
export function darknessNote(view: DirectorVisibleSky): string | null {
  const light = view.nights.filter((night) => night.dusk_ms === null).length;
  if (light === 0) return null;
  if (view.nights.length === 1) return 'It does not get dark on this night.';
  return light === view.nights.length ? 'It does not get dark on these nights.' : `It does not get dark on ${light} of these nights.`;
}

/** The server's refusal as text, and whether it is about the rig's site.
 *  The API client hands on the server's message as an `Error` whose cause
 *  is the response; a bare response error is read the same way. */
export function visibleSkyError(error: unknown): { text: string; needsSite: boolean } {
  const response = isAxiosError(error) ? error.response : error instanceof Error && isAxiosError(error.cause) ? error.cause.response : undefined;
  const text = (response?.data as { error?: string } | undefined)?.error;
  if (text) return { text, needsSite: response?.status === 422 && /Planning site/.test(text) };
  return { text: 'The sky in view could not be worked out.', needsSite: false };
}

const DEG = Math.PI / 180;

function unit([ra, dec]: LonLat): [number, number, number] {
  return [Math.cos(dec * DEG) * Math.cos(ra * DEG), Math.cos(dec * DEG) * Math.sin(ra * DEG), Math.sin(dec * DEG)];
}

function separation(a: [number, number, number], b: [number, number, number]): number {
  return Math.acos(Math.min(1, Math.max(-1, a[0] * b[0] + a[1] * b[1] + a[2] * b[2]))) / DEG;
}

/**
 * Short marks along a line, each pointing into the visible sky, so the two
 * sides can be told apart without colour. The server runs every line with
 * the visible sky on its left, seen with north up and right ascension
 * rising to the right; on the sphere that side is `p × (next − previous)`.
 * A mark is `length` map units long and one falls every `everyDegrees` of
 * line. Zoomed in, a mark is a fraction of a unit, so its ends are written
 * with enough digits to keep its direction.
 */
export function visibleSideMarks(line: LonLat[], view: SkyView, at: PathScale, everyDegrees: number, length: number): string {
  if (line.length < 3) return '';
  const points = line.map(unit);
  // Three digits more than the mark's own size: its direction to a small part of a degree.
  const digits = Math.min(8, Math.max(1, Math.ceil(-Math.log10(length)) + 3));
  let path = '';
  let travelled = everyDegrees / 2;
  for (let i = 1; i < points.length - 1; i += 1) {
    travelled += separation(points[i - 1], points[i]);
    if (travelled < everyDegrees) continue;
    travelled = 0;
    const p = points[i];
    const along = [points[i + 1][0] - points[i - 1][0], points[i + 1][1] - points[i - 1][1], points[i + 1][2] - points[i - 1][2]];
    const side = [p[1] * along[2] - p[2] * along[1], p[2] * along[0] - p[0] * along[2], p[0] * along[1] - p[1] * along[0]];
    const size = Math.hypot(side[0], side[1], side[2]);
    if (!(size > 0)) continue;
    // A degree into the visible side, to find the direction on the map.
    const reach = Math.tan(DEG) / size;
    const q = [p[0] + side[0] * reach, p[1] + side[1] * reach, p[2] + side[2] * reach];
    const norm = Math.hypot(q[0], q[1], q[2]);
    const ra = ((Math.atan2(q[1], q[0]) / DEG) % 360 + 360) % 360;
    const dec = Math.asin(Math.min(1, Math.max(-1, q[2] / norm))) / DEG;
    const from = projectedPoint(line[i][0], line[i][1], view, at);
    const to = projectedPoint(ra, dec, view, at);
    if (!from.visible || !to.visible) continue;
    const dx = to.x - from.x;
    const dy = to.y - from.y;
    const span = Math.hypot(dx, dy);
    // Across the seam the two ends land far apart: no mark there.
    if (!(span > 0) || span > at.scale * 0.2) continue;
    path += `M${from.x.toFixed(digits)},${from.y.toFixed(digits)}L${(from.x + (dx / span) * length).toFixed(digits)},${(from.y + (dy / span) * length).toFixed(digits)}`;
  }
  return path;
}

/** What the map draws for the overlay. */
export interface VisibleSkyLines {
  every: LonLat[][];
  /** Null for a single night, when it is the same sky. */
  any: LonLat[][] | null;
}

export function visibleSkyLines(view: DirectorVisibleSky): VisibleSkyLines {
  return { every: view.every_night.lines, any: view.any_night ? view.any_night.lines : null };
}

/** The line paths and side marks for one set of lines. */
export function visiblePaths(lines: LonLat[][], view: SkyView, at: PathScale, everyDegrees: number, length: number): { line: string; marks: string } {
  return {
    line: lines.map((line) => projectedPath(line, view, at)).join(''),
    marks: lines.map((line) => visibleSideMarks(line, view, at, everyDegrees, length)).join(''),
  };
}
