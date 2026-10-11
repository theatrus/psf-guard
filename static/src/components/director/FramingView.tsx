import { useCallback, useEffect, useMemo, useRef, useState, type PointerEvent as ReactPointerEvent } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { isAxiosError } from 'axios';
import { Check, Crosshair, Globe, Grid3x3, Images, Layers, LocateFixed, Orbit, RefreshCw, RotateCw, Sparkles, SquareDashedMousePointer, Sun, Telescope, Undo2 } from 'lucide-react';
import NumberInput from '../NumberInput';
import { useDraftSection } from './pageDraftsState';
import { describeFramingChanges } from './draftChanges';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { SkyPreview } from '../../api/types';
import type { DirectorCutoutRequest, DirectorFramingDraftView, DirectorFramingPreview, DirectorMosaicPanel, DirectorRigFraming, DirectorRigProfileSummary, DirectorSkyMarks, DirectorSkyPosition } from '../../api/directorTypes';
import {
  DEFAULT_STAGE, MAX_VIEW_FOV, MIN_VIEW_FOV, TILE_MAX_FOV, angleAt, clampFov, deprojectFrom, deprojectOn, draftFromState, fitViewFov, formatDec, formatDegrees, formatRaHours, framingBackdrop, framingGraticule, framingProblems, fromStage,
  compassDirections, insidePolygon, markLabelBudget, normaliseAngle, offsetFrom, panelForRig, pixelScale, polygonPoints, preferredSurveyId, previewRequest, projectOn, rigGeometries, rotationHandleSky, roundRa, stackMatrix, stageAngleAt, stageCorners, stageFor, stageProject, stateFromDraft, stateFromSeed, tileMatrix, tileSize, toStage, trueWidth, viewAt, type FramingSeed, type FramingState, type Stage, type StageView, chipSurveys, framingGeometry, tileFor, viewLeftTile, type SkyTile, defaultSurveyId,
} from './framingModel';
import VisibilityPanel from './VisibilityPanel';
import TargetSearch, { type TargetPick } from './TargetSearch';
import SkyCanvas, { type SkyTileImage } from './SkyCanvas';
import { useDebounced, useSurveyCutout } from './useSurveyCutout';
import './FramingView.css';

/** A value shown to the places that mean something; the state keeps every digit. */
const round = (value: number, places: number) => Number(value.toFixed(places));
const message = (error: unknown) => error instanceof Error ? error.message : 'Framing request failed';
/** Director writes take turns at the metadata store; one that waited its full turn answers 503 with Retry-After. */
const httpStatus = (error: unknown) => isAxiosError(error) ? error.response?.status
  : error instanceof Error && isAxiosError(error.cause) ? error.cause.response?.status : undefined;
const retryWhenBusy = (count: number, error: unknown) => httpStatus(error) === 503 && count < 5;
const DEFAULT_SURVEY = 'dss2_color';
/** A wheel notch of about 100 pixels zooms 1.2 times. */
const WHEEL_ZOOM_PER_PIXEL = Math.log(1.2) / 100;
/** A stage narrower than this, in CSS pixels, folds its survey chips behind one. */
const NARROW_STAGE_PX = 560;
/** How a drag on the stage behaves, as N.I.N.A. offers it: move the
 *  rectangle over a still sky, or keep the rectangle where it is and move
 *  the sky (and the target with it) under it. Remembered in this browser. */
type DragMode = 'rectangle' | 'sky';
const DRAG_MODE_KEY = 'psf-guard.framing.dragMode';
const ROTATE_SKY_KEY = 'psf-guard.framing.rotateSky';
/** The survey layer last picked in this browser: how the sky is shown, not
 *  part of the framing. */
const SURVEY_KEY = 'psf-guard.framing.survey';
const MARK_KEYS = { objects: 'psf-guard.framing.marks.objects', bodies: 'psf-guard.framing.marks.bodies', solar: 'psf-guard.framing.marks.solar' } as const;
const SHOWN_CATALOGS_KEY = 'psf-guard.framing.marks.catalogs';
/** Whether finished stacks are drawn on the sky, in this browser. */
const STACKS_KEY = 'psf-guard.framing.stacks';
/** The stack picked for one plan, by its key; none means each panel's best. */
const STACK_CHOICE_KEY = 'psf-guard.framing.stack.';
/** The catalog families a mark can come from, by the letters a designation
 *  starts with. Messier, NGC, IC, Sharpless, the Lynds catalogs and the
 *  supernova remnants start on: the map an imager frames by. The rest, PGC's faint galaxies and HD's stars above all,
 *  swamp a field and wait for a chip. The choice is remembered in this browser. */
const CATALOG_FAMILIES: ReadonlyArray<{ prefix: string; label: string; title: string }> = [
  { prefix: 'M', label: 'Messier', title: 'Messier objects' },
  { prefix: 'NGC', label: 'NGC', title: 'New General Catalogue' },
  { prefix: 'IC', label: 'IC', title: 'Index Catalogue' },
  { prefix: 'Sh', label: 'Sh2', title: 'Sharpless H II regions' },
  { prefix: 'LDN', label: 'LDN', title: 'Lynds dark nebulae' },
  { prefix: 'LBN', label: 'LBN', title: 'Lynds bright nebulae' },
  { prefix: 'B', label: 'Barnard', title: 'Barnard dark nebulae' },
  { prefix: 'vdB', label: 'vdB', title: 'van den Bergh reflection nebulae' },
  { prefix: 'SNR', label: 'SNR', title: 'Supernova remnants' },
  { prefix: 'UGC', label: 'UGC', title: 'Uppsala galaxies' },
  { prefix: 'PGC', label: 'PGC', title: 'Principal Galaxies Catalogue: faint galaxies, hundreds per field' },
  { prefix: 'HD', label: 'HD', title: 'Henry Draper stars: every star the survey already shows' },
  { prefix: 'WR', label: 'WR', title: 'Wolf-Rayet stars' },
];
const DEFAULT_SHOWN_CATALOGS = ['M', 'NGC', 'IC', 'Sh', 'LDN', 'LBN', 'SNR'];
function rememberedCatalogs(): string[] {
  try {
    const value = window.localStorage.getItem(SHOWN_CATALOGS_KEY);
    if (value === null) return DEFAULT_SHOWN_CATALOGS;
    const known = new Set(CATALOG_FAMILIES.map(family => family.prefix.toLowerCase()));
    return value.split(',').map(entry => entry.trim()).filter(entry => known.has(entry.toLowerCase()));
  } catch { return DEFAULT_SHOWN_CATALOGS; }
}
/** Marks are asked for at a time rounded to ten minutes, so a view that
 *  moves a little reuses the answer; comets do not move far in that. */
const MARKS_TIME_BUCKET_MS = 10 * 60 * 1000;
const SOLAR_SYSTEM_LABEL: Record<string, string> = { sun: 'Sun', moon: 'Moon' };
function remembered<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  try { const value = window.localStorage.getItem(key); return allowed.includes(value as T) ? (value as T) : fallback; } catch { return fallback; }
}
function rememberedSurvey(): string | null {
  try { return window.localStorage.getItem(SURVEY_KEY); } catch { return null; }
}
function remember(key: string, value: string) {
  try { window.localStorage.setItem(key, value); } catch { /* a private window or blocked storage keeps the default */ }
}
/** Where to point when the project has no catalog target yet: a name the
 *  catalogs know, or coordinates typed in. */
/** How the picker names a stack. */
const stackName = (stack: SkyPreview) => stack.kind === 'color' ? `Colour ${stack.label}` : stack.label;
/** The stack a panel shows: the one picked, or its best. */
const shownStack = (panel: DirectorMosaicPanel, choice: string): SkyPreview | null =>
  choice ? panel.stacks.find(stack => stack.key === choice) ?? null : panel.stacks[0] ?? panel.preview;
/** One line per panel: whose it is, how far along, and whether the stack it shows can be placed. */
function describeStack(panel: DirectorMosaicPanel, choice: string, choiceName: string): string {
  const who = `${panel.panel_id}, ${panel.catalog_name || panel.rig.name}`;
  const done = panel.progress ? `${panel.progress.accepted}/${panel.progress.desired} frames accepted` : 'no exposure plans';
  if (panel.status === 'missing_target') return `${who}: its target row is gone from the database.`;
  if (panel.status === 'missing_catalog') return `${who}: its database is no longer registered here.`;
  const stack = shownStack(panel, choice);
  if (!stack) return choice && panel.stacks.length > 0 ? `${who}: ${done}; no ${choiceName} stack.` : `${who}: ${done}; no stack yet.`;
  return stack.wcs ? `${who}: ${done}; ${stackName(stack)} stack placed by its solve.` : `${who}: ${done}; the ${stackName(stack)} stack has no plate solve yet, so it cannot be placed.`;
}

function StartFraming({ canWrite, onStart }: { canWrite: boolean; onStart: (seed: FramingSeed) => void }) {
  const [name, setName] = useState('');
  const [ra, setRa] = useState('');
  const [dec, setDec] = useState('');
  const [problem, setProblem] = useState('');
  const byCoordinates = () => {
    const [r, d] = [Number(ra), Number(dec)];
    if (!Number.isFinite(r) || !Number.isFinite(d) || r < 0 || r >= 360 || d < -90 || d > 90) { setProblem('Enter RA in degrees from 0 to 360 and Dec from -90 to 90.'); return; }
    setProblem('');
    onStart({ name: name.trim() || 'Target', center: { ra_degrees: r, dec_degrees: d }, position_angle_degrees: 0 });
  };
  if (!canWrite) return <p className="director-muted">This project has no framing yet.</p>;
  return <form className="framing-start" aria-label="Start framing" onSubmit={event => event.preventDefault()}>
    <p className="director-muted">No linked catalog target yet. Start from a name the catalogs know, or type the center.</p>
    <TargetSearch label="Object name" ariaLabel="Object name to resolve" placeholder="IC 1805" buttonLabel="Look up name" hint="The local catalog answers as you type; the last row asks CDS Sesame."
      onPick={pick => onStart({ name: pick.name, center: pick.center, position_angle_degrees: 0 })} />
    <div className="framing-grid">
      <label>Name for coordinates<input aria-label="Name for typed coordinates" value={name} maxLength={128} placeholder="Target" onChange={event => setName(event.target.value)} /></label>
      <label>RA<span className="framing-input"><input aria-label="Start RA degrees" type="number" step="any" min={0} max={359.99999} value={ra} onChange={event => setRa(event.target.value)} /><small>°</small></span></label>
      <label>Dec<span className="framing-input"><input aria-label="Start Dec degrees" type="number" step="any" min={-90} max={90} value={dec} onChange={event => setDec(event.target.value)} /><small>°</small></span></label>
    </div>
    {problem && <p className="director-error" role="alert">{problem}</p>}
    <div className="director-actions">
      <button type="button" disabled={ra.trim() === '' || dec.trim() === ''} onClick={byCoordinates}>Use these coordinates</button>
    </div>
  </form>;
}

