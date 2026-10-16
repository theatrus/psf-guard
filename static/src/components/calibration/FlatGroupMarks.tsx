import { useState } from 'react';
import { useMutation, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import type {
  FlatMeasureStatus,
  FlatGroupsRig,
  FlatGroupsView,
  FlatUnmeasuredSession,
} from '../../api/types';
import type { FlatMark, FlatSessionPlace } from './flatGroupIndex';
import { editFlatLimits } from './flatLimitsLink';
import { FLAT_STAGES, flatMeasureKey, flatMeasureLabel, stoppedRig } from './flatMeasureLabel';
import {
  eventSeenIn,
  eventSideLabel,
  familyLabel,
  groupLabel,
  limitPercent,
  nightRange,
  percent,
  plural,
  windowLabel,
} from './flatGroupLabels';
import './FlatGroups.css';

/** "Measuring flats · 12 of 40 sessions", with the rig when the page shows
 *  several, and a word while the pass waits for a person's work. */
export function MeasuringNote({ progress, showRig }: { progress: FlatMeasureStatus; showRig: boolean }) {
  const extra = [
    showRig && progress.rig_name ? progress.rig_name : null,
    progress.waiting_for ? `waiting for ${progress.waiting_for}` : null,
  ].filter(Boolean);
  // The counts tick every poll; the live region beside it says only when
  // the stage changes.
  return (
    <span className="flat-groups-measuring" aria-hidden="true" data-testid="flat-groups-measuring"
      data-priority={progress.priority ?? undefined}>
      <span className="calibration-spinner" aria-hidden="true" />
      {flatMeasureLabel(progress)}
      {extra.length > 0 && <span className="calibration-muted"> · {extra.join(' · ')}</span>}
    </span>
  );
}

/** Stop in the job line paused measuring: say how it comes back, and offer
 *  to take it up now. */
function StoppedNote({ dbId }: { dbId: string }) {
  const access = useAccess();
  const queryClient = useQueryClient();
  const resume = useMutation({
    mutationFn: () => apiClient.resumeFlatMeasure(dbId),
    onSuccess: (status) => queryClient.setQueryData(flatMeasureKey(dbId), status),
  });
  return (
    <p className="calibration-muted flat-measure-stopped" data-testid="flat-measure-stopped">
      Flat measuring is stopped. It resumes when these flats change, or when the server starts
      again.
      {access.canWrite && (
        <>
          {' '}
          <button type="button" className="link-button" data-testid="flat-measure-resume"
            disabled={resume.isPending} onClick={() => resume.mutate()}>
            Resume now
          </button>
        </>
      )}
    </p>
  );
}

interface FlatGroupsSummaryProps {
  dbId: string;
  view: FlatGroupsView | undefined;
  error: Error | null;
  /** The rigs in view: the one the Rig filter chose, or every one. */
  rigs: FlatGroupsRig[];
  /** The flat measuring job. */
  progress: FlatMeasureStatus | undefined;
  /** Whether it measures a rig in view now. */
  measuring: boolean;
  canManage: boolean;
}

/**
 * How the flats in view group, above Frames' sessions: each rig's limits
 * and angle term with a way to change them, what is not measured yet, and
 * whether the background pass is measuring now.
 */
/**
 * What a screen reader hears about the background pass: each stage as it
 * starts, and when it is done. Always mounted, so the first announcement
 * is not lost, and quiet between stages.
 */
function useMeasuringAnnouncement(measuring: boolean, stage: string | null | undefined): string {
  const [was, setWas] = useState(measuring);
  const [finished, setFinished] = useState(false);
  if (was !== measuring) {
    setWas(measuring);
    setFinished(was && !measuring);
  }
  if (measuring) return `Measuring flats: ${stage ?? FLAT_STAGES.measuring}`;
  return finished ? 'Flats measured; the groups are up to date.' : '';
}

export function FlatGroupsSummary({ dbId, view, error, rigs, progress, measuring, canManage }: FlatGroupsSummaryProps) {
  const announcement = useMeasuringAnnouncement(measuring, progress?.stage);
  const live = (
    <span className="visually-hidden" role="status" data-testid="flat-groups-status">{announcement}</span>
  );
  if (error) {
    return (
      <>
        {live}
        <p className="calibration-error">Could not read the flat groups: {error.message}</p>
      </>
    );
  }
  if (!view) return live;
  const showName = rigs.length > 1;
  const stopped = !measuring && stoppedRig(progress, rigs.length === 1 ? rigs[0].rig_uuid : undefined);
  return (
    <section className="flat-groups-summary" aria-label="Flat groups" data-testid="flat-groups-summary">
      {live}
      {measuring && progress && <MeasuringNote progress={progress} showRig={showName} />}
      {stopped && <StoppedNote dbId={dbId} />}
      {!view.available && (
        <p className="calibration-muted">This catalog cannot be written, so flats are grouped by time.</p>
      )}
      {view.available && rigs.length === 0 && !measuring && (
        <p className="calibration-muted">No flats measured yet.</p>
      )}
      {view.available && rigs.map((rig) => {
        const waiting = rig.unmeasured.filter((entry) => entry.reason === 'not measured yet').length;
        const tooFew = rig.unmeasured.length - waiting;
        const parts = [
          rig.measured
            ? `Splits at ${limitPercent(rig.dust_limit_pct)} dust, ${limitPercent(rig.pattern_limit_pct)} light pattern`
            : 'Grouped by time',
          rig.angle_term
            ? `angle term ${percent(rig.angle_term.dust_pct)} dust, ${percent(rig.angle_term.broad_pct)} pattern (${plural(rig.angle_pairs, 'pair')})`
            : null,
          rig.measured && rig.families.length === 0 ? 'no flats measured yet' : null,
          waiting > 0 ? `${plural(waiting, 'session')} not measured yet` : null,
          tooFew > 0 ? `${plural(tooFew, 'session')} with too few flats to measure` : null,
        ].filter(Boolean);
        return (
          <p key={rig.rig_uuid} className="flat-rig-summary" data-testid="flat-rig-summary">
            {showName && <strong>{rig.name}: </strong>}
            {parts.join(' · ')}
            {canManage && (
              <>
                {' · '}
                <button type="button" className="link-button" data-testid="flat-edit-limits"
                  aria-label={`Edit limits for ${rig.name}`} onClick={() => editFlatLimits(rig)}>
                  Edit limits
                </button>
              </>
            )}
          </p>
        );
      })}
    </section>
  );
}

/** A flat session's group, as its row in the session list shows it. */
export function FlatGroupBadge({
  place,
  unmeasured,
}: {
  place?: FlatSessionPlace;
  unmeasured?: FlatUnmeasuredSession;
}) {
  if (place) {
    return (
      <span className="flat-group-badge" data-tone={place.tone} data-testid="flat-group-badge">
        {place.rig.measured ? groupLabel(place.number, place.group) : 'Grouped by time'}
      </span>
    );
  }
  if (unmeasured) {
    return (
      <span className="flat-group-badge" data-tone="none" data-testid="flat-group-badge">
        {unmeasured.reason === 'not measured yet' ? 'Not measured yet' : `Not measured: ${unmeasured.reason}`}
      </span>
    );
  }
  return null;
}

/** The chosen flat session's group, beside its title. */
export function FlatSessionGroupLine({
  place,
  unmeasured,
}: {
  place?: FlatSessionPlace;
  unmeasured?: FlatUnmeasuredSession;
}) {
  if (unmeasured && !place) {
    return (
      <p className="calibration-muted" data-testid="flat-session-group">
        Flat groups: not measured{unmeasured.reason === 'not measured yet' ? ' yet' : `, ${unmeasured.reason}`}
      </p>
    );
  }
  if (!place) return null;
  const { group, family } = place;
  const previous = place.session.previous;
  return (
    <p className="flat-session-group" data-tone={place.tone} data-testid="flat-session-group">
      <span className="flat-group-swatch" aria-hidden="true" />
      <span>
        {place.rig.measured ? `Group ${place.number} of ${place.groups}` : 'Grouped by time'} in {familyLabel(family)}
        {' · '}{nightRange(group.first_night, group.last_night)} · {plural(group.sessions.length, 'night')} · {plural(group.frames, 'flat')}
        {previous && (
          <span className="calibration-muted">
            {' · '}dust {percent(previous.dust_pct)}, pattern {percent(previous.broad_pct)} against the night before
          </span>
        )}
      </span>
    </p>
  );
}

interface FlatMarkRowProps {
  mark: FlatMark;
  /** The nights of each response, for the button's name. */
  nightOf: (responseKey: string) => string | undefined;
  /** Whether this mark's pair is the one compared now. */
  open: boolean;
  /** `opener` is the button pressed, for focus to return to. */
  onCompare: (earlier: string, later: string, opener: HTMLElement) => void;
}

/** A split or a change of the whole rig, between two sessions of the list. */
export function FlatMarkRow({ mark, nightOf, open, onCompare }: FlatMarkRowProps) {
  const [earlier, later] = mark.kind === 'event'
    ? [mark.event.before_response, mark.event.after_response]
    : [mark.split.before_response, mark.split.after_response];
  const pair = [nightOf(later), nightOf(earlier)].every(Boolean)
    ? ` ${nightOf(later)} with ${nightOf(earlier)}`
    : '';
  const compare = (
    <button
      type="button"
      className="tool-button flat-mark-compare"
      data-testid="flat-compare"
      aria-pressed={open}
      aria-label={`Compare${pair}`}
      onClick={(event) => onCompare(earlier, later, event.currentTarget)}
    >
      Compare
    </button>
  );
  if (mark.kind === 'event') {
    const { event } = mark;
    return (
      <li className="flat-mark" data-kind="event" data-testid="flat-change-event"
        aria-label={`Change in ${mark.rig.name}: ${event.what}`}>
        <div className="flat-mark-text">
          <strong>Change · {eventSideLabel(event)}</strong>
          <span>{event.what}</span>
          <span>dust {percent(event.dust_pct)} · pattern {percent(event.broad_pct)} · seen in {eventSeenIn(event)}</span>
          <span className="calibration-muted">{windowLabel(event.start, event.end, mark.rig.clock)}</span>
          {event.within_flat_run && (
            <span data-testid="flat-change-within-run">
              Narrower than one flat run: it landed between one twilight&apos;s flats, or two changes met there.
            </span>
          )}
        </div>
        {compare}
      </li>
    );
  }
  return (
    <li className="flat-mark" data-kind="split" data-testid="flat-split"
      aria-label={`Split in ${familyLabel(mark.family)}`}>
      <div className="flat-mark-text">
        <strong>Split · {familyLabel(mark.family)}</strong>
        <span>{mark.split.reason}</span>
      </div>
      {compare}
    </li>
  );
}
