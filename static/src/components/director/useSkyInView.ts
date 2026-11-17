import { useEffect, useMemo, useRef, useState } from 'react';
import { keepPreviousData, useMutationState, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorSkyPosition, DirectorVisiblePlacesRequest, DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { formatNight } from '../sky/skyModel';
import { SEASON_NIGHTS, seasonDarknessNote, seasonDates, seasonFrom, seasonNights, seasonSky, seasonStarts, unpackMeasured } from '../sky/skySeason';
import { unpackCounts, type SkyCounts } from '../sky/skyShade';
import { darknessNote, nightCount, skyLegend, standing, visibleSkyError, visibleSkyLegend, visibleSkyLines, visibleSkyShares, type VisibleSkyLines } from '../sky/visibleSky';
import { retryWhenBusy } from './retry';
import { SKY_INPUTS_MUTATION } from './skyInputs';

/** Now and Year are the server's two time frames. Season shows the
 *  server's year a month at a time, made here. */
export type Span = 'now' | 'year' | 'season';
export const SPANS: ReadonlyArray<readonly [Span, string]> = [['now', 'Now'], ['year', 'Year'], ['season', 'Season']];
const SPAN_LABEL: Record<Span, string> = { now: 'Now', year: 'Year', season: 'Season' };
export const MAX_MINUTES = 24 * 60;
/** How long the minimum field waits after the last keystroke or spinner
 *  step before it asks for a new picture. */
export const MINIMUM_SETTLE_MS = 500;
/** How long after the last slider step the cover is drawn at full size. */
export const SEASON_SETTLE_MS = 180;
/** Page Up and Page Down move the slider this many weeks: about a month. */
export const SEASON_PAGE = 4;
const DAY_MS = 86_400_000;
/** A month's name is centred on the night the month begins. One that
 *  begins within this share of the slider's length from an end is left out. */
const MONTH_END_ROOM = 0.04;
/** The cover's raster, in pixels across: at rest, and while the view or
 *  the slider moves. */
export const COVER_ACROSS = 384;
export const COVER_ACROSS_MOVING = 128;
/** The view's center gets words of its own once it is this far from the
 *  target, in degrees: about the accuracy of the lines. */
const VIEW_APART_DEGREES = 1;

export interface SkyInViewRig {
  id: string;
  name: string;
  /** The rig's database, for the link to its Rig setup. */
  slug?: string | null;
}

/** A place to a millionth of a degree: enough to tell two places apart,
 *  and steady from one render to the next. */
const rounded = (center: DirectorSkyPosition): DirectorSkyPosition => ({ ra_degrees: (Math.round(center.ra_degrees * 1e6) / 1e6) % 360, dec_degrees: Math.round(center.dec_degrees * 1e6) / 1e6 });

function apart(a: DirectorSkyPosition, b: DirectorSkyPosition): number {
  const rad = Math.PI / 180;
  const cos = Math.sin(a.dec_degrees * rad) * Math.sin(b.dec_degrees * rad) + Math.cos(a.dec_degrees * rad) * Math.cos(b.dec_degrees * rad) * Math.cos((a.ra_degrees - b.ra_degrees) * rad);
  return Math.acos(Math.min(1, Math.max(-1, cos))) / rad;
}

/** Where each month begins along the slider, as a share of its length:
 *  the slider runs from the year's first night to `lastStart` nights on. */
export function monthMarks(first: string | undefined, lastStart: number): Array<{ label: string; share: number }> {
  if (!first || lastStart <= 0) return [];
  const start = Date.parse(`${first}T00:00:00Z`);
  const marks: Array<{ label: string; share: number }> = [];
  const month = new Date(start);
  month.setUTCDate(1);
  for (let i = 0; i < 14; i += 1) {
    month.setUTCMonth(month.getUTCMonth() + 1);
    const nights = Math.round((month.getTime() - start) / DAY_MS);
    if (nights > lastStart) break;
    // A month that begins at either end of the slider has no room for its
    // name inside the track.
    if (nights < MONTH_END_ROOM * lastStart || nights > (1 - MONTH_END_ROOM) * lastStart) continue;
    marks.push({ label: month.toLocaleDateString(undefined, { month: 'short', timeZone: 'UTC' }), share: nights / lastStart });
  }
  return marks;
}

/** A place's nights in words. */
function placeWords(what: string, nights: number, counted: number, year: boolean): string {
  const of = year ? `the ${counted} nights of the year` : counted === 1 ? 'the night counted' : `the ${counted} nights counted`;
  const where = standing(nights, counted);
  if (where === 'none') return `${what} is never in view on ${of}.`;
  if (where === 'every') return counted === 1 ? `${what} is in view on ${of}.` : `${what} is in view on all of ${of}.`;
  return `${what} is in view on ${nights} of ${of}.`;
}

/** What Framing gets back: what to draw on the stage, the few words for
 *  the stage, and all the panel needs. */
export interface SkyInViewModel {
  rig: SkyInViewRig | null;
  on: boolean;
  /** The cover's counts and the lines, when there is a picture to draw. */
  layer: { counts: SkyCounts; lines: VisibleSkyLines } | null;
  /** The cover is changing many times a second, under the slider. */
  changing: boolean;
  /** A few words for the stage when the target, or the sky at the middle
   *  of the view, is not in view every night; `title` is the sentence. */
  notes: Array<{ text: string; title: string }>;
  panel: {
    busy: boolean;
    /** One line for the panel's summary. */
    status: string;
    failure: { text: string; needsSite: boolean } | null;
    /** What the picture was drawn under, and the nights counted. */
    legend: string | null;
    /** The shares of the sky, and a note on nights that never get dark. */
    shares: string | null;
    /** The view's center in words, when it is not the target. */
    viewWords: string | null;
    span: Span;
    setSpan: (span: Span) => void;
    typed: string;
    typeMinutes: (text: string) => void;
    sendMinutes: (text: string) => void;
    /** The rig's own minimum, when another is in use. */
    rigMinimum: number | null;
    useRigMinimum: () => void;
    season: { position: number; last: number; from: string | null; dates: string | null; slideTo: (next: number) => void; months: Array<{ label: string; share: number }> } | null;
    year: boolean;
  };
}

/**
 * The sky a rig can shoot, for Framing to draw over its stage: sky the rig
 * cannot shoot covered, more heavily the fewer nights it is in view for
 * the minimum time, with a solid line round the sky in view every night
 * and a dashed one round the sky in view on any. Now counts tonight and
 * the 30 nights after; Year counts every night of a year, of which the
 * server measures one a week and works out the rest; Season shows 31
 * nights of that year at a time, added up here from the year's answer, so
 * moving its slider asks for nothing.
 *
 * `target` and `viewCenter` get words of their own: the server counts the
 * nights for each place itself, so the words agree with the lines.
 */
export function useSkyInView({ rig, on, target, viewCenter }: { rig: SkyInViewRig | null; on: boolean; target: DirectorSkyPosition | null; viewCenter: DirectorSkyPosition | null }): SkyInViewModel {
  const now = useDirectorClock();
  const [span, setSpan] = useState<Span>('now');
  // Minutes typed here; null uses the rig's minimum time. Another rig
  // starts from its own.
  const [minimumFor, setMinimumFor] = useState<{ rig: string; minutes: number } | null>(null);
  const minimum = minimumFor && minimumFor.rig === rig?.id ? minimumFor.minutes : null;
  const setMinimum = (minutes: number | null) => setMinimumFor(minutes === null || !rig ? null : { rig: rig.id, minutes });

  // Season is made from the year's answer with its measured nights, which
  // only Season asks for: they treble the answer.
  const wantsSeason = span === 'season';
  const rigId = rig?.id ?? null;
  const request: DirectorVisibleSkyRequest | null = useMemo(
    () => (rigId ? { rig: rigId, span: wantsSeason ? 'year' : span, ...(wantsSeason ? { measured: true } : {}), ...(minimum !== null ? { minimum_minutes: minimum } : {}) } : null),
    [rigId, span, wantsSeason, minimum]
  );
  // The picture for fixed inputs does not change, so it is not fetched on
  // the app's timer or on focus. A superseded request is cancelled, and
  // the server stops measuring for it.
  const query = useQuery({
    queryKey: ['directorVisibleSky', request],
    queryFn: ({ signal }) => apiClient.getDirectorVisibleSky(request!, signal),
    enabled: on && request !== null,
    retry: retryWhenBusy,
    retryDelay: 1200,
    staleTime: 5 * 60_000,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    placeholderData: keepPreviousData,
  });
  // The picture on screen: the one asked for, or, while that loads, the
  // last one for this rig. Another rig's picture is never shown.
  const view: DirectorVisibleSky | undefined = on && request && !query.isError && query.data?.rig.id === request.rig ? query.data : undefined;
  const awaited = view !== undefined && (wantsSeason ? query.isPlaceholderData : view.span !== span);
  // The nights start again at the next local noon at the rig's site.
  const longitude = query.data?.site.longitude_degrees;
  const nightIsOver = on && request !== null && longitude !== undefined && !query.isPlaceholderData && nightCount(now, longitude) !== nightCount(query.dataUpdatedAt, longitude);
  const refetch = query.refetch;
  useEffect(() => {
    if (nightIsOver) void refetch();
  }, [nightIsOver, refetch]);
  // A save that can move the rig's site, horizon, limits or minimum time
  // makes the picture out of date; no other save does.
  const client = useQueryClient();
  const lastSave = Math.max(0, ...useMutationState({ filters: { mutationKey: SKY_INPUTS_MUTATION, status: 'success' }, select: (mutation) => mutation.state.submittedAt }));
  const seenSave = useRef(lastSave);
  useEffect(() => {
    if (lastSave <= seenSave.current) return;
    seenSave.current = lastSave;
    void client.invalidateQueries({ queryKey: ['directorVisibleSky'] });
  }, [lastSave, client]);
  // Season's answer is the year's and more: once it is here Year reads it,
  // and going between the two asks for nothing.
  const seasonAnswer = wantsSeason && !query.isPlaceholderData && query.data?.measured ? query.data : undefined;
  useEffect(() => {
    if (!seasonAnswer || !request) return;
    const { measured: _measured, ...year } = request;
    void _measured;
    client.setQueryData(['directorVisibleSky', year], (kept: DirectorVisibleSky | undefined) => kept ?? seasonAnswer);
  }, [seasonAnswer, request, client]);

  // The rig's own minimum time, from any answer about that rig: kept so
  // the field does not empty while another time frame loads.
  const rigMinimums = useRef(new Map<string, number>());
  if (query.data && !query.isError) rigMinimums.current.set(query.data.rig.id, query.data.rig_minimum_minutes);
  const rigMinimum = rig ? rigMinimums.current.get(rig.id) ?? null : null;
  // What is typed in the minimum field, which may not be a number yet.
  // Each value asked for measures the whole sky, so a value is sent once
  // the person stops, or at once on Enter or on leaving the field.
  const shownMinutes = minimum ?? rigMinimum;
  const [typed, setTyped] = useState<string>(shownMinutes === null ? '' : String(shownMinutes));
  const settle = useRef<number | null>(null);
  const cancelSettle = () => {
    if (settle.current !== null) window.clearTimeout(settle.current);
    settle.current = null;
  };
  useEffect(() => {
    cancelSettle();
    setTyped(shownMinutes === null ? '' : String(shownMinutes));
  }, [shownMinutes]);
  useEffect(() => cancelSettle, []);
  const latestShown = useRef(shownMinutes);
  latestShown.current = shownMinutes;
  const sendMinutes = (text: string) => {
    cancelSettle();
    const minutes = Number(text);
    if (text.trim() === '' || !Number.isInteger(minutes) || minutes < 1 || minutes > MAX_MINUTES) return;
    if (minutes !== latestShown.current) setMinimum(minutes);
  };
  const typeMinutes = (text: string) => {
    setTyped(text);
    cancelSettle();
    settle.current = window.setTimeout(() => sendMinutes(text), MINIMUM_SETTLE_MS);
  };

  // Season: the year's measured nights, and which 31 nights of the year
  // are shown. The slider's place is kept while the year is the same one,
  // and starts again with another rig or minimum time.
  const measured = useMemo(() => (wantsSeason && view?.measured ? unpackMeasured(view) : null), [wantsSeason, view]);
  // The year came and its nights cannot be read: Season has nothing to show.
  const seasonBroken = wantsSeason && view !== undefined && !query.isPlaceholderData && measured === null;
  const starts = useMemo(() => (measured ? seasonStarts(measured) : [0]), [measured]);
  const [place, setPlace] = useState(0);
  const yearOf = `${rigId ?? ''} ${minimum ?? ''}`;
  const placedFor = useRef(yearOf);
  if (placedFor.current !== yearOf) {
    placedFor.current = yearOf;
    if (place !== 0) setPlace(0);
  }
  const position = Math.min(place, starts.length - 1);
  const season = useMemo(() => (view && measured ? seasonSky(view, measured, starts[position], SEASON_NIGHTS) : null), [view, measured, starts, position]);
  // While the slider moves the cover is drawn small, and at full size
  // once it rests.
  const [sliding, setSliding] = useState(false);
  const slideRest = useRef<number | null>(null);
  useEffect(() => () => { if (slideRest.current !== null) window.clearTimeout(slideRest.current); }, []);
  const slideTo = (next: number) => {
    setPlace(Math.max(0, Math.min(starts.length - 1, next)));
    setSliding(true);
    if (slideRest.current !== null) window.clearTimeout(slideRest.current);
    slideRest.current = window.setTimeout(() => setSliding(false), SEASON_SETTLE_MS);
  };
  // The server's picture, drawn as it came: not while a time of year made
  // from it is drawn instead, nor when that could not be made.
  const picture: DirectorVisibleSky | undefined = season || seasonBroken ? undefined : view;

  const counts = useMemo(() => (season ? season.counts : picture ? unpackCounts(picture.counts, picture.nights_counted, picture.grid.columns, picture.grid.rows) : null), [season, picture]);
  const lines = useMemo(() => (season ? visibleSkyLines(season.view) : picture ? visibleSkyLines(picture) : null), [season, picture]);
  const layer = useMemo(() => (counts && lines ? { counts, lines } : null), [counts, lines]);

  // The target, and the middle of the view when it is somewhere else: the
  // server counts each place itself, for the nights of the picture drawn,
  // so the words never disagree with the lines. A time of year is counted
  // here, from the grid its lines are drawn from.
  const targetAt = useMemo(() => (target ? rounded(target) : null), [target?.ra_degrees, target?.dec_degrees]); // eslint-disable-line react-hooks/exhaustive-deps
  const viewAt = useMemo(() => (viewCenter ? rounded(viewCenter) : null), [viewCenter?.ra_degrees, viewCenter?.dec_degrees]); // eslint-disable-line react-hooks/exhaustive-deps
  const viewApart = viewAt !== null && (targetAt === null || apart(viewAt, targetAt) > VIEW_APART_DEGREES);
  const first = view?.nights[0]?.date;
  const asks = (at: DirectorSkyPosition | null): DirectorVisiblePlacesRequest | null =>
    !view || !first || !at || wantsSeason ? null : { rig: view.rig.id, span: view.span, minimum_minutes: view.minimum_minutes, first_night: first, places: [at] };
  // Asked apart, so the target's words stay while the view is moved.
  const targetAsk = asks(targetAt);
  const viewAsk = asks(viewApart ? viewAt : null);
  const placeQuery = (ask: DirectorVisiblePlacesRequest | null) => ({
    queryKey: ['directorVisibleSky', 'places', ask],
    queryFn: ({ signal }: { signal: AbortSignal }) => apiClient.getDirectorVisiblePlaces(ask!, signal),
    enabled: on && ask !== null,
    retry: retryWhenBusy,
    retryDelay: 1200,
    staleTime: 5 * 60_000,
    refetchInterval: false as const,
    refetchOnWindowFocus: false,
  });
  const targetQuery = useQuery(placeQuery(targetAsk));
  const viewQuery = useQuery(placeQuery(viewAsk));
  const counted = season?.view.nights_counted ?? picture?.nights_counted ?? 0;
  /** On how many nights a place is in view: null until it is known. */
  const inView = (at: DirectorSkyPosition | null, answer: typeof targetQuery.data): number | null => {
    if (!at) return null;
    if (season) return seasonNights(season, at.ra_degrees, at.dec_degrees);
    if (wantsSeason || !picture) return null;
    const found = answer?.places.find((entry) => entry.ra_degrees === at.ra_degrees && entry.dec_degrees === at.dec_degrees);
    return found && answer?.nights_counted === picture.nights_counted ? found.nights_in_view : null;
  };
  const year = !season && picture?.span === 'year';
  const targetNights = inView(targetAt, targetQuery.data);
  const viewNights = viewApart ? inView(viewAt, viewQuery.data) : null;
  const targetWords = targetNights === null ? null : placeWords('The target', targetNights, counted, year);
  const viewWords = viewNights === null ? null : placeWords('The middle of the view', viewNights, counted, year);
  const notes: Array<{ text: string; title: string }> = [];
  if (targetNights !== null && targetWords && targetNights < counted) notes.push({ text: targetNights === 0 ? 'Target never in view' : 'Target not in view every night', title: targetWords });
  if (viewNights !== null && viewWords && viewNights < counted) notes.push({ text: viewNights === 0 ? 'This sky is never in view' : 'This sky is not in view every night', title: viewWords });

  const failure = on && query.isError ? visibleSkyError(query.error) : null;
  const broken = "The year's nights could not be read, so Season has nothing to show. Reload the page, or choose Year.";
  const status = !rig ? 'This plan has no rig yet.'
    : !on ? 'Hidden'
    : failure ? failure.text
    : seasonBroken ? broken
    : !picture && !season ? 'Working out the sky in view…'
    : targetWords ?? (targetQuery.isError ? "The target's nights could not be counted." : 'Counting the nights the target is in view…');
  const legend = !rig ? null
    : season && view ? `${skyLegend(view, rig.name, `${SEASON_NIGHTS} nights at a time from tonight, ${formatNight(view.nights[0]?.date ?? null)}, to ${formatNight(view.nights[view.nights.length - 1]?.date ?? null)}, one a week measured`)}.`
    : picture ? `${awaited ? `${SPAN_LABEL[picture.span]} is shown while ${SPAN_LABEL[span]} loads. ` : ''}${visibleSkyLegend(picture, rig.name)}.`
    : null;
  const note = season ? seasonDarknessNote(season.view) : picture ? darknessNote(picture) : null;
  const shares = season ? `${seasonDates(season.view)}. ${visibleSkyShares(season.view)}${note ? ` ${note}` : ''}`
    : picture ? `${visibleSkyShares(picture)}${note ? ` ${note}` : ''}`
    : null;
  const months = useMemo(() => (view && measured ? monthMarks(view.nights[0]?.date, starts[starts.length - 1]) : []), [view, measured, starts]);

  return {
    rig,
    on,
    layer: on ? layer : null,
    changing: sliding,
    notes: on ? notes : [],
    panel: {
      busy: on && query.isFetching,
      status,
      failure,
      legend: on && !failure && !seasonBroken ? legend : null,
      shares: on && !failure && !seasonBroken ? shares : null,
      viewWords: on ? viewWords : null,
      span,
      setSpan,
      typed,
      typeMinutes,
      sendMinutes,
      rigMinimum: minimum !== null && rigMinimum !== null && minimum !== rigMinimum ? rigMinimum : null,
      useRigMinimum: () => setMinimum(null),
      season: wantsSeason && !seasonBroken ? { position, last: starts.length - 1, from: season ? seasonFrom(season.view) : null, dates: season ? seasonDates(season.view) : null, slideTo, months } : null,
      year,
    },
  };
}

