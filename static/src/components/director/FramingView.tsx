import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Crosshair, LocateFixed, RefreshCw, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorFramingDraftView, DirectorFramingPreview } from '../../api/directorTypes';
import {
  MAX_VIEW_FOV, MIN_VIEW_FOV, STAGE_HEIGHT, STAGE_WIDTH, clampFov, draftFromState, formatDec, formatDegrees, formatRaHours,
  moveBy, panelForRig, pixelScale, polygonPoints, previewRequest, stateFromDraft, stateFromSeed, toStage, type FramingSeed, type FramingState,
} from './framingModel';
import './FramingView.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Framing request failed';
/** Director admits one metadata request at a time and answers 503 with Retry-After while busy. */
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;
const DEFAULT_SURVEY = 'dss2_color';
const IMAGE_POLL_MS = 1000;
const IMAGE_POLL_LIMIT = 90;

function useDebounced<T>(value: T, delay: number): T {
  const [debounced, setDebounced] = useState(value);
  useEffect(() => { const timer = setTimeout(() => setDebounced(value), delay); return () => clearTimeout(timer); }, [value, delay]);
  return debounced;
}

/** The survey image behind the footprints. Keeps the last image up while the
 *  next one loads, polls a 202, and names a failure instead of hiding it. */
function useCutout(state: FramingState | null) {
  const request = useMemo(() => state ? {
    survey: state.surveyId, ra: Number(state.viewCenter.ra_degrees.toFixed(5)), dec: Number(state.viewCenter.dec_degrees.toFixed(5)),
    fov: Number(state.viewFov.toFixed(5)), width: STAGE_WIDTH, height: STAGE_HEIGHT, rotation: 0,
  } : null, [state]);
  const debounced = useDebounced(request, 400);
  const [image, setImage] = useState<{ url: string; key: string } | null>(null);
  const [status, setStatus] = useState<'idle' | 'loading' | 'ready' | 'failed'>('idle');
  const [error, setError] = useState('');
  useEffect(() => {
    if (!debounced) return;
    const key = JSON.stringify(debounced);
    let cancelled = false;
    let attempts = 0;
    setStatus('loading'); setError('');
    const poll = async () => {
      try {
        const result = await apiClient.fetchDirectorCutout(debounced);
        if (cancelled) return;
        if (result.state === 'ready') {
          const url = URL.createObjectURL(result.blob);
          setImage(previous => { if (previous) URL.revokeObjectURL(previous.url); return { url, key }; });
          setStatus('ready');
        } else if (result.state === 'generating') {
          if (++attempts >= IMAGE_POLL_LIMIT) { setStatus('failed'); setError('Survey image is taking too long; the last one stays up.'); return; }
          setTimeout(poll, IMAGE_POLL_MS);
        } else { setStatus('failed'); setError(result.error); }
      } catch (cause) { if (!cancelled) { setStatus('failed'); setError(message(cause)); } }
    };
    void poll();
    return () => { cancelled = true; };
  }, [debounced]);
  useEffect(() => () => { setImage(previous => { if (previous) URL.revokeObjectURL(previous.url); return null; }); }, []);
  return { image, status, error, stale: image !== null && image.key !== JSON.stringify(debounced) };
}

export interface FramingViewProps {
  projectId: string;
  seed: FramingSeed | null;
}

