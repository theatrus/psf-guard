import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Crosshair, LocateFixed, RefreshCw, Search, Undo2 } from 'lucide-react';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { DirectorCutoutRequest, DirectorFramingDraftView, DirectorFramingPreview, DirectorMosaicPanel, DirectorRigProfileSummary, DirectorSkyPosition } from '../../api/directorTypes';
import {
  DEFAULT_STAGE, MAX_VIEW_FOV, MIN_VIEW_FOV, TILE_MAX_FOV, angleAt, clampFov, deprojectOn, draftFromState, formatDec, formatDegrees, formatRaHours, framingBackdrop, framingGraticule, fromStage, handleSky,
  compassDirections, insidePolygon, panelForRig, pixelScale, polygonPoints, preferredSurveyId, previewRequest, projectOn, stackMatrix, stageCorners, stageFor, stageProject, stateFromDraft, stateFromSeed, tileMatrix, tileSize, toStage, trueWidth, viewAt, type FramingSeed, type FramingState, type Stage, type StageView, chipSurveys, framingGeometry, tileFor, viewLeftTile, type SkyTile, defaultSurveyId,
} from './framingModel';
import VisibilityPanel from './VisibilityPanel';
import SkyCanvas, { type SkyTileImage } from './SkyCanvas';
import { useDebounced, useSurveyCutout } from './useSurveyCutout';
import './FramingView.css';

const message = (error: unknown) => error instanceof Error ? error.message : 'Framing request failed';
/** Director admits one metadata request at a time and answers 503 with Retry-After while busy. */
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;
const DEFAULT_SURVEY = 'dss2_color';
/** How a drag on the stage behaves, as N.I.N.A. offers it: move the
 *  rectangle over a still sky, or keep the rectangle where it is and move
 *  the sky (and the target with it) under it. Remembered in this browser. */
type DragMode = 'rectangle' | 'sky';
const DRAG_MODE_KEY = 'psf-guard.framing.dragMode';
const ROTATE_SKY_KEY = 'psf-guard.framing.rotateSky';
function remembered<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try { const value = window.localStorage.getItem(key); return allowed.includes(value as T) ? (value as T) : fallback; } catch { return fallback; }
}
function remember(key: string, value: string) {
  try { window.localStorage.setItem(key, value); } catch { /* a private window or blocked storage keeps the default */ }
}
/** The rotation handle on the sky: past the top edge of the whole mosaic,
 *  along the camera's up direction on the target's own plane. */
