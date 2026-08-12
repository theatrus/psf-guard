import { useQualityBackfill } from '../hooks/useQualityBackfill';
import { setStarMetadataFill, useStarMetadataFill } from '../hooks/useStarMetadataFill';

export default function QualityBackfillControls({ dbId }: { dbId: string }) {
  const job = useQualityBackfill(dbId);
  const progress = job.status?.progress;
  // One preference for every analyze action (scan, backfill, import), so the
  // checkbox here is also the remembered default.
  const fillMetadata = useStarMetadataFill();

  if (job.isRunning && progress) {
    return (
      <div className="quality-backfill-status" aria-live="polite">
        Analyzing quality in the background… {progress.processed_targets}/
        {progress.total_targets} targets
      </div>
    );
  }

  return (
    <div className="quality-backfill-controls">
      <button
        type="button"
        className="browse-button"
        onClick={() => job.start(false)}
        disabled={job.isStarting}
        title="Analyze images without recomputing valid cached results"
      >
        Analyze Missing Quality
      </button>
      <button
        type="button"
        className="browse-button"
        onClick={() => {
          if (window.confirm('Recompute cached star, background, photometry, and pointing data for every image in this database?')) {
            job.start(true);
          }
        }}
        disabled={job.isStarting}
        title="Recompute star counts and all other cached quality evidence"
      >
        Rescan All Quality
      </button>
      <div className="quality-backfill-option">
        <label title="Fill measured star count and HFR into images imported without them. Only missing values are written; N.I.N.A.-recorded measurements are never replaced. Remembered as the default for every analyze action.">
          <input
            type="checkbox"
            checked={fillMetadata}
            onChange={(event) => setStarMetadataFill(event.target.checked)}
          />
          Write star count and HFR into image metadata
        </label>
        <small>
          Writes only to this catalog&apos;s database records. Your FITS and
          XISF files are never modified.
        </small>
      </div>
      {job.error && <span className="quality-backfill-error">{job.error.message}</span>}
    </div>
  );
}
