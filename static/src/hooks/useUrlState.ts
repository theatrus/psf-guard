import { useLocation, useSearchParams } from 'react-router-dom';
import { useCallback, useMemo, useRef } from 'react';
import { type GroupingMode, DEFAULT_SINGLE_PROJECT_MODE, MULTI_PROJECT_MODES } from '../types/grouping';
import { parseStatusFilter } from '../utils/statusFilter';
import { listFilterOf } from '../utils/listFilter';
import {
  DEFAULT_SCORE_CUTOFF,
  parseScoreCutoff,
  parseSelectionRule,
  type SelectionRuleId,
} from '../utils/sequenceSelectionRules';
import {
  DEFAULT_IMAGE_ORDER,
  IMAGE_ORDER_PARAM,
  parseImageOrder,
  type ImageOrder,
} from '../utils/imageOrder';
import { SCORING_PARAM_NAMES } from '../utils/scoring';
import {
  DEFAULT_GRID_THUMBNAIL_SIZE,
  DEFAULT_SEQUENCE_THUMBNAIL_SIZE,
  getDisplayPreferences,
  setDisplayPreferences,
  useDisplayPreferences,
} from './useDisplayPreferences';

export interface UpdateParamsOptions {
  /**
   * Render the new params before the event that set them ends. The router
   * otherwise applies a change as a transition, and until it renders, a
   * control showing a param shows the old value: a range input is put back
   * to it, so a second arrow key steps from there and the first is lost.
   * For a control the person moves directly, such as a slider.
   */
  immediate?: boolean;
}

/**
 * Hook for managing URL search parameters as state
 */
export function useUrlParams() {
  const [searchParams, setSearchParams] = useSearchParams();
  const setSearchParamsRef = useRef(setSearchParams);
  const searchParamsRef = useRef(searchParams);
  setSearchParamsRef.current = setSearchParams;
  searchParamsRef.current = searchParams;
  
  const getParam = useCallback((key: string): string | null => {
    return searchParams.get(key);
  }, [searchParams]);
  
  const getNumberParam = useCallback((key: string): number | null => {
    const value = searchParams.get(key);
    return value ? parseInt(value, 10) : null;
  }, [searchParams]);
  
  const getBooleanParam = useCallback((key: string): boolean => {
    return searchParams.get(key) === 'true';
  }, [searchParams]);
  
  const getArrayParam = useCallback((key: string): string[] => {
    const value = searchParams.get(key);
    return value
      ? value.split(',').filter(Boolean).map((item) => decodeURIComponent(item))
      : [];
  }, [searchParams]);
  
  const getNumberArrayParam = useCallback((key: string): number[] => {
    const value = searchParams.get(key);
    return value ? value.split(',').map(id => parseInt(id, 10)).filter(id => !isNaN(id)) : [];
  }, [searchParams]);
  
  const updateParams = useCallback((
    updates: Record<string, string | number | boolean | string[] | number[] | null | undefined>,
    options: UpdateParamsOptions = {},
  ) => {
    const newParams = new URLSearchParams(searchParamsRef.current);
    const scope = scopeKey(newParams);

    Object.entries(updates).forEach(([key, value]) => {
      if (value === null || value === undefined || value === '' || value === 'all') {
        newParams.delete(key);
      } else if (Array.isArray(value)) {
        if (value.length === 0) {
          newParams.delete(key);
        } else {
          newParams.set(key, value.map((item) => encodeURIComponent(String(item))).join(','));
        }
      } else {
        newParams.set(key, String(value));
      }
    });

    // A view's own scoring stays with the database and project it was set in.
    if (scopeKey(newParams) !== scope) dropScoringParams(newParams);
    searchParamsRef.current = newParams;
    setSearchParamsRef.current(newParams, { replace: true, flushSync: options.immediate === true });
  }, []);
  
  return {
    getParam,
    getNumberParam,
    getBooleanParam,
    getArrayParam,
    getNumberArrayParam,
    updateParams,
    searchParams
  };
}

/** The database and project a set of params names. */
function scopeKey(params: URLSearchParams): string {
  return `${params.get('db') ?? ''}\n${params.get('project') ?? ''}`;
}

function dropScoringParams(params: URLSearchParams): void {
  for (const name of SCORING_PARAM_NAMES) params.delete(name);
}

/**
 * Params for a move from `from` to the scope `to` names. A view's own
 * scoring (see SCORING_PARAMS) belongs to the database and project it was
 * set in, so it goes when either changes: Images, the detail view, Stacks
 * and the project's other targets keep it, another project does not.
 * Every way of moving to another scope that copies the params passes them
 * through here (updateParams does so itself).
 */
