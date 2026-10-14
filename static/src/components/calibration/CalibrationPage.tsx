import { useEffect, useMemo, useRef } from 'react';
import { keepPreviousData, useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type { CalibrationSession } from '../../api/types';
import { useDbProjectTarget } from '../../hooks/useUrlState';
import { useAllDatabases, useMergedProjects } from '../../hooks/useDatabases';
import { isTauriApp } from '../../utils/tauri';
import SchedulerFlatHistory from '../SchedulerFlatHistory';
import PillSelect from '../toolbar/PillSelect';
import CalibrationFilters from './CalibrationFilters';
import CalibrationFramePanel from './CalibrationFramePanel';
import CalibrationFrameStrip from './CalibrationFrameStrip';
import CalibrationFrameViewer from './CalibrationFrameViewer';
import CalibrationMasterList from './CalibrationMasterList';
import CalibrationSessionList from './CalibrationSessionList';
import { fileName, nightLabel, plural, sessionDetails, sessionTitle } from './calibrationLabels';
import { useCalibrationActions } from './useCalibrationActions';
import type { ValidityChoice } from './useCalibrationActions';
import { ALL_RIGS, listFilters, useCalibrationUrlState } from './useCalibrationUrlState';
import type { CalibrationTab } from './useCalibrationUrlState';
import './CalibrationPage.css';

const SESSIONS_PER_PAGE = 50;
/** Frames of one session shown in the strip, at most. */
const STRIP_FRAMES = 500;

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
  const { setDatabase, resetForDatabase, filters, setFilters } = state;

  // A page opened without a database takes the first one.
  useEffect(() => {
    if (!dbId && databases && databases.length > 0) setDatabase(databases[0].id);
  }, [databases, dbId, setDatabase]);

  // A project in scope opens on its rig: the one whose frames came from the
  // project's N.I.N.A. profile, when exactly one did, and only when the
  // library holds more than one rig: with one, the page shows no Rig choice.
  // A rig chosen this way gives way to the next project's; one a person
  // chose stays, so choosing All sticks.
  const summary = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId!),
    enabled: !!dbId,
  });
  const { data: projects } = useMergedProjects();
  const profile = projects.find((project) => project.db_id === dbId && project.id === projectId)?.profile_id;
  const shownDb = useRef(dbId);
  const openedFor = useRef<string | null>(null);
  const defaultRig = useRef<string | null>(null);
  useEffect(() => {
    let rig = filters.rig;
    if (shownDb.current !== dbId) {
      // Another database chosen in the header: what was chosen in this one
      // means nothing there. Its rig may stay in the URL a moment longer,
      // so it counts as no one's choice.
      if (shownDb.current !== null) {
        resetForDatabase();
        defaultRig.current = rig ?? null;
        rig = undefined;
      }
      shownDb.current = dbId;
      // Back in a database, a project opens on its rig again.
      openedFor.current = null;
    }
    const rigs = summary.data?.rigs;
    const scope = `${dbId}:${projectId}`;
    if (!dbId || !profile || !rigs || rigs.length < 2 || openedFor.current === scope) return;
    openedFor.current = scope;
    if (rig && rig !== defaultRig.current) return;
    const own = rigs.filter((entry) => entry.profile_ids?.includes(profile));
    defaultRig.current = own.length === 1 ? own[0].rig_uuid : ALL_RIGS;
    setFilters({ rig: defaultRig.current });
  }, [dbId, filters.rig, profile, projectId, resetForDatabase, setFilters, summary.data]);

  const database = databases?.find((entry) => entry.id === dbId);
  // The header's project picker chooses the database, as for every view.
  return (
    <div className="calibration-page">
      <header className="calibration-page-header">
        <div>
          <h1>Calibration</h1>
          {database && <span className="calibration-muted">{database.name}</span>}
        </div>
        <div className="calibration-tabs" role="tablist" aria-label="Calibration views">
          {TABS.map((tab) => (
            <button key={tab.value} type="button" role="tab" aria-selected={state.tab === tab.value}
              onClick={() => state.setTab(tab.value)}>
              {tab.label}
            </button>
          ))}
        </div>
      </header>
      {!dbId ? (
        <p className="calibration-empty">{databases?.length === 0 ? 'No databases yet.' : 'Loading…'}</p>
      ) : (
        <CalibrationLibrary key={dbId} dbId={dbId} dbName={database?.name ?? dbId} state={state} />
      )}
    </div>
  );
}

