import { useEffect, useRef, useState } from 'react';
import { useScopedDbId } from '../hooks/useUrlState';
import { useQualityBackfillStatus } from '../hooks/useQualityBackfill';
import { useSpatialScanStatus } from '../hooks/useSpatialScan';
import CacheRefreshStatus from './CacheRefreshStatus';
import StackActivityStatus from './StackActivityStatus';
import './CacheRefreshStatus.css';

interface DatabaseActivityStatusProps {
  className?: string;
}

interface QualityCompletion {
  kind: 'scan' | 'backfill';
  completed: number;
  total: number;
  errors: number;
  lastError?: string;
}

export default function DatabaseActivityStatus({
  className = '',
}: DatabaseActivityStatusProps) {
  const dbId = useScopedDbId();
  const qualityScan = useSpatialScanStatus(dbId);
  const qualityBackfill = useQualityBackfillStatus(dbId);
  const scanProgress = qualityScan.status?.progress;
  const backfillProgress = qualityBackfill.status?.progress;
  const qualityRunning = qualityScan.isRunning || qualityBackfill.isRunning;
  const [completion, setCompletion] = useState<QualityCompletion | null>(null);
  const wasRunning = useRef(false);
  const activityKind = useRef<QualityCompletion['kind']>('scan');
  const completionTimer = useRef<number | null>(null);

  useEffect(() => () => {
    if (completionTimer.current != null) window.clearTimeout(completionTimer.current);
  }, []);

  useEffect(() => {
    if (qualityRunning) {
      if (!wasRunning.current) {
        setCompletion(null);
        if (completionTimer.current != null) {
          window.clearTimeout(completionTimer.current);
          completionTimer.current = null;
        }
      }
      if (!wasRunning.current || qualityBackfill.isRunning) {
        activityKind.current = qualityBackfill.isRunning ? 'backfill' : 'scan';
      }
      wasRunning.current = true;
      return;
    }
    if (!wasRunning.current) return;

    wasRunning.current = false;
    const kind = activityKind.current;
    setCompletion({
      kind,
      completed: kind === 'backfill'
        ? backfillProgress?.processed_targets ?? 0
        : scanProgress?.processed ?? 0,
      total: kind === 'backfill'
        ? backfillProgress?.total_targets ?? 0
        : scanProgress?.total ?? 0,
      errors: kind === 'scan' ? scanProgress?.errors ?? 0 : 0,
      lastError: kind === 'scan' ? scanProgress?.last_error ?? undefined : undefined,
    });
    completionTimer.current = window.setTimeout(() => {
      setCompletion(null);
      completionTimer.current = null;
    }, 2500);
  }, [
    backfillProgress?.processed_targets,
    backfillProgress?.total_targets,
    qualityBackfill.isRunning,
    qualityRunning,
    scanProgress?.errors,
    scanProgress?.last_error,
    scanProgress?.processed,
    scanProgress?.total,
  ]);

  // Stacking shares this slot with analysis and cache refresh: it stays a
  // secondary chip so a long stack build is visible from every view.
  if (!qualityRunning && !completion) {
    return (
      <>
        <CacheRefreshStatus className={className} />
        <StackActivityStatus className={className} />
      </>
    );
  }

  const showingBackfill = qualityRunning
    ? qualityBackfill.isRunning || activityKind.current === 'backfill'
    : completion?.kind === 'backfill';
  const completed = qualityRunning
    ? showingBackfill
      ? backfillProgress?.processed_targets ?? 0
      : scanProgress?.processed ?? 0
    : completion?.completed ?? 0;
  const total = qualityRunning
    ? showingBackfill
      ? backfillProgress?.total_targets ?? 0
      : scanProgress?.total ?? 0
    : completion?.total ?? 0;
  const errors = qualityRunning ? 0 : completion?.errors ?? 0;
  const percentage = qualityRunning && total > 0
    ? Math.min((completed / total) * 100, 100)
    : 100;
  const scanStage = scanProgress?.stage === 'astrometry' ? 'Solving' : 'Scanning';
  const detail = qualityRunning
    ? showingBackfill
      ? `${completed}/${total} targets${scanProgress?.running
        ? ` · ${scanStage} ${scanProgress.processed}/${scanProgress.total} frames`
        : backfillProgress?.current_target_id != null
          ? ` · Target ${backfillProgress.current_target_id}`
          : ''}`
      : `${scanStage} ${completed}/${total} frames${scanProgress?.current_file
        ? ` · ${scanProgress.current_file}`
        : ''}`
    : `${completed}/${total} ${showingBackfill ? 'targets' : 'frames'}${errors > 0
      ? ` · ${errors} error${errors === 1 ? '' : 's'}`
      : ''}`;
  const label = qualityRunning
    ? showingBackfill ? 'Analyzing database quality' : 'Analyzing quality'
    : `${showingBackfill ? 'Database quality analysis' : 'Quality analysis'} finished${errors > 0 ? ' with errors' : ''}`;
  const title = qualityRunning
    ? scanProgress?.current_file ?? undefined
    : completion?.lastError;

  return (
    <>
      <div
        className={`cache-refresh-status visible quality-analysis-status ${className}`}
        aria-live="polite"
        title={title}
      >
        <div className="cache-status-content">
          <div className="progress-indicator">
            {qualityRunning
              ? <div className="pulsating-bar" />
              : <span aria-hidden="true">{errors > 0 ? '!' : '✓'}</span>}
          </div>
          <div className="cache-status-main">
            <div className="cache-status-label">{label}</div>
            {total > 0 && (
              <div className="cache-progress-bar">
                <div className="cache-progress-fill" style={{ width: `${percentage}%` }} />
                <span className="cache-progress-text">{Math.round(percentage)}%</span>
              </div>
            )}
          </div>
          <div className="cache-status-details">
            <div className="progress-info-row">{detail}</div>
          </div>
        </div>
      </div>
      <StackActivityStatus className={className} />
    </>
  );
}
