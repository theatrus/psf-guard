import { useCallback, useMemo } from 'react';
import { CALIBRATION_PARAMS, useUrlParams } from '../../hooks/useUrlState';
import type {
  CalibrationFrameSort,
  CalibrationFrameStretch,
  CalibrationFrameView,
  CalibrationGradeFilter,
  CalibrationKind,
  CalibrationListFilters,
} from '../../api/types';

export type CalibrationTab = 'frames' | 'masters' | 'scheduler';

/** The tab flat groups once had. Its links open Frames, on flats. */
const OLD_GROUPS_TAB = 'groups';

const KINDS: CalibrationKind[] = ['bias', 'dark', 'dark_flat', 'flat'];
const GRADES: CalibrationGradeFilter[] = ['ok', 'suspect', 'poor', 'ungraded'];
const SORTS: CalibrationFrameSort[] = ['newest', 'oldest', 'file', 'grade'];
const STRETCHES: CalibrationFrameStretch[] = ['wide', 'normal', 'tight'];

function oneOf<T extends string>(value: string | null, allowed: readonly T[]): T | undefined {
  return allowed.find((item) => item === value);
}

function number(value: string | null): number | undefined {
  if (value === null || value.trim() === '') return undefined;
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : undefined;
}

/** A project id the address names, or none. */
function projectId(value: string | null): number | undefined {
  const parsed = number(value);
  return parsed !== undefined && Number.isInteger(parsed) && parsed > 0 ? parsed : undefined;
}

/** The rig filter's value for every rig, chosen on purpose. An absent rig
 *  lets a project in scope open on its own rig; this one does not. (The
 *  URL helper drops a value of `all`.) */
export const ALL_RIGS = 'any';

/** The filters as the page's URL holds them. */
export interface CalibrationFilterState {
  rig?: string;
  kind?: CalibrationKind;
  filter?: string;
  exposure?: number;
  tempMin?: number;
  tempMax?: number;
  from?: string;
  to?: string;
  grade?: CalibrationGradeFilter;
  keptOut?: boolean;
  missing?: boolean;
  /** Show exposures recorded more than once instead of sessions. */
  duplicates?: boolean;
  /** A rotator angle in degrees: flats at that angle. */
  rotation?: number;
  /** The project whose calibration the page shows: the sessions, masters
   *  and flat groups its lights use. Its scope chip removes it. */
  project?: number;
  sort?: CalibrationFrameSort;
}

const FILTER_KEYS: Record<keyof CalibrationFilterState, (typeof CALIBRATION_PARAMS)[number]> = {
  rig: 'calRig',
  kind: 'calKind',
  filter: 'calFilter',
  exposure: 'calExposure',
  tempMin: 'calTempMin',
  tempMax: 'calTempMax',
  from: 'calFrom',
  to: 'calTo',
  grade: 'calGrade',
  keptOut: 'calKept',
  missing: 'calMissing',
  duplicates: 'calDupes',
  rotation: 'calRotation',
  project: 'calProject',
  sort: 'calSort',
};

/** The filters in the form the frame and session routes take. */
export function listFilters(state: CalibrationFilterState): CalibrationListFilters {
  const filters: CalibrationListFilters = {
    rig: state.rig === ALL_RIGS ? undefined : state.rig,
    kind: state.kind,
    filter: state.filter,
    exposure: state.exposure,
    temp_min: state.tempMin,
    temp_max: state.tempMax,
    night_from: state.from,
    night_to: state.to,
    grade: state.grade,
    kept_out: state.keptOut,
    missing: state.missing || undefined,
    rotation: state.rotation,
    project: state.project,
  };
  return Object.fromEntries(
    Object.entries(filters).filter(([, value]) => value !== undefined)
  ) as CalibrationListFilters;
}

/**
 * Everything the calibration page shows lives in the URL, so a reload or a
 * shared link opens the same view: the tab, the filters, the session page,
 * the session and frame, and how the frame is drawn.
 */
