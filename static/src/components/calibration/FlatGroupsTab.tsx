import { Fragment, useEffect, useMemo, useRef } from 'react';
import { useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type {
  CalibrationQualityProgress,
  FlatChangeEvent,
  FlatFamily,
  FlatGroupsRig,
  FlatSplit,
} from '../../api/types';
import PillSelect from '../toolbar/PillSelect';
import ViewToolbar from '../toolbar/ViewToolbar';
import FlatRatioViewer from './FlatRatioViewer';
import {
  angleLabel,
  eventSeenIn,
  eventSideLabel,
  familyLabel,
  findPair,
  limitPercent,
  nightRange,
  percent,
  plural,
  sessionsByKey,
  windowLabel,
} from './flatGroupLabels';
import { ALL_RIGS, type useCalibrationUrlState } from './useCalibrationUrlState';
import './FlatGroups.css';

/** The quality job's stages that work on flat groups. */
const FLAT_STAGES = new Set([
  'measuring flat sessions',
  'comparing flat sessions',
  'reading lights shot during a change',
]);

/** How often the job's progress is asked for while it runs, and while it rests. */
const RUNNING_POLL_MS = 3_000;
const IDLE_POLL_MS = 30_000;

/** Two angles are one when they round to the same tenth of a degree. */
const sameAngle = (left: number | null, right: number | undefined) =>
  right === undefined || (left !== null && Math.abs(left - right) < 0.05);

interface FlatGroupsTabProps {
  dbId: string;
  state: ReturnType<typeof useCalibrationUrlState>;
}

/**
 * How each rig's flats group: nights whose flats measure alike share a
 * group, and a measured change, such as new dust, splits them. The rig,
 * filter, angle and the pair set side by side live in the URL.
 */
export default function FlatGroupsTab({ dbId, state }: FlatGroupsTabProps) {
  const queryClient = useQueryClient();
  const groups = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'flat-groups'],
    queryFn: () => apiClient.getFlatGroups(dbId),
  });
  const quality = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'quality'],
    queryFn: () => apiClient.getCalibrationQuality(dbId),
    refetchInterval: (query) => (query.state.data?.progress.running ? RUNNING_POLL_MS : IDLE_POLL_MS),
    retry: false,
  });
  const progress = quality.data?.progress;
  const measuring = !!progress?.running && FLAT_STAGES.has(progress.stage ?? '');

  // A pass that worked on flats has finished: read the groups again.
  const wasMeasuring = useRef(false);
  const lastFinished = useRef<number | null | undefined>(undefined);
  useEffect(() => {
    const finished = progress?.finished_at;
    const ended = wasMeasuring.current && !measuring;
    const newPass = lastFinished.current !== undefined && finished !== lastFinished.current;
    wasMeasuring.current = measuring;
    if (progress) lastFinished.current = finished;
    if (ended || newPass) {
      void queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibrations', 'flat-groups'] });
    }
  }, [dbId, measuring, progress, queryClient]);

  const { filters, rotation, ratio, setFilters, setRotation, openRatio } = state;
  const allRigs = useMemo(() => groups.data?.rigs ?? [], [groups.data]);
  const chosenRig = filters.rig && filters.rig !== ALL_RIGS ? filters.rig : undefined;
  const rigs = chosenRig && allRigs.some((rig) => rig.rig_uuid === chosenRig)
    ? allRigs.filter((rig) => rig.rig_uuid === chosenRig)
    : allRigs;
  const filterNames = useMemo(() => {
    const names = new Set<string>();
    for (const rig of rigs) {
      for (const family of rig.families) if (family.filter) names.add(family.filter);
      for (const entry of rig.unmeasured) if (entry.filter) names.add(entry.filter);
    }
    return [...names].sort();
  }, [rigs]);
  const angles = useMemo(() => {
    const values = new Set<number>();
    for (const rig of rigs) {
      for (const family of rig.families) if (family.rotation !== null) values.add(Math.round(family.rotation * 10) / 10);
    }
    return [...values].sort((left, right) => left - right);
  }, [rigs]);
  const sessions = useMemo(() => sessionsByKey(allRigs), [allRigs]);

  const keep = (entry: { filter: string | null; rotation: number | null }) =>
    (!filters.filter || entry.filter === filters.filter) && sameAngle(entry.rotation, rotation);
  const active = (chosenRig ? 1 : 0) + (filters.filter ? 1 : 0) + (rotation !== undefined ? 1 : 0);

  const pair = ratio ? findPair(allRigs, ratio.earlier, ratio.later) : null;
  const compare = (earlier: string, later: string) => openRatio({ earlier, later });

  return (
    <div className="flat-groups" role="tabpanel" aria-label="Flat groups">
      <ViewToolbar
        label="Flat group filters"
        className="calibration-filters"
        activeFilters={active}
        onClearFilters={() => state.clearGroupFilters(allRigs.length > 1 ? ALL_RIGS : null)}
        filters={(
          <>
            {allRigs.length > 1 && (
              <PillSelect label="Rig" active={!!chosenRig} value={chosenRig ?? ALL_RIGS}
                onChange={(event) => setFilters({ rig: event.target.value })}>
                <option value={ALL_RIGS}>All</option>
                {allRigs.map((rig) => <option key={rig.rig_uuid} value={rig.rig_uuid}>{rig.name}</option>)}
              </PillSelect>
            )}
            <PillSelect label="Filter" active={!!filters.filter} value={filters.filter ?? ''}
              onChange={(event) => setFilters({ filter: event.target.value || undefined })}>
              <option value="">All</option>
              {filterNames.map((name) => <option key={name} value={name}>{name}</option>)}
              {filters.filter && !filterNames.includes(filters.filter) && (
                <option value={filters.filter}>{filters.filter}</option>
              )}
            </PillSelect>
            <PillSelect label="Angle" active={rotation !== undefined} value={rotation === undefined ? '' : String(rotation)}
              onChange={(event) => setRotation(event.target.value === '' ? undefined : Number(event.target.value))}>
              <option value="">All</option>
              {angles.map((angle) => <option key={angle} value={String(angle)}>{angleLabel(angle)}</option>)}
              {rotation !== undefined && !angles.includes(rotation) && (
                <option value={String(rotation)}>{angleLabel(rotation)}</option>
              )}
            </PillSelect>
          </>
        )}
        summary={measuring && progress ? <MeasuringNote progress={progress} /> : undefined}
      />

      {groups.error && <p className="calibration-error">Could not read the flat groups: {groups.error.message}</p>}
      {groups.isPending && <p className="calibration-empty">Loading…</p>}

      {ratio && (
        <FlatRatioViewer
          dbId={dbId}
          earlier={ratio.earlier}
          later={ratio.later}
          earlierSession={sessions.get(ratio.earlier)}
          laterSession={sessions.get(ratio.later)}
          title={pair?.event?.what ?? pair?.split?.reason ?? 'Two flat sessions'}
          spots={pair?.event?.spots ?? pair?.split?.spots ?? []}
          stretch={state.stretch}
          onStretch={state.setStretch}
          onClose={() => openRatio(null)}
        />
      )}

      {groups.data && !groups.data.available && (
        <p className="calibration-notice">This catalog cannot be written, so flats are grouped by time.</p>
      )}
      {groups.data?.available && allRigs.length === 0 && (
        <p className="calibration-empty">No flats measured yet.</p>
      )}
      {groups.data?.available && rigs.map((rig) => (
        <RigGroups key={rig.rig_uuid} rig={rig} keep={keep} onCompare={compare} />
      ))}
    </div>
  );
}

