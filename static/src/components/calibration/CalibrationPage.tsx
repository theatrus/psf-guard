import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react';
import { keepPreviousData, useQuery, useQueryClient } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { CalibrationSession } from '../../api/types';
import { useDbProjectTarget } from '../../hooks/useUrlState';
import { useAllDatabases, useMergedProjects } from '../../hooks/useDatabases';
import SchedulerFlatHistory from '../SchedulerFlatHistory';
import PillSelect from '../toolbar/PillSelect';
import ScopeChip from '../toolbar/ScopeChip';
import CalibrationDuplicates from './CalibrationDuplicates';
import CalibrationFilters from './CalibrationFilters';
import CalibrationMasterList from './CalibrationMasterList';
import CalibrationSessionList from './CalibrationSessionList';
import { FlatGroupsSummary, FlatSessionGroupLine } from './FlatGroupMarks';
import FlatGroupDetail, { FlatGroupHeader } from './FlatGroupDetail';
import CalibrationFrameBrowser from './CalibrationFrameBrowser';
import FlatGroupList from './FlatGroupList';
import FlatRatioViewer from './FlatRatioViewer';
import { nightLabel, plural, sessionDetails, sessionTitle } from './calibrationLabels';
import { browseGroups } from './flatGroupBrowse';
import { indexFlatGroups } from './flatGroupIndex';
import { findPair } from './flatGroupLabels';
import { useFlatGroups } from './useFlatGroups';
import { useFlatChoiceAccess, useFlatGroupOverride } from './useFlatGroupChoices';
import { useFlatGroupUse } from '../../hooks/useFlatGroupLights';
import { scopeRigsLabel, useProjectScope, widenText, type CalibrationScope } from './calibrationScope';
import { useCalibrationActions } from './useCalibrationActions';
import type { ValidityChoice } from './useCalibrationActions';
import { ALL_RIGS, listFilters, useCalibrationUrlState } from './useCalibrationUrlState';
import type { CalibrationFilterState, CalibrationTab } from './useCalibrationUrlState';
import { findGridNavigationIndex } from '../../utils/gridNavigation';
import './CalibrationPage.css';
import FrameSitesNote from './FrameSitesNote';

const SESSIONS_PER_PAGE = 50;
/** Frames of one session shown in the strip, at most. */
const STRIP_FRAMES = 500;
/** Flats of one group shown in the strip, at most: the server's bound. */
const GROUP_STRIP_FRAMES = 5_000;

const TABS: { value: CalibrationTab; label: string }[] = [
  { value: 'frames', label: 'Frames' },
  { value: 'masters', label: 'Masters' },
  { value: 'scheduler', label: 'Scheduler flats' },
];

/**
 * The calibration library of one database: its sessions, the frames of a
 * session as pictures, one frame with its measurement against its session,
 * and the masters built from them. Everything chosen lives in the URL.
 */
