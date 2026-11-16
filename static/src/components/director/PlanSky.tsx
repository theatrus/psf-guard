import { useEffect, useMemo, useRef, useState } from 'react';
import { keepPreviousData, useMutationState, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { DirectorSkyPosition, DirectorVisiblePlacesRequest, DirectorVisibleSky, DirectorVisibleSkyRequest } from '../../api/directorTypes';
import { useDirectorClock } from '../../hooks/useDirectorClock';
import { openSettings } from '../../utils/settingsIntent';
import { formatDecShort, formatRaShort, type SkyFrame, type SkyMode } from '../../utils/skyProjection';
import SkyMap, { type SkyMark } from '../sky/SkyMap';
import { SkyChoice, SkyShapeChoices } from '../sky/SkyChoices';
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

type Span = 'now' | 'year';
const MAX_MINUTES = 24 * 60;
/** How long the minimum field waits after the last keystroke or spinner
 *  step before it asks for a new picture. */
export const MINIMUM_SETTLE_MS = 500;
const SPANS: ReadonlyArray<readonly [Span, string]> = [['now', 'Now'], ['year', 'Year']];
const SPAN_LABEL: Record<Span, string> = { now: 'Now', year: 'Year' };
/** A place to a millionth of a degree: enough to tell two places apart,
 *  and steady from one render to the next. */
const rounded = (center: DirectorSkyPosition): DirectorSkyPosition => ({ ra_degrees: Math.round(center.ra_degrees * 1e6) / 1e6 % 360, dec_degrees: Math.round(center.dec_degrees * 1e6) / 1e6 });

const placeName = (center: DirectorSkyPosition) => `${formatRaShort(center.ra_degrees)}, ${formatDecShort(center.dec_degrees)}`;

/**
 * The sky a plan's rig can shoot: shaded by how many nights each part is
 * in view for the minimum time, with a solid line round the sky in view
 * every night and a dashed one round the sky in view on any. Now counts
 * tonight and the 30 nights after; Year counts a night a week for a year.
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

  const request: DirectorVisibleSkyRequest | null = useMemo(
    () => (rig ? { rig: rig.id, span, ...(minimum !== null ? { minimum_minutes: minimum } : {}) } : null),
    [rig, span, minimum]
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
  const awaited = view !== undefined && view.span !== span;
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

  const counts = useMemo(() => (view ? unpackCounts(view.counts, view.nights.length, view.grid.columns, view.grid.rows) : null), [view]);
  const lines = useMemo(() => (view ? visibleSkyLines(view) : null), [view]);
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
    if (!view || !first || places.length === 0) return null;
    return { rig: view.rig.id, span: view.span, minimum_minutes: view.minimum_minutes, first_night: first, places };
  }, [view, target, picked]);
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
    const at = rounded(center);
    const found = placesQuery.data?.places.find((place) => place.ra_degrees === at.ra_degrees && place.dec_degrees === at.dec_degrees);
    return found && placesQuery.data?.nights_counted === view?.nights.length ? found.nights_in_view : null;
  };
  const counted = view?.nights.length ?? 0;
  const nightsWords = (nights: number) => (view?.span === 'year' ? `In view on ${nights} of the ${counted} nights counted over the year` : `In view on ${nights} of the ${counted} nights counted`);
  const standingWords = (nights: number) => {
    const where = standing(nights, counted);
    return where === 'every' ? 'Inside the every-night line.' : where === 'some' ? 'Between the lines: in view on some nights.' : 'Outside both lines: never in view for long enough.';
  };
  const placeWords = (center: DirectorSkyPosition) => {
    if (!view) return null;
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
        ) : view && rig ? (
          <>
            {awaited ? `${SPAN_LABEL[view.span]} is shown while ${SPAN_LABEL[span]} loads. ` : ''}
            {visibleSkyLegend(view, rig.name)}. {visibleSkyShares(view)}
            {darknessNote(view) ? ` ${darknessNote(view)}` : ''}
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
            marks={marks}
            onPick={(ra, dec) => setPicked({ name: placeName({ ra_degrees: ra, dec_degrees: dec }), center: { ra_degrees: ra, dec_degrees: dec }, source: 'the map' })}
            label={view && rig ? visibleSkySummary(view, rig.name) : 'Map of the sky'}
          />
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
            {view?.span === 'year' && (
              <span className="plan-sky-key-line">Year counts one night a week. Sky just inside the solid line may be out of view on the nights between, so read that line as rough to a few degrees.</span>
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
