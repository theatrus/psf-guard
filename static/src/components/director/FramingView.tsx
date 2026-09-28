import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Crosshair, LocateFixed, RefreshCw, Search, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorFramingDraftView, DirectorFramingPreview, DirectorMosaicPanel, DirectorRigProfileSummary } from '../../api/directorTypes';
import {
  MAX_VIEW_FOV, MIN_VIEW_FOV, STAGE_HEIGHT, STAGE_WIDTH, angleFromStage, clampFov, draftFromState, formatDec, formatDegrees, formatRaHours, handleOffset,
  insidePolygon, moveBy, panelForRig, pixelScale, polygonPoints, previewRequest, stackMatrix, stateFromDraft, stateFromSeed, toStage, type FramingSeed, type FramingState, chipSurveys, framingGeometry,
} from './framingModel';
import VisibilityPanel from './VisibilityPanel';
import { useSurveyCutout } from './useSurveyCutout';
import './FramingView.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Framing request failed';
/** Director admits one metadata request at a time and answers 503 with Retry-After while busy. */
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;
const DEFAULT_SURVEY = 'dss2_color';
/** The rotation handle: past the top edge of the whole mosaic, along its up direction. */
function rotationHandle(geometry: DirectorFramingPreview, state: FramingState) {
  const [cx, cy] = geometry.view_center_offset ?? [0, 0];
  const [hx, hy] = handleOffset(state.positionAngle, geometry.extent.height_degrees / 2, Math.max(0.02, state.viewFov * 0.03));
  return [cx + hx, cy + hy] as [number, number];
}

/** Where to point when the project has no catalog target yet: a name the
 *  catalogs know, or coordinates typed in. */
/** One line per activated panel: whose it is, how far along, and whether its stack can be placed. */
function describeStack(panel: DirectorMosaicPanel): string {
  const who = `${panel.panel_id}, ${panel.catalog_name || panel.rig.name}`;
  const done = panel.progress ? `${panel.progress.accepted}/${panel.progress.desired} frames accepted` : 'no exposure plans';
  switch (panel.status) {
    case 'ready': return `${who}: ${done}; ${panel.preview?.kind === 'color' ? 'colour' : panel.preview?.filter ?? 'mono'} stack placed by its solve.`;
    case 'unsolved': return `${who}: ${done}; the stack has no plate solve yet, so it cannot be placed.`;
    case 'no_stack': return `${who}: ${done}; no stack yet.`;
    case 'missing_target': return `${who}: its target row is gone from the database.`;
    default: return `${who}: its database is no longer registered here.`;
  }
}

function StartFraming({ canWrite, onStart }: { canWrite: boolean; onStart: (seed: FramingSeed) => void }) {
  const [name, setName] = useState('');
  const [ra, setRa] = useState('');
  const [dec, setDec] = useState('');
  const [problem, setProblem] = useState('');
  const resolve = useMutation({
    retry: false,
    mutationFn: (query: string) => apiClient.resolveDirectorName(query),
    onSuccess: hit => onStart({ name: hit.name, center: { ra_degrees: hit.ra_degrees, dec_degrees: hit.dec_degrees }, position_angle_degrees: 0 }),
  });
  const byCoordinates = () => {
    const [r, d] = [Number(ra), Number(dec)];
    if (!Number.isFinite(r) || !Number.isFinite(d) || r < 0 || r >= 360 || d < -90 || d > 90) { setProblem('Enter RA in degrees from 0 to 360 and Dec from -90 to 90.'); return; }
    setProblem('');
    onStart({ name: name.trim() || 'Target', center: { ra_degrees: r, dec_degrees: d }, position_angle_degrees: 0 });
  };
  if (!canWrite) return <p className="director-muted">This project has no framing yet.</p>;
  return <form className="framing-start" aria-label="Start framing" onSubmit={event => { event.preventDefault(); if (name.trim()) resolve.mutate(name.trim()); }}>
    <p className="director-muted">No linked catalog target yet. Start from a name the catalogs know, or type the center.</p>
    <div className="framing-grid">
      <label>Object name<input aria-label="Object name to resolve" value={name} maxLength={128} placeholder="IC 1805" onChange={event => setName(event.target.value)} /></label>
      <label>RA<span className="framing-input"><input aria-label="Start RA degrees" type="number" step="any" min={0} max={359.99999} value={ra} onChange={event => setRa(event.target.value)} /><small>°</small></span></label>
      <label>Dec<span className="framing-input"><input aria-label="Start Dec degrees" type="number" step="any" min={-90} max={90} value={dec} onChange={event => setDec(event.target.value)} /><small>°</small></span></label>
    </div>
    {(problem || resolve.isError) && <p className="director-error" role="alert">{problem || message(resolve.error)}</p>}
    <div className="director-actions">
      <button type="submit" disabled={!name.trim() || resolve.isPending}>{resolve.isPending ? 'Looking up...' : 'Look up name'}</button>
      <button type="button" disabled={ra.trim() === '' || dec.trim() === ''} onClick={byCoordinates}>Use these coordinates</button>
    </div>
  </form>;
}

