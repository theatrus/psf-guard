import type { AutoImportSettings } from '../api/types';
import { useAutoImport } from '../hooks/useAutoImport';
import { describeAutoImport } from '../utils/autoimport';

interface AutoImportSummaryProps {
  dbId: string;
  settings?: AutoImportSettings;
}

/**
 * Per-database status line for automatic import, with a Run now button.
 * Hidden when the database has never turned the feature on.
 */
export default function AutoImportSummary({ dbId, settings }: AutoImportSummaryProps) {
  const job = useAutoImport(dbId);
  const effective = job.status?.settings ?? settings;
  if (!effective?.enabled) return null;
  return (
    <div className="autoimport-summary" aria-live="polite">
      <span className="muted">{describeAutoImport(effective, job.status)}</span>
      <button
        type="button"
        className="browse-button"
        onClick={() => job.run()}
        disabled={job.isStarting || job.isRunning}
        title="Scan the image directories now for frames the catalog does not have yet"
      >
        {job.isRunning ? 'Importing…' : 'Run now'}
      </button>
      {job.error && <span className="quality-backfill-error">{job.error.message}</span>}
    </div>
  );
}