export function useCalibrationUrlState() {
  const { getParam, updateParams } = useUrlParams();

  const tab: CalibrationTab = oneOf(getParam('calTab'), ['frames', 'masters', 'scheduler'] as const) ?? 'frames';
  // A link to the old Flat groups tab: Frames, narrowed to flats. Its rig,
  // filter, angle and pair are Frames' own params, so they carry over.
  const oldGroupsLink = getParam('calTab') === OLD_GROUPS_TAB;
  const rig = getParam('calRig') || undefined;
  const kind = oneOf(getParam('calKind'), KINDS);
  const filter = getParam('calFilter') || undefined;
  const exposure = number(getParam('calExposure'));
  const tempMin = number(getParam('calTempMin'));
  const tempMax = number(getParam('calTempMax'));
  const from = getParam('calFrom') || undefined;
  const to = getParam('calTo') || undefined;
  const grade = oneOf(getParam('calGrade'), GRADES);
  const kept = getParam('calKept');
  const keptOut = kept === 'true' ? true : kept === 'false' ? false : undefined;
  const missing = getParam('calMissing') === 'true' || undefined;
  const duplicates = getParam('calDupes') === 'true' || undefined;
  const rotation = number(getParam('calRotation'));
  const sort = oneOf(getParam('calSort'), SORTS);
  const project = projectId(getParam('calProject'));
  const filters = useMemo<CalibrationFilterState>(
    () => ({ rig, kind, filter, exposure, tempMin, tempMax, from, to, grade, keptOut, missing, duplicates, rotation, project, sort }),
    [rig, kind, filter, exposure, tempMin, tempMax, from, to, grade, keptOut, missing, duplicates, rotation, project, sort]
  );
  const page = Math.max(0, Math.floor(number(getParam('calPage')) ?? 0));
  const session = getParam('calSession');
  const frame = getParam('calFrame');
  const view: CalibrationFrameView = getParam('calView') === 'session' ? 'session' : 'frame';
  const stretch: CalibrationFrameStretch = oneOf(getParam('calStretch'), STRETCHES) ?? 'normal';
  const full = getParam('calFull') === '1';
  const master = getParam('calMaster');
  // Two flat sessions set side by side.
  const earlier = getParam('calEarlier');
  const later = getParam('calLater');
  const ratio = useMemo(() => (earlier && later ? { earlier, later } : null), [earlier, later]);

  const setTab = useCallback(
    (next: CalibrationTab) =>
      updateParams({ calTab: next === 'frames' ? null : next, calMaster: null, calEarlier: null, calLater: null }),
    [updateParams]
  );
  /** Change filters; the session list starts again at its first page. */
  const setFilters = useCallback(
    (changes: Partial<CalibrationFilterState>) => {
      const updates: Record<string, string | null> = { calPage: null };
      for (const [key, value] of Object.entries(changes) as [keyof CalibrationFilterState, unknown][]) {
        updates[FILTER_KEYS[key]] =
          value === undefined || value === null || value === '' ? null : String(value);
      }
      updateParams(updates);
    },
    [updateParams]
  );
  const clearFilters = useCallback(
    () => updateParams(Object.fromEntries(Object.values(FILTER_KEYS).map((key) => [key, null]))),
    [updateParams]
  );
  const setPage = useCallback((next: number) => updateParams({ calPage: next > 0 ? next : null }), [updateParams]);
  const selectSession = useCallback(
    (id: string | null, frameUuid: string | null = null) =>
      updateParams({ calSession: id, calFrame: frameUuid, calView: null }),
    [updateParams]
  );
  const selectFrame = useCallback((uuid: string | null) => updateParams({ calFrame: uuid }), [updateParams]);
  const setView = useCallback(
    (next: CalibrationFrameView) => updateParams({ calView: next === 'session' ? 'session' : null }),
    [updateParams]
  );
  const setStretch = useCallback(
    (next: CalibrationFrameStretch) => updateParams({ calStretch: next === 'normal' ? null : next }),
    [updateParams]
  );
  const setFull = useCallback((next: boolean) => updateParams({ calFull: next ? '1' : null }), [updateParams]);
  const openMaster = useCallback((id: string | null) => updateParams({ calMaster: id }), [updateParams]);
  /** Open an old Flat groups link as Frames on flats, in place. */
  const openOldGroupsLink = useCallback(
    () => updateParams({ calTab: null, calKind: 'flat' }),
    [updateParams]
  );
  /** Open the ratio picture of two flat sessions, or close it. */
  const openRatio = useCallback(
    (pair: { earlier: string; later: string } | null) =>
      updateParams({ calEarlier: pair?.earlier ?? null, calLater: pair?.later ?? null }),
    [updateParams]
  );
  /** Another database chosen elsewhere, as in the header: the session,
   *  frame, page, master and filters chosen in the last one go. Its rig
   *  above all would match nothing here. A project scope moves to
   *  `project`, the same project in that database, when there is one. */
  const resetForDatabase = useCallback(
    (project?: number) =>
      updateParams({
        calSession: null,
        calFrame: null,
        calPage: null,
        calMaster: null,
        calView: null,
        calEarlier: null,
        calLater: null,
        ...Object.fromEntries(Object.values(FILTER_KEYS).map((key) => [key, null])),
        calProject: project === undefined ? null : String(project),
      }),
    [updateParams]
  );
  /** Another database: nothing chosen in this one carries over. */
  const setDatabase = useCallback(
    (db: string) =>
      updateParams({
        db,
        project: null,
        target: null,
        mosaic: null,
        ...Object.fromEntries(CALIBRATION_PARAMS.filter((key) => key !== 'calTab').map((key) => [key, null])),
      }),
    [updateParams]
  );

  return {
    tab,
    filters,
    page,
    session,
    frame,
    view,
    stretch,
    full,
    master,
    ratio,
    oldGroupsLink,
    setTab,
    setFilters,
    clearFilters,
    setPage,
    selectSession,
    selectFrame,
    setView,
    setStretch,
    setFull,
    openMaster,
    openRatio,
    openOldGroupsLink,
    resetForDatabase,
    setDatabase,
  };
}
