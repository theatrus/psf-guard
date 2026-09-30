import type { ReactNode } from 'react';
import type { LibraryDensity } from '../hooks/useDisplayPreferences';
import { ago, percentDone, stateLabel } from './libraryFamilies';

/** The pills the Library rows and the plan list share, so a project reads
 *  the same way wherever it is listed: which database, what state Target
 *  Scheduler has it in, how far along, how it graded, and when. */

export function DbPill({ name, title }: { name: string; title?: string }) {
  return <span className="library-pill library-pill-db" title={title}>{name}</span>;
}

/** Target Scheduler's project state: Draft, Active, Inactive or Closed. */
export function StatePill({ state }: { state: number | null | undefined }) {
  if (state === null || state === undefined) return null;
  return <span className={`library-pill library-pill-state is-state-${state}`}>{stateLabel(state)}</span>;
}

/** Accepted frames against the goal, with a small bar; "Done" once the goal
 *  is met; the image count when there is no goal. */
export function ProgressPill({ accepted, desired, totalImages, title }: { accepted: number; desired: number; totalImages: number; title?: string }) {
  const done = percentDone(accepted, desired);
  if (desired <= 0) return <span className="library-pill library-pill-progress is-open" title={title ?? 'No desired frame count in Target Scheduler'}>{totalImages} images</span>;
  const finished = accepted >= desired;
  return <span className={`library-pill library-pill-progress${finished ? ' is-done' : ''}`} title={title ?? `${accepted} of ${desired} desired frames accepted`}>
    <span className="library-pill-bar" aria-hidden="true"><span style={{ width: `${Math.min(100, done ?? 0)}%` }} /></span>
    {accepted} / {desired} · {finished ? 'Done' : `${done}%`}
  </span>;
}

export function GradingPill({ accepted, rejected, pending }: { accepted: number; rejected: number; pending: number }) {
  return <span className="library-pill library-pill-grading" title={`${accepted} accepted, ${rejected} rejected, ${pending} pending`}>
    <span className="grade-accepted">{accepted}</span><span className="grade-rejected">{rejected}</span><span className="grade-pending">{pending}</span>
  </span>;
}

const formatDay = (seconds: number) => new Date(seconds * 1000).toLocaleDateString();

/** First to last capture, with how long ago the last one was. */
export function DatesPill({ earliest, latest, nowMs, title }: { earliest: number | null | undefined; latest: number | null | undefined; nowMs: number; title?: string }) {
  const last = ago(latest, nowMs);
  return <span className="library-pill library-pill-dates" title={title}>
    {latest ? `${formatDay(earliest ?? latest)} – ${formatDay(latest)}` : 'No dates'}
    {last ? <small> · {last}</small> : null}
  </span>;
}

/** The outer pill's own marker: one plan shot by several rigs. */
export function FamilyPill({ rigs }: { rigs: number }) {
  return <span className="library-pill library-pill-family" title="One plan shot by several rigs: the same Target Scheduler project in each database">{rigs} rigs</span>;
}

/** The arrow that opens a row into its card and folds it back. */
export function FoldButton({ open, name, onClick }: { open: boolean; name: string; onClick: () => void }) {
  return <button type="button" className={`library-expand${open ? ' is-open' : ''}`} aria-expanded={open}
    aria-label={`${open ? 'Hide' : 'Show'} details for ${name}`} title={open ? 'Fold to a row' : 'Show details'} onClick={onClick}>{open ? '▾' : '▸'}</button>;
}

/** The Compact / Detailed switch both lists carry. */
export function DensityToggle({ density, onChange }: { density: LibraryDensity; onChange: (next: LibraryDensity) => void }) {
  return <div className="project-density" role="radiogroup" aria-label="Project view">
    <button type="button" role="radio" aria-checked={density === 'compact'} onClick={() => onChange('compact')}>Compact</button>
    <button type="button" role="radio" aria-checked={density === 'detailed'} onClick={() => onChange('detailed')}>Detailed</button>
  </div>;
}

/** One compact row: the fold arrow, the name, then pills. */
export function ProjectRow({ className, testId, children }: { className?: string; testId?: string; children: ReactNode }) {
  return <div className={['library-row', className].filter(Boolean).join(' ')} data-testid={testId ?? 'library-row'}>{children}</div>;
}