export interface FramingViewProps {
  projectId: string;
  seed: FramingSeed | null;
  /** Rigs already holding this project; the first with optics frames by default. */
  preferredRigIds?: string[];
}

/** Frame one project on the sky: target, angle, mosaic and rig footprints over a survey. */
export default function FramingView({ projectId, seed, preferredRigIds = [] }: FramingViewProps) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const draftKey = ['directorFraming', projectId];
  const draft = useQuery({ queryKey: draftKey, queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const surveys = useQuery({ queryKey: ['directorSurveys'], queryFn: apiClient.getDirectorSurveys, staleTime: Infinity, retry: retryWhenBusy, retryDelay: 700 });
  const rigs = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [state, setState] = useState<FramingState | null>(null);
  const [notice, setNotice] = useState('');
  const [problem, setProblem] = useState('');
  // A project with no linked catalog target starts from a typed or resolved
  // position instead; the start form supplies it.
  const [started, setStarted] = useState<FramingSeed | null>(null);
  useEffect(() => {
    if (!draft.data) return;
    if (draft.data.draft) setState(stateFromDraft(draft.data.draft));
    else if (seed ?? started) setState(stateFromSeed((seed ?? started)!, DEFAULT_SURVEY));
  }, [draft.data, seed, started]);
  const rigList = useMemo(() => rigs.data ?? [], [rigs.data]);
  // Like the framing assistant, start with a rectangle: the first rig that
  // holds this project and knows its optics, else any rig that does.
  useEffect(() => {
    if (!state || state.panel || state.panelRigId || rigList.length === 0) return;
    const candidates = [...preferredRigIds.map(id => rigList.find(r => r.rig.id === id)).filter((r): r is DirectorRigProfileSummary => !!r), ...rigList];
    const first = candidates.find(r => r.field_of_view);
    if (first) setState(current => current && !current.panel ? { ...current, panelRigId: first.rig.id, panel: panelForRig(rigList, first.rig.id) } : current);
  }, [state, rigList, preferredRigIds]);
  const update = useCallback((patch: Partial<FramingState> | ((current: FramingState) => Partial<FramingState>)) => {
    setState(current => current ? { ...current, ...(typeof patch === 'function' ? patch(current) : patch) } : current);
  }, []);

  // The rectangle is drawn here, with the same tangent-plane math the server
  // uses when it activates, so it follows the pointer without a round trip.
  const request = useMemo(() => state ? previewRequest(state, rigList) : null, [state, rigList]);
  const geometry = useMemo(() => request ? framingGeometry(request) : undefined, [request]);
  const [search, setSearch] = useState('');
  // A found target replaces what was on screen; keep that so one click
  // puts it back, and offer the saved draft as the other way back.
  const [undo, setUndo] = useState<FramingState | null>(null);
  const lookup = useMutation({
    retry: false,
    mutationFn: ({ query }: { query: string; before: FramingState }) => apiClient.resolveDirectorName(query),
    onSuccess: (hit, { before }) => {
      const center = { ra_degrees: hit.ra_degrees, dec_degrees: hit.dec_degrees };
      setUndo(before);
      update({ targetName: hit.name, center, viewCenter: center });
      setNotice(`Moved the target to ${hit.name}.`);
    },
  });
  const findTarget = () => { if (state && search.trim() && canWrite) lookup.mutate({ query: search.trim(), before: state }); };
  const savedState = draft.data?.draft ? stateFromDraft(draft.data.draft) : null;
  const planFields = (s: FramingState) => JSON.stringify([s.targetName, s.center, s.positionAngle, s.mosaic, s.panelRigId, s.panel, s.shownRigIds, s.surveyId]);
  const differsFromSaved = !!state && !!savedState && planFields(state) !== planFields(savedState);
  const cutout = useSurveyCutout(state ? {
    survey: state.surveyId, ra: Number(state.viewCenter.ra_degrees.toFixed(5)), dec: Number(state.viewCenter.dec_degrees.toFixed(5)),
    fov: Number(state.viewFov.toFixed(5)), width: STAGE_WIDTH, height: STAGE_HEIGHT, rotation: 0,
  } : null);
  // Finished per-panel stacks, drawn where their plate solves put them: a
  // review of coverage and seams over the plan, never a processed image.
  const [showStacks, setShowStacks] = useState(true);
  const mosaic = useQuery({ queryKey: ['directorMosaic', projectId], queryFn: () => apiClient.getDirectorMosaic(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false, staleTime: 60_000 });
  const placedStacks = useMemo(() => !showStacks || !state || !mosaic.data ? [] : mosaic.data.panels.flatMap(panel => {
    const matrix = panel.preview ? stackMatrix(panel.preview, state.viewCenter, state.viewFov) : null;
    return panel.preview && matrix ? [{ panel, preview: panel.preview, matrix }] : [];
  }), [showStacks, state, mosaic.data]);
  // A view narrower than the footprint hides its edges and handle; widen it
  // once when the geometry first arrives, and on request.
  const fitToFootprint = useCallback((extent: { width_degrees: number; height_degrees: number }) => {
    const needed = Math.max(extent.width_degrees, (extent.height_degrees * STAGE_WIDTH) / STAGE_HEIGHT) * 1.35;
    update(current => ({ viewFov: clampFov(Math.max(current.viewFov, needed)), viewCenter: current.center }));
  }, [update]);
  const fitted = useRef(false);
  const firstExtent = geometry?.extent;
  useEffect(() => {
    if (!firstExtent || fitted.current) return;
    fitted.current = true;
    fitToFootprint(firstExtent);
  }, [firstExtent, fitToFootprint]);

  const save = useMutation({
    retry: false,
    mutationFn: () => {
      if (!state || !draft.data) throw new Error('Nothing to save');
      return apiClient.saveDirectorFramingDraft(draftFromState(state, projectId, draft.data.draft?.revision ?? 0));
    },
    onSuccess: saved => { setNotice(`Saved framing revision ${saved.draft?.revision ?? 0}.`); setUndo(null); client.setQueryData<DirectorFramingDraftView>(draftKey, saved); },
  });
  const httpError = isAxiosError(save.error) ? save.error : save.error instanceof Error && isAxiosError(save.error.cause) ? save.error.cause : null;
  const stale = httpError?.response?.status === 409;

  // Drag pans the view; the wheel zooms about the center.
  const stage = useRef<HTMLDivElement>(null);
  const drag = useRef<{ kind: 'pan' | 'target' | 'rotate'; x: number; y: number; center: FramingState['viewCenter']; target: FramingState['center'] } | null>(null);
  const stageScale = () => (stage.current ? STAGE_WIDTH / stage.current.clientWidth : 1);
  const stagePoint = (event: ReactPointerEvent<HTMLDivElement>): [number, number] => {
    const rect = event.currentTarget.getBoundingClientRect();
    const k = STAGE_WIDTH / Math.max(1, rect.width);
    return [(event.clientX - rect.left) * k, (event.clientY - rect.top) * k];
  };
  const geometryRef = useRef<DirectorFramingPreview | undefined>(undefined);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!state || event.button !== 0) return;
    const point = stagePoint(event);
    const geometry = geometryRef.current;
    let kind: 'pan' | 'target' | 'rotate' = 'pan';
    if (geometry?.view_center_offset && geometry.panels.length > 0) {
      const handle = toStage(rotationHandle(geometry, state), state.viewFov);
      if (Math.hypot(handle[0] - point[0], handle[1] - point[1]) <= 18) kind = 'rotate';
      else if (geometry.panels.some(panel => panel.view_corners && insidePolygon(point, panel.view_corners, state.viewFov))) kind = 'target';
    }
    drag.current = { kind, x: event.clientX, y: event.clientY, center: state.viewCenter, target: state.center };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || !state) return;
    const scale = pixelScale(state.viewFov) * stageScale();
    const dx = (event.clientX - drag.current.x) * scale;
    const dy = (event.clientY - drag.current.y) * scale;
    if (drag.current.kind === 'pan') update({ viewCenter: moveBy(drag.current.center, dx, dy) });
    else if (drag.current.kind === 'target') update({ center: moveBy(drag.current.target, -dx, -dy) });
    else if (geometryRef.current?.view_center_offset) {
      const center = toStage(geometryRef.current.view_center_offset, state.viewFov);
      update({ positionAngle: Math.round(angleFromStage(center, stagePoint(event)) * 10) / 10 });
    }
  };
  const onPointerUp = () => { drag.current = null; };
  const turn = (delta: number) => update(current => ({ positionAngle: ((current.positionAngle + delta) % 360 + 360) % 360 }));
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
  if (!state) return <StartFraming canWrite={canWrite} onStart={setStarted} />;
  geometryRef.current = geometry;
  const handle = geometry?.view_center_offset && geometry.panels.length > 0 ? toStage(rotationHandle(geometry, state), state.viewFov) : null;
  const centerOnStage = geometry?.view_center_offset ? toStage(geometry.view_center_offset, state.viewFov) : null;
  return <section className="framing" aria-label="Framing">
    <div className="framing-stage-wrap">
      <div ref={stage} className={`framing-stage${cutout.stale ? ' is-stale' : ''}`} role="img" aria-label="Sky view" data-testid="framing-stage"
        onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        {cutout.image ? <img src={cutout.image.url} alt="" draggable={false} /> : <div className="framing-stage-empty" />}
        <svg viewBox={`0 0 ${STAGE_WIDTH} ${STAGE_HEIGHT}`} aria-hidden="true">
          {placedStacks.map(({ panel, preview, matrix }) => <image key={`${panel.rig.id}-${panel.panel_id}`} className="framing-stack" data-testid="framing-stack" href={preview.url} x={0} y={0} width={preview.width} height={preview.height} preserveAspectRatio="none" transform={matrix} />)}
          {geometry?.overlays.map(overlay => overlay.view_corners && <polygon key={overlay.id} className="framing-overlay" points={polygonPoints(overlay.view_corners, state.viewFov)} />)}
          {geometry?.panels.map(panel => panel.view_corners && <g key={panel.id} className="framing-panel">
            <polygon points={polygonPoints(panel.view_corners, state.viewFov)} />
            {geometry.panels.length > 1 && <text x={toStage(panel.view_corners[0], state.viewFov)[0] + 8} y={toStage(panel.view_corners[0], state.viewFov)[1] + 20}>{panel.id}</text>}
          </g>)}
          {centerOnStage && <g className="framing-target"><line x1={centerOnStage[0] - 14} y1={centerOnStage[1]} x2={centerOnStage[0] + 14} y2={centerOnStage[1]} /><line x1={centerOnStage[0]} y1={centerOnStage[1] - 14} x2={centerOnStage[0]} y2={centerOnStage[1] + 14} /></g>}
          {handle && centerOnStage && <g className="framing-rotate" data-testid="framing-rotate-handle"><line x1={centerOnStage[0]} y1={centerOnStage[1]} x2={handle[0]} y2={handle[1]} /><circle cx={handle[0]} cy={handle[1]} r={9} /></g>}
          <g className="framing-compass" transform={`translate(${STAGE_WIDTH - 44} 44)`}><line x1={0} y1={0} x2={0} y2={-28} /><text x={0} y={-32} textAnchor="middle">N</text><line x1={0} y1={0} x2={-28} y2={0} /><text x={-32} y={4} textAnchor="end">E</text></g>
        </svg>
        <div className="framing-stage-status">
          {cutout.status === 'loading' && <span role="status">Loading {survey?.name ?? 'survey'}...</span>}
          {cutout.status === 'failed' && <span role="alert">{cutout.error}</span>}
        </div>
        <div className="framing-stage-scale">{formatDegrees(state.viewFov)} across · N up, E left</div>
        <div className="framing-stage-surveys" role="group" aria-label="Survey layers" onPointerDown={event => event.stopPropagation()}>
          {chipSurveys(surveys.data ?? []).map(({ survey: entry, label }) => <button key={entry.id} type="button" aria-pressed={entry.id === state.surveyId} title={`${entry.name}: ${entry.bandpass}`} onClick={() => update({ surveyId: entry.id })}>{label}</button>)}
        </div>
      </div>
      <p className="framing-readout" data-testid="framing-readout">
        <span>{state.targetName || 'Target'}</span>
        <span>{formatRaHours(state.center.ra_degrees)}</span>
        <span>{formatDec(state.center.dec_degrees)}</span>
        <span>angle {state.positionAngle.toFixed(1)}°</span>
        {state.panel && <span>panel {formatDegrees(state.panel.width_degrees)} × {formatDegrees(state.panel.height_degrees)}</span>}
        {geometry && geometry.panels.length > 1 && <span>{geometry.panels.length} panels, {formatDegrees(geometry.extent.width_degrees)} × {formatDegrees(geometry.extent.height_degrees)}</span>}
      </p>
      <p className="director-muted framing-hint">Drag the rectangle to move the target, its handle to turn it, and the sky to look around.</p>
      {mosaic.data && mosaic.data.activation_revision !== null && <div className="framing-stacks" data-testid="framing-stacks">
        <label className="framing-check"><input type="checkbox" checked={showStacks} onChange={event => setShowStacks(event.target.checked)} />Show finished stacks on the sky</label>
        {mosaic.data.framing_stale && <p className="director-muted">The framing changed since the last activation. Stacks sit where their solves put them; the rectangles are the new plan.</p>}
        {mosaic.data.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
        <ul>{mosaic.data.panels.map(panel => <li key={`${panel.rig.id}-${panel.panel_id}`}>{describeStack(panel)}</li>)}</ul>
      </div>}
      <p className="director-muted framing-attribution">{survey ? `${survey.name}: ${survey.bandpass}. ${survey.attribution}.` : 'Choose a survey.'} Imagery is a composition aid, not pointing evidence.</p>
      <VisibilityPanel projectId={projectId} center={state.center} />
    </div>
    <form className="framing-controls" onSubmit={event => { event.preventDefault(); if (canWrite && !save.isPending && !stale) { setNotice(''); setProblem(''); save.mutate(); } }}>
      <fieldset>
        <legend>Target</legend>
        <label>Find a target<span className="framing-input">
          <input aria-label="Find a target" value={search} maxLength={128} placeholder="M 31, NGC 7000, Heart Nebula" disabled={!canWrite || lookup.isPending} onChange={event => setSearch(event.target.value)}
            onKeyDown={event => { if (event.key === 'Enter') { event.preventDefault(); findTarget(); } }} />
          <button type="button" disabled={!canWrite || !search.trim() || lookup.isPending} onClick={findTarget}><Search size={16} />{lookup.isPending ? 'Looking up...' : 'Go'}</button>
        </span><small>{lookup.isError ? message(lookup.error) : lookup.data ? `${lookup.data.name} from ${lookup.data.source}; the target and view moved there.` : 'Names the CDS catalogs know: Messier, NGC, IC, Sharpless, common names.'}</small></label>
        <label>Name<input aria-label="Target name" value={state.targetName} maxLength={256} onChange={event => update({ targetName: event.target.value })} /></label>
        <div className="framing-grid">
          <label>RA<span className="framing-input"><input aria-label="Right ascension degrees" type="number" step="any" min={0} max={359.99999} value={state.center.ra_degrees} onChange={event => update(current => ({ center: { ...current.center, ra_degrees: number(event.target.value, current.center.ra_degrees) } }))} /><small>°</small></span><small>{formatRaHours(state.center.ra_degrees)}</small></label>
          <label>Dec<span className="framing-input"><input aria-label="Declination degrees" type="number" step="any" min={-90} max={90} value={state.center.dec_degrees} onChange={event => update(current => ({ center: { ...current.center, dec_degrees: number(event.target.value, current.center.dec_degrees) } }))} /><small>°</small></span><small>{formatDec(state.center.dec_degrees)}</small></label>
          <label>Camera angle<span className="framing-input"><input aria-label="Position angle degrees" type="number" step="any" min={0} max={359.99} value={state.positionAngle} onChange={event => update({ positionAngle: ((number(event.target.value, state.positionAngle) % 360) + 360) % 360 })} /><small>° E of N</small></span>
            <span className="framing-turns"><button type="button" aria-label="Turn 90 degrees counter-clockwise" onClick={() => turn(-90)}>−90°</button><button type="button" aria-label="Turn 90 degrees clockwise" onClick={() => turn(90)}>+90°</button>
              {panelRig?.profile?.optics && panelRig.profile.optics.value.rotation.mode !== 'rotator' && <button type="button" onClick={() => update({ positionAngle: (panelRig.profile!.optics!.value.rotation as { angle_degrees: number }).angle_degrees })}>Rig's camera angle</button>}</span></label>
        </div>
        <div className="director-actions">
          <button type="button" onClick={() => update(current => ({ viewCenter: current.center }))}><Crosshair size={16} />Center view on target</button>
          <button type="button" disabled={!canWrite} onClick={() => update(current => ({ center: current.viewCenter }))}><LocateFixed size={16} />Move target to view center</button>
          {seed && <button type="button" disabled={!canWrite} onClick={() => update({ targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees, viewCenter: seed.center })}><Undo2 size={16} />Back to catalog target</button>}
          {savedState && <button type="button" disabled={!differsFromSaved} title="Drop every change since the last save" onClick={() => { setState(savedState); setUndo(null); setNotice(`Back to saved framing revision ${draft.data?.draft?.revision ?? 0}.`); }}><Undo2 size={16} />Back to saved framing</button>}
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
        <label>Width of view<span className="framing-input"><input aria-label="View width degrees" type="number" step="any" min={MIN_VIEW_FOV} max={MAX_VIEW_FOV} value={Number(state.viewFov.toFixed(3))} onChange={event => update({ viewFov: clampFov(number(event.target.value, state.viewFov)) })} /><small>°</small>
          {geometry && <button type="button" onClick={() => { update({ viewFov: 0.01 }); fitToFootprint(geometry.extent); }}>Fit</button>}</span></label>
        <p className="director-muted">Drag the image to pan, scroll to zoom. The view center is {formatRaHours(state.viewCenter.ra_degrees)}, {formatDec(state.viewCenter.dec_degrees)}.</p>
      </fieldset>
      {notice && <p role="status">{notice}{undo && <> <button type="button" className="link-button" onClick={() => { setState(undo); setUndo(null); setNotice('Put the target back where it was.'); }}>Undo</button></>}</p>}
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