export default function CalibrationPage() {
  const { dbId, projectId } = useDbProjectTarget();
  const { data: databases } = useAllDatabases();
  const state = useCalibrationUrlState();
  const { setDatabase, resetForDatabase, filters, setFilters, oldGroupsLink, openOldGroupsLink } = state;

  // Flat groups once had a tab of their own; its links open Frames on flats.
  useEffect(() => {
    if (oldGroupsLink) openOldGroupsLink();
  }, [oldGroupsLink, openOldGroupsLink]);

  // A page opened without a database takes the first one.
  useEffect(() => {
    if (!dbId && databases && databases.length > 0) setDatabase(databases[0].id);
  }, [databases, dbId, setDatabase]);

  const { data: projects } = useMergedProjects();
  const profile = projects.find((project) => project.db_id === dbId && project.id === projectId)?.profile_id;
  // The project whose calibration the page shows, from calProject.
  const scopedName = projects.find((project) => project.db_id === dbId && project.id === filters.project)?.name;
  const { scope: projectScope, query: scopeQuery } = useProjectScope(dbId, filters.project, scopedName);
  // Scheduler flats and Duplicates read the whole library, so they say no
  // scope; it comes back with Frames or Masters.
  const scope = state.tab !== 'scheduler' && !filters.duplicates ? projectScope : null;
  const scopeUse = scopeQuery.data;

  // The project in scope opens on its rig: the one rig its lights use, or
  // else the one whose frames came from the project's N.I.N.A. profile,
  // when exactly one did; only when the library holds more than one rig:
  // with one, the page shows no Rig choice. A rig chosen this way gives way
  // to the next project's; one a person chose stays, so choosing All
  // sticks. Removing the project scope keeps the rig, so ✕ widens to that
  // rig's whole library.
  const summary = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId!),
    enabled: !!dbId,
  });
  const shownDb = useRef(dbId);
  const openedFor = useRef<string | null>(null);
  const defaultRig = useRef<string | null>(null);
  useEffect(() => {
    let rig = filters.rig;
    if (shownDb.current !== dbId) {
      // Another database chosen in the header: what was chosen in this one
      // means nothing there. Its rig may stay in the URL a moment longer,
      // so it counts as no one's choice. A project scope follows the
      // project chosen with it, as when the header switches rig; with no
      // project there, the page widens to that rig's library.
      if (shownDb.current !== null) {
        resetForDatabase(filters.project !== undefined ? projectId ?? undefined : undefined);
        defaultRig.current = rig ?? null;
        rig = undefined;
      }
      shownDb.current = dbId;
      // Back in a database, a project opens on its rig again.
      openedFor.current = null;
    }
    const rigs = summary.data?.rigs;
    // Once per project and scope.
    const opened = `${dbId}:${projectId}:${filters.project ?? ''}`;
    if (!dbId || !rigs || rigs.length < 2) return;
    if (openedFor.current === opened) return;
    // Removing the scope, or Clear, leaves the rig as it is.
    if (filters.project === undefined && openedFor.current?.startsWith(`${dbId}:${projectId}:`)) return;
    // A scoped page waits for the rigs its own project's lights use, not
    // the last project's while the next one's are on their way.
    if (filters.project !== undefined && scopeUse?.project_id !== filters.project) return;
    const used = scopeUse?.rigs.map((entry) => entry.rig_uuid) ?? [];
    if (used.length === 0 && !profile) return;
    openedFor.current = opened;
    if (rig && rig !== defaultRig.current) return;
    const own = used.length > 0
      ? used
      : rigs.filter((entry) => entry.profile_ids?.includes(profile!)).map((entry) => entry.rig_uuid);
    defaultRig.current = own.length === 1 ? own[0] : ALL_RIGS;
    setFilters({ rig: defaultRig.current });
  }, [dbId, filters.project, filters.rig, profile, projectId, resetForDatabase, scopeUse, setFilters, summary.data]);

  // Another project chosen in the header while scoped: the scope follows
  // it, and All projects widens to the whole library. A page a person
  // widened stays wide. A database change is above.
  const shownProject = useRef(`${dbId}:${projectId}`);
  useEffect(() => {
    const key = `${dbId}:${projectId}`;
    if (shownProject.current === key) return;
    const sameDb = shownProject.current.startsWith(`${dbId}:`);
    shownProject.current = key;
    if (sameDb && filters.project !== undefined && filters.project !== projectId) {
      setFilters({ project: projectId ?? undefined });
    }
  }, [dbId, filters.project, projectId, setFilters]);

  const database = databases?.find((entry) => entry.id === dbId);
  const widen = () => setFilters({ project: undefined });
  // The header's rig: the database the page shows.
  const rigName = database?.name ?? dbId ?? '';
  const widenLabel = widenText(rigName);
  const chosenRig = filters.rig && filters.rig !== ALL_RIGS ? filters.rig : undefined;
  const rigLabel = scope ? scopeRigsLabel(scope, summary.data?.rigs ?? [], chosenRig) : undefined;
  const spans = scope?.use?.rigs.length ?? 0;
  const scopeChip = scope ? (
    <ScopeChip name={scope.name} on={rigName}
      detail={!chosenRig && spans > 1 ? `${spans} rigs` : undefined}
      removeLabel={widenLabel} onRemove={widen} />
  ) : undefined;
  // The header's project picker chooses the database, as for every view.
  return (
    <div className="calibration-page">
      <header className="calibration-page-header">
        <div className="calibration-page-title">
          <h1>
            Calibration
            {scope && (
              <span className="calibration-scope-heading" data-testid="calibration-scope-heading">
                {` · for ${scope.name}${rigLabel ? ` · ${rigLabel}` : ''}`}
              </span>
            )}
          </h1>
          {scope ? (
            <span className="calibration-muted">
              {scopeLine(scope)}
              {' '}
              <button type="button" className="calibration-link-button" onClick={widen}>{widenLabel}</button>
            </span>
          ) : database && <span className="calibration-muted">{database.name}</span>}
        </div>
        <div className="calibration-tabs" role="tablist" aria-label="Calibration views">
          {TABS.map((tab) => (
            <button key={tab.value} type="button" role="tab" aria-selected={state.tab === tab.value}
              data-testid={`calibration-tab-${tab.value}`} onClick={() => state.setTab(tab.value)}>
              {tab.label}
            </button>
          ))}
        </div>
      </header>
      {!dbId ? (
        <p className="calibration-empty">{databases?.length === 0 ? 'No databases yet.' : 'Loading…'}</p>
      ) : (
        <CalibrationLibrary key={dbId} dbId={dbId} dbName={database?.name ?? dbId} state={state}
          scope={scope} scopeChip={scopeChip} scopeRevision={scopeUse?.revision}
          scopeError={scope ? scopeQuery.error?.message ?? null : null} />
      )}
    </div>
  );
}