export function keepScoringInScope(from: string | URLSearchParams, to: URLSearchParams): URLSearchParams {
  const before = typeof from === 'string' ? new URLSearchParams(from) : from;
  if (scopeKey(before) !== scopeKey(to)) dropScoringParams(to);
  return to;
}

/**
 * Hook for reading the (db, project, target) triple from URL state, used by
 * every scoped view (grid, detail, comparison, sequence). The `dbId` is
 * required for any of the per-DB API calls; if it's missing, the view should
 * render an empty/error state rather than fall back to a default.
 *
 * `setDbProjectTarget` is the only way to navigate INTO a scoped view from
 * a list — it sets all three params atomically so the URL is never partially
 * populated.
 */
/** The URL key that scopes a view to a project's whole mosaic. */
export const MOSAIC_PARAM = 'mosaic';

export function useDbProjectTarget() {
  const { getParam, getNumberParam, updateParams } = useUrlParams();

  const dbId = useMemo(() => getParam('db'), [getParam]);
  const projectId = useMemo(() => getNumberParam('project'), [getNumberParam]);
  const targetId = useMemo(() => getNumberParam('target'), [getNumberParam]);
  // `mosaic=1` scopes to every panel of the project's mosaic at once. It
  // only means something with a project and no target.
  const mosaic = useMemo(
    () => getParam(MOSAIC_PARAM) === '1' && projectId !== null && targetId === null,
    [getParam, projectId, targetId]
  );

  const setDbProjectTarget = useCallback(
    (db: string | null, project: number | null, target: number | null) => {
      updateParams({ db, project, target, [MOSAIC_PARAM]: null });
    },
    [updateParams]
  );

  const setProjectId = useCallback(
    (id: number | null) => {
      // Reset target when project changes; dbId stays.
      updateParams({ project: id, target: null, [MOSAIC_PARAM]: null });
    },
    [updateParams]
  );

  const setTargetId = useCallback(
    (id: number | null) => {
      updateParams({ target: id, [MOSAIC_PARAM]: null });
    },
    [updateParams]
  );

  /** Scope to the project's whole mosaic. */
  const setMosaic = useCallback(
    (db: string, project: number) => {
      updateParams({ db, project, target: null, [MOSAIC_PARAM]: '1' });
    },
    [updateParams]
  );

  return {
    dbId,
    projectId,
    targetId,
    mosaic,
    setDbProjectTarget,
    setProjectId,
    setTargetId,
    setMosaic,
  };
}

/**
 * The calibration page's own URL keys. Each starts with `cal` so none
 * collides with a key another view reads, such as the Images `filter`.
 */
export const CALIBRATION_PARAMS = [
  'calTab',
  'calRig',
  'calKind',
  'calFilter',
  'calExposure',
  'calTempMin',
  'calTempMax',
  'calFrom',
  'calTo',
  'calGrade',
  'calKept',
  'calMissing',
  'calDupes',
  'calSort',
  'calPage',
  'calSession',
  'calFrame',
  'calView',
  'calStretch',
  'calFull',
  'calMaster',
  'calRotation',
  'calEarlier',
  'calLater',
] as const;

/** The image detail's chosen calibration finding, or `all`. */
export const CALIBRATION_FOCUS_PARAM = 'calibFocus';

/** Query params that belong to one view and never travel to another: the
 *  Planning workspace's plan (`plan`, and the older names), the Sky's Live
 *  panel (`live`), the image detail's calibration focus and the
 *  calibration page's. */
export const PLANNING_PARAMS = [
  'plan', 'planTab', 'directorProject', 'directorSource', 'directorView', 'directorCatalog', 'live',
  CALIBRATION_FOCUS_PARAM,
  ...CALIBRATION_PARAMS,
] as const;

/** A copy of `search` without the params one view keeps to itself. */
export function withoutPlanningParams(search: string): URLSearchParams {
  const next = new URLSearchParams(search);
  for (const key of PLANNING_PARAMS) next.delete(key);
  return next;
}

/** A plan's workspace, and the old Planning address that forwards to it. */
export function isPlanningPath(pathname: string): boolean {
  return pathname === '/plan' || pathname === '/director';
}

/** The Overview route, which merges every database instead of scoping to one. */
export function isOverviewPath(pathname: string): boolean {
  return pathname === '/' || pathname === '/overview';
}

/** The Sky route: every database at once, like the Overview. */
export function isSkyPath(pathname: string): boolean {
  return pathname === '/sky';
}

/** Routes that show every database, where a parked `db` slug means nothing. */
export function isMergedPath(pathname: string): boolean {
  return isOverviewPath(pathname) || isSkyPath(pathname) || isPlanningPath(pathname);
}

/**
 * The database a status widget should follow. The Overview carries the scope
 * of the view the user came from so it can return them there, but it shows
 * every database at once — so widgets that scope to one must ignore the slug
 * parked in the URL while it is on screen.
 */
