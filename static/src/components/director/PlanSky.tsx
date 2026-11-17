import { useEffect, useId, useMemo, useRef, useState } from 'react';
import { keepPreviousData, useMutationState, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorSkyPosition, DirectorVisiblePlacesRequest, DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { openSettings } from '../../utils/settingsIntent';
import { formatDecShort, formatRaShort, type SkyFrame, type SkyMode } from '../../utils/skyProjection';
import SkyMap, { type SkyMark } from '../sky/SkyMap';
import { SkyChoice, SkyShapeChoices } from '../sky/SkyChoices';
import { formatNight } from '../sky/skyModel';
import { SEASON_NIGHTS, seasonNights, seasonSky, seasonStarts, unpackMeasured } from '../sky/skySeason';
import { unpackCounts } from '../sky/skyShade';
import {
  darknessNote,
  nightCount,
  standing,
  visibleSkyError,
  visibleSkyLegend,
  visibleSkyLines,
  visibleSkyShares,
  visibleSkySummary,
  type SkyPicture,
} from '../sky/visibleSky';
import { retryWhenBusy } from './retry';
import { SKY_INPUTS_MUTATION } from './skyInputs';
import TargetSearch, { type TargetPick } from './TargetSearch';
import './PlanSky.css';

export interface PlanSkyRig {
  id: string;
  name: string;
  /** The rig's database, for the link to its Rig setup. */
  slug?: string | null;
}

interface Props {
  /** The plan's rigs; the first is shown until another is chosen. */
  rigs: PlanSkyRig[];
  /** The plan's target as Framing has it now, saved or not. */
  target: { name: string; center: DirectorSkyPosition } | null;
  /** Why a place cannot be sent to Framing, when it cannot. */
  frameBlocked: string | null;
  /** Send a place to Framing as the target. */
  onFrame: (pick: TargetPick) => void;
  /** Open the Rigs tab, where a plan gets its rigs. */
  onRigs: () => void;
  /** Whether the tab has been opened: nothing is asked for before. */
  shown: boolean;
}

/** Now and Year are the server's two time frames. Season shows the
 *  server's year a month at a time, made here. */
type Span = 'now' | 'year' | 'season';
/** How long after the last slider step the cover is drawn at full size. */
export const SEASON_SETTLE_MS = 180;
/** Page Up and Page Down move the slider this many weeks: about a month. */
const SEASON_PAGE = 4;
const DAY_MS = 86_400_000;
const MAX_MINUTES = 24 * 60;
/** How long the minimum field waits after the last keystroke or spinner
 *  step before it asks for a new picture. */
export const MINIMUM_SETTLE_MS = 500;
const SPANS: ReadonlyArray<readonly [Span, string]> = [['now', 'Now'], ['year', 'Year'], ['season', 'Season']];
const SPAN_LABEL: Record<Span, string> = { now: 'Now', year: 'Year', season: 'Season' };
/** A place to a millionth of a degree: enough to tell two places apart,
 *  and steady from one render to the next. */
const rounded = (center: DirectorSkyPosition): DirectorSkyPosition => ({ ra_degrees: Math.round(center.ra_degrees * 1e6) / 1e6 % 360, dec_degrees: Math.round(center.dec_degrees * 1e6) / 1e6 });

/** A slider position in words: the nights it shows. */
const seasonWords = (view: SkyPicture) => `${view.nights.length} nights from ${formatNight(view.nights[0]?.date ?? null)}`;

/** Where each month begins along the slider, as a share of its length:
 *  the slider runs from the year's first night to `lastStart` nights on. */
function monthMarks(first: string | undefined, lastStart: number): Array<{ label: string; share: number }> {
  if (!first || lastStart <= 0) return [];
  const start = Date.parse(`${first}T00:00:00Z`);
  const marks: Array<{ label: string; share: number }> = [];
  const month = new Date(start);
  month.setUTCDate(1);
  for (let i = 0; i < 14; i += 1) {
    month.setUTCMonth(month.getUTCMonth() + 1);
    const nights = Math.round((month.getTime() - start) / DAY_MS);
    if (nights > lastStart) break;
    marks.push({ label: month.toLocaleDateString(undefined, { month: 'short', timeZone: 'UTC' }), share: nights / lastStart });
  }
  return marks;
}

const placeName = (center: DirectorSkyPosition) => `${formatRaShort(center.ra_degrees)}, ${formatDecShort(center.dec_degrees)}`;

/**
 * The sky a plan's rig can shoot: sky it cannot shoot is covered, more
 * heavily the fewer nights it is in view for the minimum time, with a solid line round the sky in view
 * every night and a dashed one round the sky in view on any. Now counts
 * tonight and the 30 nights after; Year counts every night of a year,
 * of which the server measures one a week and works out the rest; Season
 * shows 31 nights of that year at a time, chosen with a slider and added
 * up here from the year's answer, so moving the slider asks for nothing.
 * A place found by name or clicked on the map can be sent to Framing.
 */
export default function PlanSky({ rigs, target, frameBlocked, onFrame, onRigs, shown }: Props) {
  const now = useDirectorClock();
  const [chosenRig, setChosenRig] = useState<string | null>(null);
  const rig = rigs.find((entry) => entry.id === chosenRig) ?? rigs[0] ?? null;
  const [span, setSpan] = useState<Span>('now');
  const [mode, setMode] = useState<SkyMode>('aitoff');
  const [frame, setFrame] = useState<SkyFrame>('equatorial');
  // Minutes typed here; null uses the rig's minimum time.
  const [minimum, setMinimum] = useState<number | null>(null);
  const [picked, setPicked] = useState<TargetPick | null>(null);

  // Season is made from the year's answer: the two ask for the same thing.
  const asked = span === 'season' ? 'year' : span;
  const request: DirectorVisibleSkyRequest | null = useMemo(
    () => (rig ? { rig: rig.id, span: asked, ...(minimum !== null ? { minimum_minutes: minimum } : {}) } : null),
    [rig, asked, minimum]
  );
  // The picture for fixed inputs does not change, so it is not fetched on
  // the app's timer or on focus. A superseded request is cancelled, and
  // the server stops measuring for it.
  const query = useQuery({
    queryKey: ['directorVisibleSky', request],
    queryFn: ({ signal }) => apiClient.getDirectorVisibleSky(request!, signal),
    enabled: shown && request !== null,
    retry: retryWhenBusy,
    retryDelay: 1200,
    staleTime: 5 * 60_000,
    refetchInterval: false,
    refetchOnWindowFocus: false,
    placeholderData: keepPreviousData,
  });
  // The picture on screen: the one asked for, or, while that loads, the
  // last one for this rig, dimmed. Another rig's picture is never shown.
  const view: DirectorVisibleSky | undefined = request && !query.isError && query.data?.rig.id === request.rig ? query.data : undefined;
  const awaited = view !== undefined && view.span !== asked;
  // The nights start again at the next local noon at the rig's site. A
  // tab behind another asks for nothing; it catches up when it is shown.
  const longitude = query.data?.site.longitude_degrees;
  const nightIsOver = shown && request !== null && longitude !== undefined && !query.isPlaceholderData && nightCount(now, longitude) !== nightCount(query.dataUpdatedAt, longitude);
  const refetch = query.refetch;
  useEffect(() => {
    if (nightIsOver) void refetch();
  }, [nightIsOver, refetch]);
  // A save that can move the rig's site, horizon, limits or minimum time
  // makes the picture out of date; no other save does. A shown tab reads
  // it again at once, a hidden one when it is next shown.
  const client = useQueryClient();
  const lastSave = Math.max(0, ...useMutationState({ filters: { mutationKey: SKY_INPUTS_MUTATION, status: 'success' }, select: (mutation) => mutation.state.submittedAt }));
  const seenSave = useRef(lastSave);
  useEffect(() => {
    if (lastSave <= seenSave.current) return;
    seenSave.current = lastSave;
    void client.invalidateQueries({ queryKey: ['directorVisibleSky'] });
  }, [lastSave, client]);

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
  const measured = useMemo(() => (view?.span === 'year' ? unpackMeasured(view) : null), [view]);
  const starts = useMemo(() => (measured ? seasonStarts(measured) : [0]), [measured]);
  const [place, setPlace] = useState(0);
  const yearOf = `${rig?.id ?? ''} ${minimum ?? ''}`;
  const placedFor = useRef(yearOf);
  if (placedFor.current !== yearOf) {
    placedFor.current = yearOf;
    if (place !== 0) setPlace(0);
  }
  const position = Math.min(place, starts.length - 1);
  const season = useMemo(() => (span === 'season' && view && measured ? seasonSky(view, measured, starts[position], SEASON_NIGHTS) : null), [span, view, measured, starts, position]);
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
  const seasonId = useId();
  // The picture drawn: the server's, or the time of year made from it.
  const picture: SkyPicture | undefined = season ? season.view : span === 'season' && view?.span === 'year' ? undefined : view;
  const fromTonight = !season || position === 0;

  const counts = useMemo(() => (season ? season.counts : picture ? unpackCounts(picture.counts, picture.nights_counted, picture.grid.columns, picture.grid.rows) : null), [season, picture]);
  const lines = useMemo(() => (picture ? visibleSkyLines(picture) : null), [picture]);
  const marks: SkyMark[] = useMemo(() => {
    const list: SkyMark[] = [];
    if (target) list.push({ id: 'target', ra: target.center.ra_degrees, dec: target.center.dec_degrees, label: target.name || 'Target', kind: 'target' });
    if (picked) list.push({ id: 'pick', ra: picked.center.ra_degrees, dec: picked.center.dec_degrees, label: picked.name, kind: 'pick' });
    return list;
  }, [target, picked]);
  // The cards ask the server about each place itself, for the nights of
  // the picture on screen, so a card never disagrees with the lines: a
  // grid node two degrees off may be on the other side of one.
  const placesRequest: DirectorVisiblePlacesRequest | null = useMemo(() => {
    const places = [target?.center, picked?.center].filter((place): place is DirectorSkyPosition => !!place).map(rounded);
    const first = view?.nights[0]?.date;
    // A time of year is counted here, from the grid the map is drawn from.
    if (!view || !first || places.length === 0 || span === 'season') return null;
    return { rig: view.rig.id, span: view.span, minimum_minutes: view.minimum_minutes, first_night: first, places };
  }, [view, target, picked, span]);
  const placesQuery = useQuery({
    queryKey: ['directorVisibleSky', 'places', placesRequest],
    queryFn: ({ signal }) => apiClient.getDirectorVisiblePlaces(placesRequest!, signal),
    enabled: shown && placesRequest !== null,
    retry: retryWhenBusy,
    retryDelay: 1200,
    staleTime: 5 * 60_000,
    refetchInterval: false,
    refetchOnWindowFocus: false,
  });
  /** On how many nights a place is in view: null until the server says. */
  const inView = (center: DirectorSkyPosition): number | null => {
    // In Season the count is read off the grid between its nodes, as the
    // lines are drawn between them: the card and the map are one thing.
    if (season) return seasonNights(season, center.ra_degrees, center.dec_degrees);
    if (span === 'season') return null;
    const at = rounded(center);
    const found = placesQuery.data?.places.find((place) => place.ra_degrees === at.ra_degrees && place.dec_degrees === at.dec_degrees);
    return found && placesQuery.data?.nights_counted === view?.nights_counted ? found.nights_in_view : null;
  };
  const counted = picture?.nights_counted ?? 0;
  const nightsWords = (nights: number) => (picture?.span === 'year' ? `In view on ${nights} of the ${counted} nights of the year` : `In view on ${nights} of the ${counted} nights counted`);
  const standingWords = (nights: number) => {
    const where = standing(nights, counted);
    return where === 'every' ? 'Inside the every-night line.' : where === 'some' ? 'Between the lines: in view on some nights.' : 'Outside both lines: never in view for long enough.';
  };
  const placeWords = (center: DirectorSkyPosition) => {
    if (!picture) return null;
    const nights = inView(center);
    if (nights === null) return placesQuery.isError ? 'Its nights in view could not be counted.' : 'Counting its nights in view…';
    return `${nightsWords(nights)}. ${standingWords(nights)}`;
  };

  const failure = query.isError ? visibleSkyError(query.error) : null;
  const changed = minimum !== null && rigMinimum !== null && minimum !== rigMinimum;

  // Until the tab is opened there is nothing to draw: the map is a heavy
  // thing to lay out behind another tab.
  if (!shown) return <section className="plan-sky" aria-label="Sky in view" />;
  if (rigs.length === 0) {
    return (
      <section className="plan-sky" aria-label="Sky in view">
        <p className="director-muted" role="status">
          This plan has no rig yet.{' '}
          <button type="button" className="link-button" onClick={onRigs}>
            Add a rig on the Rigs tab
          </button>
        </p>
      </section>
    );
  }

  return (
    <section className="plan-sky" aria-label="Sky in view" aria-busy={query.isFetching}>
      <div className="plan-sky-controls">
        <label>
          Rig
          <select aria-label="Rig" value={rig?.id ?? ''} onChange={(event) => { setChosenRig(event.target.value); setMinimum(null); }}>
            {rigs.map((entry) => (
              <option key={entry.id} value={entry.id}>
                {entry.name}
              </option>
            ))}
          </select>
        </label>
        <label className="plan-sky-minimum">
          In view for
          <input
            type="number"
            aria-label="Minimum minutes"
            min={1}
            max={MAX_MINUTES}
            step={5}
            value={typed}
            onChange={(event) => typeMinutes(event.target.value)}
            onBlur={(event) => sendMinutes(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === 'Enter') sendMinutes(event.currentTarget.value);
            }}
          />
          min or more
        </label>
        {changed && (
          <button type="button" className="link-button" onClick={() => setMinimum(null)}>
            Use rig's {rigMinimum} min
          </button>
        )}
        <SkyChoice label="Time frame" options={SPANS} value={span} onChange={setSpan} />
        <SkyShapeChoices frame={frame} mode={mode} onFrame={setFrame} onMode={setMode} shapeFirst />
      </div>

      <p className="plan-sky-legend" role="status">
        {failure ? (
          <>
            {failure.text}{' '}
            {failure.needsSite && (
              <button type="button" className="link-button" onClick={() => openSettings('rigs', { databaseId: rig?.slug ?? undefined })}>
                Open Rig setup
              </button>
            )}
          </>
        ) : picture && rig ? (
          <>
            {awaited ? `${SPAN_LABEL[picture.span]} is shown while ${SPAN_LABEL[span]} loads. ` : ''}
            {visibleSkyLegend(picture, rig.name, fromTonight)}. {visibleSkyShares(picture)}
            {darknessNote(picture) ? ` ${darknessNote(picture)}` : ''}
          </>
        ) : (
          'Working out the sky in view…'
        )}
      </p>

      <div className="plan-sky-body">
        <div className="plan-sky-map">
          <SkyMap
            frame={frame}
            mode={mode}
            showBackdrop
            showStacks={false}
            visible={lines}
            shade={counts}
            shadeChanging={sliding}
            marks={marks}
            onPick={(ra, dec) => setPicked({ name: placeName({ ra_degrees: ra, dec_degrees: dec }), center: { ra_degrees: ra, dec_degrees: dec }, source: 'the map' })}
            label={picture && rig ? visibleSkySummary(picture, rig.name, fromTonight) : 'Map of the sky'}
          />
          {span === 'season' && (
            <div className="plan-sky-season">
              <label htmlFor={seasonId}>Time of year</label>
              <span className="plan-sky-season-nights">{season ? seasonWords(season.view) : 'Working out the year…'}</span>
              <input
                id={seasonId}
                type="range"
                min={0}
                max={starts.length - 1}
                step={1}
                value={position}
                disabled={!season}
                aria-valuetext={season ? seasonWords(season.view) : undefined}
                onChange={(event) => slideTo(Number(event.target.value))}
                onKeyDown={(event) => {
                  // Arrow keys step a week, and Home and End go to the
                  // ends, as a slider does; a page is about a month.
                  if (event.key !== 'PageUp' && event.key !== 'PageDown') return;
                  event.preventDefault();
                  slideTo(position + (event.key === 'PageUp' ? SEASON_PAGE : -SEASON_PAGE));
                }}
              />
              {view && measured && (
                <div className="plan-sky-season-months" aria-hidden="true">
                  {monthMarks(view.nights[0]?.date, starts[starts.length - 1]).map((mark) => (
                    <span key={mark.label + mark.share} style={{ left: `${mark.share * 100}%` }}>
                      {mark.label}
                    </span>
                  ))}
                </div>
              )}
            </div>
          )}
          <div className="plan-sky-key" aria-label="How to read the map">
            <span className="plan-sky-scale-end">No night</span>
            <span className="plan-sky-scale" aria-hidden="true" />
            <span className="plan-sky-scale-end">Every night</span>
            <span className="plan-sky-key-line">
              <span className="plan-sky-key-solid" aria-hidden="true" />
              Edge of the sky in view every night
            </span>
            <span className="plan-sky-key-line">
              <span className="plan-sky-key-dashed" aria-hidden="true" />
              Edge of the sky in view on any night
            </span>
            <span className="plan-sky-key-line">Marks point into the sky in view.</span>
            <span className="plan-sky-key-line">Covered sky is out of view on some nights. Hatched sky is never in view.</span>
            {span === 'season' && (
              <span className="plan-sky-key-line">Season shows 31 nights of the year at a time. Its lines follow the 2° grid the year is measured on, so they are rougher than the lines in Now and Year.</span>
            )}
            {picture?.span === 'year' && (
              <span className="plan-sky-key-line">Year counts every night. One night a week is measured and the nights between are worked out from the measured nights. The solid line is within about 2° of a line measured on every night.</span>
            )}
          </div>
        </div>

        <aside className="plan-sky-side" aria-label="Places">
          <TargetSearch label="Find an object" ariaLabel="Find an object" placeholder="M 31, NGC 7000, Heart Nebula" buttonLabel="Show" hint="Or click a place on the map." onPick={setPicked} />
          {target && (
            <div className="plan-sky-place" data-testid="plan-sky-target">
              <strong>Target: {target.name || 'Target'}</strong>
              <span>{placeName(target.center)}</span>
              {placeWords(target.center) && <span>{placeWords(target.center)}</span>}
            </div>
          )}
          {picked && (
            <div className="plan-sky-place plan-sky-picked" role="group" aria-label="Picked place">
              <strong>{picked.name}</strong>
              {picked.source !== 'the map' && <span>{placeName(picked.center)}</span>}
              {placeWords(picked.center) && <span>{placeWords(picked.center)}</span>}
              <span className="plan-sky-actions">
                <button type="button" disabled={frameBlocked !== null} title={frameBlocked ?? undefined} onClick={() => onFrame(picked)}>
                  Frame this
                </button>
                <button type="button" className="link-button" onClick={() => setPicked(null)}>
                  Clear
                </button>
              </span>
              {frameBlocked && <small>{frameBlocked}</small>}
              {!frameBlocked && target && <small>Framing moves the target here. Nothing is saved until you save.</small>}
            </div>
          )}
        </aside>
      </div>
    </section>
  );
}
