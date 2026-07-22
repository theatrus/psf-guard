import { useQualityBackfill } from '../hooks/useQualityBackfill';

export default function QualityBackfillControls({ dbId }: { dbId: string }) {
  const job = useQualityBackfill(dbId);
  const progress = job.status?.progress;

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
      {job.error && <span className="quality-backfill-error">{job.error.message}</span>}
    </div>
  );
}