/** What a scope holds, in a line under the heading. */
function scopeLine(scope: CalibrationScope): string {
  const use = scope.use;
  if (!use) return 'Working out which calibration its lights use…';
  if (use.lights === 0 && use.pending > 0) return `Reading its ${plural(use.pending, 'light')}’ headers…`;
  if (use.lights === 0) {
    return use.unread > 0
      ? `None of its ${plural(use.unread, 'light')} could be read, so none is shown.`
      : 'It has no lights to match yet.';
  }
  const parts = [
    `What its ${plural(use.lights, 'light')} use: ${plural(use.sessions.length, 'session')}, ${plural(use.masters.length, 'master')}`,
  ];
  if (use.pending > 0) parts.push(`reading ${plural(use.pending, 'more light')}…`);
  if (use.unread > 0) parts.push(`${plural(use.unread, 'light')} not read`);
  return `${parts.join(' · ')}.`;
}

function CalibrationLibrary({
  dbId,
  dbName,
  state,
  scope,
  scopeChip,
  scopeRevision,
  scopeError,
}: {
  dbId: string;
  dbName: string;
  state: ReturnType<typeof useCalibrationUrlState>;
  /** The project the page is scoped to, when it is. */
  scope: CalibrationScope | null;
  /** The project scope's chip, first in the filter bar. */
  scopeChip?: ReactNode;
  /** The scope's revision: the lists ask again when it changes. */
  scopeRevision?: string;
  scopeError: string | null;
}) {
  // Keep out, validity marks, Forget and Clear masters edit catalog rows:
  // an editor's work, like grading. The database-management gate guards
  // paths a client names, which nothing here does.
  const access = useAccess();
  const canManage = access.canWrite;
  const actions = useCalibrationActions(dbId);
  const summary = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId),
  });
  // Repairing duplicates rewrites the library wholesale, so it waits for
  // the database-management gate as well.
  const serverInfo = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  const managementAllowed = serverInfo.data?.allow_database_management ?? false;

  if (state.tab === 'scheduler') {
    return <SchedulerFlatHistory dbId={dbId} canManage={canManage} />;
  }
  if (state.tab === 'masters') {
    return (
      <CalibrationMasterList dbId={dbId} dbName={dbName} canManage={canManage}
        opened={state.master} actions={actions} onOpen={state.openMaster}
        project={state.filters.project} scopeRevision={scopeRevision} scopeChip={scopeChip}
        onClearScope={() => state.setFilters({ project: undefined })} />
    );
  }
  return (
    <>
      {scopeError && <p className="calibration-error">Could not work out the project's calibration: {scopeError}</p>}
      {!canManage && (
        <p className="calibration-notice">Your account can view frames, not change them.</p>
      )}
      <FrameSitesNote dbId={dbId} />
      {state.filters.duplicates ? (
        <>
          <CalibrationFilters filters={state.filters} facets={undefined} rigs={summary.data?.rigs ?? []}
            onChange={state.setFilters} onClear={state.clearFilters} />
          <CalibrationDuplicates dbId={dbId} filters={state.filters}
            canRepair={canManage && managementAllowed} canStop={canManage}
            repairNote={!canManage ? null
              : 'Repairing duplicates needs database management: the desktop app, or a server started with --allow-database-management.'} />
        </>
      ) : summary.data && summary.data.frame_count === 0 ? (
        <p className="calibration-empty">
          No calibration frames yet. Add their folders to this database in Settings, then import.
        </p>
      ) : (
        <CalibrationFrames dbId={dbId} canManage={canManage} actions={actions}
          rigs={summary.data?.rigs ?? []} state={state} scope={scope} scopeFailed={scopeError !== null}
          scopeChip={scopeChip} scopeRevision={scopeRevision} />
      )}
    </>
  );
}

