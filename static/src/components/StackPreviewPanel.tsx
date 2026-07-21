import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackFrameDecision, StackPreviewJob } from '../api/types';

interface StackPreviewPanelProps {
  dbId: string;
  projectId: number;
  imageIds: number[];
  selectionSource: 'selected' | 'visible';
}

const terminalStates = new Set(['completed', 'failed']);

function formatExposure(seconds: number): string {
  if (seconds < 60) return `${seconds.toFixed(0)} s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.round(seconds % 60);
  return remainder ? `${minutes}m ${remainder}s` : `${minutes}m`;
}

function registrationSummary(frame: StackFrameDecision): string {
  if (frame.disposition === 'reference') return 'Reference frame';
  if (frame.registration_rms_pixels === undefined) return '—';
  const matches = frame.matched_stars === undefined ? '' : ` · ${frame.matched_stars} stars`;
  return `${frame.registration_rms_pixels.toFixed(2)} px RMS${matches}`;
}

export default function StackPreviewPanel({
  dbId,
  projectId,
  imageIds,
  selectionSource,
}: StackPreviewPanelProps) {
  const [acceptedOnly, setAcceptedOnly] = useState(false);
  const [jobId, setJobId] = useState<string | null>(null);
  const stableImageIds = useMemo(() => [...imageIds].sort((a, b) => a - b), [imageIds]);

  useEffect(() => {
    setJobId(null);
  }, [dbId, projectId]);

  const start = useMutation({
    mutationFn: (force: boolean) =>
      apiClient.startStackPreviews(dbId, projectId, {
        image_ids: stableImageIds,
        accepted_only: acceptedOnly,
        force,
      }),
    onSuccess: (job) => setJobId(job.job_id),
  });

  const status = useQuery({
    queryKey: ['db', dbId, 'project', projectId, 'stack-preview', jobId],
    queryFn: () => apiClient.getStackPreviewJob(dbId, projectId, jobId!),
    enabled: jobId !== null,
    refetchInterval: (query) =>
      query.state.data && !terminalStates.has(query.state.data.state) ? 1000 : false,
  });

  const job: StackPreviewJob | undefined = status.data ?? start.data;
  const running = start.isPending || job?.state === 'queued' || job?.state === 'running';
  const error = start.error ?? status.error;
  const sourceText = selectionSource === 'selected' ? 'selected' : 'visible';

  return (
    <section className="stack-preview-panel" aria-labelledby="stack-preview-title">
      <div className="stack-preview-heading">
        <div>
          <div className="stack-preview-eyebrow">Project integration</div>
          <h2 id="stack-preview-title">Stack previews</h2>
          <p>
            Register and integrate the {stableImageIds.length} {sourceText} images by exact target
            and channel. Rejected and quality-regrade frames are left out automatically.
          </p>
        </div>
        <div className="stack-preview-actions">
          <label className="stack-preview-checkbox">
            <input
              type="checkbox"
              checked={acceptedOnly}
              disabled={running}
              onChange={(event) => setAcceptedOnly(event.target.checked)}
            />
            Accepted only
          </label>
          <button
            className="stack-preview-build"
            type="button"
            disabled={running || stableImageIds.length < 2}
            onClick={() => start.mutate(false)}
          >
            {running ? 'Building previews…' : job ? 'Build current set' : 'Build stack previews'}
          </button>
          {job?.state === 'completed' && (
            <button
              className="stack-preview-rebuild"
              type="button"
              disabled={running}
              onClick={() => start.mutate(true)}
            >
              Rebuild
            </button>
          )}
        </div>
      </div>

      {stableImageIds.length < 2 && (
        <div className="stack-preview-message">At least two visible images are required.</div>
      )}
      {error && (
        <div className="stack-preview-message error" role="alert">
          {error instanceof Error ? error.message : 'Stack preview failed'}
        </div>
      )}
      {job?.error && <div className="stack-preview-message error">{job.error}</div>}

      {job && (
        <div className="stack-preview-results" data-job-state={job.state}>
          <div className="stack-preview-statusline">
            <span className={`stack-preview-state ${job.state}`}>{job.state}</span>
            <span>{job.groups.length} target/channel group{job.groups.length === 1 ? '' : 's'}</span>
            <span>Uncalibrated stack preview</span>
          </div>
          <div className="stack-preview-grid">
            {job.groups.map((group) => (
              <article className="stack-preview-card" key={`${group.target_id}-${group.filter_name}`}>
                <header>
                  <div>
                    <h3>{group.target_name}</h3>
                    <span className="stack-preview-channel">{group.filter_name || 'No filter'}</span>
                  </div>
                  <span className={`stack-group-state ${group.state}`}>{group.state}</span>
                </header>

                {group.state === 'ready' && (
                  <img
                    src={apiClient.getStackPreviewUrl(dbId, job.job_id, group.index)}
                    alt={`${group.target_name} ${group.filter_name} uncalibrated stack preview`}
                  />
                )}
                {(group.state === 'queued' || group.state === 'running') && (
                  <div className="stack-preview-placeholder">
                    <span className="stack-preview-spinner" aria-hidden="true" />
                    {group.state === 'queued' ? 'Waiting for stacker' : 'Registering frames'}
                  </div>
                )}
                {(group.state === 'skipped' || group.state === 'error') && (
                  <div className="stack-preview-placeholder error">
                    {group.error || 'This group could not be stacked.'}
                  </div>
                )}

                <div className="stack-preview-metrics">
                  <div><strong>{group.accepted_frames}</strong><span>integrated</span></div>
                  <div><strong>{group.rejected_frames}</strong><span>stack rejects</span></div>
                  <div><strong>{group.quality_excluded}</strong><span>quality excluded</span></div>
                  <div><strong>{formatExposure(group.total_exposure_seconds)}</strong><span>exposure</span></div>
                </div>

                <details className="stack-preview-details">
                  <summary>Frame decisions ({group.frames.length})</summary>
                  <div className="stack-frame-table-wrap">
                    <table>
                      <thead><tr><th>Image</th><th>Quality</th><th>Decision</th><th>Registration</th></tr></thead>
                      <tbody>
                        {group.frames.map((frame) => (
                          <tr key={frame.image_id}>
                            <td>#{frame.image_id}</td>
                            <td>{frame.quality_score?.toFixed(2) ?? '—'}</td>
                            <td title={frame.reason}>{frame.disposition}</td>
                            <td>{frame.reason || registrationSummary(frame)}</td>
                          </tr>
                        ))}
                      </tbody>
                    </table>
                  </div>
                </details>
              </article>
            ))}
          </div>
        </div>
      )}
    </section>
  );
}
