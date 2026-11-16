import { isAxiosError } from 'axios';
import { projectedPath, projectedPoint, type LonLat, type PathScale, type SkyView } from '../../utils/skyProjection';
import type { DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';

/** The most nights one request covers, as the server allows. */
export const MAX_VISIBLE_NIGHTS = 31;
/** The range a switch to dates starts with. */
export const DEFAULT_RANGE_NIGHTS = 14;

/** What the person chose for the Visible sky overlay. */
export interface VisibleSkyChoice {
  on: boolean;
  /** The rig the sky is judged for; null until one is picked. */
  rigId: string | null;
  /** Minutes typed on the Sky view; null uses the rig's minimum time. */
  minimumMinutes: number | null;
  /** Tonight, or a range of nights by the evening's date. */
  when: 'tonight' | 'dates';
  from: string | null;
  to: string | null;
}

export const NO_VISIBLE_SKY: VisibleSkyChoice = { on: false, rigId: null, minimumMinutes: null, when: 'tonight', from: null, to: null };

const DAY_MS = 86_400_000;
const DATE = /^\d{4}-\d{2}-\d{2}$/;

function dayOf(date: string): number | null {
  if (!DATE.test(date)) return null;
  const at = Date.parse(`${date}T00:00:00Z`);
  return Number.isFinite(at) ? at : null;
}

/** A date `days` after `date`, both `YYYY-MM-DD`. */
export function addDays(date: string, days: number): string {
  const at = dayOf(date);
  return at === null ? date : new Date(at + days * DAY_MS).toISOString().slice(0, 10);
}

/** Today's date where the browser is, `YYYY-MM-DD`. */
export function localDate(nowMs: number): string {
  const now = new Date(nowMs);
  const two = (value: number) => String(value).padStart(2, '0');
  return `${now.getFullYear()}-${two(now.getMonth() + 1)}-${two(now.getDate())}`;
}

/** How many nights a range covers, first and last included; null when the
 *  dates do not make a range. */
export function nightsBetween(from: string | null, to: string | null): number | null {
  const first = from ? dayOf(from) : null;
  const last = to ? dayOf(to) : null;
  if (first === null || last === null || last < first) return null;
  return Math.round((last - first) / DAY_MS) + 1;
}

/** The request for a choice, or null when it is off, has no rig, or its
 *  dates do not make a range the server accepts. */
export function visibleSkyRequest(choice: VisibleSkyChoice): DirectorVisibleSkyRequest | null {
  if (!choice.on || !choice.rigId) return null;
  const request: DirectorVisibleSkyRequest = { rig: choice.rigId };
  if (choice.minimumMinutes !== null) request.minimum_minutes = choice.minimumMinutes;
  if (choice.when === 'dates') {
    const nights = nightsBetween(choice.from, choice.to);
    if (nights === null || nights > MAX_VISIBLE_NIGHTS || !choice.from) return null;
    request.first_night = choice.from;
    request.nights = nights;
  }
  return request;
}

/** Why a range cannot be asked for, in a few words; null when it can. */
export function rangeProblem(choice: VisibleSkyChoice): string | null {
  if (choice.when !== 'dates') return null;
  const nights = nightsBetween(choice.from, choice.to);
  if (nights === null) return 'Choose a first and a last night.';
  if (nights > MAX_VISIBLE_NIGHTS) return `Choose ${MAX_VISIBLE_NIGHTS} nights or fewer.`;
  return null;
}

export function formatMinutes(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

const MONTHS = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

function shortDate(date: string): string {
  const [, month, day] = date.split('-').map(Number);
  return `${day} ${MONTHS[month - 1] ?? ''}`.trim();
}

/** Which horizon the lines were drawn against, and where it was set. */
export function horizonText(view: DirectorVisibleSky): string {
  const minimum = `${Number(view.limits.minimum_altitude_degrees.toFixed(1))}°`;
  if (view.horizon === 'flat') return `flat horizon at ${minimum}`;
  const from = view.horizon_from === 'site' ? "the site's" : "the rig's";
  const offset = view.limits.horizon_offset_degrees > 0 ? ` raised ${Number(view.limits.horizon_offset_degrees.toFixed(1))}°` : '';
  return `${from} custom horizon${offset}, at least ${minimum}`;
}

/** The legend under the controls and on a saved picture: the rig, the
 *  minimum time, the nights and the horizon. `tonightAtMs` is the time now
 *  when the night was chosen as Tonight, and null when it was chosen by
 *  date. Between dawn and noon the night under way is the one that just
 *  ended, as on Nights, so the legend calls it last night. */
export function visibleSkyLegend(view: DirectorVisibleSky, rigName: string, tonightAtMs: number | null): string {
  const first = view.nights[0]?.date;
  const last = view.nights[view.nights.length - 1]?.date;
  const dawn = view.nights[0]?.dawn_ms ?? null;
  const tonight = tonightAtMs !== null && dawn !== null && dawn <= tonightAtMs ? 'last night' : 'tonight';
  const when = !first || !last
    ? ''
    : view.nights.length === 1
      ? tonightAtMs !== null ? `${tonight} (${shortDate(first)})` : shortDate(first)
      : `${shortDate(first)} to ${shortDate(last)}`;
  return [`${rigName}: in view ${formatMinutes(view.minimum_minutes)} or more`, when, horizonText(view)].filter(Boolean).join(' · ');
}

/** What the lines enclose, as shares of the whole sky. */
export function visibleSkyShares(view: DirectorVisibleSky): string {
  const percent = (fraction: number) => `${Math.round(fraction * 100)}%`;
  if (!view.any_night) return `The line encloses ${percent(view.every_night.sky_fraction)} of the sky.`;
  return `Solid line: every night, ${percent(view.every_night.sky_fraction)} of the sky. Dashed line: at least one night, ${percent(view.any_night.sky_fraction)}.`;
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
  return { text: 'The visible sky could not be worked out.', needsSite: false };
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
 * A mark is `length` pixels long and one falls every `everyDegrees` of line.
 */
export function visibleSideMarks(line: LonLat[], view: SkyView, at: PathScale, everyDegrees: number, length: number): string {
  if (line.length < 3) return '';
  const points = line.map(unit);
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
    path += `M${from.x.toFixed(1)},${from.y.toFixed(1)}L${(from.x + (dx / span) * length).toFixed(1)},${(from.y + (dy / span) * length).toFixed(1)}`;
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