function MeasuringNote({ progress }: { progress: CalibrationQualityProgress }) {
  const total = progress.stage_total ?? 0;
  const counts = total > 0 ? ` ${progress.stage_done ?? 0}/${total}` : '';
  return (
    <span className="flat-groups-measuring" role="status" data-testid="flat-groups-measuring">
      <span className="calibration-spinner" aria-hidden="true" />
      Measuring flats…{counts}
      {progress.stage && progress.stage !== 'measuring flat sessions' && (
        <span className="calibration-muted"> · {progress.stage}</span>
      )}
    </span>
  );
}

interface RigGroupsProps {
  rig: FlatGroupsRig;
  keep: (entry: { filter: string | null; rotation: number | null }) => boolean;
  onCompare: (earlier: string, later: string) => void;
}

function RigGroups({ rig, keep, onCompare }: RigGroupsProps) {
  const families = rig.families.filter(keep);
  const unmeasured = rig.unmeasured.filter(keep);
  return (
    <section className="flat-groups-rig" aria-label={rig.name} data-testid="flat-groups-rig">
      <header className="flat-groups-rig-head">
        <h2>{rig.name}</h2>
        {rig.measured ? (
          <p className="calibration-muted">
            Splits at {limitPercent(rig.dust_limit_pct)} dust, {limitPercent(rig.pattern_limit_pct)} light pattern
          </p>
        ) : (
          <p className="calibration-muted">Grouped by time (Settings › Calibration › Flat groups)</p>
        )}
        {rig.angle_term && (
          <p className="calibration-muted">
            Flats at different angles differ by up to {percent(rig.angle_term.broad_pct)} pattern,{' '}
            {percent(rig.angle_term.dust_pct)} dust ({plural(rig.angle_pairs, 'pair')})
          </p>
        )}
      </header>

      {rig.events.length > 0 && (
        <div className="flat-groups-block">
          <h3>Changes</h3>
          <ul className="flat-events" aria-label="Changes" data-testid="flat-change-events">
            {rig.events.map((event) => (
              <EventRow key={event.key} event={event} onCompare={onCompare} />
            ))}
          </ul>
        </div>
      )}

      {rig.measured && rig.families.length === 0 && (
        <p className="calibration-empty">No flats measured yet.</p>
      )}
      {rig.families.length > 0 && families.length === 0 && (
        <p className="calibration-empty">No flats of this rig match these filters.</p>
      )}
      {families.map((family) => (
        <FamilyGroups key={family.key} family={family} onCompare={onCompare} />
      ))}

      {unmeasured.length > 0 && (
        <div className="flat-groups-block">
          <h3>Not measured</h3>
          <ul className="flat-unmeasured" aria-label="Not measured">
            {unmeasured.map((entry) => (
              <li key={`${entry.night}:${entry.filter}:${entry.rotation}`}>
                {[
                  entry.night,
                  entry.filter ?? 'No filter',
                  angleLabel(entry.rotation),
                  plural(entry.frames, 'flat'),
                  entry.reason,
                ].join(' · ')}
              </li>
            ))}
          </ul>
        </div>
      )}
    </section>
  );
}

