import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQueries, useQuery, useQueryClient } from '@tanstack/react-query';
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
import { isSkyOriented } from './stackOrientation';
import { useAccess } from '../auth/access';
import { useStackActivity } from '../hooks/useStackActivity';

type StackCandidateImage = Pick<
  Image,
  'id' | 'target_id' | 'target_name' | 'filter_name' | 'grading_status'
>;

interface StackPreviewPanelProps {
  dbId: string;
  projectId: number;
  images: StackCandidateImage[];
  selectionSource: 'selected' | 'visible';
  onOpenImage: (imageId: number) => void;
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

const terminalStates = new Set(['completed', 'failed', 'cancelled']);

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
  onOpenImage,
}: StackPreviewPanelProps) {
  const queryClient = useQueryClient();
  const { canCompute } = useAccess();
  const [acceptedOnly, setAcceptedOnly] = useState(false);
  const [watchedJobIds, setWatchedJobIds] = useState<string[]>([]);
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
      setWatchedJobIds((current) =>
        current.includes(job.job_id) ? current : [...current, job.job_id]
      );
      if (terminalStates.has(job.state)) {
        queryClient.invalidateQueries({ queryKey: latestStackQueryKey(dbId, projectId) });
      }
    },
  });

  const {
    mutate: stopStack,
    isPending: stopPending,
    error: stopError,
    reset: resetStop,
  } = useMutation({
    mutationFn: (jobId: string) => apiClient.cancelStackPreviewJob(dbId, projectId, jobId),
    onSuccess: (job) => {
      queryClient.setQueryData(stackJobQueryKey(dbId, projectId, job.job_id), job);
    },
  });

  const statuses = useQueries({
    queries: watchedJobIds.map((jobId) => ({
      queryKey: stackJobQueryKey(dbId, projectId, jobId),
      queryFn: () => apiClient.getStackPreviewJob(dbId, projectId, jobId),
      refetchInterval: (query: { state: { data?: StackPreviewJob } }) =>
        query.state.data && !terminalStates.has(query.state.data.state) ? 1000 : false,
    })),
  });
  const watchedJobs = useMemo(
    () =>
      statuses
        .map((status) => status.data)
        .filter((job): job is StackPreviewJob => job !== undefined)
        .sort((left, right) => left.created_unix_seconds - right.created_unix_seconds),
    // eslint-disable-next-line react-hooks/exhaustive-deps -- useQueries returns a new array each render; the join tracks content.
    [statuses.map((status) => status.dataUpdatedAt).join('|')]
  );
  const unfinishedJobs = watchedJobs.filter((job) => !terminalStates.has(job.state));
  const statusError = statuses.find((status) => status.error)?.error;
  // The job a shared status line describes: running work first, then the
  // newest of whatever remains.
  const activeJob: StackPreviewJob | undefined =
    unfinishedJobs.find((job) => job.state === 'running') ??
    unfinishedJobs[unfinishedJobs.length - 1] ??
    watchedJobs[watchedJobs.length - 1];
  const settledCount = watchedJobs.filter((job) => terminalStates.has(job.state)).length;
  useEffect(() => {
    if (settledCount > 0) {
      queryClient.invalidateQueries({ queryKey: latestStackQueryKey(dbId, projectId) });
    }
  }, [settledCount, dbId, projectId, queryClient]);

  useEffect(() => {
    setWatchedJobIds([]);
    setInspector(null);
    setStretches({});
    resetStart();
    resetStop();
  }, [dbId, projectId, resetStart, resetStop]);

  // Builds started before this panel mounted — or before the last navigation
  // — keep running on the server. Re-attach to every one of them so a queue
  // built up elsewhere stays visible. This must follow the reset above so a
  // project change adopts the new project's jobs rather than the old ones.
  const { active } = useStackActivity();
  const adoptableIds = useMemo(
    () =>
      active
        .filter(
          (entry) =>
            entry.kind === 'mono' && entry.database_id === dbId && entry.project_id === projectId
        )
        .map((entry) => entry.job_id),
    [active, dbId, projectId]
  );
  useEffect(() => {
    if (adoptableIds.length === 0) return;
    setWatchedJobIds((current) => {
      const additions = adoptableIds.filter((jobId) => !current.includes(jobId));
      return additions.length === 0 ? current : [...current, ...additions];
    });
  }, [adoptableIds]);

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
  const activeByChannel = useMemo(() => {
    const merged = new Map<string, { job: StackPreviewJob; group: StackPreviewJob['groups'][number] }>();
    for (const job of watchedJobs) {
      for (const group of job.groups) {
        const key = channelKey(group.target_id, group.filter_name);
        const existing = merged.get(key);
        // A build still holding the channel outranks a settled one; among
        // equals the newer request wins, matching iteration order.
        const groupBusy = group.state === 'queued' || group.state === 'running';
        const existingBusy =
          existing?.group.state === 'queued' || existing?.group.state === 'running';
        if (!existing || groupBusy || !existingBusy) {
          merged.set(key, { job, group });
        }
      }
    }
    return merged;
  }, [watchedJobs]);

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

  const running = startPending || unfinishedJobs.length > 0;
  const queuedBuilds = unfinishedJobs.length;
  const buildLabel = latest.data?.groups.length ? 'Build current set' : 'Build stack previews';
  const error = startError ?? stopError ?? statusError ?? latest.error;
  const sourceText = selectionSource === 'selected' ? 'selected' : 'visible';
  // A failed stop — "that build already finished" — belongs to the build the
  // user was stopping, not to the next one they start.
  const beginAll = (force: boolean) => {
    resetStop();
    startStack({ force, imageIds: stableImageIds, operationKey: 'all' });
  };
  const beginChannel = (channel: ChannelInput, force: boolean) => {
    resetStop();
    startStack({
      force,
      imageIds: channel.images.map((image) => image.id),
      operationKey: channel.key,
    });
  };

  const staleCount = displayKeys.filter((key) => {
    const activeEntry = activeByChannel.get(key);
    const latestEntry = latestByChannel.get(key);
    const artifact =
      activeEntry && activeEntry.group.state === 'ready'
        ? {
            acceptedOnly: activeEntry.job.accepted_only,
            group: activeEntry.group,
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
              disabled={!canCompute || startPending || stableImageIds.length < 2}
              title={canCompute ? undefined : 'This account can view cached stacks but cannot build them.'}
              onClick={() => beginAll(false)}
            >
              {startPending && startVariables?.operationKey === 'all' ? 'Queueing…' : buildLabel}
            </button>
            {!!latest.data?.groups.length && !running && (
              <button
                className="stack-preview-rebuild"
                type="button"
                disabled={!canCompute || stableImageIds.length < 2}
                title={canCompute ? undefined : 'This account can view cached stacks but cannot rebuild them.'}
                onClick={() => beginAll(true)}
              >
                Rebuild current set
              </button>
            )}
            {unfinishedJobs.length > 0 && (
              <button
                className="stack-preview-stop"
                type="button"
                disabled={stopPending}
                title="Stop every queued and running build. Channels already finished keep their previews."
                onClick={() => unfinishedJobs.forEach((job) => stopStack(job.job_id))}
              >
                {stopPending
                  ? 'Stopping…'
                  : unfinishedJobs.length > 1
                    ? `Stop all (${unfinishedJobs.length})`
                    : 'Stop'}
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
          canCompute={canCompute}
          onOpenImage={onOpenImage}
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
              <span>Stack preview</span>
              {queuedBuilds > 1 && (
                <span className="stack-preview-queue-depth">{queuedBuilds} builds in the queue</span>
              )}
              {staleCount > 0 && (
                <span className="stack-preview-outdated-count">{staleCount} out of date</span>
              )}
            </div>
            <div className="stack-preview-grid">
              {displayKeys.map((key) => {
                const current = currentChannels.get(key);
                const activeEntry = activeByChannel.get(key);
                const activeGroup = activeEntry?.group;
                const latestEntry = latestByChannel.get(key);
                const activeArtifact: StackArtifact | undefined =
                  activeEntry && activeEntry.group.state === 'ready'
                    ? {
                        jobId: activeEntry.job.job_id,
                        artifactRevision: activeEntry.job.artifact_revision,
                        acceptedOnly: activeEntry.job.accepted_only,
                        group: activeEntry.group,
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
                const calibration = progressGroup?.calibration;
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
                      ? progressGroup?.phase === 'calibration'
                        ? 'Building calibration masters'
                        : progressGroup?.phase === 'orienting'
                          ? 'Solving and orienting sky view'
                        : progressGroup?.phase === 'rendering'
                          ? 'Rendering preview'
                          : artifact
                            ? 'Rebuilding stack'
                            : 'Registering frames'
                      : progressState === 'ready'
                        ? 'Stack ready'
                        : progressState === 'skipped'
                          ? 'Stack skipped'
                          : progressState === 'cancelled'
                            ? 'Stack stopped'
                            : progressState === 'error'
                              ? 'Stack failed'
                              : 'Not built';
                const reusedFrames = progressGroup?.reused_frames ?? 0;
                const progressDetail = progressGroup
                  ? `${processedFrames}/${eligibleFrames} frames${
                      reusedFrames > 0 ? ` · ${reusedFrames} resumed` : ''
                    }`
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
                            disabled={!canCompute || groupBusy || startPending || !canBuildChannel}
                            aria-label={artifact ? 'Rebuild channel' : 'Build channel'}
                            title={!canCompute
                              ? 'This account can view cached stacks but cannot build them.'
                              : artifact ? 'Rebuild channel' : 'Build channel'}
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
                    {progressGroup?.resume_note && (
                      <div className="stack-preview-resume-note">{progressGroup.resume_note}</div>
                    )}

                    {artifact && (
                      <div className="stack-preview-image">
                        <img
                          src={appliedStretch?.preview_url ?? apiClient.getStackPreviewUrl(
                            dbId, artifact.jobId, artifact.group.index, artifact.artifactRevision
                          )}
                          alt={`${targetName} ${filterName} stack preview`}
                        />
                        {isSkyOriented(artifact.group.sky_orientation) && (
                          <span className="stack-preview-orientation">N ↑ · E ←</span>
                        )}
                      </div>
                    )}
                    {artifact && stretchKey && (
                      <StackStretchControls
                        key={stretchKey}
                        label={`${targetName} ${filterName || 'no filter'}`}
                        channels={artifact.group.output_channels === 3 ? 3 : 1}
                        disabled={!canCompute || running}
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
                        {activeGroup?.state === 'queued'
                          ? 'Waiting for stacker'
                          : activeGroup?.phase === 'calibration'
                            ? 'Matching and building calibration masters'
                            : activeGroup?.phase === 'rendering'
                              ? 'Rendering preview'
                              : 'Registering frames'}
                      </div>
                    )}
                    {!artifact && !groupBusy && (
                      <div className={`stack-preview-placeholder ${activeGroup?.state === 'error' ? 'error' : ''}`}>
                        {activeGroup?.error ??
                          (activeGroup?.state === 'cancelled'
                            ? 'This channel was stopped before it finished. Build it again when you are ready.'
                            : canBuildChannel
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

                    {calibration && calibration.state !== 'none' && (
                      <div className="stack-preview-calibration">
                        <strong>
                          {calibration.state === 'applied'
                            ? 'Calibration applied'
                            : calibration.state === 'incomplete'
                              ? 'Calibration set incomplete'
                              : 'Matching calibration'}
                        </strong>
                        <span>
                          {calibration.bias_frames} bias · {calibration.dark_frames} dark ·{' '}
                          {calibration.dark_flat_frames} dark-flat · {calibration.flat_frames} flat
                        </span>
                        {calibration.warning && <span>{calibration.warning}</span>}
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
            ...(isSkyOriented(inspector.group.sky_orientation)
              ? ['North up · East left']
              : []),
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
          artifactSource={{
            kind: 'mono',
            dbId,
            jobId: inspector.jobId,
            groupIndex: inspector.group.index,
            artifactRevision: inspector.artifactRevision,
          }}
          artifactEnabled={canCompute}
          onOpenImage={onOpenImage}
          onClose={() => setInspector(null)}
        />
      )}
    </>
  );
}
