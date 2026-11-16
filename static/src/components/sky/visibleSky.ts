import { isAxiosError } from 'axios';
import { projectedPath, projectedPoint, type LonLat, type PathScale, type SkyView } from '../../utils/skyProjection';
import type { DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { formatNightDate, isDate, localDate } from '../nights/nightsModel';

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
/** The first and last nights the server times. */
export const FIRST_NIGHT = '1970-01-02';
export const LAST_NIGHT = '2098-12-31';

/** A date as a day count, for arithmetic that no time zone can shift. */
function dayOf(date: string): number | null {
  if (!isDate(date)) return null;
  const [year, month, day] = date.split('-').map(Number);
  return Date.UTC(year, month - 1, day);
}

/** A date `days` after `date`, both `YYYY-MM-DD`. */
export function addDays(date: string, days: number): string {
  const at = dayOf(date);
  return at === null ? date : new Date(at + days * DAY_MS).toISOString().slice(0, 10);
}

/** Whether the night the server called the night under way has ended. */
function ended(night: DirectorVisibleSky['nights'][number] | undefined, nowMs: number): boolean {
  return night !== undefined && night.dawn_ms !== null && night.dawn_ms <= nowMs;
}

/**
 * The first night a range of dates opens with. When the lines for Tonight
 * are known, it is the rig's night as the server dated it, by the site's
 * longitude, or the next one once that night has ended. Otherwise it is
 * today where the browser is, which may be a day off for a far site.
 */
export function firstNightFrom(tonight: DirectorVisibleSky | undefined, nowMs: number): string {
  const night = tonight?.nights.length === 1 ? tonight.nights[0] : undefined;
  if (!night) return localDate(new Date(nowMs));
  return ended(night, nowMs) ? addDays(night.date, 1) : night.date;
}

/**
 * A choice with a range that is still worth showing. A remembered range
 * whose last night is over is dropped, so Dates opens with the two weeks
 * from `today` again, not with nights long gone.
 */
export function currentChoice(choice: VisibleSkyChoice, today: string): VisibleSkyChoice {
  if (!choice.to || choice.to >= today) return choice;
  return choice.when === 'dates'
    ? { ...choice, from: today, to: addDays(today, DEFAULT_RANGE_NIGHTS - 1) }
    : { ...choice, from: null, to: null };
}

/**
 * Which night is under way at a longitude at an instant: a count that
 * steps at each local mean noon there, as the server's nights do. Two
 * instants with the same count are in the same night.
 */
export function nightCount(atMs: number, longitudeDegrees: number): number {
  return Math.floor((atMs + (longitudeDegrees / 15) * 3_600_000 - DAY_MS / 2) / DAY_MS);
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
    if (nights === null || nights > MAX_VISIBLE_NIGHTS || !choice.from || rangeProblem(choice)) return null;
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
  if ((choice.from ?? '') < FIRST_NIGHT || (choice.to ?? '') > LAST_NIGHT) return `Choose nights from ${FIRST_NIGHT} to ${LAST_NIGHT}.`;
  if (nights > MAX_VISIBLE_NIGHTS) return `Choose ${MAX_VISIBLE_NIGHTS} nights or fewer.`;
  return null;
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

/** The legend under the controls and on a saved picture: the rig, the
 *  minimum time, the nights and the horizon. `tonightAtMs` is the time now
 *  when the night was chosen as Tonight, and null when it was chosen by
 *  date. Between dawn and noon the night under way is the one that just
 *  ended, as on Nights, so the legend calls it last night. */
export function visibleSkyLegend(view: DirectorVisibleSky, rigName: string, tonightAtMs: number | null): string {
  const first = view.nights[0]?.date;
  const last = view.nights[view.nights.length - 1]?.date;
  const tonight = tonightAtMs !== null && ended(view.nights[0], tonightAtMs) ? 'last night' : 'tonight';
  const when = !first || !last
    ? ''
    : view.nights.length === 1
      ? tonightAtMs !== null ? `${tonight} (${formatNightDate(first)})` : formatNightDate(first)
      : `${formatNightDate(first)} to ${formatNightDate(last)}`;
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