function EventRow({ event, onCompare }: { event: FlatChangeEvent; onCompare: (earlier: string, later: string) => void }) {
  return (
    <li className="flat-event" data-testid="flat-change-event">
      <div className="flat-event-text">
        <span className="calibration-muted">{windowLabel(event.start, event.end)}</span>
        <strong>{event.what}</strong>
        <span>
          dust {percent(event.dust_pct)} · pattern {percent(event.broad_pct)} · seen in {eventSeenIn(event)}
        </span>
        <span className="calibration-muted">{eventSideLabel(event)}</span>
      </div>
      <button type="button" className="tool-button" data-testid="flat-compare"
        onClick={() => onCompare(event.before_response, event.after_response)}>
        Compare
      </button>
    </li>
  );
}

function FamilyGroups({ family, onCompare }: { family: FlatFamily; onCompare: (earlier: string, later: string) => void }) {
  return (
    <div className="flat-groups-block flat-family" data-testid="flat-family">
      <h3>{familyLabel(family)}</h3>
      {family.groups.map((group, index) => {
        const split: FlatSplit | undefined = family.splits[index];
        return (
          <Fragment key={`${group.first_night}:${group.last_night}`}>
            <div className="flat-group" data-testid="flat-group">
              <div className="flat-group-head">
                {nightRange(group.first_night, group.last_night)} · {plural(group.sessions.length, 'night')} · {plural(group.frames, 'flat')}
              </div>
              <ul>
                {group.sessions.map((session) => (
                  <li key={session.response_key}>
                    <span className="flat-session-night">{session.night}</span>
                    <span>{plural(session.frames, 'flat')}</span>
                    {session.previous && (
                      <span className="calibration-muted">
                        dust {percent(session.previous.dust_pct)} · pattern {percent(session.previous.broad_pct)} against the night before
                      </span>
                    )}
                  </li>
                ))}
              </ul>
            </div>
            {split && index < family.groups.length - 1 && <SplitRow split={split} onCompare={onCompare} />}
          </Fragment>
        );
      })}
    </div>
  );
}

function SplitRow({ split, onCompare }: { split: FlatSplit; onCompare: (earlier: string, later: string) => void }) {
  return (
    <div className="flat-split" data-testid="flat-split">
      <p>
        <strong>Split</strong> · {split.reason}
        {split.event_key && <span className="calibration-muted"> · part of a change above</span>}
      </p>
      <button type="button" className="tool-button" data-testid="flat-compare"
        onClick={() => onCompare(split.before_response, split.after_response)}>
        Compare
      </button>
    </div>
  );
}
