import { isAxiosError } from 'axios';
import type { LonLat } from '../../utils/skyProjection';
import type { DirectorVisibleSky } from '../../api/directorTypes';
import { formatNightDate } from '../nights/nightsModel';
import { formatNight } from './skyModel';

/** What a picture of the sky in view was drawn under: the minimum time,
 *  the horizon and the altitude limits. */
export type SkyTerms = Pick<DirectorVisibleSky, 'minimum_minutes' | 'horizon' | 'horizon_from' | 'limits'>;
/** The two regions of a picture: sky in view every night, and on any. */
export type SkyRegions = Pick<DirectorVisibleSky, 'every_night' | 'any_night'>;

const DAY_MS = 86_400_000;

/**
 * Which night is under way at a longitude at an instant: a count that
 * steps at each local mean noon there, as the server's nights do. Two
 * instants with the same count are in the same night.
 */
export function nightCount(atMs: number, longitudeDegrees: number): number {
  return Math.floor((atMs + (longitudeDegrees / 15) * 3_600_000 - DAY_MS / 2) / DAY_MS);
}

/** The instant the night after the one at `atMs` begins at a longitude:
 *  when [`nightCount`] next steps. */
export function nextNightAt(atMs: number, longitudeDegrees: number): number {
  return (nightCount(atMs, longitudeDegrees) + 1) * DAY_MS + DAY_MS / 2 - (longitudeDegrees / 15) * 3_600_000;
}

export function formatMinutes(minutes: number): string {
  if (minutes < 60) return `${minutes} min`;
  const hours = Math.floor(minutes / 60);
  const rest = minutes % 60;
  return rest === 0 ? `${hours} h` : `${hours} h ${rest} min`;
}

/** Which horizon the lines were drawn against, and where it was set. */
export function horizonText(view: SkyTerms): string {
  const minimum = `${Number(view.limits.minimum_altitude_degrees.toFixed(1))}°`;
  if (view.horizon === 'flat') return `flat horizon at ${minimum}`;
  const from = view.horizon_from === 'site' ? "the site's" : "the rig's";
  const offset = view.limits.horizon_offset_degrees > 0 ? ` raised ${Number(view.limits.horizon_offset_degrees.toFixed(1))}°` : '';
  return `${from} custom horizon${offset}, at least ${minimum}`;
}

/**
 * Which nights were counted, in a few words. The nights are asked for
 * without a date: the server starts them from the night under way at the
 * rig's site, or from the next once its dark has ended, which either way
 * is the night a person there calls tonight.
 */
export function nightsText(view: DirectorVisibleSky): string {
  const first = view.nights[0]?.date;
  const last = view.nights[view.nights.length - 1]?.date;
  if (!first || !last) return '';
  if (view.span === 'year') return `${view.nights_counted} nights from tonight, ${formatNight(first)}, to ${formatNight(last)}, one a week measured`;
  if (view.nights.length === 1) return `tonight, ${formatNightDate(first)}`;
  return `${view.nights.length} nights from tonight, ${formatNightDate(first)}, to ${formatNightDate(last)}`;
}

/** A legend: the rig, the minimum time, the nights in the words given,
 *  and the horizon. */
export function skyLegend(view: SkyTerms, rigName: string, nights: string): string {
  return [`${rigName}: in view ${formatMinutes(view.minimum_minutes)} or more`, nights, horizonText(view)].filter(Boolean).join(' · ');
}

/** The legend of the server's picture. */
export function visibleSkyLegend(view: DirectorVisibleSky, rigName: string): string {
  return skyLegend(view, rigName, nightsText(view));
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
export function visibleSkyShares(view: SkyRegions): string {
  if (!view.any_night) return `In view: ${regionText(view.every_night)}.`;
  return `In view every night: ${regionText(view.every_night)}. On at least one night: ${regionText(view.any_night)}.`;
}

/** What a year's lines enclose, in words: the sky in view on enough
 *  nights to be left clear, and the sky in view on any. */
export function clearSkyShares(view: { at_least: DirectorVisibleSky['every_night'] & { nights: number }; any_night: DirectorVisibleSky['any_night'] }): string {
  const clear = `In view on ${view.at_least.nights} nights or more: ${regionText(view.at_least)}.`;
  return view.any_night ? `${clear} On at least one night: ${regionText(view.any_night)}.` : clear;
}

/** Where a place stands, from the nights it is in view out of those counted. */
export function standing(nights: number, counted: number): 'every' | 'some' | 'none' {
  return nights >= counted ? 'every' : nights > 0 ? 'some' : 'none';
}

/** A note when some of the nights never get dark. */
export function darknessNote(view: Pick<DirectorVisibleSky, 'nights' | 'span'>): string | null {
  const light = view.nights.filter((night) => night.dusk_ms === null).length;
  if (light === 0) return null;
  if (view.nights.length === 1) return 'It does not get dark on this night.';
  if (view.span === 'year') return `It does not get dark on ${light} of the ${view.nights.length} nights measured.`;
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

/** What the map draws for the overlay. */
export interface VisibleSkyLines {
  every: LonLat[][];
  /** Null for a single night, when it is the same sky. */
  any: LonLat[][] | null;
}

export function visibleSkyLines(view: SkyRegions): VisibleSkyLines {
  return { every: view.every_night.lines, any: view.any_night ? view.any_night.lines : null };
}
