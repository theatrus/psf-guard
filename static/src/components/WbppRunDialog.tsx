import { useEffect, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { WbppOptions, WbppOutputFile, WbppRunProgress } from '../api/types';
import { useWbppRun } from '../hooks/useWbppRun';
import { openSettings } from '../utils/settingsIntent';
import Dialog from './Dialog';
import WbppOptionsFields from './WbppOptionsFields';
import { describePixInsight, formatFree } from '../utils/pixinsight';
import './WbppRunDialog.css';

/** The project or target to stack. */
export interface WbppRunRequest {
  dbId: string;
  scope: { project_id?: number; target_id?: number };
  label: string;
}

interface Props {
  request: WbppRunRequest;
  /** What the settings start from, per the settings panel. */
  defaultOptions: WbppOptions;
  onClose: () => void;
}

function formatBytes(bytes: number): string {
  if (bytes >= 1 << 30) return `${(bytes / (1 << 30)).toFixed(2)} GiB`;
  if (bytes >= 1 << 20) return `${(bytes / (1 << 20)).toFixed(1)} MiB`;
  if (bytes >= 1 << 10) return `${Math.round(bytes / (1 << 10))} KiB`;
  return `${bytes} B`;
}

function formatElapsed(seconds: number): string {
  const h = Math.floor(seconds / 3600);
  const m = Math.floor((seconds % 3600) / 60);
  const s = Math.floor(seconds % 60);
  return h > 0 ? `${h}h ${m}m ${s}s` : m > 0 ? `${m}m ${s}s` : `${s}s`;
}

/** Seconds since a run began, ticking while it runs. */
function useElapsed(progress: WbppRunProgress | undefined): number | null {
  const [now, setNow] = useState(() => Date.now() / 1000);
  const running = progress?.running ?? false;
  useEffect(() => {
    if (!running) return;
    const timer = window.setInterval(() => setNow(Date.now() / 1000), 1000);
    return () => window.clearInterval(timer);
  }, [running]);
  if (!progress?.started_at) return null;
  const end = progress.finished_at ?? now;
  return Math.max(0, end - progress.started_at);
}

const STAGE_LABEL: Record<string, string> = {
  planning: 'Planning the frames',
  launching: 'Starting PixInsight',
  running: 'PixInsight is running WBPP',
  complete: 'Finished',
  error: 'Failed',
  cancelled: 'Stopped',
};

/**
 * Stack a project or target with WBPP on the server: the settings, then
 * WBPP's own progress, then what it wrote. One run per catalog at a time,
 * so an open dialog shows whatever run is under way.
 */
export default function WbppRunDialog({ request, defaultOptions, onClose }: Props) {
  const queryClient = useQueryClient();
  const pixinsight = useQuery({
    queryKey: ['pixinsight-settings'],
    queryFn: apiClient.getPixInsightSettings,
  });
  const run = useWbppRun(request.dbId);
  const progress = run.progress;
  const elapsed = useElapsed(progress);

  const [options, setOptions] = useState<WbppOptions>(defaultOptions);
  const [includePending, setIncludePending] = useState(true);
  const [extra, setExtra] = useState('');
  const [workRoot, setWorkRoot] = useState('');
  // The form shows when there is no run to look at, and again on request
  // once a run has finished; a run under way or just finished shows itself.
  const [formRequested, setFormRequested] = useState(false);

  const start = useMutation({
    mutationFn: () =>
      apiClient.startWbppRun(request.dbId, {
        ...request.scope,
        include_pending: includePending,
        options,
        extra_params: extra
          .split(/[\n,]/)
          .map((param) => param.trim())
          .filter(Boolean),
        scope_label: request.label,
        ...(workRoot.trim() ? { work_root: workRoot.trim() } : {}),
      }),
    onSuccess: (status) => {
      queryClient.setQueryData(['db', request.dbId, 'wbpp-run'], status);
      setFormRequested(false);
      if (!status.started) {
        alert('A WBPP run is already under way for this database; its progress is shown here.');
      }
    },
  });
  const cancel = useMutation({
    mutationFn: () => apiClient.cancelWbppRun(request.dbId),
    onSuccess: (status) => queryClient.setQueryData(['db', request.dbId, 'wbpp-run'], status),
  });

  const running = progress?.running ?? false;
  const ready = pixinsight.data?.ready ?? false;
  const hasResult = !!progress && !running && !!progress.finished_at;
  const formVisible = !running && (formRequested || !hasResult);
  const fileUrl = (path: string) => apiClient.wbppRunFileUrl(request.dbId, path);
  const relativeLog =
    progress?.log_path && progress.work_dir && progress.log_path.startsWith(progress.work_dir)
      ? progress.log_path.slice(progress.work_dir.length).replace(/^[\\/]+/, '').replace(/\\/g, '/')
      : null;

  return (
    <Dialog
      open
      title={`Stack with WBPP — ${request.label}`}
      onClose={onClose}
      className="wbpp-run-dialog"
      footer={
        <>
          <button type="button" className="header-button" onClick={onClose}>
            Close
          </button>
          {running && (
            <button
              type="button"
              className="header-button"
              disabled={cancel.isPending}
              onClick={() => cancel.mutate()}
            >
              Stop PixInsight
            </button>
          )}
          {hasResult && !formVisible && (
            <button type="button" className="header-button" onClick={() => setFormRequested(true)}>
              Stack again
            </button>
          )}
          {formVisible && (
            <button
              type="button"
              className="action-button"
              disabled={!ready || start.isPending}
              onClick={() => start.mutate()}
            >
              Start stacking
            </button>
          )}
        </>
      }
    >
      {pixinsight.data && (
        <p
          className={`wbpp-run-pixinsight${pixinsight.data.ready ? ' is-ready' : ' is-missing'}`}
          role="status"
        >
          {describePixInsight(pixinsight.data)}{' '}
          {!pixinsight.data.ready && (
            <button type="button" className="link-button" onClick={() => openSettings()}>
              Open settings
            </button>
          )}
        </p>
      )}
      {pixinsight.isError && (
        <p className="wbpp-run-error">{(pixinsight.error as Error).message}</p>
      )}

      {formVisible && (
        <>
          <p className="wbpp-run-intro">
            PixInsight runs WBPP on the server against the frames where they are, with each
            night&apos;s flats matched to its lights. Rejected lights never go in. Results land
            in a run folder under the cache and are listed here when WBPP finishes.
          </p>
          <label className="export-dialog-option">
            <input
              type="checkbox"
              checked={includePending}
              onChange={(event) => setIncludePending(event.target.checked)}
            />
            <span>
              <strong>Include ungraded lights</strong>
              <small>Frames the grader has not judged yet, as the stack previews do.</small>
            </span>
          </label>
          <fieldset className="export-layout-options">
            <legend>WBPP settings</legend>
            <WbppOptionsFields value={options} onChange={setOptions} idPrefix="run-wbpp" />
          </fieldset>
          <label className="wbpp-run-extra">
            <span>
              More WBPP parameters
              <small>
                Optional, one <code>name=value</code> per line, as WBPP&apos;s automation help
                lists them (Alt+A in WBPP).
              </small>
            </span>
            <textarea
              rows={2}
              value={extra}
              placeholder="maxStars=500"
              onChange={(event) => setExtra(event.target.value)}
            />
          </label>
          <label className="wbpp-run-extra">
            <span>
              Run folder
              <small>
                Where this run&apos;s script and WBPP&apos;s output go; a run writes gigabytes.
                Empty uses{' '}
                {pixinsight.data?.runs_dir
                  ? `the runs folder in Settings (${pixinsight.data.runs_dir}${
                      pixinsight.data.runs_dir_free_bytes != null
                        ? `, ${formatFree(pixinsight.data.runs_dir_free_bytes)} free`
                        : ''
                    })`
                  : "the database's export directory, else the cache"}
                .
              </small>
            </span>
            <input
              type="text"
              value={workRoot}
              placeholder="/data/wbpp-runs"
              onChange={(event) => setWorkRoot(event.target.value)}
            />
          </label>
          {start.isError && <p className="wbpp-run-error">{(start.error as Error).message}</p>}
        </>
      )}

      {progress && (running || !formVisible) && (
        <div className="wbpp-run-progress" aria-live="polite">
          <div className="wbpp-run-stage">
            <strong>{STAGE_LABEL[progress.stage] ?? progress.stage}</strong>
            {elapsed !== null && <span className="wbpp-run-elapsed">{formatElapsed(elapsed)}</span>}
          </div>
          {progress.free_bytes_at_start != null && (
            <p className="wbpp-run-muted">
              {formatFree(progress.free_bytes_at_start)} free at the run folder when it began.
            </p>
          )}
          {progress.frames > 0 && (
            <p className="wbpp-run-muted">
              {progress.lights} light{progress.lights === 1 ? '' : 's'} and{' '}
              {progress.frames - progress.lights} calibration frame
              {progress.frames - progress.lights === 1 ? '' : 's'}
              {progress.missing_files > 0 &&
                `; ${progress.missing_files} catalog row(s) had no file and were left out`}
              .
            </p>
          )}
          {progress.wbpp_stage && (
            <p className="wbpp-run-step">
              WBPP: {progress.wbpp_stage}
              {progress.wbpp_steps > 1 ? ` (step ${progress.wbpp_steps})` : ''}
            </p>
          )}
          {progress.error && <p className="wbpp-run-error">{progress.error}</p>}
          {progress.log_errors.length > 0 && (
            <ul className="wbpp-run-log-errors">
              {progress.log_errors.slice(-5).map((line, index) => (
                <li key={`${index}-${line}`}>{line}</li>
              ))}
            </ul>
          )}
          {progress.log_tail.length > 0 && (
            <pre className="wbpp-run-log">{progress.log_tail.join('\n')}</pre>
          )}
          {hasResult && (
            <div className="wbpp-run-results">
              {progress.outputs.filter((file) => file.kind === 'master').length > 0 && (
                <>
                  <h4>Masters</h4>
                  <ul className="wbpp-run-files">
                    {progress.outputs
                      .filter((file: WbppOutputFile) => file.kind === 'master')
                      .map((file) => (
                        <li key={file.path}>
                          <a href={fileUrl(`wbpp-out/${file.path}`)} download>
                            {file.path.replace(/^master\//, '')}
                          </a>
                          <small> {formatBytes(file.size_bytes)}</small>
                        </li>
                      ))}
                  </ul>
                </>
              )}
              <p className="wbpp-run-muted">
                Run folder: <code>{progress.work_dir}</code>
                {' · '}
                <a href={fileUrl('run-wbpp.js')} download>
                  run-wbpp.js
                </a>
                {relativeLog && (
                  <>
                    {' · '}
                    <a href={fileUrl(relativeLog)} download>
                      WBPP log
                    </a>
                  </>
                )}
                {' · '}
                <a href={fileUrl('pixinsight.log')} download>
                  console
                </a>
              </p>
            </div>
          )}
        </div>
      )}
    </Dialog>
  );
}