/** Frame one project on the sky: target, angle, mosaic and rig footprints over a survey. */
export default function FramingView({ projectId, seed }: FramingViewProps) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const draftKey = ['directorFraming', projectId];
  const draft = useQuery({ queryKey: draftKey, queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const surveys = useQuery({ queryKey: ['directorSurveys'], queryFn: apiClient.getDirectorSurveys, staleTime: Infinity, retry: retryWhenBusy, retryDelay: 700 });
  const rigs = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [state, setState] = useState<FramingState | null>(null);
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');
  useEffect(() => {
    if (!draft.data) return;
    if (draft.data.draft) setState(stateFromDraft(draft.data.draft));
    else if (seed) setState(stateFromSeed(seed, DEFAULT_SURVEY));
  }, [draft.data, seed]);
  const rigList = useMemo(() => rigs.data ?? [], [rigs.data]);
  const update = useCallback((patch: Partial<FramingState> | ((current: FramingState) => Partial<FramingState>)) => {
    setState(current => current ? { ...current, ...(typeof patch === 'function' ? patch(current) : patch) } : current);
  }, []);

  const request = useMemo(() => state ? previewRequest(state, rigList) : null, [state, rigList]);
  const debouncedRequest = useDebounced(request, 250);
  const preview = useQuery({
    queryKey: ['directorFramingPreview', debouncedRequest],
    queryFn: () => apiClient.previewDirectorFraming(debouncedRequest!),
    enabled: !!debouncedRequest, retry: false, placeholderData: previous => previous, staleTime: 60_000,
  });
  const cutout = useCutout(state);

  const save = useMutation({
    retry: false,
    mutationFn: () => {
      if (!state || !draft.data) throw new Error('Nothing to save');
      return apiClient.saveDirectorFramingDraft(draftFromState(state, projectId, draft.data.draft?.revision ?? 0));
    },
    onSuccess: saved => { setNotice(`Saved framing revision ${saved.draft?.revision ?? 0}.`); client.setQueryData<DirectorFramingDraftView>(draftKey, saved); },
  });
  const httpError = isAxiosError(save.error) ? save.error : save.error instanceof Error && isAxiosError(save.error.cause) ? save.error.cause : null;
  const stale = httpError?.response?.status === 409;

  // Drag pans the view; the wheel zooms about the center.
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<{ x: number; y: number; center: FramingState['viewCenter'] } | null>(null);
  const stageScale = () => (stage.current ? STAGE_WIDTH / stage.current.clientWidth : 1);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!state || event.button !== 0) return;
    drag.current = { x: event.clientX, y: event.clientY, center: state.viewCenter };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || !state) return;
    const scale = pixelScale(state.viewFov) * stageScale();
    const dx = (event.clientX - drag.current.x) * scale;
    const dy = (event.clientY - drag.current.y) * scale;
    update({ viewCenter: moveBy(drag.current.center, dx, dy) });
  };
  const onPointerUp = () => { drag.current = null; };
  const hasState = state !== null;
  useEffect(() => {
    const element = stage.current;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      update(current => ({ viewFov: clampFov(current.viewFov * (event.deltaY > 0 ? 1.2 : 1 / 1.2)) }));
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, [update, hasState]);

  const survey = surveys.data?.find(entry => entry.id === state?.surveyId);
  const panelRig = rigList.find(entry => entry.rig.id === state?.panelRigId);
  const chooseRig = (rigId: string) => update({ panelRigId: rigId || null, panel: rigId ? panelForRig(rigList, rigId) : null });
  const number = (value: string, fallback: number) => { const parsed = Number(value); return Number.isFinite(parsed) ? parsed : fallback; };

  if (draft.isPending) return <p role="status">Loading framing...</p>;
  if (draft.isError) return <p className="director-error" role="alert">{message(draft.error)}</p>;
  if (!state) return <p className="director-muted">Add a target to this project first; framing starts from its coordinates.</p>;
  const geometry: DirectorFramingPreview | undefined = preview.data;
  return <section className="framing" aria-label="Framing">
    <div className="framing-stage-wrap">
      <div ref={stage} className={`framing-stage${cutout.stale ? ' is-stale' : ''}`} role="img" aria-label="Sky view" data-testid="framing-stage"
        onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        {cutout.image ? <img src={cutout.image.url} alt="" draggable={false} /> : <div className="framing-stage-empty" />}
        <svg viewBox={`0 0 ${STAGE_WIDTH} ${STAGE_HEIGHT}`} aria-hidden="true">
          {geometry?.overlays.map(overlay => overlay.view_corners && <polygon key={overlay.id} className="framing-overlay" points={polygonPoints(overlay.view_corners, state.viewFov)} />)}
          {geometry?.panels.map(panel => panel.view_corners && <g key={panel.id} className="framing-panel">
            <polygon points={polygonPoints(panel.view_corners, state.viewFov)} />
            {geometry.panels.length > 1 && <text x={toStage(panel.view_corners[0], state.viewFov)[0] + 8} y={toStage(panel.view_corners[0], state.viewFov)[1] + 20}>{panel.id}</text>}
          </g>)}
          {geometry?.view_center_offset && (() => { const [x, y] = toStage(geometry.view_center_offset, state.viewFov); return <g className="framing-target"><line x1={x - 14} y1={y} x2={x + 14} y2={y} /><line x1={x} y1={y - 14} x2={x} y2={y + 14} /></g>; })()}
        </svg>
        <div className="framing-stage-status">
          {cutout.status === 'loading' && <span role="status">Loading {survey?.name ?? 'survey'}...</span>}
          {cutout.status === 'failed' && <span role="alert">{cutout.error}</span>}
          {preview.isError && <span role="alert">{message(preview.error)}</span>}
        </div>
        <div className="framing-stage-scale">{formatDegrees(state.viewFov)} across · N up, E left</div>
      </div>
      <p className="director-muted framing-attribution">{survey ? `${survey.name}: ${survey.bandpass}. ${survey.attribution}.` : 'Choose a survey.'} Imagery is a composition aid, not pointing evidence.</p>
    </div>
    <form className="framing-controls" onSubmit={event => { event.preventDefault(); if (canWrite && !save.isPending && !stale) { setNotice(''); setProblem(''); save.mutate(); } }}>
      <fieldset>
        <legend>Target</legend>
        <label>Name<input aria-label="Target name" value={state.targetName} maxLength={256} onChange={event => update({ targetName: event.target.value })} /></label>
        <div className="framing-grid">
          <label>RA<span className="framing-input"><input aria-label="Right ascension degrees" type="number" step="any" min={0} max={359.99999} value={state.center.ra_degrees} onChange={event => update(current => ({ center: { ...current.center, ra_degrees: number(event.target.value, current.center.ra_degrees) } }))} /><small>°</small></span><small>{formatRaHours(state.center.ra_degrees)}</small></label>
          <label>Dec<span className="framing-input"><input aria-label="Declination degrees" type="number" step="any" min={-90} max={90} value={state.center.dec_degrees} onChange={event => update(current => ({ center: { ...current.center, dec_degrees: number(event.target.value, current.center.dec_degrees) } }))} /><small>°</small></span><small>{formatDec(state.center.dec_degrees)}</small></label>
          <label>Camera angle<span className="framing-input"><input aria-label="Position angle degrees" type="number" step="any" min={0} max={359.99} value={state.positionAngle} onChange={event => update({ positionAngle: ((number(event.target.value, state.positionAngle) % 360) + 360) % 360 })} /><small>° E of N</small></span></label>
        </div>
        <div className="director-actions">
          <button type="button" onClick={() => update(current => ({ viewCenter: current.center }))}><Crosshair size={16} />Center view on target</button>
          <button type="button" disabled={!canWrite} onClick={() => update(current => ({ center: current.viewCenter }))}><LocateFixed size={16} />Move target to view center</button>
          {seed && <button type="button" disabled={!canWrite} onClick={() => update({ targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees, viewCenter: seed.center })}><Undo2 size={16} />Back to catalog target</button>}
        </div>
      </fieldset>
      <fieldset>
        <legend>Panels</legend>
        <label>Panel rig
          <select aria-label="Panel rig" value={state.panelRigId ?? ''} onChange={event => chooseRig(event.target.value)}>
            <option value="">Enter a size by hand</option>
            {state.panelRigId && !panelRig && <option value={state.panelRigId} disabled>{rigs.isPending ? 'Loading rig...' : rigs.isError ? 'Saved rig (list unavailable)' : 'Saved rig is no longer listed'}</option>}
            {rigList.map(entry => <option key={entry.rig.id} value={entry.rig.id} disabled={!entry.field_of_view}>{entry.catalog_name}{entry.field_of_view ? ` (${formatDegrees(entry.field_of_view.width_degrees)} × ${formatDegrees(entry.field_of_view.height_degrees)})` : ' (no optics yet)'}</option>)}
          </select>
        </label>
        {!panelRig && <div className="framing-grid">
          <label>Panel width<span className="framing-input"><input aria-label="Panel width degrees" type="number" step="any" min={0.01} max={30} value={state.panel?.width_degrees ?? ''} onChange={event => update(current => ({ panel: { width_degrees: number(event.target.value, 0), height_degrees: current.panel?.height_degrees ?? number(event.target.value, 0) * 0.67 } }))} /><small>°</small></span></label>
          <label>Panel height<span className="framing-input"><input aria-label="Panel height degrees" type="number" step="any" min={0.01} max={30} value={state.panel?.height_degrees ?? ''} onChange={event => update(current => ({ panel: { width_degrees: current.panel?.width_degrees ?? number(event.target.value, 0) * 1.5, height_degrees: number(event.target.value, 0) } }))} /><small>°</small></span></label>
        </div>}
        <div className="framing-grid">
          <label>Rows<input aria-label="Mosaic rows" type="number" min={1} max={16} step={1} value={state.mosaic.rows} onChange={event => update(current => ({ mosaic: { ...current.mosaic, rows: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } }))} /></label>
          <label>Columns<input aria-label="Mosaic columns" type="number" min={1} max={16} step={1} value={state.mosaic.columns} onChange={event => update(current => ({ mosaic: { ...current.mosaic, columns: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } }))} /></label>
          <label>Overlap<span className="framing-input"><input aria-label="Panel overlap percent" type="number" min={0} max={90} step={1} value={state.mosaic.overlap_percent} onChange={event => update(current => ({ mosaic: { ...current.mosaic, overlap_percent: Math.max(0, Math.min(90, Math.round(number(event.target.value, 0)))) } }))} /><small>%</small></span></label>
        </div>
        <p className="director-muted" data-testid="framing-extent">{geometry ? `${geometry.panels.length} panel${geometry.panels.length === 1 ? '' : 's'}, ${formatDegrees(geometry.extent.width_degrees)} × ${formatDegrees(geometry.extent.height_degrees)} in all.` : state.panel ? 'Computing footprint...' : 'Choose a rig or enter a panel size to see the footprint.'}</p>
      </fieldset>
      <fieldset>
        <legend>Compare rigs</legend>
        {rigs.isError && <p className="director-error" role="alert">Rigs could not be loaded: {message(rigs.error)} <button type="button" onClick={() => void rigs.refetch()}>Retry</button></p>}
        {!rigs.isError && rigList.length === 0 && <p className="director-muted">{rigs.isPending ? 'Loading rigs...' : 'No rig has planning enabled yet.'}</p>}
        {rigList.map(entry => <label key={entry.rig.id} className="framing-check">
          <input type="checkbox" disabled={!entry.field_of_view} checked={state.shownRigIds.includes(entry.rig.id)} onChange={event => update(current => ({ shownRigIds: event.target.checked ? [...current.shownRigIds, entry.rig.id] : current.shownRigIds.filter(id => id !== entry.rig.id) }))} />
          {entry.catalog_name}{entry.field_of_view ? <small> {formatDegrees(entry.field_of_view.width_degrees)} × {formatDegrees(entry.field_of_view.height_degrees)}, {entry.field_of_view.pixel_scale_arcsec.toFixed(2)}″/px</small> : <small> no optics in its rig profile</small>}
        </label>)}
      </fieldset>
      <fieldset>
        <legend>View</legend>
        <label>Survey
          <select aria-label="Survey" value={state.surveyId} onChange={event => update({ surveyId: event.target.value })}>
            {(surveys.data ?? []).map(entry => <option key={entry.id} value={entry.id}>{entry.name}{entry.kind === 'narrowband' ? ' (narrowband)' : ''}</option>)}
          </select>
        </label>
        <label>Width of view<span className="framing-input"><input aria-label="View width degrees" type="number" step="any" min={MIN_VIEW_FOV} max={MAX_VIEW_FOV} value={Number(state.viewFov.toFixed(3))} onChange={event => update({ viewFov: clampFov(number(event.target.value, state.viewFov)) })} /><small>°</small></span></label>
        <p className="director-muted">Drag the image to pan, scroll to zoom. The view center is {formatRaHours(state.viewCenter.ra_degrees)}, {formatDec(state.viewCenter.dec_degrees)}.</p>
      </fieldset>
      {notice && <p role="status">{notice}</p>}
      {stale && <p className="director-error" role="alert">This framing changed since you loaded it. Reload to see the saved draft before editing again.</p>}
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      <div className="director-actions">
        {canWrite && <button type="submit" disabled={save.isPending || stale}><Check size={16} />{save.isPending ? 'Saving...' : 'Save framing'}</button>}
        <button type="button" aria-label="Reload framing" title="Reload framing" onClick={() => { setNotice(''); setProblem(''); save.reset(); void draft.refetch(); }}><RefreshCw size={16} /></button>
        {!canWrite && <span className="director-muted">Read only</span>}
      </div>
    </form>
  </section>;
}
