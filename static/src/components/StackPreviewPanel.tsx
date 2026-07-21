import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  Image,
  LatestStackPreviewGroup,
  StackFrameDecision,
  StackGroupStatus,
  StackInputImage,
  StackPreviewJob,
  StackStretchPreview,
} from '../api/types';
import StackPreviewInspector from './StackPreviewInspector';
import StackColorPreviewPanel from './StackColorPreviewPanel';
import StackStretchControls from './StackStretchControls';

type StackCandidateImage = Pick<
  Image,
  'id' | 'target_id' | 'target_name' | 'filter_name' | 'grading_status'
>;

interface StackPreviewPanelProps {
  dbId: string;
  projectId: number;
  images: StackCandidateImage[];
  selectionSource: 'selected' | 'visible';
}

interface ChannelInput {
  key: string;
  targetId: number;
  targetName: string;
  filterName: string;
  images: StackCandidateImage[];
}

interface StackArtifact {
  jobId: string;
  artifactRevision: string;
  acceptedOnly: boolean;
  group: StackGroupStatus;
}

const terminalStates = new Set(['completed', 'failed']);

function stackJobQueryKey(dbId: string, projectId: number, jobId: string | null) {
  return ['db', dbId, 'project', projectId, 'stack-preview', jobId] as const;
}

function latestStackQueryKey(dbId: string, projectId: number) {
  return ['db', dbId, 'project', projectId, 'stack-preview', 'latest'] as const;
}

function channelKey(targetId: number, filterName: string | null) {
  return `${targetId}:${filterName ?? ''}`;
}