function CalibrationLibrary({
  dbId,
  dbName,
  state,
}: {
  dbId: string;
  dbName: string;
  state: ReturnType<typeof useCalibrationUrlState>;
}) {
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  // Writes need the role's write access as well as the server's
  // database-management gate, as in Stacks.
  const access = useAccess();
  const canManage = access.canWrite && (isTauriApp() || (serverInfo?.allow_database_management ?? false));
  const actions = useCalibrationActions(dbId);
  const summary = useQuery({
    queryKey: ['db', dbId, 'calibrations'],
    queryFn: () => apiClient.getCalibrationLibrary(dbId),
  });

  if (state.tab === 'scheduler') {
    return <SchedulerFlatHistory dbId={dbId} canManage={canManage} />;
  }
  if (state.tab === 'masters') {
    return (
      <CalibrationMasterList dbId={dbId} dbName={dbName} canManage={canManage}
        opened={state.master} actions={actions} onOpen={state.openMaster} />
    );
  }
  return (
    <>
      {!canManage && (
        <p className="calibration-notice">
          {access.canWrite
            ? 'This server is read-only: frames can be viewed, not changed.'
            : 'Your account can view frames, not change them.'}
        </p>
      )}
      {summary.data && summary.data.frame_count === 0 ? (
        <p className="calibration-empty">
          No calibration frames yet. Add their folders to this database in Settings, then import.
        </p>
      ) : (
        <CalibrationFrames dbId={dbId} canManage={canManage} actions={actions}
          rigs={summary.data?.rigs ?? []} state={state} />
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
}: {
  dbId: string;
  canManage: boolean;
  actions: ReturnType<typeof useCalibrationActions>;
  rigs: NonNullable<Awaited<ReturnType<typeof apiClient.getCalibrationLibrary>>>['rigs'];
  state: ReturnType<typeof useCalibrationUrlState>;
}) {
  const { filters, page, session: sessionId, frame: frameUuid, selectSession, selectFrame } = state;
  const listed = useMemo(() => listFilters(filters), [filters]);
  const sessions = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'sessions', listed, page],
    queryFn: () => apiClient.getCalibrationSessions(dbId, { ...listed, page, per_page: SESSIONS_PER_PAGE }),
    // Keep the old list while a new one loads, so nothing below it jumps.
    placeholderData: keepPreviousData,
  });
  const sessionList = useMemo(() => sessions.data?.sessions ?? [], [sessions.data]);
  const strip = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frames', listed, sessionId, filters.sort],
    queryFn: () => apiClient.getCalibrationFrames(dbId, {
      ...listed,
      session: sessionId ?? undefined,
      sort: filters.sort ?? 'oldest',
      per_page: STRIP_FRAMES,
    }),
    enabled: !!sessionId,
    placeholderData: (previous, query) =>
      // The last strip stays while the same session reloads, not another's.
      query?.queryKey[5] === sessionId ? previous : undefined,
  });
  const frames = useMemo(() => strip.data?.frames ?? [], [strip.data]);
  // The last frame's detail stays, faded, while the next one loads, so the
  // panel keeps its height.
  const detail = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'frame', frameUuid],
    queryFn: () => apiClient.getCalibrationFrame(dbId, frameUuid!),
    enabled: !!frameUuid,
    placeholderData: keepPreviousData,
  });

  // The first session and its first frame are chosen until a person
  // chooses; a session or frame the filters or the page drop gives way to
  // the first one left.
  useEffect(() => {
    if (sessions.isPlaceholderData || sessionList.length === 0) return;
    if (!sessionId || !sessionList.some((entry) => entry.id === sessionId)) {
      selectSession(sessionList[0].id);
    }
  }, [selectSession, sessionId, sessionList, sessions.isPlaceholderData]);
  useEffect(() => {
    if (!sessionId || !strip.data || strip.isPlaceholderData) return;
    if (frames.length === 0) return;
    if (!frameUuid || !frames.some((frame) => frame.frame_uuid === frameUuid)) {
      selectFrame(frames[0].frame_uuid);
    }
  }, [frameUuid, frames, selectFrame, sessionId, strip.data, strip.isPlaceholderData]);

  const session: CalibrationSession | undefined = sessionList.find((entry) => entry.id === sessionId);
  const frameIndex = frames.findIndex((frame) => frame.frame_uuid === frameUuid);
  const current = detail.data && detail.data.frame_uuid === frameUuid ? detail.data : undefined;
  const shownDetail = frameUuid ? detail.data : undefined;
  // Whether the frame can be set against its session. While the next
  // frame's detail loads, the last one of the same session answers, so
  // stepping through a session in compare mode stays in it.
  const compare = (current ?? (shownDetail?.session === sessionId ? shownDetail : undefined))?.compare ?? false;
  const sessionIndex = sessionList.findIndex((entry) => entry.id === sessionId);

  const hotkeys = { enabled: !state.master, preventDefault: true };
  useHotkeys('left', () => {
    if (frameIndex > 0) selectFrame(frames[frameIndex - 1].frame_uuid);
  }, hotkeys, [frameIndex, frames, selectFrame]);
  useHotkeys('right', () => {
    if (frameIndex >= 0 && frameIndex < frames.length - 1) selectFrame(frames[frameIndex + 1].frame_uuid);
  }, hotkeys, [frameIndex, frames, selectFrame]);
  useHotkeys('up', () => {
    if (sessionIndex > 0) selectSession(sessionList[sessionIndex - 1].id);
  }, hotkeys, [selectSession, sessionIndex, sessionList]);
  useHotkeys('down', () => {
    if (sessionIndex >= 0 && sessionIndex < sessionList.length - 1) selectSession(sessionList[sessionIndex + 1].id);
  }, hotkeys, [selectSession, sessionIndex, sessionList]);
  useHotkeys('c', () => {
    if (compare) state.setView(state.view === 'session' ? 'frame' : 'session');
  }, { enabled: !state.master }, [compare, state.setView, state.view]);

  // Keep the chosen session in sight in its list as the keys move it.
  const listRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    const list = listRef.current?.querySelector('ul');
    const item = sessionId ? list?.querySelector<HTMLElement>(`[data-session-id="${sessionId}"]`) : null;
    if (!list || !item) return;
    const top = item.offsetTop;
    const bottom = top + item.offsetHeight;
    if (top < list.scrollTop) list.scrollTop = top;
    else if (bottom > list.scrollTop + list.clientHeight) list.scrollTop = bottom - list.clientHeight;
  }, [sessionId, sessionList.length]);

  const markSession = (direction: ValidityChoice) =>
    actions.mark.mutate({ frameUuids: frames.map((frame) => frame.frame_uuid), direction });
  const forgetSession = () => {
    if (!session) return;
    if (window.confirm(`Forget these ${plural(frames.length, 'frame')}? Their catalog records and the masters built from them go. The files stay.`)) {
      actions.forget.mutate(frames.map((frame) => frame.frame_uuid), {
        onSuccess: () => selectSession(null),
      });
    }
  };
  const nextAfterForget = () => {
    const next = frames[frameIndex + 1] ?? frames[frameIndex - 1];
    selectFrame(next ? next.frame_uuid : null);
  };
  const spots = current?.measurement?.type === 'flat' && current.measurement.features
    ? { width: current.measurement.width, height: current.measurement.height, features: current.measurement.features }
    : undefined;

  return (
    <>
      <CalibrationFilters filters={filters} facets={sessions.data?.facets} rigs={rigs}
        onChange={state.setFilters} onClear={state.clearFilters} />
      {sessions.error && <p className="calibration-error">Could not read the sessions: {sessions.error.message}</p>}
      <div className="calibration-layout">
        <div className="calibration-sessions-column" ref={listRef}>
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
            onSelect={(id) => selectSession(id)} onPage={state.setPage} />
        </div>
        <div className="calibration-main">
          {sessionId && (
            <div className="calibration-session-bar">
              <div>
                <h2>{session ? sessionTitle(session) : 'Session'}</h2>
                {session && (
                  <p className="calibration-muted">
                    {[nightLabel(session.night), session.rig_name, sessionDetails(session)].filter(Boolean).join(' · ')}
                  </p>
                )}
              </div>
              <PillSelect label="Order" value={filters.sort ?? 'oldest'}
                onChange={(event) => state.setFilters({ sort: event.target.value === 'oldest' ? undefined : event.target.value as typeof filters.sort })}>
                <option value="oldest">As shot</option>
                <option value="newest">Newest first</option>
                <option value="file">By file</option>
                <option value="grade">Worst first</option>
              </PillSelect>
              {canManage && frames.length > 0 && (
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
          {sessionId && strip.data && frames.length === 0 && (
            <p className="calibration-empty">No frames of this session match the filters.</p>
          )}
          {frames.length > 0 && (
            <CalibrationFrameStrip dbId={dbId} frames={frames} selected={frameUuid}
              stretch={state.stretch} onSelect={selectFrame} />
          )}
          {frameUuid && (
            <div className="calibration-frame">
              <CalibrationFrameViewer
                dbId={dbId}
                frameUuid={frameUuid}
                sessionId={sessionId ?? ''}
                kind={current?.kind ?? frames[frameIndex]?.kind ?? 'flat'}
                label={current ? fileName(current.source_path) : 'Calibration frame'}
                view={state.view}
                stretch={state.stretch}
                full={state.full}
                canCompare={compare}
                spots={spots}
                onView={state.setView}
                onStretch={state.setStretch}
                onFull={state.setFull}
              />
              <CalibrationFramePanel
                detail={shownDetail}
                stale={!current}
                loading={detail.isLoading}
                error={detail.error ? detail.error.message : null}
                canManage={canManage}
                actions={actions}
                onForgotten={nextAfterForget}
              />
            </div>
          )}
          {frameIndex >= 0 && frames.length > 1 && (
            <p className="calibration-muted calibration-hint">
              Frame {frameIndex + 1} of {frames.length} · ← → frames · ↑ ↓ sessions{compare ? ' · C compare' : ''}
            </p>
          )}
        </div>
      </div>
    </>
  );
}