export interface FramingViewProps {
  projectId: string;
  seed: FramingSeed | null;
  /** Rigs already holding this project; the first with optics frames by default. */
  preferredRigIds?: string[];
  /** On a page that keeps the plan: the rigs shooting it, and how to add or
   *  drop one. A rig's row then toggles it. `false` says the rig did not
   *  join, such as when no exposure template matches it. */
  shootingRigIds?: string[];
  onToggleRig?: (rigId: string, on: boolean) => boolean | void;
  /** Why a rig cannot join yet, such as a plan with no objectives. */
  joinBlocked?: string;
}

/** Frame one project on the sky: target, angle, mosaic and rig footprints over a survey. */
export default function FramingView({ projectId, seed, preferredRigIds = [], shootingRigIds, onToggleRig, joinBlocked }: FramingViewProps) {
  const { canWrite } = useAccess();
  const client = useQueryClient();
  const draftKey = ['directorFraming', projectId];
  const draft = useQuery({ queryKey: draftKey, queryFn: () => apiClient.getDirectorFramingDraft(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const surveys = useQuery({ queryKey: ['directorSurveys'], queryFn: apiClient.getDirectorSurveys, staleTime: Infinity, retry: retryWhenBusy, retryDelay: 700 });
  const rigs = useQuery({ queryKey: ['directorRigProfiles'], queryFn: apiClient.getDirectorRigProfiles, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false });
  const [state, setState] = useState<FramingState | null>(null);
  // What the framing was when it loaded, with the same automatic choices
  // applied (a survey this server stands in for, a panel rig picked for an
  // empty framing). Only a difference from this is an edit someone made.
  const [baseline, setBaseline] = useState<FramingState | null>(null);
  const adjust = useCallback((change: (current: FramingState | null) => FramingState | null) => {
    setState(change);
    setBaseline(change);
  }, []);
  const [notice, setNotice] = useState('');
  // What the save bar was told when a drag began, held until it ends.
  const [heldReport, setHeldReport] = useState<{ unsaved: boolean; changes: string[] } | null>(null);
  const [problem, setProblem] = useState('');
  // A project with no linked catalog target starts from a typed or resolved
  // position instead; the start form supplies it.
  const [started, setStarted] = useState<FramingSeed | null>(null);
  // The saved revision the state on screen stands on: the one it loaded
  // from, or the one it was saved as. An answer at that revision is this
  // view's own save coming back, or a refetch that found nothing new; it
  // leaves the state, the view and any edit typed meanwhile alone. Only a
  // revision saved from elsewhere replaces them.
  const shownRevision = useRef<number | null>(null);
  useEffect(() => {
    if (!draft.data) return;
    if (draft.data.draft && draft.data.draft.revision === shownRevision.current) return;
    shownRevision.current = draft.data.draft?.revision ?? null;
    const loaded = draft.data.draft ? stateFromDraft(draft.data.draft)
      : seed ? stateFromSeed(seed, DEFAULT_SURVEY)
      : null;
    if (loaded) {
      setState(loaded);
      setBaseline(loaded);
    } else if (started) {
      // Typed into the start form: that is an edit, against nothing saved.
      setState(stateFromSeed(started, DEFAULT_SURVEY));
      setBaseline(null);
    }
  }, [draft.data, seed, started]);
  // The layer last picked in this browser comes first. Else a fresh framing
  // starts on the offline DSS map when the server has one, and a saved
  // survey gives way to the offline map that stands in for it. The list may
  // land after the draft did; a layer picked by hand stays.
  const surveyChosen = useRef(false);
  useEffect(() => {
    if (!surveys.data || surveyChosen.current) return;
    adjust(current => {
      if (!current) return current;
      const picked = rememberedSurvey();
      const wanted = picked && surveys.data!.some(entry => entry.id === picked) ? picked
        : current.surveyId === DEFAULT_SURVEY && !draft.data?.draft ? defaultSurveyId(surveys.data, DEFAULT_SURVEY) : current.surveyId;
      const preferred = preferredSurveyId(wanted, surveys.data, DEFAULT_SURVEY);
      return preferred === current.surveyId ? current : { ...current, surveyId: preferred };
    });
  }, [surveys.data, draft.data, state?.surveyId, adjust]);
  // A layer picked by hand is kept in this browser for every plan; the
  // framing's next save carries it along, but it is never an edit.
  const chooseSurvey = (id: string) => {
    surveyChosen.current = true;
    remember(SURVEY_KEY, id);
    adjust(current => current && { ...current, surveyId: id });
  };
  const rigList = useMemo(() => rigs.data ?? [], [rigs.data]);
  // A rig framed on a center of its own is timed where the view has it,
  // saved or not, so the visibility chart follows the drag.
  const liveRigCenters = useMemo(() => Object.fromEntries((state?.rigFramings ?? [])
    .filter(own => own.center !== null)
    .map(own => [own.rig_id, own.center!])), [state?.rigFramings]);
  // The shared framing is drawn only while some rig shoots it: when every
  // rig that is on has a separate framing, no shared target is planned.
  const sharedInUse = !onToggleRig || !state || rigList.some(entry => (shootingRigIds ?? []).includes(entry.rig.id) && !state.rigFramings.some(own => own.rig_id === entry.rig.id));
  // A rig that is off, or gone from the list, keeps its separate framing,
  // drawn dim and left where it is.
  const rigLive = (id: string) => rigList.some(entry => entry.rig.id === id) && (!onToggleRig || (shootingRigIds ?? []).includes(id));
  // The rigs that can size the shared framing: on, when the page keeps a
  // plan, knowing their optics, and not framed separately.
  const sizesShared = useCallback((current: FramingState, rig: DirectorRigProfileSummary) => !!rig.field_of_view
    && (!onToggleRig || (shootingRigIds ?? []).includes(rig.rig.id)) && !current.rigFramings.some(own => own.rig_id === rig.rig.id), [onToggleRig, shootingRigIds]);
  // Like the framing assistant, start with a rectangle: the first rig that
  // can size it, else the first that holds this project and knows its
  // optics, else any rig that does.
  useEffect(() => {
    if (!state || state.panel || state.panelRigId || rigList.length === 0) return;
    const candidates = [...rigList.filter(r => sizesShared(state, r)), ...preferredRigIds.map(id => rigList.find(r => r.rig.id === id)).filter((r): r is DirectorRigProfileSummary => !!r), ...rigList];
    const first = candidates.find(r => r.field_of_view);
    if (first) adjust(current => current && !current.panel ? { ...current, panelRigId: first.rig.id, panel: panelForRig(rigList, first.rig.id) } : current);
  }, [state, rigList, preferredRigIds, adjust, sizesShared]);
  const update = useCallback((patch: Partial<FramingState> | ((current: FramingState) => Partial<FramingState>)) => {
    setState(current => current ? { ...current, ...(typeof patch === 'function' ? patch(current) : patch) } : current);
  }, []);
  // The rig that sizes the shared framing must shoot it. Once it is off,
  // here or on another tab, the first rig that can size the framing takes
  // over; with none, the size stays and says its rig is off. The check runs
  // when the rigs that are on change, not on every edit: a rig switched on
  // here frames the plan at once, before the page has heard it is on. The
  // first check, once the plan's rigs are known, is the view's own choice
  // and is saved quietly; after that a switch is an edit the save bar lists.
  const checkedShooting = useRef<string | null>(null);
  useEffect(() => {
    if (!state || !onToggleRig || !shootingRigIds?.length || rigList.length === 0) return;
    const key = shootingRigIds.join(',');
    if (checkedShooting.current === key) return;
    const first = checkedShooting.current === null;
    checkedShooting.current = key;
    if (!state.panelRigId || shootingRigIds.includes(state.panelRigId)) return;
    const next = rigList.find(r => sizesShared(state, r));
    if (!next) return;
    const take = (current: FramingState | null) => current ? { ...current, panelRigId: next.rig.id, panel: panelForRig(rigList, next.rig.id) } : current;
    if (first) adjust(take); else setState(take);
  }, [state, onToggleRig, shootingRigIds, rigList, sizesShared, adjust]);

  // The rectangle is drawn here, with the same tangent-plane math the server
  // uses when it activates, so it follows the pointer without a round trip.
  const request = useMemo(() => state ? previewRequest(state, rigList) : null, [state, rigList]);
  const geometry = useMemo(() => request ? framingGeometry(request) : undefined, [request]);

  // A found target moves the target and the view, and a separate framing
  // with a center of its own keeps its offset from the target. Undo puts
  // back only what the pick moved, so a later edit stays; the saved draft
  // is the other way back.
  const [undo, setUndo] = useState<Pick<FramingState, 'targetName' | 'center' | 'viewCenter'> | null>(null);
  const carry = (framings: DirectorRigFraming[], from: DirectorSkyPosition, to: DirectorSkyPosition) => framings.map(own => {
    if (!own.center) return own;
    const offset = offsetFrom(from, own.center);
    return { ...own, center: offset ? deprojectFrom(to, offset) : null };
  });
  const pickTarget = (pick: TargetPick) => {
    if (!state || !canWrite) return;
    setUndo({ targetName: state.targetName, center: state.center, viewCenter: state.viewCenter });
    update(current => ({ targetName: pick.name, center: pick.center, viewCenter: pick.center, rigFramings: carry(current.rigFramings, current.center, pick.center) }));
    setNotice(`Moved the target to ${pick.name}.`);
  };
  const undoPick = () => {
    if (!undo) return;
    update(current => ({ ...undo, rigFramings: carry(current.rigFramings, current.center, undo.center) }));
    setUndo(null);
    setNotice('Put the target back where it was.');
  };
  const savedState = draft.data?.draft ? stateFromDraft(draft.data.draft) : null;
  // The name is saved trimmed, so a trailing space is no difference.
  // The survey layer is how the sky is shown, so it is no difference.
  const planFields = (s: FramingState) => JSON.stringify([s.targetName.trim(), s.center, s.positionAngle, s.mosaic, s.panelRigId, s.panel, s.shownRigIds, s.rigFramings]);
  const differsFromSaved = !!state && !!savedState && planFields(state) !== planFields(savedState);
  // The stage takes the shape of its element, so the sky fills whatever
  // width and height the window gives it. The element is held in state as
  // well, so its listeners follow it if it is drawn anew.
  const stage = useRef<HTMLDivElement | null>(null);
  const [stageElement, setStageElement] = useState<HTMLDivElement | null>(null);
  const stageRef = useCallback((element: HTMLDivElement | null) => { stage.current = element; setStageElement(element); }, []);
  // The stage projects about the view center, as N.I.N.A.'s framing
  // assistant does: a drag turns the globe under the pointer, and a
  // rectangle away from the center leans with its local north. Every move
  // of a drag is read against the view as it was when the drag began: with
  // the sky turned by the camera, the handle's own turn would otherwise
  // turn the view it is read in.
  const drag = useRef<{ rig?: string | null; kind: 'look' | 'sky' | 'target' | 'rotate'; x: number; y: number; view: StageView; center: FramingState['viewCenter']; target: FramingState['center']; grab: [number, number] } | null>(null);
  const [dragMode, setDragMode] = useState<DragMode>(() => remembered(DRAG_MODE_KEY, ['rectangle', 'sky'] as const, 'rectangle'));
  const [rotateSky, setRotateSky] = useState(() => remembered(ROTATE_SKY_KEY, ['true', 'false'] as const, 'false') === 'true');
  const chooseDragMode = (mode: DragMode) => { setDragMode(mode); remember(DRAG_MODE_KEY, mode); };
  const chooseRotateSky = (on: boolean) => { setRotateSky(on); remember(ROTATE_SKY_KEY, String(on)); };
  const [showObjects, setShowObjects] = useState(() => remembered(MARK_KEYS.objects, ['true', 'false'] as const, 'true') === 'true');
  // Comets, asteroids and the solar system wait for their switch: a zoomed-in
  // field otherwise fills with faint asteroids.
  const [showBodies, setShowBodies] = useState(() => remembered(MARK_KEYS.bodies, ['true', 'false'] as const, 'false') === 'true');
  const [showSolar, setShowSolar] = useState(() => remembered(MARK_KEYS.solar, ['true', 'false'] as const, 'false') === 'true');
  const [shownCatalogs, setShownCatalogs] = useState<string[]>(rememberedCatalogs);
  const toggleCatalog = (prefix: string) => setShownCatalogs(current => {
    const next = current.some(entry => entry.toLowerCase() === prefix.toLowerCase()) ? current.filter(entry => entry.toLowerCase() !== prefix.toLowerCase()) : [...current, prefix];
    remember(SHOWN_CATALOGS_KEY, next.join(','));
    return next;
  });
  const chooseMarks = (key: keyof typeof MARK_KEYS, on: boolean) => {
    ({ objects: setShowObjects, bodies: setShowBodies, solar: setShowSolar })[key](on);
    remember(MARK_KEYS[key], String(on));
  };
  const skyRotation = rotateSky && state ? state.positionAngle : 0;
  const stageView: StageView | null = useMemo(() => state ? viewAt(state.viewCenter, state.viewCenter, skyRotation) : null, [state, skyRotation]);
  const [stageSize, setStageSize] = useState<Stage>(DEFAULT_STAGE);
  // The stage's width in device pixels, so survey tiles are asked for at the
  // density the screen shows; two pixels per logical unit until measured.
  const [stagePixels, setStagePixels] = useState(DEFAULT_STAGE.width * 2);
  // A stage narrower than a tablet has no room for a row of survey chips.
  const [narrow, setNarrow] = useState(false);
  const [layersOpen, setLayersOpen] = useState(false);
  useEffect(() => {
    const element = stageElement;
    if (!element || typeof ResizeObserver === 'undefined') return;
    const measure = () => {
      const { width, height } = element.getBoundingClientRect();
      if (width > 0 && height > 0) {
        setStageSize(stageFor(width / height));
        setStagePixels(Math.round(width * Math.min(2, window.devicePixelRatio || 1)));
        setNarrow(width < NARROW_STAGE_PX);
      }
    };
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [stageElement]);
  // The sky behind the rectangle is a tile twice the view. While the pointer
  // moves, the loaded tile slides and scales under the view at once; a new
  // tile is asked for when the view leaves it, and once the pointer rests
  // the settled view gets a tile of its own so the rectangle sits exactly.
  // A picture is asked for at every zoom, out to a hemisphere.
  const [tile, setTile] = useState<SkyTile | null>(null);
  // One object per distinct view, so the settle timers below count from the
  // last move and not from the last render (a tile arriving must not push
  // the settled view back).
  const viewNow = useMemo(() => state ? { center: state.viewCenter, fov: state.viewFov } : null,
    [state?.viewCenter.ra_degrees, state?.viewCenter.dec_degrees, state?.viewFov]); // eslint-disable-line react-hooks/exhaustive-deps
  const settledView = useDebounced(viewNow, 700);
  // Marks are cheap to ask for, so they follow the view sooner than the tile.
  const marksView = useDebounced(viewNow, 250);
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
  const tilePixels = tileSize(stageSize, stagePixels);
  const tileRequest = (center: DirectorSkyPosition, fov: number, survey: string): DirectorCutoutRequest => ({
    survey, ra: roundRa(center.ra_degrees, 5), dec: Number(center.dec_degrees.toFixed(5)),
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
  // Marks for the settled view: what the catalogs know is in the field, and
  // where the Sun, Moon, planets, comets and asteroids are at this moment.
  const marksWanted = showObjects || showBodies || showSolar;
  const marksQuery = marksView ? {
    ra: roundRa(marksView.center.ra_degrees, 2), dec: Number(marksView.center.dec_degrees.toFixed(2)),
    fov: Number(marksView.fov.toFixed(2)), aspect: Number((stageSize.width / stageSize.height).toFixed(3)),
    at: Math.floor(Date.now() / MARKS_TIME_BUCKET_MS) * MARKS_TIME_BUCKET_MS,
    catalogs: shownCatalogs.join(','),
  } : null;
  const marks = useQuery({
    queryKey: ['directorSkyMarks', marksQuery],
    queryFn: () => apiClient.getDirectorSkyMarks(marksQuery!),
    enabled: marksWanted && !!marksQuery, staleTime: 5 * 60_000, retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false, placeholderData: previous => previous,
  });
  // The picture is re-projected on the GPU for every frame from the tiles
  // fetched so far. Without WebGL the newest tile is laid in through one
  // fitted matrix instead, which is right at the center and drifts toward
  // the edges of a wide view.
  const [webgl, setWebgl] = useState(true);
  // The GPU can drop the canvas's context (a driver reset, too many pages
  // open); the sky is then black until it comes back, so the drawn stars
  // stand in meanwhile.
  const [skyLost, setSkyLost] = useState(false);
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
  const pictureUp = !!cutout.image && (webgl ? !skyLost : !!skyMatrix);
  // Finished per-panel stacks, drawn where their plate solves put them: a
  // review of coverage and seams over the plan, never a processed image.
  // Both are remembered in this browser: the layer for every plan, the
  // stack picked for this one.
  const [showStacks, setShowStacks] = useState(() => remembered(STACKS_KEY, ['on', 'off'] as const, 'on') === 'on');
  const chooseShowStacks = (on: boolean) => { setShowStacks(on); remember(STACKS_KEY, on ? 'on' : 'off'); };
  const [stackPick, setStackPick] = useState(() => { try { return window.localStorage.getItem(STACK_CHOICE_KEY + projectId) ?? ''; } catch { return ''; } });
  const chooseStack = (key: string) => { setStackPick(key); remember(STACK_CHOICE_KEY + projectId, key); };
  const mosaic = useQuery({ queryKey: ['directorMosaic', projectId], queryFn: () => apiClient.getDirectorMosaic(projectId), retry: retryWhenBusy, retryDelay: 700, refetchOnWindowFocus: false, staleTime: 60_000 });
  // Every stack some panel has, colour first, each once.
  const stackChoices = useMemo(() => {
    const found = new Map<string, SkyPreview>();
    for (const panel of mosaic.data?.panels ?? []) for (const stack of panel.stacks ?? []) if (!found.has(stack.key)) found.set(stack.key, stack);
    return [...found.values()].sort((a, b) => (a.kind === b.kind ? 0 : a.kind === 'color' ? -1 : 1) || a.label.localeCompare(b.label));
  }, [mosaic.data]);
  // A stack picked before that no panel has now falls back to each panel's best.
  const stackChoice = stackChoices.some(stack => stack.key === stackPick) ? stackPick : '';
  const stackChoiceName = stackChoices.find(stack => stack.key === stackChoice);
  const placedStacks = useMemo(() => !showStacks || !state || !stageView || !mosaic.data ? [] : mosaic.data.panels.flatMap(panel => {
    const preview = shownStack(panel, stackChoice);
    const matrix = preview ? stackMatrix(preview, stageView, state.viewFov, stageSize) : null;
    return preview && matrix ? [{ panel, preview, matrix }] : [];
  }), [showStacks, state, stageView, mosaic.data, stageSize, stackChoice]);
  // A view narrower than the footprint hides its edges and handle. The fit
  // takes every panel corner as it is drawn, turned by the camera angle,
  // and the handle past the top edge. It widens the view once when the
  // geometry first arrives, and fits it exactly on request.
  const fitView = useCallback((widenOnly: boolean) => update(current => {
    const request = previewRequest(current, rigList);
    const own = rigGeometries(current, rigList).flatMap(entry => entry.geometry.panels);
    const shared = request && (sharedInUse || own.length === 0) ? framingGeometry(request) : null;
    const corners = [...(shared?.panels ?? []), ...own].flatMap(panel => panel.corners);
    const handle = shared && shared.panels.length > 0 ? (fov: number) => rotationHandleSky(current, shared.extent, fov) : null;
    const fov = fitViewFov(corners, handle, current.center, rotateSky ? current.positionAngle : 0, stageSize);
    return fov === null ? {} : { viewFov: widenOnly ? Math.max(current.viewFov, fov) : fov, viewCenter: current.center };
  }), [update, rigList, sharedInUse, rotateSky, stageSize]);
  const fitted = useRef(false);
  const hasGeometry = !!geometry;
  useEffect(() => {
    if (!hasGeometry || fitted.current) return;
    fitted.current = true;
    fitView(true);
  }, [hasGeometry, fitView]);

  const save = useMutation({
    retry: false,
    mutationFn: (sent: FramingState | null) => {
      if (!sent || !draft.data) throw new Error('Nothing to save');
      return apiClient.saveDirectorFramingDraft(draftFromState(sent, projectId, draft.data.draft?.revision ?? 0));
    },
    onSuccess: (saved, sent) => {
      setNotice(`Saved framing revision ${saved.draft?.revision ?? 0}.`);
      setUndo(null);
      // What was saved is the new baseline. The state stays as it is, so
      // the view does not jump back to the target and an edit typed while
      // the save was out is still an edit.
      if (saved.draft && sent) {
        shownRevision.current = saved.draft.revision;
        const stored = stateFromDraft(saved.draft);
        setBaseline({ ...stored, targetName: sent.targetName.trim() === stored.targetName ? sent.targetName : stored.targetName, viewCenter: sent.viewCenter, viewFov: sent.viewFov });
      }
      client.setQueryData<DirectorFramingDraftView>(draftKey, saved);
    },
  });
  const httpError = isAxiosError(save.error) ? save.error : save.error instanceof Error && isAxiosError(save.error.cause) ? save.error.cause : null;
  const stale = httpError?.response?.status === 409;
  // On the project page the save bar saves the framing with the plan. Only
  // someone's edit counts as unsaved; a framing taken from the catalog, or
  // one this view filled in, is saved quietly with the next save, since
  // activation needs it stored.
  // Unsaved means the bar can say what changed: a difference the list cannot
  // name, such as the same rigs in another order, is no edit.
  const changes = baseline ? describeFramingChanges(baseline, state, id => rigList.find(rig => rig.rig.id === id)?.catalog_name ?? 'a rig') : ['new framing'];
  const edited = !!state && !!draft.data && changes.length > 0;
  const firstSave = !!state && !!draft.data && !edited && (!savedState || planFields(state) !== planFields(savedState));
  const reported = { unsaved: canWrite && edited, changes };
  // What the server would refuse, said at the field before a save.
  const problems = useMemo(() => state ? framingProblems(state, rigList, id => rigList.find(entry => entry.rig.id === id)?.catalog_name ?? 'A rig') : [], [state, rigList]);
  const refusal = problems.length > 0 ? problems.map(problem => problem.message).join('; ') : null;
  const problemAt = (field: string) => problems.some(problem => problem.field === field);
  const managed = useDraftSection('framing', {
    label: 'Framing',
    order: 1,
    unsaved: heldReport?.unsaved ?? reported.unsaved,
    pending: canWrite && firstSave,
    changes: heldReport?.changes ?? reported.changes,
    save: async () => {
      if (stale) return 'the framing changed elsewhere; reload it first';
      if (refusal) return refusal;
      if (!state || !draft.data) return false;
      setNotice('');
      setProblem('');
      try { await save.mutateAsync(state); } catch (error) { return message(error); }
      return true;
    },
    discard: () => {
      setState(baseline ?? savedState);
      setUndo(null);
      setNotice('');
      save.reset();
    },
  });

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
    // The rig being framed on its own comes first: a drag on its grid moves
    // its own center, and detaches it from the shared one if it followed it.
    // A rig that is off, or no longer listed, is drawn dim and stays put.
    const own = ownRigsRef.current.find(entry => entry.rigId === editingRef.current && rigLive(entry.rigId));
    let rig: string | null = null;
    if (own && kind === 'look' && !event.shiftKey && dragMode === 'rectangle' && own.geometry.panels.some(panel => { const corners = stageCorners(panel.corners, stageView); return corners && insidePolygon(point, corners, state.viewFov, stageSize); })) {
      kind = 'target';
      rig = own.rigId;
    } else if (geometry && geometry.panels.length > 0 && sharedInUse) {
      // A hidden shared framing has no handle to take hold of.
      const handleAt = projectOn(stageView, rotationHandleSky(state, geometry.extent));
      const handle = handleAt ? toStage(handleAt, state.viewFov, stageSize) : null;
      if (handle && Math.hypot(handle[0] - point[0], handle[1] - point[1]) <= 18) kind = 'rotate';
      else if (kind === 'look' && !event.shiftKey && dragMode === 'rectangle' && geometry.panels.some(panel => { const corners = stageCorners(panel.corners, stageView); return corners && insidePolygon(point, corners, state.viewFov, stageSize); })) kind = 'target';
    }
    // Where the pointer took hold, relative to the target, so the target
    // follows the hand instead of jumping to it.
    const pointerOffset = fromStage(point[0], point[1], state.viewFov, stageSize);
    const target = own && rig ? own.layout.center : state.center;
    const targetOffset = projectOn(stageView, target) ?? [0, 0];
    drag.current = { kind, rig, x: event.clientX, y: event.clientY, view: stageView, center: state.viewCenter, target, grab: [pointerOffset[0] - targetOffset[0], pointerOffset[1] - targetOffset[1]] };
    // The save bar keeps what it said until the drag ends: a bar appearing
    // mid-drag would push the stage down under the pointer.
    setHeldReport({ unsaved: reported.unsaved, changes: reported.changes });
    event.currentTarget.setPointerCapture(event.pointerId);
  };
  const onPointerMove = (event: ReactPointerEvent<HTMLDivElement>) => {
    if (!drag.current || !state) return;
    const { view } = drag.current;
    if (drag.current.kind === 'look' || drag.current.kind === 'sky') {
      const scale = pixelScale(state.viewFov, stageSize) * stageScale();
      const dx = (event.clientX - drag.current.x) * scale;
      const dy = (event.clientY - drag.current.y) * scale;
      if (drag.current.kind === 'look') {
        // Slide the window over the plane from where the drag began; the target stays.
        update({ viewCenter: deprojectOn(viewAt(drag.current.center, drag.current.center, view.rotation), [dx, dy]) });
      } else {
        // Move the sky: the view turns and the target goes with it, keeping
        // its place on the stage, as N.I.N.A. does with a pinned rectangle.
        const from = viewAt(drag.current.center, drag.current.center, view.rotation);
        const targetAt = projectOn(from, drag.current.target) ?? [0, 0];
        const viewCenter = deprojectOn(from, [dx, dy]);
        update({ viewCenter, center: deprojectOn(viewAt(viewCenter, viewCenter, view.rotation), targetAt) });
      }
      return;
    }
    const point = stagePoint(event);
    const pointerOffset = fromStage(point[0], point[1], state.viewFov, stageSize);
    if (drag.current.kind === 'target') {
      const { grab, rig } = drag.current;
      const center = deprojectOn(view, [pointerOffset[0] - grab[0], pointerOffset[1] - grab[1]]);
      if (rig) update(current => ({ rigFramings: current.rigFramings.map(own => own.rig_id === rig ? { ...own, center } : own) }));
      else update({ center });
    } else {
      // The angle is read on the target's plane, where the camera angle
      // lives, so the handle and the rectangle agree wherever the view is.
      const at = deprojectOn(view, pointerOffset);
      update({ positionAngle: normaliseAngle(Math.round(angleAt(state.center, at) * 10) / 10) });
    }
  };
  const onPointerUp = () => { drag.current = null; setHeldReport(null); };
  const turn = (delta: number) => update(current => ({ positionAngle: normaliseAngle(current.positionAngle + delta) }));
  const zoomBy = (factor: number) => update(current => ({ viewFov: clampFov(current.viewFov * factor) }));
  useEffect(() => {
    const element = stageElement;
    if (!element) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      // The zoom follows how far the wheel turned: a mouse notch is about
      // 100 pixels and zooms 1.2 times, and a trackpad's many small steps
      // add up to the same for the same distance.
      const pixels = event.deltaY * (event.deltaMode === 1 ? 16 : event.deltaMode === 2 ? element.clientHeight || 800 : 1);
      const factor = Math.exp(Math.max(-Math.LN2, Math.min(Math.LN2, pixels * WHEEL_ZOOM_PER_PIXEL)));
      update(current => ({ viewFov: clampFov(current.viewFov * factor) }));
    };
    element.addEventListener('wheel', onWheel, { passive: false });
    return () => element.removeEventListener('wheel', onWheel);
  }, [update, stageElement]);

  const survey = surveys.data?.find(entry => entry.id === state?.surveyId);
  const panelRig = rigList.find(entry => entry.rig.id === state?.panelRigId);
  // Rigs framed on their own: their grids over the same target, each in its own colour.
  const ownRigs = useMemo(() => state ? rigGeometries(state, rigList) : [], [state, rigList]);
  const [editingRig, setEditingRig] = useState<string | null>(null);
  const ownRigsRef = useRef(ownRigs);
  ownRigsRef.current = ownRigs;
  const editingRef = useRef<string | null>(null);
  editingRef.current = editingRig;
  const editing = state?.rigFramings.find(own => own.rig_id === editingRig) ?? null;
  const rigName = (id: string) => rigList.find(entry => entry.rig.id === id)?.catalog_name ?? 'Rig';
  const editOwn = (patch: Partial<DirectorRigFraming>) => update(current => ({ rigFramings: current.rigFramings.map(own => own.rig_id === editingRig ? { ...own, ...patch } : own) }));
  const frameRig = (id: string) => {
    if (!id) return;
    update(current => current.rigFramings.some(own => own.rig_id === id) ? {} : ({ rigFramings: [...current.rigFramings, { rig_id: id, center: null, position_angle_degrees: null, mosaic: { ...current.mosaic }, panel: panelForRig(rigList, id) ? null : current.panel }] }));
    setEditingRig(id);
  };
  const chooseRig = (rigId: string) => update({ panelRigId: rigId || null, panel: rigId ? panelForRig(rigList, rigId) : null });
  const number = (value: string, fallback: number) => { const parsed = Number(value); return Number.isFinite(parsed) ? parsed : fallback; };

  // The rigs that shoot this plan come first; the rest fold away.
  const inPlan = (id: string) => preferredRigIds.includes(id) || (shootingRigIds ?? []).includes(id);
  const planRigs = rigList.filter(entry => inPlan(entry.rig.id));
  const otherRigs = rigList.filter(entry => !inPlan(entry.rig.id));
  const onRigs = rigList.filter(entry => (shootingRigIds ?? []).includes(entry.rig.id));
  const offRigs = rigList.filter(entry => !(shootingRigIds ?? []).includes(entry.rig.id));
  const orphans = state?.rigFramings.filter(own => !rigList.some(entry => entry.rig.id === own.rig_id)) ?? [];
  if (draft.isPending) return <p role="status">Loading framing...</p>;
  // A refetch that fails keeps the framing on screen, edits and all, under
  // a line that offers to try again; only a first load has nothing to show.
  if (draft.isError && !draft.data) return <p className="director-error" role="alert">{message(draft.error)} <button type="button" onClick={() => void draft.refetch()}>Retry</button></p>;
  if (!state) return <StartFraming canWrite={canWrite} onStart={setStarted} />;
  geometryRef.current = geometry;
  const ownEditor = editing && (() => {
          const own = ownRigs.find(entry => entry.rigId === editing.rig_id);
          const field = panelForRig(rigList, editing.rig_id);
          const size = editing.panel ?? field;
          const center = editing.center ?? state.center;
          const at = (field: string) => problemAt(`rig:${editing.rig_id}:${field}`);
          const said = problems.filter(problem => problem.field.startsWith(`rig:${editing.rig_id}:`));
          return <div className="framing-own" data-testid="framing-own-rig">
            <div className="framing-grid">
              <label>RA<span className="framing-input"><NumberInput aria-label={`${rigName(editing.rig_id)} right ascension degrees`} step="any" min={0} max={359.99999} value={Number(center.ra_degrees.toFixed(5))} onChange={event => editOwn({ center: { ra_degrees: normaliseAngle(number(event.target.value, center.ra_degrees)), dec_degrees: center.dec_degrees } })} /><small>°</small></span></label>
              <label>Dec<span className="framing-input"><NumberInput aria-label={`${rigName(editing.rig_id)} declination degrees`} step="any" min={-90} max={90} value={Number(center.dec_degrees.toFixed(5))} onChange={event => editOwn({ center: { ra_degrees: center.ra_degrees, dec_degrees: Math.max(-90, Math.min(90, number(event.target.value, center.dec_degrees))) } })} /><small>°</small></span></label>
            </div>
            <p className="director-muted"><label className="framing-check"><input type="checkbox" aria-label={`${rigName(editing.rig_id)} follows the shared center`} checked={editing.center === null} onChange={event => editOwn({ center: event.target.checked ? null : state.center })} />Shared center</label>
              {editing.center && <> <button type="button" className="link-button" onClick={() => editOwn({ center: state.viewCenter })}>Move to view center</button></>}
              </p>
            <label>Camera angle<span className="framing-input">
              <NumberInput aria-label={`${rigName(editing.rig_id)} camera angle degrees`} step="any" min={0} max={359.99} value={Number((editing.position_angle_degrees ?? state.positionAngle).toFixed(2))} onChange={event => editOwn({ position_angle_degrees: normaliseAngle(number(event.target.value, 0)) })} /><small>° E of N</small>
              <label className="framing-check"><input type="checkbox" aria-label={`${rigName(editing.rig_id)} follows the shared angle`} checked={editing.position_angle_degrees === null} onChange={event => editOwn({ position_angle_degrees: event.target.checked ? null : state.positionAngle })} />Shared angle</label>
            </span></label>
            <div className="framing-grid">
              <label>Panel width<span className="framing-input"><NumberInput aria-label={`${rigName(editing.rig_id)} panel width degrees`} aria-invalid={at('panel') || undefined} step="any" min={0.01} max={30} value={size?.width_degrees ?? ''} onChange={event => editOwn({ panel: { width_degrees: number(event.target.value, size?.width_degrees ?? 1), height_degrees: size?.height_degrees ?? 1 } })} /><small>°</small></span></label>
              <label>Panel height<span className="framing-input"><NumberInput aria-label={`${rigName(editing.rig_id)} panel height degrees`} aria-invalid={at('panel') || undefined} step="any" min={0.01} max={30} value={size?.height_degrees ?? ''} onChange={event => editOwn({ panel: { width_degrees: size?.width_degrees ?? 1, height_degrees: number(event.target.value, size?.height_degrees ?? 1) } })} /><small>°</small></span></label>
            </div>
            <p className="director-muted">{editing.panel ? <>Typed size {field && <button type="button" className="link-button" onClick={() => editOwn({ panel: null })}>Use rig field</button>}</> : field ? `Rig field ${formatDegrees(field.width_degrees)} × ${formatDegrees(field.height_degrees)}` : 'No optics: type a size'}</p>
            <div className="framing-grid">
              <label>Rows<NumberInput aria-label={`${rigName(editing.rig_id)} rows`} aria-invalid={at('mosaic') || undefined} min={1} max={16} step={1} value={editing.mosaic.rows} onChange={event => editOwn({ mosaic: { ...editing.mosaic, rows: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } })} /></label>
              <label>Columns<NumberInput aria-label={`${rigName(editing.rig_id)} columns`} aria-invalid={at('mosaic') || undefined} min={1} max={16} step={1} value={editing.mosaic.columns} onChange={event => editOwn({ mosaic: { ...editing.mosaic, columns: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } })} /></label>
              <label>Overlap<span className="framing-input"><NumberInput aria-label={`${rigName(editing.rig_id)} overlap percent`} aria-invalid={at('mosaic') || undefined} min={0} max={90} step={1} value={editing.mosaic.overlap_percent} onChange={event => editOwn({ mosaic: { ...editing.mosaic, overlap_percent: Math.max(0, Math.min(90, Math.round(number(event.target.value, 0)))) } })} /><small>%</small></span></label>
            </div>
            <p className="director-muted" data-testid="framing-own-extent">{own ? `${own.geometry.panels.length} panel${own.geometry.panels.length === 1 ? '' : 's'} · ${formatDegrees(own.geometry.extent.width_degrees)} × ${formatDegrees(own.geometry.extent.height_degrees)}` : 'Type a size'}</p>
            {said.map(problem => <p key={problem.field} className="framing-problem">{problem.message}</p>)}
          </div>;
  })();
  /** One rig: its outline colour, its field, and what it shoots. */
  const rigCard = (entry: DirectorRigProfileSummary) => {
    const id = entry.rig.id;
    const own = state.rigFramings.find(framing => framing.rig_id === id);
    const ownIndex = ownRigs.findIndex(framed => framed.rigId === id);
    const sizes = state.panelRigId === id;
    const shown = state.shownRigIds.includes(id);
    const field = entry.field_of_view;
    const grid = (m: DirectorRigFraming['mosaic']) => m.rows * m.columns > 1 ? `${m.rows} × ${m.columns}, ${m.overlap_percent}%` : '1 panel';
    const shooting = shootingRigIds?.includes(id) ?? false;
    const off = !!onToggleRig && !shooting;
    const role = own
      ? `Separate framing · ${grid(own.mosaic)} · ${round(own.position_angle_degrees ?? state.positionAngle, 1)}°${own.center ? ' · moved center' : ''}`
      : sizes ? `Frames the plan · ${grid(state.mosaic)}`
      : field ? `Shared framing · ${grid(state.mosaic)}` : 'No optics';
    const swatch = own ? `is-own-${ownIndex % 4}` : sizes ? 'is-shared' : shown ? 'is-compared' : 'is-none';
    const blocked = !shooting && !!joinBlocked;
    // A rig that is off shoots nothing; it may still hold the panel size
    // until a rig that is on can take it.
    const shownRole = off && !own ? (sizes ? 'Not in this plan · sets the panel size' : 'Not in this plan') : off ? `Not in this plan · ${role}` : role;
    // Only a rig that is on frames the plan, on a page that keeps one.
    const canFrame = !own && !!field && !off;
    return <li key={id} className={`framing-rig-card${editingRig === id ? ' is-editing' : ''}${sizes && !own && !off ? ' is-framing' : ''}`} role="group" aria-label={entry.catalog_name}>
      <div className="framing-rig-head">
        {onToggleRig && <button type="button" role="switch" className="framing-rig-switch" aria-checked={shooting}
          aria-label={`${entry.catalog_name} on`} title={blocked ? joinBlocked : shooting ? 'Shoots this plan' : 'Off: not in this plan'}
          disabled={!canWrite || blocked} onClick={() => {
            const joined = onToggleRig(id, !shooting) !== false;
            // A rig turned on frames the plan at once, as though its heading
            // were clicked too; one that could not join frames nothing.
            if (!shooting && joined && field && !own) chooseRig(id);
          }}><span aria-hidden="true" /></button>}
        {(() => {
          // Swatch, name and field are one click target that frames the
          // plan with this rig; the framing rig's card is highlighted.
          const inner = <>
            <span className={`framing-rig-swatch ${swatch}`} aria-hidden="true" />
            <strong>{entry.catalog_name}</strong>
            {field && <small>{formatDegrees(field.width_degrees)} × {formatDegrees(field.height_degrees)}, {field.pixel_scale_arcsec.toFixed(2)}″/px</small>}
          </>;
          return canFrame
            ? <button type="button" className="framing-rig-pick" aria-pressed={sizes}
                aria-label={sizes ? `${entry.catalog_name} frames the plan` : `Frame with ${entry.catalog_name}`}
                title={sizes ? 'Frames the plan' : 'Frame with this rig'} disabled={!canWrite || sizes} onClick={() => chooseRig(id)}>{inner}</button>
            : <span className="framing-rig-pick is-static">{inner}</span>;
        })()}
      </div>
      <p className="framing-rig-role">{shownRole}</p>
      {canWrite && <div className="framing-rig-actions">
        {!own && field && <label className="framing-check" title="Draw its field on the sky"><input type="checkbox" aria-label={`${entry.catalog_name} outline`} checked={shown} onChange={event => update(current => ({ shownRigIds: event.target.checked ? [...current.shownRigIds, id] : current.shownRigIds.filter(other => other !== id) }))} />Outline</label>}
        {!own && <button type="button" className="link-button" onClick={() => frameRig(id)}>Frame separately</button>}
        {own && <button type="button" className="link-button" aria-expanded={editingRig === id} onClick={() => setEditingRig(editingRig === id ? null : id)}>{editingRig === id ? 'Done' : 'Edit'}</button>}
        {own && <button type="button" className="link-button" onClick={() => { update(current => ({ rigFramings: current.rigFramings.filter(framing => framing.rig_id !== id) })); if (editingRig === id) setEditingRig(null); }}>Use shared</button>}
      </div>}
      {editingRig === id && ownEditor}
    </li>;
  };
  const view = stageView ?? viewAt(state.center, state.viewCenter);
  const onStage = (position: { ra_degrees: number; dec_degrees: number }) => { const offset = projectOn(view, position); return offset ? toStage(offset, state.viewFov, stageSize) : null; };
  const handle = sharedInUse && geometry && geometry.panels.length > 0 ? onStage(rotationHandleSky(state, geometry.extent)) : null;
  const centerOnStage = geometry ? onStage(state.center) : null;
  const panelPolygons = sharedInUse ? (geometry?.panels ?? []).map(panel => ({ panel, corners: stageCorners(panel.corners, view) })) : [];
  const chips = chipSurveys(surveys.data ?? []);
  // The layer's short chip name, so a note on the sky stays a few words.
  const layerLabel = chips.find(chip => chip.survey.id === state.surveyId)?.label ?? survey?.name;
  return <section className="framing" aria-label="Framing">
    {draft.isError && <p className="director-error framing-banner" role="alert">The framing could not be reloaded: {message(draft.error)} <button type="button" onClick={() => void draft.refetch()}>Retry</button></p>}
    <div className="framing-stage-wrap">
      {/* The sky and the line under it share the room the page gives them. */}
      <div className="framing-sky-box">
      <div ref={stageRef} className={`framing-stage${cutout.stale ? ' is-stale' : ''}`} role="img" aria-label="Sky view" data-testid="framing-stage"
        onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        {!pictureUp && <div className="framing-stage-empty" />}
        {webgl && view && <SkyCanvas className={`framing-sky${cutout.image ? '' : ' is-empty'}`} tiles={skyTiles} view={view} viewFov={state.viewFov} stage={stageSize} onUnsupported={() => setWebgl(false)} onLost={setSkyLost} />}
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
          {marks.data && marksWanted && <SkyMarks marks={marks.data} view={view} viewFov={state.viewFov} stage={stageSize} objects={showObjects} bodies={showBodies} solar={showSolar} />}
          {placedStacks.map(({ panel, preview, matrix }) => <image key={`${panel.rig.id}-${panel.panel_id}`} className="framing-stack" data-testid="framing-stack" href={preview.url} x={0} y={0} width={preview.width} height={preview.height} preserveAspectRatio="none" transform={matrix} />)}
          {geometry?.overlays.map(overlay => { const corners = stageCorners(overlay.corners, view); return corners && <g key={overlay.id} className="framing-overlay-group">
            <polygon className="framing-overlay" points={polygonPoints(corners, state.viewFov, stageSize)} />
            <text className="framing-overlay-label" x={toStage(corners[0], state.viewFov, stageSize)[0] + 6} y={toStage(corners[0], state.viewFov, stageSize)[1] + 14}>{rigName(overlay.id)}</text>
          </g>; })}
          {ownRigs.map(({ rigId, geometry: own }, index) => <g key={rigId} className={`framing-rig-panels framing-rig-${index % 4}${rigId === editingRig ? ' is-editing' : ''}${rigLive(rigId) ? '' : ' is-off'}`} data-testid="framing-rig-panels" data-rig={rigId}>
            {own.panels.map(panel => { const corners = stageCorners(panel.corners, view); return corners && <g key={panel.id}>
              <polygon points={polygonPoints(corners, state.viewFov, stageSize)} />
              <text x={toStage(corners[0], state.viewFov, stageSize)[0] + 8} y={toStage(corners[0], state.viewFov, stageSize)[1] + 20}>{rigName(rigId)}{own.panels.length > 1 ? ` ${panel.id}` : ''}</text>
            </g>; })}
          </g>)}
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
        {/* The tools and the notes under them share one column, so a note
            always sits below the tools however many rows they wrap to. */}
        <div className="framing-stage-top">
        <div className="framing-stage-tools" onPointerDown={event => event.stopPropagation()}>
          <div className="framing-seg" role="group" aria-label="A drag moves">
            <button type="button" aria-pressed={dragMode === 'rectangle'} aria-label="Drag moves the rectangle" title="A drag moves the rectangle over a still sky. Drag its handle to turn the camera; drag the sky to look around." onClick={() => chooseDragMode('rectangle')}><SquareDashedMousePointer size={15} /><span>Rectangle</span></button>
            <button type="button" aria-pressed={dragMode === 'sky'} aria-label="Drag moves the sky" title="A drag moves the sky under a pinned rectangle, and the target with it. Drag the handle to turn the camera; Shift-drag to look around without moving the target." onClick={() => chooseDragMode('sky')}><Globe size={15} /><span>Sky</span></button>
          </div>
          <div className="framing-seg framing-seg-icons" role="group" aria-label="Sky layers">
            <button type="button" aria-pressed={rotateSky} aria-label="Turn the sky with the camera" title="Turn the sky with the camera, so the rectangle stands upright" onClick={() => chooseRotateSky(!rotateSky)}><RotateCw size={15} /></button>
            <button type="button" aria-pressed={showGrid} aria-label="Equatorial grid" title="Equatorial grid" onClick={() => setShowGrid(!showGrid)}><Grid3x3 size={15} /></button>
            <button type="button" aria-pressed={showChart} aria-label="Constellations" title="Constellation figures and names when zoomed out" onClick={() => setShowChart(!showChart)}><Sparkles size={15} /></button>
            <button type="button" aria-pressed={showObjects} aria-label="Deep-sky marks" title="Deep-sky marks from the object catalog" onClick={() => chooseMarks('objects', !showObjects)}><Telescope size={15} /></button>
            <button type="button" aria-pressed={showBodies} aria-label="Comets and asteroids" title="Comets and asteroids where they are now" onClick={() => chooseMarks('bodies', !showBodies)}><Orbit size={15} /></button>
            <button type="button" aria-pressed={showSolar} aria-label="Sun, Moon and planets" title="Sun, Moon and planets where they are now" onClick={() => chooseMarks('solar', !showSolar)}><Sun size={15} /></button>
            {stackChoices.length > 0 && <button type="button" aria-pressed={showStacks} aria-label="Finished stacks" title="Finished stacks, placed by their plate solves" onClick={() => chooseShowStacks(!showStacks)}><Images size={15} /></button>}
          </div>
        </div>
        <div className="framing-stage-status">
          {cutout.status === 'loading' && <span role="status">Loading {layerLabel ?? 'survey'}...</span>}
          {cutout.status === 'failed' && <span role="alert">{cutout.error}</span>}
          {/* Notes stay a few words, so they keep clear of the sky; the reason is in the title. */}
          {marks.data && showObjects && !marks.data.objects.available && <span role="note" title={`Deep-sky marks need the Seiza object catalog on this server${marks.data.objects.note ? ` (${marks.data.objects.note})` : ''}.`}>No deep-sky catalog</span>}
          {marks.data && showBodies && !marks.data.minor_bodies.available && <span role="note" title={`Comets and asteroids need the Seiza minor-body catalog on this server${marks.data.minor_bodies.note ? ` (${marks.data.minor_bodies.note})` : ''}.`}>No comet catalog</span>}
          {marks.isError && marksWanted && <span role="alert">Marks could not be loaded: {message(marks.error)}</span>}
        </div>
        </div>
        <div className="framing-stage-zoom" onPointerDown={event => event.stopPropagation()}>
          <button type="button" aria-label="Zoom in" title="Zoom in" onClick={() => zoomBy(1 / 1.5)}>+</button>
          <button type="button" aria-label="Zoom out" title="Zoom out" onClick={() => zoomBy(1.5)}>−</button>
          {geometry && <button type="button" aria-label="Fit the footprint" title="Fit the footprint" onClick={() => fitView(false)}>⌖</button>}
        </div>
        <div className="framing-stage-scale">{formatDegrees(trueWidth(state.viewFov))} across · {skyRotation === 0 ? 'N up, E left' : `sky turned ${skyRotation.toFixed(1)}°`}</div>
        {/* On a narrow stage the chips wait behind one, so they do not
            climb over the sky and the scale. */}
        <div className={`framing-stage-surveys${narrow ? ' is-narrow' : ''}`} onPointerDown={event => event.stopPropagation()}>
          {narrow && <button type="button" className="framing-layers-chip" aria-expanded={layersOpen} aria-label="Survey layers" title="Survey layers" onClick={() => setLayersOpen(!layersOpen)}>
            <Layers size={14} aria-hidden="true" /><span>{layerLabel ?? 'Layers'}</span></button>}
          {(!narrow || layersOpen) && <div className="framing-stage-chips" role="group" aria-label="Survey layers">
            {chips.map(({ survey: entry, label }) => <button key={entry.id} type="button" aria-pressed={entry.id === state.surveyId} title={`${entry.name}: ${entry.bandpass}`} onClick={() => { setLayersOpen(false); chooseSurvey(entry.id); }}>{label}</button>)}
          </div>}
        </div>
      </div>
      <p className="framing-readout" data-testid="framing-readout">
        <span className="director-muted" data-testid="framing-view-center">view {formatRaHours(state.viewCenter.ra_degrees)}, {formatDec(state.viewCenter.dec_degrees)}</span>
      </p>
      </div>
      {mosaic.data && mosaic.data.panels.length > 0 && <div className="framing-stacks" data-testid="framing-stacks">
        {stackChoices.length > 0 && <div className="framing-stacks-pick">
          <label className="framing-check"><input type="checkbox" checked={showStacks} onChange={event => chooseShowStacks(event.target.checked)} />Stacks</label>
          <select aria-label="Stack shown" value={stackChoice} disabled={!showStacks} onChange={event => chooseStack(event.target.value)}>
            <option value="">Best per panel</option>
            {stackChoices.map(stack => <option key={stack.key} value={stack.key}>{stackName(stack)}</option>)}
          </select>
        </div>}
        {mosaic.data.framing_stale && <p className="director-muted">The framing changed since the last activation. Stacks sit where their solves put them; the rectangles are the new plan.</p>}
        {mosaic.data.warnings.map(warning => <p key={warning} className="director-muted">{warning}</p>)}
        <ul>{mosaic.data.panels.map(panel => <li key={`${panel.rig.id}-${panel.panel_id}`}>{describeStack(panel, stackChoice, stackChoiceName ? stackName(stackChoiceName) : '')}</li>)}</ul>
      </div>}
      <p className="director-muted framing-attribution">{survey ? `${survey.name}: ${survey.bandpass}. ${survey.attribution}.` : 'Choose a survey.'} For composition only.</p>
      <VisibilityPanel projectId={projectId} center={state.center} rigCenters={liveRigCenters} compact />
    </div>
    <form className="framing-controls" onSubmit={event => { event.preventDefault(); if (canWrite && !save.isPending && !stale && !refusal) { setNotice(''); setProblem(''); save.mutate(state); } }}>
      <fieldset>
        <legend>Target</legend>
        <TargetSearch label="Find a target" ariaLabel="Find a target" placeholder="M 31, NGC 7000, Heart Nebula" buttonLabel="Go" disabled={!canWrite}
          hint="The local catalog answers as you type; the last row asks CDS Sesame (Simbad, NED, VizieR)." onPick={pickTarget} />
        <label>Name<input aria-label="Target name" value={state.targetName} maxLength={256} onChange={event => update({ targetName: event.target.value })} /></label>
        <div className="framing-grid">
          <label>RA<span className="framing-input"><NumberInput aria-label="Right ascension degrees" aria-invalid={problemAt('center') || undefined} step="any" min={0} max={359.99999} value={round(state.center.ra_degrees, 5)} onChange={event => update(current => ({ center: { ...current.center, ra_degrees: normaliseAngle(number(event.target.value, current.center.ra_degrees)) } }))} /><small>°</small></span><small>{formatRaHours(state.center.ra_degrees)}</small></label>
          <label>Dec<span className="framing-input"><NumberInput aria-label="Declination degrees" aria-invalid={problemAt('center') || undefined} step="any" min={-90} max={90} value={round(state.center.dec_degrees, 5)} onChange={event => update(current => ({ center: { ...current.center, dec_degrees: Math.max(-90, Math.min(90, number(event.target.value, current.center.dec_degrees))) } }))} /><small>°</small></span><small>{formatDec(state.center.dec_degrees)}</small></label>
          <label className="framing-span">Camera angle<span className="framing-input"><NumberInput aria-label="Position angle degrees" step="any" min={0} max={359.99} value={round(state.positionAngle, 2)} onChange={event => update({ positionAngle: normaliseAngle(number(event.target.value, state.positionAngle)) })} /><small>° E of N</small></span>
            <span className="framing-turns"><button type="button" aria-label="Turn 90 degrees counter-clockwise" onClick={() => turn(-90)}>−90°</button><button type="button" aria-label="Turn 90 degrees clockwise" onClick={() => turn(90)}>+90°</button>
              {panelRig?.profile?.optics && panelRig.profile.optics.value.rotation.mode !== 'rotator' && <button type="button" onClick={() => update({ positionAngle: normaliseAngle((panelRig.profile!.optics!.value.rotation as { angle_degrees: number }).angle_degrees) })}>Rig's camera angle</button>}</span></label>
        </div>
        <div className="director-actions">
          <button type="button" onClick={() => update(current => ({ viewCenter: current.center }))}><Crosshair size={16} />Center view on target</button>
          <button type="button" disabled={!canWrite} onClick={() => update(current => ({ center: current.viewCenter }))}><LocateFixed size={16} />Move target to view center</button>
          {seed && <button type="button" disabled={!canWrite} onClick={() => update({ targetName: seed.name, center: seed.center, positionAngle: seed.position_angle_degrees, viewCenter: seed.center })}><Undo2 size={16} />Back to catalog target</button>}
          {savedState && <button type="button" disabled={!differsFromSaved} title="Drop every change since the last save" onClick={() => { setState(savedState); setUndo(null); setNotice(`Back to saved framing revision ${draft.data?.draft?.revision ?? 0}.`); }}><Undo2 size={16} />Back to saved framing</button>}
        </div>
      </fieldset>
      <fieldset className="framing-rigs">
        <legend>Rigs</legend>
        {rigs.isError && <p className="director-error" role="alert">Rigs could not be loaded: {message(rigs.error)} <button type="button" onClick={() => void rigs.refetch()}>Retry</button></p>}
        {!rigs.isError && rigList.length === 0 && <p className="director-muted">{rigs.isPending ? 'Loading rigs...' : 'No rigs yet'}</p>}
        {onToggleRig ? <>
          {/* On a page that keeps the plan: rigs that shoot it, then the rest. */}
          <h4 className="framing-rig-group">On</h4>
          {onRigs.length > 0 ? <ul className="framing-rig-list">{onRigs.map(rigCard)}</ul> : <p className="director-muted">No rigs on</p>}
          {offRigs.length > 0 && <><h4 className="framing-rig-group">Off</h4><ul className="framing-rig-list">{offRigs.map(rigCard)}</ul></>}
        </> : <>
          {planRigs.length > 0 && <ul className="framing-rig-list">{planRigs.map(rigCard)}</ul>}
          {otherRigs.length > 0 && <details className="framing-other-rigs" open>
            <summary>Other rigs ({otherRigs.length})</summary>
            <ul className="framing-rig-list">{otherRigs.map(rigCard)}</ul>
          </details>}
        </>}
        {/* A rig gone from the list has no card; its framing is listed so it can go. */}
        {rigs.isSuccess && orphans.length > 0 && <ul className="framing-orphans" aria-label="Framings of rigs not listed">
          {orphans.map(own => <li key={own.rig_id}>
            <span>Rig not listed · separate framing, {own.mosaic.rows} × {own.mosaic.columns}</span>
            {canWrite && <button type="button" className="link-button" onClick={() => update(current => ({ rigFramings: current.rigFramings.filter(framing => framing.rig_id !== own.rig_id) }))}>Remove</button>}
          </li>)}
        </ul>}
      </fieldset>
      <fieldset>
        <legend>Shared framing <span className="framing-legend-swatch" title="Its outline on the sky" aria-hidden="true" /></legend>
        {/* The panel size comes from the rig picked by its swatch, or is typed. */}
        {/* The size shown is the one the rectangle uses, copied from the
            rig when it was picked; the rig's field may have moved since. */}
        <p className="framing-panel-source" data-testid="framing-panel-source">
          {(() => {
            if (!state.panelRigId) return <>Typed size{rigList.some(entry => entry.field_of_view) ? ' · or pick a rig by its swatch' : ''}</>;
            if (!panelRig) return rigs.isPending ? 'Loading the panel rig…' : 'The saved panel rig is no longer listed; pick a rig or type a size.';
            const field = panelForRig(rigList, panelRig.rig.id);
            const off = !!onToggleRig && !(shootingRigIds ?? []).includes(panelRig.rig.id);
            const moved = !!field && !!state.panel && (Math.abs(field.width_degrees - state.panel.width_degrees) > 1e-9 || Math.abs(field.height_degrees - state.panel.height_degrees) > 1e-9);
            return <>Size from <strong>{panelRig.catalog_name}</strong>{off ? ' (off)' : ''}
              {state.panel && ` · ${formatDegrees(state.panel.width_degrees)} × ${formatDegrees(state.panel.height_degrees)}`}{field ? '' : ' · no optics now'}
              {canWrite && moved && <> <button type="button" className="link-button" title={`The rig's field is now ${formatDegrees(field!.width_degrees)} × ${formatDegrees(field!.height_degrees)}`} onClick={() => update({ panel: field })}>Use current field</button></>}
              {canWrite && <> <button type="button" className="link-button" onClick={() => update(current => ({ panelRigId: null, panel: current.panel }))}>Type a size</button></>}</>;
          })()}
        </p>
        {!panelRig && <div className="framing-grid">
          <label>Panel width<span className="framing-input"><NumberInput aria-label="Panel width degrees" aria-invalid={problemAt('panel') || undefined} step="any" min={0.01} max={30} value={state.panel ? round(state.panel.width_degrees, 3) : ''} onChange={event => update(current => ({ panel: { width_degrees: number(event.target.value, current.panel?.width_degrees ?? 1), height_degrees: current.panel?.height_degrees ?? 1 } }))} /><small>°</small></span></label>
          <label>Panel height<span className="framing-input"><NumberInput aria-label="Panel height degrees" aria-invalid={problemAt('panel') || undefined} step="any" min={0.01} max={30} value={state.panel ? round(state.panel.height_degrees, 3) : ''} onChange={event => update(current => ({ panel: { width_degrees: current.panel?.width_degrees ?? 1, height_degrees: number(event.target.value, current.panel?.height_degrees ?? 1) } }))} /><small>°</small></span></label>
        </div>}
        <div className="framing-grid">
          <label>Rows<NumberInput aria-label="Mosaic rows" aria-invalid={problemAt('mosaic') || undefined} min={1} max={16} step={1} value={state.mosaic.rows} onChange={event => update(current => ({ mosaic: { ...current.mosaic, rows: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } }))} /></label>
          <label>Columns<NumberInput aria-label="Mosaic columns" aria-invalid={problemAt('mosaic') || undefined} min={1} max={16} step={1} value={state.mosaic.columns} onChange={event => update(current => ({ mosaic: { ...current.mosaic, columns: Math.max(1, Math.min(16, Math.round(number(event.target.value, 1)))) } }))} /></label>
          <label>Overlap<span className="framing-input"><NumberInput aria-label="Panel overlap percent" aria-invalid={problemAt('mosaic') || undefined} min={0} max={90} step={1} value={state.mosaic.overlap_percent} onChange={event => update(current => ({ mosaic: { ...current.mosaic, overlap_percent: Math.max(0, Math.min(90, Math.round(number(event.target.value, 0)))) } }))} /><small>%</small></span></label>
        </div>
        <p className="director-muted" data-testid="framing-extent">{geometry ? `${geometry.panels.length} panel${geometry.panels.length === 1 ? '' : 's'} · ${formatDegrees(geometry.extent.width_degrees)} × ${formatDegrees(geometry.extent.height_degrees)}` : 'Pick a rig or type a size'}</p>
        {problems.filter(problem => !problem.field.startsWith('rig:')).map(problem => <p key={problem.field} className="framing-problem" data-testid="framing-problem">{problem.message}</p>)}
      </fieldset>
      <fieldset>
        <legend>View</legend>
        <div className="framing-grid">
          <label>Survey
            <select aria-label="Survey" value={state.surveyId} onChange={event => chooseSurvey(event.target.value)}>
              {(surveys.data ?? []).map(entry => <option key={entry.id} value={entry.id}>{entry.name}{entry.kind === 'narrowband' ? ' (narrowband)' : ''}</option>)}
            </select>
          </label>
          <label>Width of view<span className="framing-input"><NumberInput aria-label="View width degrees" step="any" min={MIN_VIEW_FOV} max={MAX_VIEW_FOV} value={round(state.viewFov, 2)} onChange={event => update({ viewFov: clampFov(number(event.target.value, state.viewFov)) })} /><small>°</small>
            {geometry && <button type="button" onClick={() => fitView(false)}>Fit</button>}</span></label>
        </div>
        <div className="framing-catalogs" role="group" aria-label="Catalogs marked">
          <span className="framing-catalogs-title">Catalogs marked</span>
          {CATALOG_FAMILIES.map(family => { const shown = shownCatalogs.some(entry => entry.toLowerCase() === family.prefix.toLowerCase()); return <button key={family.prefix} type="button" aria-pressed={shown} title={family.title} onClick={() => toggleCatalog(family.prefix)}>{family.label}</button>; })}
        </div>
      </fieldset>
      {notice && <p role="status">{notice}{undo && <> <button type="button" className="link-button" onClick={undoPick}>Undo</button></>}</p>}
      {stale && <p className="director-error" role="alert">This framing changed since you loaded it. Reload to see the saved draft before editing again.</p>}
      {(problem || (save.isError && !stale)) && <p className="director-error" role="alert">{problem || message(save.error)}</p>}
      <div className="director-actions">
        {canWrite && !managed && <button type="submit" disabled={save.isPending || stale || !!refusal} title={refusal ?? undefined}><Check size={16} />{save.isPending ? 'Saving...' : 'Save framing'}</button>}
        <button type="button" aria-label="Reload framing" title="Reload framing" onClick={() => { setNotice(''); setProblem(''); save.reset(); void draft.refetch(); }}><RefreshCw size={16} /></button>
        {!canWrite && <span className="director-muted">Read only</span>}
      </div>
    </form>
  </section>;
}

/** Catalog marks over the sky: deep-sky objects at their size and angle,
 *  comets with their tail, asteroids with their motion, and the Sun, Moon
 *  and planets. Positions are catalog places at the asked time, drawn
 *  through the same projection as everything else on the stage. */
function SkyMarks({ marks, view, viewFov, stage, objects, bodies, solar }: { marks: DirectorSkyMarks; view: StageView; viewFov: number; stage: Stage; objects: boolean; bodies: boolean; solar: boolean }) {
  const scale = pixelScale(viewFov, stage);
  const place = (position: DirectorSkyPosition): [number, number] | null => {
    const offset = projectOn(view, position);
    if (!offset) return null;
    const at = toStage(offset, viewFov, stage);
    return at[0] < -40 || at[0] > stage.width + 40 || at[1] < -40 || at[1] > stage.height + 40 ? null : at;
  };
  const labels = markLabelBudget(viewFov);
  return <g className="framing-marks" data-testid="framing-marks">
    {objects && marks.objects.items.map((object, index) => {
      const position = { ra_degrees: object.ra_degrees, dec_degrees: object.dec_degrees };
      const at = place(position);
      if (!at) return null;
      // The catalog's major axis lies along the position angle, east of
      // north, turned onto the stage at this place. Without an angle the
      // shape is not known, so it is drawn as a circle of the same area
      // rather than an ellipse lying along the screen.
      const known = object.position_angle_degrees !== null;
      const major = ((object.major_arcmin ?? 0) / 60 / 2) / scale;
      const minor = ((object.minor_arcmin ?? object.major_arcmin ?? 0) / 60 / 2) / scale;
      const rx = Math.max(4, known ? major : Math.sqrt(major * minor));
      const ry = Math.max(4, known ? minor : Math.sqrt(major * minor));
      const angle = known ? stageAngleAt(view, position, object.position_angle_degrees!, viewFov, stage) ?? 0 : 0;
      const label = object.common_name || object.name;
      // An object wider than the stage (Cygnus X is 18° across) would be
      // one arc through everything: it gets a dotted centre mark instead,
      // and a label that stays near the centre for anything big.
      const wider = 2 * Math.min(rx, ry) > stage.width * 1.25;
      return <g key={`${object.id || object.name}#${index}`} className={`framing-mark framing-mark-${object.kind}${wider ? ' is-wider-than-view' : ''}`} data-testid="framing-mark-object">
        {wider
          ? <ellipse cx={at[0]} cy={at[1]} rx={10} ry={10} />
          : <ellipse cx={0} cy={0} rx={rx} ry={ry} transform={`translate(${at[0].toFixed(1)} ${at[1].toFixed(1)}) rotate(${angle.toFixed(1)})`} />}
        {index < labels && <text x={at[0] + Math.min(wider ? 10 : rx, 40) + 6} y={at[1] + 4}>{label}</text>}
      </g>;
    })}
    {bodies && marks.minor_bodies.items.map(body => {
      const at = place({ ra_degrees: body.ra_degrees, dec_degrees: body.dec_degrees });
      if (!at) return null;
      const angle = body.direction_pa_degrees !== null ? stageAngleAt(view, { ra_degrees: body.ra_degrees, dec_degrees: body.dec_degrees }, body.direction_pa_degrees, viewFov, stage) : null;
      const reach = body.kind === 'comet' ? 22 : 10;
      return <g key={`${body.kind}:${body.name}`} className={`framing-mark framing-mark-${body.kind}`} data-testid="framing-mark-body">
        <polygon points={`${at[0]},${at[1] - 6} ${at[0] + 6},${at[1]} ${at[0]},${at[1] + 6} ${at[0] - 6},${at[1]}`} />
        {angle !== null && <line x1={at[0]} y1={at[1]} x2={at[0] + reach * Math.cos((angle * Math.PI) / 180)} y2={at[1] + reach * Math.sin((angle * Math.PI) / 180)} />}
        <text x={at[0] + 9} y={at[1] + 4}>{body.name}{Number.isFinite(body.mag) ? ` ${body.mag.toFixed(1)}` : ''}</text>
      </g>;
    })}
    {solar && marks.solar_system.map(body => {
      const at = place({ ra_degrees: body.ra_degrees, dec_degrees: body.dec_degrees });
      if (!at) return null;
      // The Moon and Sun are half a degree across; the planets get a fixed mark.
      const r = body.kind === 'planet' ? 6 : Math.max(7, (0.26 / scale));
      return <g key={body.name} className={`framing-mark framing-mark-${body.kind}`} data-testid="framing-mark-solar">
        <circle cx={at[0]} cy={at[1]} r={r} />
        <text x={at[0] + r + 6} y={at[1] + 4}>{SOLAR_SYSTEM_LABEL[body.kind] ?? body.name}</text>
      </g>;
    })}
  </g>;
}