function artifactStretchKey(artifact: StackArtifact) {
  return `${artifact.jobId}:${artifact.group.index}:${artifact.artifactRevision}`;
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

function staleReason(
  current: ChannelInput | undefined,
  inputImages: StackInputImage[],
  builtAcceptedOnly: boolean,
  acceptedOnly: boolean
): string | null {
  if (!current) return 'Out of date — this channel is not in the current input';
  if (inputImages.length === 0) return 'Out of date — rebuild required';

  const currentGrades = new Map(current.images.map((image) => [image.id, image.grading_status]));
  const builtGrades = new Map(inputImages.map((image) => [image.image_id, image.grading_status]));
  if (
    currentGrades.size !== builtGrades.size ||
    [...currentGrades.keys()].some((imageId) => !builtGrades.has(imageId))
  ) {
    return 'Out of date — input images changed';
  }
  if ([...currentGrades].some(([imageId, grade]) => builtGrades.get(imageId) !== grade)) {
    return 'Out of date — image grades changed';
  }
  if (builtAcceptedOnly !== acceptedOnly) {
    return 'Out of date — Accepted only changed';
  }
  return null;
}

function artifactFromLatest(latest: LatestStackPreviewGroup | undefined): StackArtifact | undefined {
  if (!latest) return undefined;
  return {
    jobId: latest.job_id,
    artifactRevision: latest.artifact_revision,
    acceptedOnly: latest.accepted_only,
    group: latest.group,
  };
}

export default function StackPreviewPanel({
  dbId,
  projectId,
  images,
  selectionSource,
}: StackPreviewPanelProps) {
  const queryClient = useQueryClient();
  const [acceptedOnly, setAcceptedOnly] = useState(false);
  const [activeJobId, setActiveJobId] = useState<string | null>(null);
  const [inspector, setInspector] = useState<StackArtifact | null>(null);
  const [stretches, setStretches] = useState<Record<string, StackStretchPreview>>({});

  const currentChannels = useMemo(() => {
    const channels = new Map<string, ChannelInput>();
    for (const image of images) {
      const key = channelKey(image.target_id, image.filter_name);
      const existing = channels.get(key);
      if (existing) {
        existing.images.push(image);
      } else {
        channels.set(key, {
          key,
          targetId: image.target_id,
          targetName: image.target_name,
          filterName: image.filter_name ?? '',
          images: [image],
        });
      }
    }
    for (const channel of channels.values()) {
      channel.images.sort((left, right) => left.id - right.id);
    }
    return channels;
  }, [images]);

  const stableImageIds = useMemo(
    () => [...images].map((image) => image.id).sort((left, right) => left - right),
    [images]
  );

  const latest = useQuery({
    queryKey: latestStackQueryKey(dbId, projectId),
    queryFn: () => apiClient.getLatestStackPreviews(dbId, projectId),
  });

  const {
    mutate: startStack,
    isPending: startPending,
    error: startError,
    variables: startVariables,
    reset: resetStart,
  } = useMutation({
    mutationFn: (variables: {
      force: boolean;
      imageIds: number[];
      operationKey: string;
    }) =>
      apiClient.startStackPreviews(dbId, projectId, {
        image_ids: variables.imageIds,
        accepted_only: acceptedOnly,
        force: variables.force,
      }),
    onSuccess: (job) => {
      queryClient.setQueryData(stackJobQueryKey(dbId, projectId, job.job_id), job);
      setActiveJobId(job.job_id);
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({ queryKey: latestStackQueryKey(dbId, projectId) });
      }
    },
  });

  const status = useQuery({
    queryKey: stackJobQueryKey(dbId, projectId, activeJobId),
    queryFn: () => apiClient.getStackPreviewJob(dbId, projectId, activeJobId!),
    enabled: activeJobId !== null,
    refetchInterval: (query) =>
      query.state.data && !terminalStates.has(query.state.data.state) ? 1000 : false,
  });

  const activeJob: StackPreviewJob | undefined = status.data;
  useEffect(() => {
    if (activeJob && terminalStates.has(activeJob.state)) {
      queryClient.invalidateQueries({ queryKey: latestStackQueryKey(dbId, projectId) });
    }
  }, [activeJob, dbId, projectId, queryClient]);

  useEffect(() => {
    setActiveJobId(null);
    setInspector(null);
    setStretches({});
    resetStart();
  }, [dbId, projectId, resetStart]);

  const latestByChannel = useMemo(
    () =>
      new Map(
        (latest.data?.groups ?? []).map((entry) => [
          channelKey(entry.group.target_id, entry.group.filter_name),
          entry,
        ])
      ),
    [latest.data]
  );
  const activeByChannel = useMemo(
    () =>
      new Map(
        (activeJob?.groups ?? []).map((group) => [
          channelKey(group.target_id, group.filter_name),
          group,
        ])
      ),
    [activeJob]
  );

  const displayKeys = useMemo(() => {
    const keys = new Set([...currentChannels.keys(), ...latestByChannel.keys(), ...activeByChannel.keys()]);
    return [...keys].sort((leftKey, rightKey) => {
      const leftCurrent = currentChannels.get(leftKey);
      const rightCurrent = currentChannels.get(rightKey);
      const leftRemembered = latestByChannel.get(leftKey)?.group;
      const rightRemembered = latestByChannel.get(rightKey)?.group;
      const leftTarget = leftCurrent?.targetName ?? leftRemembered?.target_name ?? '';
      const rightTarget = rightCurrent?.targetName ?? rightRemembered?.target_name ?? '';
      const byTarget = leftTarget.localeCompare(rightTarget);
      if (byTarget !== 0) return byTarget;
      const leftFilter = leftCurrent?.filterName ?? leftRemembered?.filter_name ?? '';
      const rightFilter = rightCurrent?.filterName ?? rightRemembered?.filter_name ?? '';
      return leftFilter.localeCompare(rightFilter);
    });
  }, [activeByChannel, currentChannels, latestByChannel]);

  const running = startPending || activeJob?.state === 'queued' || activeJob?.state === 'running';
  const error = startError ?? status.error ?? latest.error;
  const sourceText = selectionSource === 'selected' ? 'selected' : 'visible';
  const beginAll = (force: boolean) =>
    startStack({ force, imageIds: stableImageIds, operationKey: 'all' });
  const beginChannel = (channel: ChannelInput, force: boolean) =>
    startStack({
      force,
      imageIds: channel.images.map((image) => image.id),
      operationKey: channel.key,
    });

  const staleCount = displayKeys.filter((key) => {
    const activeGroup = activeByChannel.get(key);
    const latestEntry = latestByChannel.get(key);
    const artifact =
      activeGroup?.state === 'ready' && activeJob
        ? {
            acceptedOnly: activeJob.accepted_only,
            group: activeGroup,
          }
        : latestEntry
          ? { acceptedOnly: latestEntry.accepted_only, group: latestEntry.group }
          : undefined;
    return artifact
      ? staleReason(
          currentChannels.get(key),
          artifact.group.input_images,
          artifact.acceptedOnly,
          acceptedOnly
        ) !== null
      : false;
  }).length;
  const outdatedTargetIds = useMemo(() => {
    const targetIds = new Set<number>();
    for (const entry of latest.data?.groups ?? []) {
      const key = channelKey(entry.group.target_id, entry.group.filter_name);
      if (
        staleReason(
          currentChannels.get(key),
          entry.group.input_images,
          entry.accepted_only,
          acceptedOnly
        )
      ) {
        targetIds.add(entry.group.target_id);
      }
    }
    return targetIds;
  }, [acceptedOnly, currentChannels, latest.data]);
  const colorSourceRevision = useMemo(
    () => (latest.data?.groups ?? [])
      .map((entry) => `${entry.job_id}:${entry.group.index}:${entry.artifact_revision}`)
      .sort()
      .join('|'),
    [latest.data?.groups]
  );

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
              onClick={() => beginAll(false)}
            >
              {running && startVariables?.operationKey === 'all'
                ? 'Building previews…'
                : latest.data?.groups.length
                  ? 'Build current set'
                  : 'Build stack previews'}
            </button>
            {!!latest.data?.groups.length && (
              <button
                className="stack-preview-rebuild"
                type="button"
                disabled={running || stableImageIds.length < 2}
                onClick={() => beginAll(true)}
              >
                Rebuild current set
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
        {activeJob?.error && <div className="stack-preview-message error">{activeJob.error}</div>}

        <StackColorPreviewPanel
          dbId={dbId}
          projectId={projectId}
          sourceRevision={colorSourceRevision}
          channelBuildRunning={running}
          outdatedTargetIds={outdatedTargetIds}
        />

        {displayKeys.length > 0 && (
          <div
            className="stack-preview-results"
            data-job-state={activeJob?.state ?? 'remembered'}
          >
            <div className="stack-preview-statusline">
              <span className={`stack-preview-state ${activeJob?.state ?? 'remembered'}`}>
                {activeJob?.state ?? 'remembered'}
              </span>
              <span>
                {displayKeys.length} target/channel group{displayKeys.length === 1 ? '' : 's'}
              </span>
              <span>Uncalibrated stack preview</span>
              {staleCount > 0 && (
                <span className="stack-preview-outdated-count">{staleCount} out of date</span>
              )}
            </div>
            <div className="stack-preview-grid">
              {displayKeys.map((key) => {
                const current = currentChannels.get(key);
                const activeGroup = activeByChannel.get(key);
                const latestEntry = latestByChannel.get(key);
                const activeArtifact: StackArtifact | undefined =
                  activeGroup?.state === 'ready' && activeJob
                    ? {
                        jobId: activeJob.job_id,
                        artifactRevision: activeJob.artifact_revision,
                        acceptedOnly: activeJob.accepted_only,
                        group: activeGroup,
                      }
                    : undefined;
                const artifact = activeArtifact ?? artifactFromLatest(latestEntry);
                const stretchKey = artifact ? artifactStretchKey(artifact) : null;
                const appliedStretch = stretchKey ? stretches[stretchKey] : undefined;
                const group = artifact?.group ?? activeGroup;
                const targetName = current?.targetName ?? group?.target_name ?? 'Unknown target';
                const filterName = current?.filterName ?? group?.filter_name ?? '';
                const outdated = artifact
                  ? staleReason(
                      current,
                      artifact.group.input_images,
                      artifact.acceptedOnly,
                      acceptedOnly
                    )
                  : null;
                const groupBusy =
                  activeGroup?.state === 'queued' || activeGroup?.state === 'running';
                const canBuildChannel = (current?.images.length ?? 0) >= 2;
                const progressGroup = activeGroup ?? artifact?.group;
                const progressState = progressGroup?.state ?? 'not-built';
                const processedFrames = progressGroup?.processed_frames ?? 0;
                const eligibleFrames =
                  progressGroup?.eligible_frames ?? current?.images.length ?? 0;
                const progressPercentage =
                  progressState === 'ready'
                    ? 100
                    : eligibleFrames > 0
                      ? Math.min(100, (processedFrames / eligibleFrames) * 100)
                      : 0;
                const progressLabel =
                  progressState === 'queued'
                    ? artifact
                      ? 'Rebuild queued'
                      : 'Waiting for stacker'
                    : progressState === 'running'
                      ? artifact
                        ? 'Rebuilding stack'
                        : 'Registering frames'
                      : progressState === 'ready'
                        ? 'Stack ready'
                        : progressState === 'skipped'
                          ? 'Stack skipped'
                          : progressState === 'error'
                            ? 'Stack failed'
                            : 'Not built';
                const progressDetail = progressGroup
                  ? `${processedFrames}/${eligibleFrames} frames`
                  : `${current?.images.length ?? 0} candidates`;

                return (
                  <article
                    className={`stack-preview-card ${outdated ? 'outdated' : ''}`}
                    data-outdated={outdated ? 'true' : 'false'}
                    key={key}
                  >
                    <header>
                      <div className="stack-preview-card-title">
                        <h3>{targetName}</h3>
                        <span className="stack-preview-channel">{filterName || 'No filter'}</span>
                      </div>
                      <div className="stack-preview-card-actions">
                        <span className={`stack-group-state ${activeGroup?.state ?? group?.state ?? 'not-built'}`}>
                          {activeGroup?.state ?? group?.state ?? 'not built'}
                        </span>
                        {artifact && (
                          <button
                            className="stack-preview-card-action"
                            type="button"
                            aria-label="Inspect full size"
                            title="Inspect full size"
                            onClick={() => setInspector(artifact)}
                          >
                            Inspect
                          </button>
                        )}
                        {artifact && (
                          <a
                            className="stack-preview-card-action"
                            href={apiClient.getStackFitsUrl(
                              dbId,
                              artifact.jobId,
                              artifact.group.index,
                              artifact.artifactRevision
                            )}
                            download
                            aria-label="Download linear FITS"
                            title="Download linear FITS"
                          >
                            FITS
                          </a>
                        )}
                        {current && (
                          <button
                            className="stack-preview-card-action"
                            type="button"
                            disabled={running || !canBuildChannel}
                            aria-label={artifact ? 'Rebuild channel' : 'Build channel'}
                            title={artifact ? 'Rebuild channel' : 'Build channel'}
                            onClick={() => beginChannel(current, Boolean(artifact))}
                          >
                            {groupBusy || (startPending && startVariables?.operationKey === key)
                              ? 'Building…'
                              : artifact
                                ? 'Rebuild'
                                : 'Build'}
                          </button>
                        )}
                      </div>
                    </header>

                    {outdated && <div className="stack-preview-outdated">{outdated}</div>}

                    {artifact && (
                      <div className="stack-preview-image">
                        <img
                          src={appliedStretch?.preview_url ?? apiClient.getStackPreviewUrl(
                            dbId, artifact.jobId, artifact.group.index, artifact.artifactRevision
                          )}
                          alt={`${targetName} ${filterName} uncalibrated stack preview`}
                        />
                      </div>
                    )}
                    {artifact && stretchKey && (
                      <StackStretchControls
                        key={stretchKey}
                        label={`${targetName} ${filterName || 'no filter'}`}
                        channels={artifact.group.output_channels === 3 ? 3 : 1}
                        disabled={running}
                        applied={appliedStretch}
                        apply={(request) => apiClient.applyStackStretch(
                          dbId, artifact.jobId, artifact.group.index, request
                        )}
                        onApplied={(preview) => setStretches((currentStretches) => ({
                          ...currentStretches,
                          [stretchKey]: preview,
                        }))}
                        onRevert={() => setStretches((currentStretches) => {
                          const next = { ...currentStretches };
                          delete next[stretchKey];
                          return next;
                        })}
                      />
                    )}
                    {!artifact && groupBusy && (
                      <div className="stack-preview-placeholder">
                        <span className="stack-preview-spinner" aria-hidden="true" />
                        {activeGroup?.state === 'queued' ? 'Waiting for stacker' : 'Registering frames'}
                      </div>
                    )}
                    {!artifact && !groupBusy && (
                      <div className={`stack-preview-placeholder ${activeGroup?.state === 'error' ? 'error' : ''}`}>
                        {activeGroup?.error ??
                          (canBuildChannel
                            ? 'No preview has been built for this channel.'
                            : 'At least two current images are required for this channel.')}
                      </div>
                    )}

                    <div
                      className={`stack-preview-progress ${progressState}`}
                      data-stack-state={progressState}
                      role="status"
                      aria-live="polite"
                    >
                      <div className="stack-preview-progress-copy">
                        <span>{progressLabel}</span>
                        <span>{progressDetail}</span>
                      </div>
                      <div
                        className="stack-preview-progress-track"
                        role="progressbar"
                        aria-label={`${targetName} ${filterName || 'no filter'} stack progress`}
                        aria-valuemin={0}
                        aria-valuemax={eligibleFrames}
                        aria-valuenow={processedFrames}
                      >
                        <span style={{ width: `${progressPercentage}%` }} />
                      </div>
                    </div>

                    {progressGroup && (
                      <div className="stack-preview-metrics">
                        <div><strong>{progressGroup.accepted_frames}</strong><span>integrated</span></div>
                        <div><strong>{progressGroup.rejected_frames}</strong><span>stack rejects</span></div>
                        <div><strong>{progressGroup.quality_excluded}</strong><span>quality excluded</span></div>
                        <div><strong>{formatExposure(progressGroup.total_exposure_seconds)}</strong><span>exposure</span></div>
                      </div>
                    )}

                    {artifact && (
                      <>
                        <details className="stack-preview-details">
                          <summary>Frame decisions ({artifact.group.frames.length})</summary>
                          <div className="stack-frame-table-wrap">
                            <table>
                              <thead><tr><th>Image</th><th>Quality</th><th>Decision</th><th>Registration</th></tr></thead>
                              <tbody>
                                {artifact.group.frames.map((frame) => (
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
                      </>
                    )}
                  </article>
                );
              })}
            </div>
          </div>
        )}
      </section>
      {inspector && (
        <StackPreviewInspector
          eyebrow="Full-resolution integration"
          title={inspector.group.target_name}
          label={inspector.group.filter_name || 'No filter'}
          summary={[
            `${inspector.group.accepted_frames} frames`,
            `${Math.round(inspector.group.total_exposure_seconds)} s`,
          ]}
          imageUrl={stretches[artifactStretchKey(inspector)]?.original_preview_url ??
            apiClient.getStackPreviewUrl(
              dbId,
              inspector.jobId,
              inspector.group.index,
              inspector.artifactRevision,
              'original'
            )}
          fitsUrl={stretches[artifactStretchKey(inspector)]?.fits_url ??
            apiClient.getStackFitsUrl(
              dbId,
              inspector.jobId,
              inspector.group.index,
              inspector.artifactRevision
            )}
          imageAlt={`Full-resolution stack for ${inspector.group.target_name} ${inspector.group.filter_name || 'No filter'}`}
          downloadLabel={stretches[artifactStretchKey(inspector)]?.fits_url
            ? 'Download deconvolved linear FITS'
            : 'Download linear FITS'}
          onClose={() => setInspector(null)}
        />
      )}
    </>
  );
}
