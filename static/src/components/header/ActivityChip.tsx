import { useEffect, useId, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { Link } from 'react-router-dom';
import { apiClient } from '../../api/client';
import { useAccess } from '../../auth/access';
import { STACK_ACTIVITY_QUERY_KEY } from '../../hooks/useStackActivity';
import { lineLengths, type ActivityControl, type ActivityItem, type ActivityQueue } from './activityItems';
import { useHeaderActivity, WBPP_ACTIVITY_QUERY_KEY, type ScanErrorNote } from './useHeaderActivity';
import './header.css';

/** How long the list stays after the pointer leaves, so it can be reached. */
const HOVER_CLOSE_MS = 200;

function ProgressRing({ percent, idle }: { percent: number | null; idle: boolean }) {
  const radius = 7;
  const circumference = 2 * Math.PI * radius;
  return (
    <svg
      className={`activity-ring${percent == null && !idle ? ' is-indeterminate' : ''}`}
      viewBox="0 0 18 18"
      width="18"
      height="18"
      aria-hidden="true"
    >
      <circle className="activity-ring-track" cx="9" cy="9" r={radius} />
      <circle
        className="activity-ring-fill"
        cx="9"
        cy="9"
        r={radius}
        strokeDasharray={circumference}
        // Nothing running: an empty ring, not a spinner caught mid-turn.
        strokeDashoffset={
          percent != null
            ? circumference * (1 - percent / 100)
            : idle
              ? circumference
              : circumference * 0.7
        }
      />
    </svg>
  );
}

type ActivityAction =
  | { type: 'move'; control: ActivityControl; position: number }
  | { type: 'stop'; control: ActivityControl };

function runAction(action: ActivityAction): Promise<unknown> {
  const { control } = action;
  if (action.type === 'move') {
    if (control.kind === 'stack') return apiClient.moveStackJob(control.jobId, action.position);
    if (control.kind === 'wbpp-queued') return apiClient.moveQueuedWbppRun(control.queueId, action.position);
    return Promise.resolve();
  }
  if (control.kind === 'stack') return apiClient.cancelStackJob(control.jobId);
  if (control.kind === 'wbpp-running') return apiClient.cancelWbppRun(control.dbId);
  return apiClient.removeQueuedWbppRun(control.dbId, control.queueId);
}

interface RowControls {
  /** Whether this person may change this row's line. */
  allowed: (item: ActivityItem) => boolean;
  run: (action: ActivityAction) => void;
  busy: boolean;
  lengths: Record<ActivityQueue, number>;
  /** Close the list as a link takes the person away. */
  leave: () => void;
}

/** A scan's frame errors, with the way to its target and to dismiss it. */
function ScanErrorRow({
  note,
  onDismiss,
  onLeave,
}: {
  note: ScanErrorNote;
  onDismiss: (dbId: string) => void;
  onLeave: () => void;
}) {
  return (
    <div className="activity-error" role="note">
      <span className="activity-error-text">
        A quality scan finished with errors. {note.message}
        {note.href && (
          <>
            {' '}
            <Link to={note.href} className="activity-error-link" onClick={onLeave}>
              Open the sequence
            </Link>
          </>
        )}
      </span>
      <button
        type="button"
        className="activity-error-dismiss"
        aria-label={`Dismiss the quality scan errors in ${note.dbName}`}
        title="Dismiss"
        onClick={() => onDismiss(note.dbId)}
      >
        ✕
      </button>
    </div>
  );
}

function ActivityRow({ item, controls }: { item: ActivityItem; controls: RowControls }) {
  const [confirming, setConfirming] = useState(false);
  const control = item.control;
  const allowed = !!control && controls.allowed(item);
  const waiting = item.position != null && !!item.queue;
  const last = item.queue ? controls.lengths[item.queue] - 1 : 0;
  const name = `${item.title}: ${item.scope}`;
  return (
    <li className={`activity-row${item.queued ? ' is-queued' : ''}`} title={item.hint}>
      <div className="activity-row-head">
        <span className="activity-row-title">{item.title}</span>
        <span className="activity-row-state">
          {item.queued
            ? waiting
              ? `queued · ${item.position! + 1}`
              : 'queued'
            : item.percent != null ? `${Math.round(item.percent)}%` : 'working'}
        </span>
      </div>
      <div className="activity-row-scope">
        {item.href ? (
          <Link
            to={item.href}
            className="activity-row-link"
            aria-label={`Go to ${name}`}
            title="Go to it"
            onClick={controls.leave}
          >
            {item.scope}
          </Link>
        ) : (
          item.scope
        )}
        {item.automatic && <span className="activity-row-tag">automatic</span>}
      </div>
      {!item.queued && (
        <div className="activity-row-bar" aria-hidden="true">
          <div
            className={`activity-row-fill${item.percent == null ? ' is-indeterminate' : ''}`}
            style={item.percent == null ? undefined : { width: `${item.percent}%` }}
          />
        </div>
      )}
      <div className="activity-row-foot">
        <div className="activity-row-detail">{item.detail}</div>
        {allowed && control && (item.queued || item.stoppable) && (
          <div className="activity-row-actions">
            {waiting && (
              <>
                <button
                  type="button"
                  aria-label={`Run ${name} earlier`}
                  title="Earlier"
                  disabled={controls.busy || item.position === 0}
                  onClick={() => controls.run({ type: 'move', control, position: item.position! - 1 })}
                >
                  ↑
                </button>
                <button
                  type="button"
                  aria-label={`Run ${name} later`}
                  title="Later"
                  disabled={controls.busy || item.position === last}
                  onClick={() => controls.run({ type: 'move', control, position: item.position! + 1 })}
                >
                  ↓
                </button>
              </>
            )}
            {item.queued ? (
              <button
                type="button"
                className="activity-row-stop"
                aria-label={`Remove ${name} from the line`}
                title="Remove from the line"
                disabled={controls.busy}
                onClick={() => controls.run({ type: 'stop', control })}
              >
                ✕
              </button>
            ) : confirming ? (
              <>
                <button
                  type="button"
                  className="activity-row-stop"
                  disabled={controls.busy}
                  onClick={() => {
                    setConfirming(false);
                    controls.run({ type: 'stop', control });
                  }}
                >
                  Stop it
                </button>
                <button type="button" onClick={() => setConfirming(false)}>
                  Keep
                </button>
              </>
            ) : (
              <button
                type="button"
                className="activity-row-stop"
                aria-label={`Stop ${name}`}
                disabled={controls.busy}
                // Stopping running work loses it, so it asks once.
                onClick={() => setConfirming(true)}
              >
                Stop
              </button>
            )}
          </div>
        )}
      </div>
    </li>
  );
}

/**
 * Background work in the header, kept small: the overall progress and how
 * many jobs there are. Hover, focus or click opens the queue: every catalog
 * refresh, quality scan and stack build, running or queued, on any database.
 * Each row links to what it works on. A scan's frame errors stay, on the
 * busy chip or as "Finished with errors", until dismissed.
 */
export default function ActivityChip() {
  const { items, summary, finished, scanErrors, dismissScanError } = useHeaderActivity();
  const scanError = scanErrors[scanErrors.length - 1];
  const access = useAccess();
  const { data: serverInfo } = useQuery({
    queryKey: ['serverInfo'],
    queryFn: apiClient.getServerInfo,
    staleTime: 5 * 60 * 1000,
  });
  const queryClient = useQueryClient();
  const action = useMutation({
    mutationFn: runAction,
    // Every route answers with the line as it now stands; showing it at
    // once keeps a second click from acting on the old positions.
    onSuccess: (result, variables) => {
      if (!result || typeof result !== 'object') return;
      if (variables.control.kind === 'stack') {
        queryClient.setQueryData(STACK_ACTIVITY_QUERY_KEY, result);
      } else if (variables.type === 'move') {
        queryClient.setQueryData(WBPP_ACTIVITY_QUERY_KEY, result);
      }
    },
    onSettled: () => {
      queryClient.invalidateQueries({ queryKey: STACK_ACTIVITY_QUERY_KEY });
      queryClient.invalidateQueries({ queryKey: WBPP_ACTIVITY_QUERY_KEY });
    },
  });
  const [pinned, setPinned] = useState(false);
  const [hovered, setHovered] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const wrapper = useRef<HTMLDivElement>(null);
  const listId = useId();
  const open = (pinned || hovered) && (summary.count > 0 || scanErrors.length > 0);

  useEffect(() => () => {
    if (closeTimer.current != null) window.clearTimeout(closeTimer.current);
  }, []);

  // A pinned list closes on a click elsewhere. Escape is handled on the
  // chip itself, so it never takes the key from the view underneath.
  useEffect(() => {
    if (!open) return;
    const onPointer = (event: PointerEvent) => {
      if (!wrapper.current?.contains(event.target as Node)) {
        setPinned(false);
        setHovered(false);
      }
    };
    document.addEventListener('pointerdown', onPointer);
    return () => document.removeEventListener('pointerdown', onPointer);
  }, [open]);

  // Nothing left to show: forget the pin, so the next job starts closed.
  useEffect(() => {
    if (summary.count === 0 && scanErrors.length === 0) {
      setPinned(false);
      setHovered(false);
    }
  }, [summary.count, scanErrors.length]);

  // Always mounted, so a screen reader hears when work ends or fails.
  const announcement = finished
    ? finished.errors
      ? `Background jobs finished with errors. ${scanErrors.map((note) => note.message).join('. ')}`
      : 'Background jobs finished.'
    : scanError?.message
      ? `A quality scan finished with errors. ${scanError.message}`
      : '';
  const live = <span className="activity-live" aria-live="polite">{announcement}</span>;

  // Idle with errors held: the chip stays, as a button that lists them.
  const idleWithErrors = summary.count === 0 && scanErrors.length > 0;
  if (summary.count === 0 && !idleWithErrors && !finished) return live;

  if (summary.count === 0 && !idleWithErrors && finished) {
    return (
      <div className="activity-chip-slot">
        {live}
        <span className="header-button utility-button activity-chip is-finished">
          <span className="activity-chip-mark" aria-hidden="true">✓</span>
          <span className="activity-chip-text">Done</span>
        </span>
      </div>
    );
  }

  // Stack builds are anyone's who can write; WBPP runs also need the
  // server's database management, as on the run dialog.
  const controls: RowControls = {
    allowed: (item) =>
      access.canWrite
      && (item.control?.kind === 'stack' || !!serverInfo?.allow_database_management),
    run: (next) => {
      // A control was used, so the list stays open after the pointer leaves.
      setPinned(true);
      action.mutate(next);
    },
    busy: action.isPending,
    lengths: lineLengths(items),
    leave: () => {
      setPinned(false);
      setHovered(false);
    },
  };

  const jobs = `${summary.count} job${summary.count === 1 ? '' : 's'}`;
  const percent = summary.percent == null ? null : Math.round(summary.percent);
  const running = summary.count - summary.queued;
  const label = [
    percent != null ? `${percent}% done` : 'Working',
    `${running} running`,
    summary.queued > 0 ? `${summary.queued} queued` : '',
    scanError ? 'a quality scan had errors' : '',
  ].filter(Boolean).join(', ');

  const hover = (inside: boolean) => {
    if (closeTimer.current != null) {
      window.clearTimeout(closeTimer.current);
      closeTimer.current = null;
    }
    if (inside) setHovered(true);
    else closeTimer.current = window.setTimeout(() => setHovered(false), HOVER_CLOSE_MS);
  };

  return (
    <div
      ref={wrapper}
      className="activity-chip-slot"
      onPointerEnter={(event) => event.pointerType === 'mouse' && hover(true)}
      onPointerLeave={(event) => event.pointerType === 'mouse' && hover(false)}
      onKeyDown={(event) => {
        if (event.key === 'Escape' && open) {
          event.stopPropagation();
          setPinned(false);
          setHovered(false);
        }
      }}
    >
      {live}
      <button
        type="button"
        className={`header-button utility-button activity-chip${scanError ? ' has-errors' : ''}${
          idleWithErrors ? ' is-finished' : ''
        }`}
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={idleWithErrors ? 'Background jobs: finished with errors' : `Background jobs: ${label}`}
        title={idleWithErrors ? scanErrors.map((note) => note.message).join('\n') : undefined}
        // The first click keeps the list open after the pointer leaves; the
        // second closes it.
        onClick={() => {
          if (pinned) {
            setPinned(false);
            setHovered(false);
          } else {
            setPinned(true);
          }
        }}
        onFocus={() => hover(true)}
        onBlur={(event) => {
          if (!wrapper.current?.contains(event.relatedTarget as Node)) hover(false);
        }}
      >
        {scanError && <span className="activity-chip-mark" aria-hidden="true" title={scanError.message}>!</span>}
        {idleWithErrors ? (
          <span className="activity-chip-text">Finished with errors</span>
        ) : (
          <>
            <ProgressRing percent={summary.percent} idle={running === 0} />
            {percent != null && <span className="activity-chip-percent">{percent}%</span>}
            <span className="activity-chip-count">{jobs}</span>
          </>
        )}
      </button>
      {open && (
        <div id={listId} className="activity-popover" role="region" aria-label="Background jobs">
          <div className="activity-popover-head">
            <span>Background jobs</span>
            <span className="activity-popover-counts">
              {idleWithErrors
                ? 'Nothing running'
                : `${running} running${summary.queued > 0 ? ` · ${summary.queued} queued` : ''}`}
            </span>
          </div>
          {scanErrors.map((note) => (
            <ScanErrorRow
              key={note.dbId}
              note={note}
              onDismiss={dismissScanError}
              onLeave={controls.leave}
            />
          ))}
          {action.isError && (
            <p className="activity-error" role="alert">{(action.error as Error).message}</p>
          )}
          {items.length > 0 && (
            <ul className="activity-list">
              {items.map((item) => <ActivityRow key={item.key} item={item} controls={controls} />)}
            </ul>
          )}
        </div>
      )}
    </div>
  );
}