export function useScopedDbId(): string | null {
  const { dbId } = useDbProjectTarget();
  const { pathname } = useLocation();
  return isMergedPath(pathname) ? null : dbId;
}

/**
 * Backwards-compatible shim. Same signature as the old `useProjectTarget`
 * but the `dbId` is now part of URL state too — readable via
 * `useDbProjectTarget`. Existing call sites that only need project/target
 * (e.g. selectors) can keep using this.
 */
export function useProjectTarget() {
  const { projectId, targetId, setProjectId, setTargetId } = useDbProjectTarget();
  return { projectId, targetId, setProjectId, setTargetId };
}

/**
 * Hook for managing filter state in URL
 */
export function useFilters() {
  const { getParam, updateParams } = useUrlParams();
  
  const filters = useMemo(() => ({
    status: parseStatusFilter(getParam('status')),
    // Filters and flags to keep; none keeps them all.
    filterNames: listFilterOf(getParam('filter')),
    dateRange: {
      start: getParam('dateStart') || null,
      end: getParam('dateEnd') || null,
    },
    searchTerm: getParam('search') || '',
    flags: listFilterOf(getParam('flag'), (value) => value.toLowerCase()),
    // Calibration levels or finding codes to keep; see utils/calibrationFilter.
    calibration: listFilterOf(getParam('calib'), (value) => value.toLowerCase()),
  }), [getParam]);
  
  const updateFilters = useCallback((updates: {
    status?: string;
    filterNames?: string[];
    dateStart?: string;
    dateEnd?: string;
    searchTerm?: string;
    flags?: string[];
    calibration?: string[];
  }) => {
    updateParams({
      status: updates.status,
      filter: updates.filterNames,
      dateStart: updates.dateStart,
      dateEnd: updates.dateEnd, 
      search: updates.searchTerm,
      // updateParams drops the key for undefined, empty, or 'all', like
      // the other filters: no filter or flag chosen leaves the URL clean.
      flag: updates.flags,
      calib: updates.calibration,
    });
  }, [updateParams]);
  
  const clearFilters = useCallback(() => {
    updateParams({
      status: null,
      filter: null,
      dateStart: null,
      dateEnd: null,
      search: null,
      flag: null,
      calib: null,
    });
  }, [updateParams]);
  
  return {
    filters,
    updateFilters,
    clearFilters,
  };
}

/** Which view's thumbnails a size belongs to. */
export type ThumbnailView = 'grid' | 'sequence';

/**
 * Hook for managing grid state in URL. The grouping and the thumbnail size
 * fall back to the person's saved choices when the link names none, and
 * choosing one saves it (see useDisplayPreferences).
 */
