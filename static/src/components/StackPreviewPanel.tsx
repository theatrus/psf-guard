import { useEffect, useMemo, useRef, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StackFrameDecision, StackGroupStatus, StackPreviewJob } from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';

interface StackPreviewPanelProps {
  dbId: string;
  projectId: number;
  imageIds: number[];
  selectionSource: 'selected' | 'visible';
}

const terminalStates = new Set(['completed', 'failed']);

function stackJobQueryKey(dbId: string, projectId: number, jobId: string | null) {
  return ['db', dbId, 'project', projectId, 'stack-preview', jobId] as const;
}

function formatExposure(seconds: number): string {
  if (seconds < 60) return `${seconds.toFixed(0)} s`;
  const minutes = Math.floor(seconds / 60);
  const remainder = Math.round(seconds % 60);
  return remainder ? `${minutes}m ${remainder}s` : `${minutes}m`;
}

function registrationSummary(frame: StackFrameDecision): string {
  if (frame.disposition === 'reference') return 'Reference frame';
  if (frame.registration_rms_pixels == null) return '—';
  const matches = frame.matched_stars == null ? '' : ` · ${frame.matched_stars} stars`;
  return `${frame.registration_rms_pixels.toFixed(2)} px RMS${matches}`;
}

export default function StackPreviewPanel({
  dbId,
  projectId,
  imageIds,
  selectionSource,
}: StackPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [acceptedOnly, setAcceptedOnly] = useState(false);
  const [jobId, setJobId] = useState<string | null>(null);
  const [jobRequestFingerprint, setJobRequestFingerprint] = useState<string | null>(null);
  const [inspector, setInspector] = useState<{
    jobId: string;
    artifactRevision: string;
    group: StackGroupStatus;
  } | null>(null);
  const stableImageIds = useMemo(() => [...imageIds].sort((a, b) => a - b), [imageIds]);
  const requestFingerprint = `${dbId}:${projectId}:${acceptedOnly ? 1 : 0}:${stableImageIds.join(',')}`;
  const currentRequestRef = useRef(requestFingerprint);
  currentRequestRef.current = requestFingerprint;

  const {
    mutate: startStack,
    isPending: startPending,
    error: startError,
    reset: resetStart,
  } = useMutation({
    mutationFn: (variables: {
      force: boolean;
      requestFingerprint: string;
      imageIds: number[];
      acceptedOnly: boolean;
    }) =>
      apiClient.startStackPreviews(dbId, projectId, {
        image_ids: variables.imageIds,
        accepted_only: variables.acceptedOnly,
        force: variables.force,
      }),
    onSuccess: (job, variables) => {
      if (variables.requestFingerprint !== currentRequestRef.current) return;
      queryClient.setQueryData(stackJobQueryKey(dbId, projectId, job.job_id), job);
      setJobId(job.job_id);
      setJobRequestFingerprint(variables.requestFingerprint);
    },
  });

  useEffect(() => {
    setJobId(null);
    setJobRequestFingerprint(null);
    setInspector(null);
    resetStart();
  }, [requestFingerprint, resetStart]);

  const status = useQuery({
    queryKey: stackJobQueryKey(dbId, projectId, jobId),
    queryFn: () => apiClient.getStackPreviewJob(dbId, projectId, jobId!),
    enabled: jobId !== null,
    refetchInterval: (query) =>
      query.state.data && !terminalStates.has(query.state.data.state) ? 1000 : false,
  });

  const job: StackPreviewJob | undefined =
    jobRequestFingerprint === requestFingerprint ? status.data : undefined;
  const running = startPending || job?.state === 'queued' || job?.state === 'running';
  const error = startError ?? status.error;
  const sourceText = selectionSource === 'selected' ? 'selected' : 'visible';
  const begin = (force: boolean) =>
    startStack({
      force,
      requestFingerprint,
      imageIds: stableImageIds,
      acceptedOnly,
    });

  return (
    <>
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
            onClick={() => begin(false)}
          >
            {running ? 'Building previews…' : job ? 'Build current set' : 'Build stack previews'}
          </button>
          {job?.state === 'completed' && (
            <button
              className="stack-preview-rebuild"
              type="button"
              disabled={running}
              onClick={() => begin(true)}
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
                  <div className="stack-preview-card-actions">
                    <span className={`stack-group-state ${group.state}`}>{group.state}</span>
                    {group.state === 'ready' && (
                      <a
                        className="stack-preview-download"
                        href={apiClient.getStackFitsUrl(
                          dbId,
                          job.job_id,
                          group.index,
                          job.artifact_revision
                        )}
                        download
                      >
                        Download linear FITS
                      </a>
                    )}
                  </div>
                </header>

                {group.state === 'ready' && (
                  <div className="stack-preview-image">
                    <img
                      src={apiClient.getStackPreviewUrl(
                        dbId,
                        job.job_id,
                        group.index,
                        job.artifact_revision
                      )}
                      alt={`${group.target_name} ${group.filter_name} uncalibrated stack preview`}
                    />
                    <button
                      className="stack-preview-inspect"
                      type="button"
                      onClick={() => setInspector({
                        jobId: job.job_id,
                        artifactRevision: job.artifact_revision,
                        group,
                      })}
                    >
                      Inspect full size
                    </button>
                  </div>
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
                            <td title={frame.reason ?? undefined}>{frame.disposition}</td>
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
      {inspector && (
        <StackPreviewInspector
          dbId={dbId}
          jobId={inspector.jobId}
          artifactRevision={inspector.artifactRevision}
          group={inspector.group}
          onClose={() => setInspector(null)}
        />
      )}
    </>
  );
}