function CalibrationFrames({
  dbId,
  canManage,
  actions,
  rigs,
  state,
  scope,
  scopeFailed,
  scopeChip,
  scopeRevision,
}: {
  dbId: string;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  rigs: NonNullable<Awaited<ReturnType<typeof apiClient.getCalibrationLibrary>>>['rigs'];
  state: ReturnType<typeof useCalibrationUrlState>;
  scope: CalibrationScope | null;
  /** The scope could not be worked out: Groups stops waiting for it. */
  scopeFailed: boolean;
  scopeChip?: ReactNode;
  scopeRevision?: string;
}) {
  const { filters, page, session: sessionId, frame: frameUuid, selectSession, selectFrame, ratio, openRatio } = state;
  const { selectGroup, chooseGroup } = state;
  // Splits and joins: an editor's, on a server that allows database
  // management; everyone sees them.
  const canChoose = useFlatChoiceAccess();
  const groupOverride = useFlatGroupOverride(dbId);
  const groupEdit = canChoose
    ? {
        send: (request: Parameters<typeof groupOverride.mutate>[0]) => groupOverride.mutate(request),
        busy: groupOverride.isPending,
        error: groupOverride.error instanceof Error ? groupOverride.error.message : null,
      }
    : undefined;
  const listed = useMemo(() => listFilters(filters), [filters]);
  // A scoped list is asked again when its scope's revision moves on.
  const revision = filters.project !== undefined ? scopeRevision ?? null : null;
  // Flat groups ride along wherever the list can hold flats.
  const flatsListed = filters.kind === undefined || filters.kind === 'flat';
  const chosenRig = filters.rig && filters.rig !== ALL_RIGS ? filters.rig : undefined;
  // The page shows this rig's flats: its pass runs at interactive priority.
  const flat = useFlatGroups(dbId, flatsListed, filters.project, revision, chosenRig);
  const flatRigs = useMemo(() => flat.groups.data?.rigs ?? [], [flat.groups.data]);
  const flatIndex = useMemo(() => indexFlatGroups(flatRigs), [flatRigs]);
  const rigsInView = useMemo(
    () => (chosenRig ? flatRigs.filter((rig) => rig.rig_uuid === chosenRig) : flatRigs),
    [chosenRig, flatRigs],
  );

  // Groups: the sidebar lists flat groups instead of sessions. It is
  // offered whenever there are groups to list: the list can hold flats
  // (Type All or Flat) and the rigs in view have measured some.
  const wantsGroups = state.browse === 'groups';
  const scoped = filters.project !== undefined;
  // A Groups link holds the sidebar until the groups, and the scope they
  // are narrowed to, are in: it neither shows the session list for a
  // moment nor chooses a session in the address.
  const groupsLoading = wantsGroups && flatsListed
    && (flat.groups.isLoading || (scoped && !scope?.use && !scopeFailed));
  const groupsOffered = flatsListed && !!flat.groups.data?.available
    && rigsInView.some((rig) => rig.families.length > 0);
  const browse = wantsGroups && (groupsOffered || groupsLoading) ? 'groups' : 'sessions';
  const groupUse = useFlatGroupUse(dbId, browse === 'groups');
  const scopeSessions = useMemo(
    () => (scope?.use ? new Set(scope.use.sessions) : undefined),
    [scope?.use],
  );
  // The groups the project's own lights take. A group the scope holds only
  // for a correction's map is not one; known once every light is read.
  const useData = groupUse.data;
  const projectGroups = useMemo(() => {
    if (!scope || !useData?.computed || useData.pending > 0 || useData.unread > 0) return undefined;
    return new Set(useData.groups
      .filter((group) => group.projects.some((project) => project.project_id === scope.projectId))
      .map((group) => group.key));
  }, [scope, useData]);
  const groupBrowse = useMemo(
    () => browseGroups(rigsInView, { filter: filters.filter, rotation: filters.rotation, scopeSessions, projectGroups }),
    [filters.filter, filters.rotation, projectGroups, rigsInView, scopeSessions],
  );
  const groupEntry = browse === 'groups' ? groupBrowse.find(state.group) : undefined;
  // A key no group answers to, as when its oldest night was kept out: the
  // session it names finds the group of its family around its night.
  const lost = browse === 'groups' && !groupsLoading && !!flat.groups.data && !!state.group && !groupEntry;
  const lostSession = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'flat-groups', 'place', state.group],
    queryFn: () => apiClient.getFlatSessionPlace(dbId, state.group!),
    enabled: lost,
    staleTime: Infinity,
    retry: false,
  });
  // The first group is chosen until a person chooses; a key that now
  // names a regrouped group by one of its sessions is put in its own name.
  useEffect(() => {
    if (browse !== 'groups' || groupsLoading || !flat.groups.data) return;
    let chosen = groupBrowse.find(state.group);
    if (!chosen && state.group) {
      if (lostSession.isLoading) return;
      chosen = lostSession.data ? groupBrowse.near(lostSession.data) : undefined;
    }
    const next = chosen?.key ?? (groupBrowse.navigable[0] ?? groupBrowse.entries[0])?.key;
    if (next && next !== state.group) selectGroup(next);
  }, [browse, flat.groups.data, groupBrowse, groupsLoading, lostSession.data, lostSession.isLoading, selectGroup, state.group]);
  // Closing the comparison hands focus back to the Compare that opened it,
  // which also brings the list back to where it was.
  const opener = useRef<HTMLElement | null>(null);
  const [returnFocus, setReturnFocus] = useState(false);
  const compareFlats = (earlier: string, later: string, button: HTMLElement) => {
    if (ratio?.earlier === earlier && ratio.later === later) {
      openRatio(null);
      return;
    }
    opener.current = button;
    openRatio({ earlier, later });
  };
  const closeRatio = () => {
    openRatio(null);
    setReturnFocus(true);
  };
  useEffect(() => {
    if (!returnFocus || ratio) return;
    setReturnFocus(false);
    if (opener.current?.isConnected) opener.current.focus();
  }, [ratio, returnFocus]);
  const pair = ratio ? findPair(flatRigs, ratio.earlier, ratio.later) : {};
  const sessionsKey = (shown: typeof listed, at: number, scoped: string | null) =>
    ['db', dbId, 'calibrations', 'sessions', shown, at, scoped] as const;
  const sessions = useQuery({
    queryKey: sessionsKey(listed, page, revision),
    queryFn: () => apiClient.getCalibrationSessions(dbId, { ...listed, page, per_page: SESSIONS_PER_PAGE }),
    enabled: browse === 'sessions',
    // Keep the old list while a new one loads, so nothing below it jumps.
    placeholderData: keepPreviousData,
  });

  // A group's session opens in the session list, on the page that holds
  // it. Filters that leave it out give way: first the narrowing ones, then
  // the project scope, as its ✕ would.
  const queryClient = useQueryClient();
  const [opening, setOpening] = useState<string | null>(null);
  const [openError, setOpenError] = useState<string | null>(null);
  const openGroupSession = async (id: string) => {
    const unnarrowed: Partial<CalibrationFilterState> = {
      filter: undefined, exposure: undefined, tempMin: undefined, tempMax: undefined, from: undefined,
      to: undefined, grade: undefined, keptOut: undefined, missing: undefined, rotation: undefined,
    };
    const tries: Partial<CalibrationFilterState>[] = [{}, unnarrowed];
    if (filters.project !== undefined) tries.push({ ...unnarrowed, project: undefined });
    setOpening(id);
    setOpenError(null);
    try {
      for (const changes of tries) {
        const shown = listFilters({ ...filters, ...changes });
        const found = await apiClient.getCalibrationSessions(dbId, { ...shown, locate: id, per_page: SESSIONS_PER_PAGE });
        if (!found.located) continue;
        const scoped = shown.project !== undefined ? revision : null;
        queryClient.setQueryData(sessionsKey(shown, found.page, scoped), { ...found, located: undefined });
        state.openGroupSession(id, found.page, changes);
        return;
      }
      setOpenError('That session is no longer in the library.');
    } catch (error) {
      setOpenError(error instanceof Error ? error.message : String(error));
    } finally {
      setOpening(null);
    }
  };
  const sessionList = useMemo(() => sessions.data?.sessions ?? [], [sessions.data]);
  const strip = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frames', listed, sessionId, filters.sort, revision],
    queryFn: () => apiClient.getCalibrationFrames(dbId, {
      ...listed,
      session: sessionId ?? undefined,
      sort: filters.sort ?? 'oldest',
      per_page: STRIP_FRAMES,
    }),
    enabled: !!sessionId && browse === 'sessions',
    placeholderData: (previous, query) =>
      // The last strip stays while the same session reloads, not another's.
      query?.queryKey[5] === sessionId ? previous : undefined,
  });
  const sessionFrames = useMemo(() => strip.data?.frames ?? [], [strip.data]);
  // A flat group's frames: every flat of its nights, night by night, the
  // group found on the server by its key. The page's filters narrow them
  // as they narrow a session's; a project scope does not, since the group
  // is the same whoever uses it.
  const groupKey = groupEntry?.key ?? null;
  const unscoped = useMemo(() => {
    const rest = { ...listed };
    delete rest.project;
    return rest;
  }, [listed]);
  const groupStrip = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frames', unscoped, 'group', groupKey],
    queryFn: () => apiClient.getCalibrationFrames(dbId, {
      ...unscoped,
      group: groupKey!,
      sort: 'night',
      per_page: GROUP_STRIP_FRAMES,
    }),
    enabled: browse === 'groups' && groupKey !== null,
    placeholderData: (previous, query) =>
      // The last strip stays while the same group reloads, as after a
      // regroup or a Forget, not another group's.
      query?.queryKey[6] === groupKey ? previous : undefined,
  });
  const groupTotal = groupStrip.data?.total ?? 0;
  const groupFrames = useMemo(() => groupStrip.data?.frames ?? [], [groupStrip.data]);
  const frames = browse === 'groups' ? groupFrames : sessionFrames;
  const framesReady = browse === 'groups'
    ? !!groupStrip.data && !groupStrip.isPlaceholderData
    : !!sessionId && !!strip.data && !strip.isPlaceholderData;
  // Each night of the group marked where it begins in the strip.
  const groupRunOf = useMemo(() => {
    const counts = new Map<string, number>();
    for (const frame of groupFrames) counts.set(frame.session, (counts.get(frame.session) ?? 0) + 1);
    const tone = groupEntry?.tone;
    return (frame: (typeof groupFrames)[number]) => ({
      key: frame.session,
      label: nightLabel(frame.night),
      detail: plural(counts.get(frame.session) ?? 0, 'flat'),
      tone,
    });
  }, [groupEntry?.tone, groupFrames]);
  // A member's card brings the strip to that night.
  const showNight = (session: string) => {
    const first = groupFrames.find((frame) => frame.session === session);
    if (!first) return;
    selectFrame(first.frame_uuid);
    document.querySelector('.calibration-strip')?.scrollIntoView?.({ block: 'nearest' });
  };

  // The first session and its first frame are chosen until a person
  // chooses; a session or frame the filters or the page drop gives way to
  // the first one left.
  useEffect(() => {
    if (browse !== 'sessions' || sessions.isPlaceholderData || sessionList.length === 0) return;
    if (!sessionId || !sessionList.some((entry) => entry.id === sessionId)) {
      selectSession(sessionList[0].id);
    }
  }, [browse, selectSession, sessionId, sessionList, sessions.isPlaceholderData]);
  const session: CalibrationSession | undefined = sessionList.find((entry) => entry.id === sessionId);
  const sessionIndex = sessionList.findIndex((entry) => entry.id === sessionId);

  // ← → step through the frames in the browser; ↑ ↓ through the list
  // beside it, sessions or groups.
  const hotkeys = { enabled: !state.master && browse === 'sessions', preventDefault: true };
  useHotkeys('up', () => {
    if (sessionIndex > 0) selectSession(sessionList[sessionIndex - 1].id);
  }, hotkeys, [selectSession, sessionIndex, sessionList]);
  useHotkeys('down', () => {
    if (sessionIndex >= 0 && sessionIndex < sessionList.length - 1) selectSession(sessionList[sessionIndex + 1].id);
  }, hotkeys, [selectSession, sessionIndex, sessionList]);

  // ↑ ↓ move through the groups in scope, as through Grid and Sequence.
  const listRef = useRef<HTMLDivElement>(null);
  const moveGroup = useCallback((direction: 'prev' | 'next') => {
    const items = groupBrowse.navigable;
    const index = items.findIndex((entry) => entry.key === groupEntry?.key);
    if (index < 0) {
      if (items[0]) chooseGroup(items[0].key);
      return;
    }
    const next = findGridNavigationIndex(items, index, direction, (entry) =>
      listRef.current?.querySelector(`[data-group-key="${entry.key}"]`)?.getBoundingClientRect() ?? null);
    if (next !== index) chooseGroup(items[next].key);
  }, [chooseGroup, groupBrowse.navigable, groupEntry?.key]);
  const groupHotkeys = { enabled: !state.master && browse === 'groups', preventDefault: true };
  useHotkeys('up', () => moveGroup('prev'), groupHotkeys, [moveGroup]);
  useHotkeys('down', () => moveGroup('next'), groupHotkeys, [moveGroup]);

  // The sidebar sticks to the top of the page's scroll area at no more than
  // its height, so its list scrolls on its own and its foot (a pager, the
  // last group) stays in reach however long the frame's side runs.
  useEffect(() => {
    const column = listRef.current;
    const area = column?.closest<HTMLElement>('.app-main');
    if (!column || !area) return;
    const fit = () => {
      // A sticky box stops at its container's end, so the padding below it,
      // the page's and the scroll area's, comes off its height too.
      let below = 0;
      for (let element = column.parentElement; element; element = element.parentElement) {
        const style = getComputedStyle(element);
        below += (parseFloat(style.paddingBottom) || 0) + (parseFloat(style.borderBottomWidth) || 0);
        if (element === area) break;
        below += parseFloat(style.marginBottom) || 0;
      }
      column.style.setProperty('--calibration-sidebar-height', `${area.clientHeight - below}px`);
    };
    fit();
    if (typeof ResizeObserver === 'undefined') {
      window.addEventListener('resize', fit);
      return () => window.removeEventListener('resize', fit);
    }
    const observer = new ResizeObserver(fit);
    observer.observe(area);
    return () => observer.disconnect();
  }, []);

  // Keep the chosen session or group in sight in its list as the keys move it.
  const shownKey = browse === 'groups' ? groupEntry?.key : sessionId;
  useEffect(() => {
    const list = listRef.current?.querySelector('ul');
    const selector = browse === 'groups' ? 'data-group-key' : 'data-session-id';
    const item = shownKey ? list?.querySelector<HTMLElement>(`[${selector}="${shownKey}"]`) : null;
    if (!list || !item) return;
    const top = item.offsetTop;
    const bottom = top + item.offsetHeight;
    // A step past the edge scrolls by the step; a jump, such as a group
    // opened from a link, brings it to the middle.
    const far = top > list.scrollTop + list.clientHeight + item.offsetHeight
      || bottom < list.scrollTop - item.offsetHeight;
    if (browse === 'groups' && far) list.scrollTop = top - (list.clientHeight - item.offsetHeight) / 2;
    else if (top < list.scrollTop) list.scrollTop = top;
    else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
  }, [browse, shownKey, sessionList.length, groupBrowse.entries.length]);

  const markSession = (direction: ValidityChoice) =>
    actions.mark.mutate({ frameUuids: sessionFrames.map((frame) => frame.frame_uuid), direction });
  const forgetSession = () => {
    if (!session) return;
    if (window.confirm(`Forget these ${plural(sessionFrames.length, 'frame')}? Their catalog records and the masters built from them go. The files stay.`)) {
      actions.forget.mutate(sessionFrames.map((frame) => frame.frame_uuid), {
        onSuccess: () => selectSession(null),
      });
    }
  };
  const frameBrowser = (
    <CalibrationFrameBrowser
      dbId={dbId}
      frames={frames}
      ready={framesReady}
      frameUuid={frameUuid}
      onSelectFrame={selectFrame}
      onKeepListedFrame={state.keepListedFrame}
      zoomKey={browse === 'groups' ? `group:${groupEntry?.key ?? ''}` : sessionId ?? ''}
      runOf={browse === 'groups' ? groupRunOf : undefined}
      view={state.view}
      stretch={state.stretch}
      full={state.full}
      onView={state.setView}
      onStretch={state.setStretch}
      onFull={state.setFull}
      showSpots={state.spots}
      showDust={state.dust}
      onShowSpots={state.setSpots}
      onShowDust={state.setDust}
      canManage={canManage}
      actions={actions}
      keysEnabled={!state.master}
      upDown={browse === 'groups' ? 'groups' : 'sessions'}
      note={browse === 'groups' && groupTotal > groupFrames.length
        ? <p className="calibration-muted" data-testid="flat-group-strip-truncated">
          Showing the first {groupFrames.length.toLocaleString()} of {plural(groupTotal, 'flat')}.
        </p>
        : undefined}
      empty={<p className="calibration-empty">
        {browse === 'groups' ? 'No flats of this group match the filters.' : 'No frames of this session match the filters.'}
      </p>}
    />
  );

  return (
    <>
      <CalibrationFilters filters={filters} facets={sessions.data?.facets} rigs={rigs}
        scope={scopeChip} onChange={state.setFilters} onClear={state.clearFilters} />
      {flatsListed && (
        <FlatGroupsSummary dbId={dbId} view={flat.groups.data} error={flat.groups.error} rigs={rigsInView}
          progress={flat.progress} measuring={flat.measuring} canManage={canManage} />
      )}
      {sessions.error && browse === 'sessions' && (
        <p className="calibration-error">Could not read the sessions: {sessions.error.message}</p>
      )}
      <div className="calibration-layout">
        <div className="calibration-sessions-column" ref={listRef}>
          {(groupsOffered || groupsLoading) && (
            <div className="tool-segmented calibration-browse-toggle" role="group" aria-label="List">
              <button type="button" aria-pressed={browse === 'sessions'} data-testid="calibration-browse-sessions"
                onClick={() => state.setBrowse('sessions')}>
                Sessions
              </button>
              <button type="button" aria-pressed={browse === 'groups'} data-testid="calibration-browse-groups"
                onClick={() => state.setBrowse('groups')}>
                Groups
              </button>
            </div>
          )}
          {browse === 'groups' ? (
            <>
              <p className="calibration-muted calibration-count">
                {groupsLoading
                  ? 'Loading groups…'
                  : `${plural(groupBrowse.navigable.length, 'group')} · ${plural(groupBrowse.families.length, 'filter and angle', 'filters and angles')}`}
              </p>
              {!groupsLoading && groupBrowse.entries.length === 0 && (
                <p className="calibration-empty">No flat groups match these filters.</p>
              )}
              <FlatGroupList families={groupBrowse.families} selected={groupEntry?.key}
                showRig={rigsInView.length > 1} scopeName={scope?.name} onSelect={chooseGroup} edit={groupEdit} />
            </>
          ) : (
            <>
              <p className="calibration-muted calibration-count">
                {sessions.data
                  ? `${plural(sessions.data.total, 'session')} · ${plural(sessions.data.frames, 'frame')}`
                  : 'Loading…'}
              </p>
              {sessions.data && sessions.data.total === 0 && (
                <p className="calibration-empty">No frames match these filters.</p>
              )}
              <CalibrationSessionList sessions={sessionList} selected={sessionId} showRig={rigs.length > 1}
                page={page} perPage={sessions.data?.per_page ?? SESSIONS_PER_PAGE} total={sessions.data?.total ?? 0}
                onSelect={(id) => selectSession(id)} onPage={state.setPage}
                flatGroups={flatsListed && flat.groups.data?.available
                  ? { index: flatIndex, open: ratio, onCompare: compareFlats }
                  : undefined} />
            </>
          )}
        </div>
        <div className="calibration-main">
          {ratio && flatsListed && (
            <FlatRatioViewer
              dbId={dbId}
              earlier={ratio.earlier}
              later={ratio.later}
              earlierSession={flatIndex.response(ratio.earlier)}
              laterSession={flatIndex.response(ratio.later)}
              title={pair.event?.what ?? pair.split?.reason ?? 'Two flat sessions'}
              spots={pair.event?.spots ?? pair.split?.spots ?? []}
              vignetting={pair.event?.vignetting ?? pair.split?.vignetting ?? []}
              showSpots={state.ratioSpots}
              showDust={state.dust}
              onShowSpots={state.setRatioSpots}
              onShowDust={state.setDust}
              stretch={state.stretch}
              onStretch={state.setStretch}
              onClose={closeRatio}
            />
          )}
          {browse === 'groups' && openError && (
            <p className="calibration-error" role="alert">Could not open the session: {openError}</p>
          )}
          {browse === 'groups' && groupEntry && (
            <FlatGroupHeader entry={groupEntry} scopeName={scope?.name} />
          )}
          {browse === 'groups' && groupEntry && frameBrowser}
          {browse === 'groups' && groupEntry && (
            <FlatGroupDetail key={groupEntry.key}
              dbId={dbId}
              entry={groupEntry}
              use={groupUse.data}
              useError={groupUse.error}
              scope={scope ? { projectId: scope.projectId, name: scope.name } : undefined}
              canManage={canManage}
              open={ratio}
              onCompare={compareFlats}
              onOpenSession={(id) => void openGroupSession(id)}
              onShowNight={showNight}
              shownSession={frames.find((frame) => frame.frame_uuid === frameUuid)?.session ?? null}
              opening={opening}
              edit={groupEdit}
            />
          )}
          {browse === 'sessions' && sessionId && (
            <div className="calibration-session-bar">
              <div>
                <h2>{session ? sessionTitle(session) : 'Session'}</h2>
                {session && (
                  <p className="calibration-muted">
                    {[nightLabel(session.night), session.rig_name, sessionDetails(session)].filter(Boolean).join(' · ')}
                  </p>
                )}
                {session?.kind === 'flat' && flatsListed && (
                  <FlatSessionGroupLine place={flatIndex.place(session.id)} unmeasured={flatIndex.unmeasured(session.id)} />
                )}
              </div>
              <PillSelect label="Order" value={filters.sort ?? 'oldest'}
                onChange={(event) => state.setFilters({ sort: event.target.value === 'oldest' ? undefined : event.target.value as typeof filters.sort })}>
                <option value="oldest">As shot</option>
                <option value="newest">Newest first</option>
                <option value="file">By file</option>
                <option value="grade">Worst first</option>
              </PillSelect>
              {canManage && sessionFrames.length > 0 && (
                <div className="calibration-session-actions">
                  <div className="tool-segmented" role="group" aria-label="Session serves lights">
                    <button type="button" disabled={actions.pending} title="Every frame serves only lights shot before it"
                      onClick={() => markSession('backward')}>◂ Before only</button>
                    <button type="button" disabled={actions.pending} title="Every frame serves lights on either side"
                      onClick={() => markSession('both')}>Both</button>
                    <button type="button" disabled={actions.pending} title="Every frame serves only lights shot after it"
                      onClick={() => markSession('forward')}>After only ▸</button>
                  </div>
                  <button type="button" className="tool-button tool-button-danger"
                    disabled={actions.pending} onClick={forgetSession}>
                    Forget session
                  </button>
                </div>
              )}
            </div>
          )}
          {browse === 'sessions' && sessionId && frameBrowser}
        </div>
      </div>
    </>
  );
}