export function useGridState(view: ThumbnailView = 'grid') {
  const { getParam, getNumberParam, getBooleanParam, getArrayParam, getNumberArrayParam, updateParams } = useUrlParams();
  const preferences = useDisplayPreferences();
  // No project in the link is All projects, which has groupings of its own.
  const allProjects = getParam('project') === null;
  const preferredGrouping: GroupingMode = (allProjects
    ? preferences.imageGroupingAllProjects
    : preferences.imageGrouping) ?? DEFAULT_SINGLE_PROJECT_MODE;
  const preferredSize = (view === 'sequence'
    ? preferences.sequenceThumbnailSize
    : preferences.gridThumbnailSize)
    ?? (view === 'sequence' ? DEFAULT_SEQUENCE_THUMBNAIL_SIZE : DEFAULT_GRID_THUMBNAIL_SIZE);

  const groupingMode = useMemo(
    () => (getParam('grouping') as GroupingMode) || preferredGrouping,
    [getParam, preferredGrouping],
  );
  const imageSize = useMemo(
    () => getNumberParam('size') || preferredSize,
    [preferredSize, getNumberParam],
  );
  const showStats = useMemo(() => getBooleanParam('stats'), [getBooleanParam]);
  const expandedGroups = useMemo(() => new Set(getArrayParam('expanded')), [getArrayParam]);
  const currentImageId = useMemo(() => getNumberParam('current'), [getNumberParam]);
  const selectedImages = useMemo(() => new Set(getNumberArrayParam('selected')), [getNumberArrayParam]);
  const selectedImagesRef = useRef(selectedImages);
  selectedImagesRef.current = selectedImages;
  
  /** Choose a grouping, and keep it as the default for links that name
   *  none, for one project or for All projects as the mode says. */
  const setGroupingMode = useCallback((mode: GroupingMode) => {
    const saved = getDisplayPreferences();
    if ((MULTI_PROJECT_MODES as readonly GroupingMode[]).includes(mode)) {
      setDisplayPreferences({ ...saved, imageGroupingAllProjects: mode });
    } else {
      setDisplayPreferences({ ...saved, imageGrouping: mode });
    }
    updateParams({ grouping: mode, expanded: null });
  }, [updateParams]);

  /** Drop the link's grouping, as when the view moves between one project
   *  and All projects: the saved choice for the new scope applies. */
  const resetGroupingMode = useCallback(() => {
    updateParams({ grouping: null, expanded: null });
  }, [updateParams]);

  const setImageSize = useCallback((size: number) => {
    const saved = getDisplayPreferences();
    setDisplayPreferences(view === 'sequence'
      ? { ...saved, sequenceThumbnailSize: size }
      : { ...saved, gridThumbnailSize: size });
    updateParams({ size }, { immediate: true });
  }, [updateParams, view]);
  
  const setShowStats = useCallback((show: boolean) => {
    updateParams({ stats: show ? true : null });
  }, [updateParams]);
  
  const setExpandedGroups = useCallback((groups: Set<string> | ((prev: Set<string>) => Set<string>)) => {
    const newGroups = typeof groups === 'function' ? groups(expandedGroups) : groups;
    updateParams({ expanded: Array.from(newGroups) });
  }, [updateParams, expandedGroups]);
  
  const setSelectedImages = useCallback((images: Set<number> | ((prev: Set<number>) => Set<number>)) => {
    const newImages = typeof images === 'function' ? images(selectedImagesRef.current) : images;
    selectedImagesRef.current = new Set(newImages);
    updateParams({ selected: Array.from(selectedImagesRef.current) });
  }, [updateParams]);

  const setCurrentImageId = useCallback((imageId: number) => {
    updateParams({
      current: imageId,
      groupIndex: null,
      imageIndex: null,
    });
  }, [updateParams]);

  const setCurrentImageSelection = useCallback((imageId: number) => {
    selectedImagesRef.current = new Set([imageId]);
    updateParams({
      current: imageId,
      groupIndex: null,
      imageIndex: null,
      selected: [imageId],
    });
  }, [updateParams]);

  return {
    groupingMode,
    imageSize,
    showStats,
    expandedGroups,
    currentImageId,
    selectedImages,
    setGroupingMode,
    resetGroupingMode,
    setImageSize,
    setShowStats,
    setExpandedGroups,
    setSelectedImages,
    setCurrentImageId,
    setCurrentImageSelection,
  };
}

/**
 * The Sequence view's score cutoff (`cutoff`) and its selection rule
 * (`selectRule`), kept in the URL so a reload, a trip to Detail and back,
 * or a shared link opens with the same rule still selecting. The default
 * cutoff and no rule leave the URL clean.
 */
export function useSequenceSelectionRule() {
  const { getParam, updateParams } = useUrlParams();
  const cutoff = parseScoreCutoff(getParam('cutoff'));
  const rule = parseSelectionRule(getParam('selectRule'));

  const setCutoff = useCallback((value: number) => {
    const rounded = Math.round(value * 100) / 100;
    updateParams({ cutoff: rounded === DEFAULT_SCORE_CUTOFF ? null : rounded.toFixed(2) });
  }, [updateParams]);

  /** Turn a rule on or off, and with `selection` set the selected images
   *  in the same write. Each URL hook writes from the params it last saw,
   *  so a rule and a selection written by two hooks in one turn would lose
   *  the first write. */
  const setRule = useCallback((next: SelectionRuleId | null, selection?: ReadonlySet<number>) => {
    updateParams({
      selectRule: next,
      ...(selection ? { selected: Array.from(selection) } : {}),
    });
  }, [updateParams]);

  return { cutoff, rule, setCutoff, setRule };
}

/**
 * The order Images and Sequence show their frames in (`order`), one key for
 * both views, so it travels between them and into the detail view's next
 * and previous. A link that names no order opens in the one last chosen in
 * this browser. Choosing writes both; oldest first leaves the URL clean.
 */
export function useImageOrder() {
  const { getParam, updateParams } = useUrlParams();
  const preferences = useDisplayPreferences();
  const order: ImageOrder = parseImageOrder(getParam(IMAGE_ORDER_PARAM))
    ?? preferences.imageOrder
    ?? DEFAULT_IMAGE_ORDER;

  /** Choose an order. `alongside` writes other keys in the same change,
   *  such as the new `current`: each URL hook writes from the params it
   *  last saw, so two writes in one turn would lose the first. */
  const setOrder = useCallback((
    next: ImageOrder,
    alongside: Record<string, string | number | null> = {},
  ) => {
    setDisplayPreferences({ ...getDisplayPreferences(), imageOrder: next });
    updateParams({
      ...alongside,
      [IMAGE_ORDER_PARAM]: next === DEFAULT_IMAGE_ORDER ? null : next,
    });
  }, [updateParams]);

  return { order, setOrder };
}