function rotationHandle(geometry: DirectorFramingPreview, state: FramingState) {
  return handleSky(state.center, state.positionAngle, geometry.extent.height_degrees / 2, Math.max(0.02, state.viewFov * 0.03));
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
  // A fresh framing starts on the offline DSS map when the server has one,
  // and a saved survey gives way to the offline map that stands in for it.
  // The list may land after the draft did; a layer picked by hand stays.
  const surveyChosen = useRef(false);
  useEffect(() => {
    if (!surveys.data || surveyChosen.current) return;
    setState(current => {
      if (!current) return current;
      const wanted = current.surveyId === DEFAULT_SURVEY && !draft.data?.draft ? defaultSurveyId(surveys.data, DEFAULT_SURVEY) : current.surveyId;
      const preferred = preferredSurveyId(wanted, surveys.data, DEFAULT_SURVEY);
      return preferred === current.surveyId ? current : { ...current, surveyId: preferred };
    });
  }, [surveys.data, draft.data, state?.surveyId]);
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
  // The stage takes the shape of its element, so the sky fills whatever
  // width and height the window gives it.
  const stage = useRef<HTMLDivElement>(null);
  // The stage projects about the view center, as N.I.N.A.'s framing
  // assistant does: a drag turns the globe under the pointer, and a
  // rectangle away from the center leans with its local north.
  const drag = useRef<{ kind: 'look' | 'sky' | 'target' | 'rotate'; x: number; y: number; center: FramingState['viewCenter']; target: FramingState['center']; grab: [number, number] } | null>(null);
  const [dragMode, setDragMode] = useState<DragMode>(() => remembered(DRAG_MODE_KEY, ['rectangle', 'sky'] as const, 'rectangle'));
  const [rotateSky, setRotateSky] = useState(() => remembered(ROTATE_SKY_KEY, ['true', 'false'] as const, 'false') === 'true');
  const chooseDragMode = (mode: DragMode) => { setDragMode(mode); remember(DRAG_MODE_KEY, mode); };
  const chooseRotateSky = (on: boolean) => { setRotateSky(on); remember(ROTATE_SKY_KEY, String(on)); };
  const skyRotation = rotateSky && state ? state.positionAngle : 0;
  const stageView: StageView | null = useMemo(() => state ? viewAt(state.viewCenter, state.viewCenter, skyRotation) : null, [state, skyRotation]);
  const [stageSize, setStageSize] = useState<Stage>(DEFAULT_STAGE);
  const hasState = state !== null;
  useEffect(() => {
    const element = stage.current;
    if (!element || typeof ResizeObserver === 'undefined') return;
    const measure = () => { const { width, height } = element.getBoundingClientRect(); if (width > 0 && height > 0) setStageSize(stageFor(width / height)); };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [hasState]);
  // The sky behind the rectangle is a tile twice the view. While the pointer
  // moves, the loaded tile slides and scales under the view at once; a new
  // tile is asked for when the view leaves it, and once the pointer rests
  // the settled view gets a tile of its own so the rectangle sits exactly.
  // A picture is asked for at every zoom, out to a hemisphere.
  const [tile, setTile] = useState<SkyTile | null>(null);
  const viewNow = state ? { center: state.viewCenter, fov: state.viewFov } : null;
  const settledView = useDebounced(viewNow, 700);
  useEffect(() => {
    if (!viewNow) return;
    if (!tile || viewLeftTile(tile, viewNow.center, viewNow.fov, stageSize)) setTile(tileFor(viewNow.center, viewNow.fov));
  }, [viewNow?.center.ra_degrees, viewNow?.center.dec_degrees, viewNow?.fov, stageSize]); // eslint-disable-line react-hooks/exhaustive-deps
  useEffect(() => {
    if (!settledView || !tile) return;
    const rested = tileFor(settledView.center, settledView.fov);
    const moved = stageProject(tile.center, rested.center);
    if (!moved || Math.hypot(moved[0], moved[1]) > settledView.fov * 0.01 || Math.abs(rested.fov - tile.fov) > tile.fov * 0.05) setTile(rested);
  }, [settledView?.center.ra_degrees, settledView?.center.dec_degrees, settledView?.fov]); // eslint-disable-line react-hooks/exhaustive-deps
  const tilePixels = tileSize(stageSize);
  const tileRequest = (center: DirectorSkyPosition, fov: number, survey: string): DirectorCutoutRequest => ({
    survey, ra: Number(center.ra_degrees.toFixed(5)), dec: Number(center.dec_degrees.toFixed(5)),
    fov: Number(fov.toFixed(5)), width: tilePixels.width, height: tilePixels.height, rotation: 0,
  });
  // Once the settled view's tile is up, wider views of the same place are
  // fetched quietly, out to a hemisphere, so a zoom out already has a
  // picture; the server keeps them, so the next visit has them at once.
  const prefetch = useMemo(() => {
    if (!state || !settledView) return [];
    const steps: number[] = [];
    for (let fov = tileFor(settledView.center, settledView.fov).fov * 4; fov < TILE_MAX_FOV; fov *= 4) steps.push(fov);
    steps.push(TILE_MAX_FOV);
    return steps.map(fov => tileRequest(settledView.center, fov, state.surveyId));
  }, [state?.surveyId, settledView?.center.ra_degrees, settledView?.center.dec_degrees, settledView?.fov, tilePixels.width, tilePixels.height]); // eslint-disable-line react-hooks/exhaustive-deps
  const cutout = useSurveyCutout(state && tile && viewNow ? tileRequest(tile.center, tile.fov, state.surveyId) : null, 150, prefetch);
  // The picture is re-projected on the GPU for every frame from the tiles
  // fetched so far. Without WebGL the newest tile is laid in through one
  // fitted matrix instead, which is right at the center and drifts toward
  // the edges of a wide view.
  const [webgl, setWebgl] = useState(true);
  const skyTiles: SkyTileImage[] = useMemo(() => cutout.tiles.map(tile => ({
    key: tile.key, url: tile.url, center: { ra_degrees: tile.request.ra, dec_degrees: tile.request.dec }, fov: tile.request.fov, width: tile.request.width, height: tile.request.height,
  })), [cutout.tiles]);
  const skyMatrix = !webgl && cutout.image && state && stageView
    ? tileMatrix({ center: { ra_degrees: cutout.image.request.ra, dec_degrees: cutout.image.request.dec }, fov: cutout.image.request.fov }, { width: cutout.image.request.width, height: cutout.image.request.height }, stageView, state.viewFov, stageSize)
    : null;
  // The chart under the sky: grid, stars, figures and names, each once the view is wide enough.
  const [showGrid, setShowGrid] = useState(true);
  const [showChart, setShowChart] = useState(true);
  const graticule = useMemo(() => state && stageView && showGrid ? framingGraticule(stageView, state.viewFov, stageSize) : null, [stageView, state?.viewFov, stageSize, showGrid]); // eslint-disable-line react-hooks/exhaustive-deps
  const backdrop = useMemo(() => state && stageView && showChart ? framingBackdrop(stageView, state.viewFov, stageSize) : null, [stageView, state?.viewFov, stageSize, showChart]); // eslint-disable-line react-hooks/exhaustive-deps
  // The survey shows the real stars and the Milky Way; the drawn ones stand in only until a picture is up.
  const pictureUp = !!cutout.image && (webgl || !!skyMatrix);
  // Finished per-panel stacks, drawn where their plate solves put them: a
  // review of coverage and seams over the plan, never a processed image.
  const [showStacks, setShowStacks] = useState(true);
  const mosaic = useQuery({ queryKey: ['directorMosaic', projectId], queryFn: () => apiClient.getDirectorMosaic(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false, staleTime: 60_000 });
  const placedStacks = useMemo(() => !showStacks || !state || !stageView || !mosaic.data ? [] : mosaic.data.panels.flatMap(panel => {
    const matrix = panel.preview ? stackMatrix(panel.preview, stageView, state.viewFov, stageSize) : null;
    return panel.preview && matrix ? [{ panel, preview: panel.preview, matrix }] : [];
  }), [showStacks, state, stageView, mosaic.data, stageSize]);
  // A view narrower than the footprint hides its edges and handle; widen it
  // once when the geometry first arrives, and on request.
  const fitToFootprint = useCallback((extent: { width_degrees: number; height_degrees: number }) => {
    const needed = Math.max(extent.width_degrees, (extent.height_degrees * stageSize.width) / stageSize.height) * 1.35;
    update(current => ({ viewFov: clampFov(Math.max(current.viewFov, needed)), viewCenter: current.center }));
  }, [update, stageSize]);
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

  // Drag pans the view; the wheel zooms about the center. Every drag works
  // on the sky: the grabbed point is deprojected from the stage, so a pan
  // near the pole or a wide view behaves like a narrow one at the equator.
  const stageScale = () => (stage.current ? stageSize.width / Math.max(1, stage.current.clientWidth) : 1);
  const stagePoint = (event: ReactPointerEvent<HTMLDivElement>): [number, number] => {
    const rect = event.currentTarget.getBoundingClientRect();
    const k = stageSize.width / Math.max(1, rect.width);
    return [(event.clientX - rect.left) * k, (event.clientY - rect.top) * k];
  };
  const geometryRef = useRef<DirectorFramingPreview | undefined>(undefined);
  const onPointerDown = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!state || !stageView || event.button !== 0) return;
    const point = stagePoint(event);
    const geometry = geometryRef.current;
    // The handle turns the camera in either mode. Otherwise a drag moves the
    // rectangle when it starts on it, or the sky under a pinned rectangle;
    // Shift looks around without moving the target in both modes.
    let kind: 'look' | 'sky' | 'target' | 'rotate' = event.shiftKey ? 'look' : dragMode === 'sky' ? 'sky' : 'look';
    if (geometry && geometry.panels.length > 0) {
      const handleAt = projectOn(stageView, rotationHandle(geometry, state));
      const handle = handleAt ? toStage(handleAt, state.viewFov, stageSize) : null;
      if (handle && Math.hypot(handle[0] - point[0], handle[1] - point[1]) <= 18) kind = 'rotate';
      else if (kind === 'look' && !event.shiftKey && dragMode === 'rectangle' && geometry.panels.some(panel => { const corners = stageCorners(panel.corners, stageView); return corners && insidePolygon(point, corners, state.viewFov, stageSize); })) kind = 'target';
    }
    // Where the pointer took hold, relative to the target, so the target
    // follows the hand instead of jumping to it.
    const pointerOffset = fromStage(point[0], point[1], state.viewFov, stageSize);
    const targetOffset = projectOn(stageView, state.center) ?? [0, 0];
    drag.current = { kind, x: event.clientX, y: event.clientY, center: state.viewCenter, target: state.center, grab: [pointerOffset[0] - targetOffset[0], pointerOffset[1] - targetOffset[1]] };
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || !state || !stageView) return;
    if (drag.current.kind === 'look' || drag.current.kind === 'sky') {
      const scale = pixelScale(state.viewFov, stageSize) * stageScale();
      const dx = (event.clientX - drag.current.x) * scale;
      const dy = (event.clientY - drag.current.y) * scale;
      if (drag.current.kind === 'look') {
        // Slide the window over the plane from where the drag began; the target stays.
        update({ viewCenter: deprojectOn(viewAt(drag.current.center, drag.current.center, stageView.rotation), [dx, dy]) });
      } else {
        // Move the sky: the view turns and the target goes with it, keeping
        // its place on the stage, as N.I.N.A. does with a pinned rectangle.
        const from = viewAt(drag.current.center, drag.current.center, stageView.rotation);
        const targetAt = projectOn(from, drag.current.target) ?? [0, 0];
        const viewCenter = deprojectOn(from, [dx, dy]);
        update({ viewCenter, center: deprojectOn(viewAt(viewCenter, viewCenter, stageView.rotation), targetAt) });
      }
      return;
    }
    const point = stagePoint(event);
    const pointerOffset = fromStage(point[0], point[1], state.viewFov, stageSize);
    if (drag.current.kind === 'target') {
      const { grab } = drag.current;
      update({ center: deprojectOn(stageView, [pointerOffset[0] - grab[0], pointerOffset[1] - grab[1]]) });
    } else {
      // The angle is read on the target's plane, where the camera angle
      // lives, so the handle and the rectangle agree wherever the view is.
      const at = deprojectOn(stageView, pointerOffset);
      update({ positionAngle: Math.round(angleAt(state.center, at) * 10) / 10 });
    }
  };
  const onPointerUp = () => { drag.current = null; };
  const turn = (delta: number) => update(current => ({ positionAngle: ((current.positionAngle + delta) % 360 + 360) % 360 }));
  const zoomBy = (factor: number) => update(current => ({ viewFov: clampFov(current.viewFov * factor) }));
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
  const view = stageView ?? viewAt(state.center, state.viewCenter);
  const onStage = (position: { ra_degrees: number; dec_degrees: number }) => { const offset = projectOn(view, position); return offset ? toStage(offset, state.viewFov, stageSize) : null; };
  const handle = geometry && geometry.panels.length > 0 ? onStage(rotationHandle(geometry, state)) : null;
  const centerOnStage = geometry ? onStage(state.center) : null;
  const panelPolygons = (geometry?.panels ?? []).map(panel => ({ panel, corners: stageCorners(panel.corners, view) }));
  return <section className="framing" aria-label="Framing">
    <div className="framing-stage-wrap">
      <div ref={stage} className={`framing-stage${cutout.stale ? ' is-stale' : ''}`} role="img" aria-label="Sky view" data-testid="framing-stage"
        onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        {!pictureUp && <div className="framing-stage-empty" />}
        {webgl && view && <SkyCanvas className={`framing-sky${cutout.image ? '' : ' is-empty'}`} tiles={skyTiles} view={view} viewFov={state.viewFov} stage={stageSize} onUnsupported={() => setWebgl(false)} />}
        <svg viewBox={`0 0 ${stageSize.width} ${stageSize.height}`} aria-hidden="true">
          {!webgl && cutout.image && skyMatrix && <image className="framing-sky" data-testid="framing-sky" href={cutout.image.url} x={0} y={0} width={cutout.image.request.width} height={cutout.image.request.height} preserveAspectRatio="none" transform={skyMatrix} />}
          {backdrop && !pictureUp && backdrop.milkyWay.length > 0 && <g className="framing-milky-way">{backdrop.milkyWay.map((d, i) => <path key={i} d={d} />)}</g>}
          {graticule && <g className="framing-graticule" data-testid="framing-graticule">
            {graticule.paths.map((line, i) => <path key={i} d={line.d} />)}
            {graticule.labels.map((label, i) => <text key={i} x={label.x} y={label.y}>{label.text}</text>)}
          </g>}
          {backdrop && backdrop.figures.length > 0 && <g className="framing-figures">{backdrop.figures.map((d, i) => <path key={i} d={d} />)}</g>}
          {backdrop && !pictureUp && backdrop.stars.length > 0 && <g className="framing-stars" data-testid="framing-stars">{backdrop.stars.map((star, i) => <circle key={i} cx={star.x} cy={star.y} r={star.r} />)}</g>}
          {backdrop && backdrop.names.length > 0 && <g className="framing-names" data-testid="framing-names">{backdrop.names.map(name => <text key={name.text} x={name.x} y={name.y}>{name.text}</text>)}</g>}
          {placedStacks.map(({ panel, preview, matrix }) => <image key={`${panel.rig.id}-${panel.panel_id}`} className="framing-stack" data-testid="framing-stack" href={preview.url} x={0} y={0} width={preview.width} height={preview.height} preserveAspectRatio="none" transform={matrix} />)}
          {geometry?.overlays.map(overlay => { const corners = stageCorners(overlay.corners, view); return corners && <polygon key={overlay.id} className="framing-overlay" points={polygonPoints(corners, state.viewFov, stageSize)} />; })}
          {panelPolygons.map(({ panel, corners }) => corners && <g key={panel.id} className="framing-panel">
            <polygon points={polygonPoints(corners, state.viewFov, stageSize)} />
            {panelPolygons.length > 1 && <text x={toStage(corners[0], state.viewFov, stageSize)[0] + 8} y={toStage(corners[0], state.viewFov, stageSize)[1] + 20}>{panel.id}</text>}
          </g>)}
          {centerOnStage && <g className="framing-target"><line x1={centerOnStage[0] - 14} y1={centerOnStage[1]} x2={centerOnStage[0] + 14} y2={centerOnStage[1]} /><line x1={centerOnStage[0]} y1={centerOnStage[1] - 14} x2={centerOnStage[0]} y2={centerOnStage[1] + 14} /></g>}
          {handle && centerOnStage && <g className="framing-rotate" data-testid="framing-rotate-handle"><line x1={centerOnStage[0]} y1={centerOnStage[1]} x2={handle[0]} y2={handle[1]} /><circle cx={handle[0]} cy={handle[1]} r={9} /></g>}
          {(() => { const { north, east } = compassDirections(skyRotation); return <g className="framing-compass" data-testid="framing-compass" data-rotation={skyRotation.toFixed(1)} transform={`translate(${stageSize.width - 44} 44)`}>
            <line x1={0} y1={0} x2={north[0] * 28} y2={north[1] * 28} /><text x={north[0] * 36} y={north[1] * 36 + 5} textAnchor="middle">N</text>
            <line x1={0} y1={0} x2={east[0] * 28} y2={east[1] * 28} /><text x={east[0] * 36} y={east[1] * 36 + 5} textAnchor="middle">E</text>
          </g>; })()}
        </svg>
        <div className="framing-stage-status">
          {cutout.status === 'loading' && <span role="status">Loading {survey?.name ?? 'survey'}...</span>}
          {cutout.status === 'failed' && <span role="alert">{cutout.error}</span>}
        </div>
        <div className="framing-stage-zoom" onPointerDown={event => event.stopPropagation()}>
          <button type="button" aria-label="Zoom in" title="Zoom in" onClick={() => zoomBy(1 / 1.5)}>+</button>
          <button type="button" aria-label="Zoom out" title="Zoom out" onClick={() => zoomBy(1.5)}>−</button>
          {geometry && <button type="button" aria-label="Fit the footprint" title="Fit the footprint" onClick={() => { update({ viewFov: MIN_VIEW_FOV }); fitToFootprint(geometry.extent); }}>⌖</button>}
        </div>
        <div className="framing-stage-scale">{formatDegrees(trueWidth(state.viewFov))} across · {skyRotation === 0 ? 'N up, E left' : `sky turned ${skyRotation.toFixed(1)}°`}</div>
        <div className="framing-stage-surveys" role="group" aria-label="Survey layers" onPointerDown={event => event.stopPropagation()}>
          {chipSurveys(surveys.data ?? []).map(({ survey: entry, label }) => <button key={entry.id} type="button" aria-pressed={entry.id === state.surveyId} title={`${entry.name}: ${entry.bandpass}`} onClick={() => { surveyChosen.current = true; update({ surveyId: entry.id }); }}>{label}</button>)}
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
      <p className="director-muted framing-hint">{dragMode === 'rectangle'
        ? 'Drag the rectangle to move the target, its handle to turn it, and the sky to look around.'
        : 'Drag the sky to move it under the rectangle; the target goes with it. Drag the handle to turn the camera; Shift-drag to look around without moving the target.'}</p>
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
          <select aria-label="Survey" value={state.surveyId} onChange={event => { surveyChosen.current = true; update({ surveyId: event.target.value }); }}>
            {(surveys.data ?? []).map(entry => <option key={entry.id} value={entry.id}>{entry.name}{entry.kind === 'narrowband' ? ' (narrowband)' : ''}</option>)}
          </select>
        </label>
        <label>Width of view<span className="framing-input"><input aria-label="View width degrees" type="number" step="any" min={MIN_VIEW_FOV} max={MAX_VIEW_FOV} value={Number(state.viewFov.toFixed(3))} onChange={event => update({ viewFov: clampFov(number(event.target.value, state.viewFov)) })} /><small>°</small>
          {geometry && <button type="button" onClick={() => { update({ viewFov: MIN_VIEW_FOV }); fitToFootprint(geometry.extent); }}>Fit</button>}</span></label>
        <fieldset className="framing-modes"><legend>A drag moves</legend>
          <label className="framing-check"><input type="radio" name="framing-drag-mode" value="rectangle" checked={dragMode === 'rectangle'} onChange={() => chooseDragMode('rectangle')} />The rectangle over a still sky</label>
          <label className="framing-check"><input type="radio" name="framing-drag-mode" value="sky" checked={dragMode === 'sky'} onChange={() => chooseDragMode('sky')} />The sky under a pinned rectangle</label>
        </fieldset>
        <label className="framing-check"><input type="checkbox" checked={rotateSky} onChange={event => chooseRotateSky(event.target.checked)} />Turn the sky with the camera, rectangle upright</label>
        <label className="framing-check"><input type="checkbox" checked={showGrid} onChange={event => setShowGrid(event.target.checked)} />Equatorial grid</label>
        <label className="framing-check"><input type="checkbox" checked={showChart} onChange={event => setShowChart(event.target.checked)} />Constellations when zoomed out</label>
        <p className="director-muted">Drag the sky to turn it, scroll to zoom out to a hemisphere; the survey follows at every zoom. The view center is {formatRaHours(state.viewCenter.ra_degrees)}, {formatDec(state.viewCenter.dec_degrees)}.</p>
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
