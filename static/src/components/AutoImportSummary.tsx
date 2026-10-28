import type { AutoImportSettings } from '../api/types';
import { useAutoImport } from '../hooks/useAutoImport';
import { describeAutoImport } from '../utils/autoimport';
import { scanRuleNotes, unmatchedNote } from '../utils/importNotes';

interface AutoImportSummaryProps {
  dbId: string;
  settings?: AutoImportSettings;
  /** Preview an import of these folders; offered for held frames. */
  onReview?: (folders: string[]) => void;
  /** Save edited rules before starting a run with the saved settings. */
  disabled?: boolean;
}

/**
 * Per-database status line for automatic import, with a Run now button.
 * Under it, what the last run left out, and a Review button for frames it
 * held because they match no project. Hidden when the database has never
 * turned the feature on.
 */
export default function AutoImportSummary({ dbId, settings, onReview, disabled = false }: AutoImportSummaryProps) {
  const job = useAutoImport(dbId);
  const effective = job.status?.settings ?? settings;
  if (!effective?.enabled) return null;
  const progress = job.status?.progress;
  const finished = progress && !progress.running && progress.stage === 'complete';
  const notes = finished ? scanRuleNotes(progress.outcome) : [];
  const unmatched = finished ? unmatchedNote(progress.outcome) : null;
  return (
    <div className="autoimport-summary" aria-live="polite">
      <span className="muted">{describeAutoImport(effective, job.status)}</span>
      <button
        type="button"
        className="browse-button"
        onClick={() => job.run()}
        disabled={disabled || job.isStarting || job.isRunning}
        title="Scan the image directories now for frames the database does not have yet"
      >
        {job.isRunning ? 'Importing…' : 'Run now'}
      </button>
      {(notes.length > 0 || unmatched) && (
        <div className="autoimport-notes">
          {notes.map((note) => (
            <span key={note} className="autoimport-note muted">
              {note}
            </span>
          ))}
          {unmatched && (
            <span className={`autoimport-note${unmatched.held ? ' autoimport-held' : ' muted'}`}>
              {unmatched.text}
              {unmatched.held && onReview && unmatched.folders.length > 0 && (
                <button
                  type="button"
                  className="browse-button"
                  disabled={disabled}
                  onClick={() => onReview(unmatched.folders)}
                  title={`Preview an import of ${unmatched.folders.join(', ')}`}
                >
                  Review
                </button>
              )}
            </span>
          )}
        </div>
      )}
      {job.error && <span className="quality-backfill-error">{job.error.message}</span>}
    </div>
  );
}
