import { useEffect, useId, useRef, useState } from 'react';
import type { ActivityItem } from './activityItems';
import { useHeaderActivity } from './useHeaderActivity';
import './header.css';

/** How long the list stays after the pointer leaves, so it can be reached. */
const HOVER_CLOSE_MS = 200;

function ProgressRing({ percent }: { percent: number | null }) {
  const radius = 7;
  const circumference = 2 * Math.PI * radius;
  return (
    <svg
      className={`activity-ring${percent == null ? ' is-indeterminate' : ''}`}
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
        strokeDashoffset={percent == null ? circumference * 0.7 : circumference * (1 - percent / 100)}
      />
    </svg>
  );
}

function ActivityRow({ item }: { item: ActivityItem }) {
  return (
    <li className={`activity-row${item.queued ? ' is-queued' : ''}`} title={item.hint}>
      <div className="activity-row-head">
        <span className="activity-row-title">{item.title}</span>
        <span className="activity-row-state">
          {item.queued ? 'queued' : item.percent != null ? `${Math.round(item.percent)}%` : 'working'}
        </span>
      </div>
      <div className="activity-row-scope">
        {item.scope}
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
      <div className="activity-row-detail">{item.detail}</div>
    </li>
  );
}

/**
 * Background work in the header, kept small: the overall progress and how
 * many jobs there are. Hover, focus or click opens the queue: every catalog
 * refresh, quality scan and stack build, running or queued, on any database.
 */
export default function ActivityChip() {
  const { items, summary, finished, scanError } = useHeaderActivity();
  const [pinned, setPinned] = useState(false);
  const [hovered, setHovered] = useState(false);
  const closeTimer = useRef<number | null>(null);
  const wrapper = useRef<HTMLDivElement>(null);
  const listId = useId();
  const open = (pinned || hovered) && summary.count > 0;

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
    if (summary.count === 0) {
      setPinned(false);
      setHovered(false);
    }
  }, [summary.count]);

  // Always mounted, so a screen reader hears when work ends or fails.
  const announcement = finished
    ? finished.errors
      ? `Background jobs finished with errors. ${finished.message ?? ''}`
      : 'Background jobs finished.'
    : scanError?.message
      ? `A quality scan finished with errors. ${scanError.message}`
      : '';
  const live = <span className="activity-live" aria-live="polite">{announcement}</span>;

  if (summary.count === 0 && !finished) return live;

  if (summary.count === 0 && finished) {
    return (
      <div className="activity-chip-slot">
        {live}
        <span
          className={`header-button utility-button activity-chip is-finished${finished.errors ? ' has-errors' : ''}`}
          title={finished.message}
        >
          <span className="activity-chip-mark" aria-hidden="true">{finished.errors ? '!' : '✓'}</span>
          <span className="activity-chip-text">{finished.errors ? 'Finished with errors' : 'Done'}</span>
        </span>
      </div>
    );
  }

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
        className={`header-button utility-button activity-chip${scanError ? ' has-errors' : ''}`}
        aria-expanded={open}
        aria-controls={open ? listId : undefined}
        aria-label={`Background jobs: ${label}`}
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
        <ProgressRing percent={summary.percent} />
        {percent != null && <span className="activity-chip-percent">{percent}%</span>}
        <span className="activity-chip-count">{jobs}</span>
      </button>
      {open && (
        <div id={listId} className="activity-popover" role="region" aria-label="Background jobs">
          <div className="activity-popover-head">
            <span>Background jobs</span>
            <span className="activity-popover-counts">
              {running} running{summary.queued > 0 ? ` · ${summary.queued} queued` : ''}
            </span>
          </div>
          {scanError && (
            <p className="activity-error" role="note">
              A quality scan finished with errors. {scanError.message}
            </p>
          )}
          <ul className="activity-list">
            {items.map((item) => <ActivityRow key={item.key} item={item} />)}
          </ul>
        </div>
      )}
    </div>
  );
}
