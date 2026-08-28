import { useEffect, useMemo, useState } from 'react';
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { CalibrationFrameSummary } from '../api/types';

interface CalibrationLibraryDialogProps {
  dbId: string;
  dbName: string;
  canManage: boolean;
  onClose: () => void;
  onImport?: () => void;
}

const KIND_LABELS: Record<CalibrationFrameSummary['kind'], string> = {
  bias: 'Bias',
  dark: 'Dark',
  dark_flat: 'Dark-flat',
  flat: 'Flat',
};

function fileName(path: string): string {
  return path.split(/[\\/]/).pop() || path;
}

function formatDate(timestamp?: number | null): string {
  if (timestamp === undefined || timestamp === null) return 'Date unknown';
  return new Date(timestamp * 1000).toLocaleString();
}

/**
 * The imaging night a capture belongs to: twelve hours back, then the date,
 * so frames from one session share a group across local midnight — the same
 * rule the server's calibration report uses.
 */
function nightKey(timestamp?: number | null): string {
  if (timestamp === undefined || timestamp === null) return 'unknown';
  return new Date((timestamp - 12 * 3600) * 1000).toISOString().slice(0, 10);
}

function nightLabel(key: string): string {
  return key === 'unknown' ? 'Date unknown' : `Night of ${key}`;
}

const VALIDITY_LABELS: Record<'forward' | 'backward', string> = {
  forward: '▸ after only',
  backward: '◂ before only',
};

const VALIDITY_TITLES: Record<'forward' | 'backward', string> = {
  forward:
    'Marked usable only for lights captured after this frame — shot after an optics change or cleaning.',
  backward:
    'Marked usable only for lights captured before this frame — shot before an optics change or cleaning.',
};

function formatSettings(frame: CalibrationFrameSummary): string {
  const values = [
    frame.width && frame.height ? `${frame.width}×${frame.height}` : null,
    frame.binning_x
      ? `${frame.binning_x}×${frame.binning_y ?? frame.binning_x} bin`
      : null,
    frame.gain !== null && frame.gain !== undefined ? `gain ${frame.gain}` : null,
    frame.offset !== null && frame.offset !== undefined ? `offset ${frame.offset}` : null,
    frame.exposure_s !== null && frame.exposure_s !== undefined
      ? `${frame.exposure_s}s`
      : null,
    frame.camera_temp !== null && frame.camera_temp !== undefined
      ? `${frame.camera_temp} °C`
      : null,
    frame.filter ? `filter ${frame.filter}` : null,
  ];
  return values.filter(Boolean).join(' · ') || 'No matching settings in the FITS header';
}

export default function CalibrationLibraryDialog({
  dbId,
  dbName,
  canManage,
  onClose,
  onImport,
}: CalibrationLibraryDialogProps) {
  const queryClient = useQueryClient();
  const [kind, setKind] = useState<'all' | CalibrationFrameSummary['kind']>('all');
  const [rig, setRig] = useState('all');
  const [missingOnly, setMissingOnly] = useState(false);
  const [visibleCount, setVisibleCount] = useState(100);
  const details = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'details'],
    queryFn: () => apiClient.getCalibrationLibraryDetails(dbId),
  });
  useEffect(() => {
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === 'Escape') onClose();
    };
    window.addEventListener('keydown', closeOnEscape);
    return () => window.removeEventListener('keydown', closeOnEscape);
  }, [onClose]);
  const refresh = () => {
    queryClient.invalidateQueries({ queryKey: ['db', dbId, 'calibrations'] });
  };
  const forget = useMutation({
    mutationFn: (frameUuid: string) => apiClient.forgetCalibrationFrame(dbId, frameUuid),
    onSuccess: refresh,
  });
  const clearMasters = useMutation({
    mutationFn: () => apiClient.clearCalibrationMasters(dbId),
    onSuccess: refresh,
  });
  const [selectedNights, setSelectedNights] = useState<Set<string>>(new Set());
  const [markDirection, setMarkDirection] = useState<'both' | 'forward' | 'backward'>('forward');
  const markValidity = useMutation({
    mutationFn: (input: { frameUuids: string[]; direction: 'both' | 'forward' | 'backward' }) =>
      apiClient.setCalibrationValidity(dbId, input.frameUuids, input.direction),
    onSuccess: () => {
      setSelectedNights(new Set());
      refresh();
    },
  });

  const frames = useMemo(
    () =>
      (details.data?.frames ?? []).filter(
        (frame) =>
          (kind === 'all' || frame.kind === kind) &&
          (rig === 'all' || frame.rig_uuid === rig) &&
          (!missingOnly || !frame.source_exists)
      ),
    [details.data?.frames, kind, missingOnly, rig]
  );
  useEffect(() => {
    setVisibleCount(100);
    setSelectedNights(new Set());
  }, [kind, missingOnly, rig]);
  const visibleFrames = frames.slice(0, visibleCount);

  // Grouped display: one section per imaging night, newest first, so a
  // day's frames — or a range of days — can be selected together and
  // marked around an optics change or cleaning.
  const nightGroups = useMemo(() => {
    const groups = new Map<string, CalibrationFrameSummary[]>();
    for (const frame of visibleFrames) {
      const key = nightKey(frame.captured_at);
      const bucket = groups.get(key);
      if (bucket) bucket.push(frame);
      else groups.set(key, [frame]);
    }
    return [...groups.entries()].sort(([left], [right]) => {
      if (left === 'unknown') return 1;
      if (right === 'unknown') return -1;
      return right.localeCompare(left);
    });
  }, [visibleFrames]);

  const toggleNight = (night: string) => {
    setSelectedNights((current) => {
      const next = new Set(current);
      if (next.has(night)) next.delete(night);
      else next.add(night);
      return next;
    });
  };
  const selectedFrames = useMemo(
    () => visibleFrames.filter((frame) => selectedNights.has(nightKey(frame.captured_at))),
    [visibleFrames, selectedNights]
  );

  const handleForget = (frame: CalibrationFrameSummary) => {
    if (
      window.confirm(
        `Forget ${fileName(frame.source_path)}? PSF Guard will remove its catalog record and dependent master records. The FITS file will not be deleted.`
      )
    ) {
      forget.mutate(frame.frame_uuid);
    }
  };

  const handleClearMasters = () => {
    if (
      window.confirm(
        'Clear all generated calibration masters for this database? Raw calibration FITS files and their catalog records will stay in place.'
      )
    ) {
      clearMasters.mutate();
    }
  };

  const summary = details.data?.summary;
  return (
    <div
      className="calibration-library-overlay"
      onClick={(event) => {
        event.stopPropagation();
        onClose();
      }}
    >
      <section
        className="calibration-library-dialog"
        role="dialog"
        aria-modal="true"
        aria-labelledby="calibration-library-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="calibration-library-header">
          <div>
            <h2 id="calibration-library-title">Calibration library</h2>
            <p>
              {dbName} · {summary?.frame_count ?? 0} raw frame
              {summary?.frame_count === 1 ? '' : 's'} · {summary?.master_count ?? 0} generated
              master{summary?.master_count === 1 ? '' : 's'}
            </p>
          </div>
          <button className="close-button" onClick={onClose} aria-label="Close calibration library">
            ×
          </button>
        </header>

        <div className="calibration-library-toolbar">
          <label>
            Rig
            <select value={rig} onChange={(event) => setRig(event.target.value)}>
              <option value="all">All rigs</option>
              {summary?.rigs.map((item) => (
                <option value={item.rig_uuid} key={item.rig_uuid}>
                  {item.name}
                </option>
              ))}
            </select>
          </label>
          <label>
            Type
            <select
              value={kind}
              onChange={(event) =>
                setKind(event.target.value as 'all' | CalibrationFrameSummary['kind'])
              }
            >
              <option value="all">All types</option>
              <option value="bias">Bias</option>
              <option value="dark">Dark</option>
              <option value="dark_flat">Dark-flat</option>
              <option value="flat">Flat</option>
            </select>
          </label>
          <label className="calibration-missing-filter">
            <input
              type="checkbox"
              checked={missingOnly}
              onChange={(event) => setMissingOnly(event.target.checked)}
            />
            Missing files only
          </label>
          <div className="calibration-library-actions">
            {canManage && onImport && (
              <button
                className="save-button"
                onClick={() => {
                  onClose();
                  onImport();
                }}
              >
                Scan configured folders
              </button>
            )}
            {canManage && (summary?.master_count ?? 0) > 0 && (
              <button
                className="browse-button"
                onClick={handleClearMasters}
                disabled={clearMasters.isPending}
              >
                {clearMasters.isPending ? 'Clearing…' : 'Clear generated masters'}
              </button>
            )}
          </div>
        </div>

        {!canManage && (
          <div className="calibration-library-notice">
            This server is read-only. Restart it with database management enabled to scan, forget
            entries, or clear generated masters.
          </div>
        )}
        {canManage && selectedNights.size > 0 && (
          <div className="calibration-validity-bar">
            <span>
              {selectedFrames.length} frame{selectedFrames.length === 1 ? '' : 's'} across{' '}
              {selectedNights.size} night{selectedNights.size === 1 ? '' : 's'} — usable
            </span>
            <select
              value={markDirection}
              aria-label="Validity direction"
              onChange={(event) =>
                setMarkDirection(event.target.value as 'both' | 'forward' | 'backward')
              }
            >
              <option value="forward">only for lights after them (after a change)</option>
              <option value="backward">only for lights before them (before a change)</option>
              <option value="both">in both directions again</option>
            </select>
            <button
              className="save-button"
              disabled={markValidity.isPending || selectedFrames.length === 0}
              onClick={() =>
                markValidity.mutate({
                  frameUuids: selectedFrames.map((frame) => frame.frame_uuid),
                  direction: markDirection,
                })
              }
            >
              {markValidity.isPending ? 'Marking…' : 'Mark'}
            </button>
            <button
              className="browse-button"
              onClick={() => setSelectedNights(new Set())}
              disabled={markValidity.isPending}
            >
              Clear selection
            </button>
          </div>
        )}
        {(forget.error || clearMasters.error || markValidity.error) && (
          <div className="calibration-library-error">
            {forget.error?.message || clearMasters.error?.message || markValidity.error?.message}
          </div>
        )}

        <div className="calibration-library-content">
          {details.isLoading && <div className="detecting-database">Loading calibration frames…</div>}
          {details.error && (
            <div className="calibration-library-error">
              Could not read the calibration library: {details.error.message}
            </div>
          )}
          {!details.isLoading && !details.error && frames.length === 0 && (
            <div className="calibration-library-empty">
              {summary?.frame_count
                ? 'No calibration frames match these filters.'
                : 'No calibration frames are cataloged yet. Add their folders to this database, then scan the configured folders.'}
            </div>
          )}
          {frames.length > 0 && (
            <table className="calibration-frame-table">
              <thead>
                <tr>
                  <th>Type</th>
                  <th>File</th>
                  <th>Capture settings</th>
                  <th>Status</th>
                  {canManage && <th aria-label="Actions" />}
                </tr>
              </thead>
              {nightGroups.map(([night, nightFrames]) => (
                <tbody key={night} className="calibration-night-group">
                  <tr className="calibration-night-row">
                    <td colSpan={canManage ? 5 : 4}>
                      <label>
                        {canManage && (
                          <input
                            type="checkbox"
                            checked={selectedNights.has(night)}
                            onChange={() => toggleNight(night)}
                            aria-label={`Select ${nightLabel(night)}`}
                          />
                        )}
                        <strong>{nightLabel(night)}</strong>
                        <span>
                          {nightFrames.length} frame{nightFrames.length === 1 ? '' : 's'}
                        </span>
                      </label>
                    </td>
                  </tr>
                  {nightFrames.map((frame) => (
                    <tr key={frame.frame_uuid} className={frame.source_exists ? '' : 'missing'}>
                      <td>
                        <span className={`calibration-kind calibration-kind-${frame.kind}`}>
                          {KIND_LABELS[frame.kind]}
                        </span>
                      </td>
                      <td>
                        <strong>{fileName(frame.source_path)}</strong>
                        <small className="calibration-frame-path" title={frame.source_path}>
                          {frame.source_path}
                        </small>
                        <small>{formatDate(frame.captured_at)}</small>
                      </td>
                      <td>
                        <span>{formatSettings(frame)}</span>
                        {frame.camera && <small>{frame.camera}</small>}
                      </td>
                      <td>
                        <span
                          className={
                            frame.source_exists
                              ? 'calibration-source-ready'
                              : 'calibration-source-missing'
                          }
                        >
                          {frame.source_exists ? '✓ Available' : '⚠ Missing'}
                        </span>
                        {frame.valid_direction && (
                          <span
                            className={`calibration-validity calibration-validity-${frame.valid_direction}`}
                            title={VALIDITY_TITLES[frame.valid_direction]}
                          >
                            {VALIDITY_LABELS[frame.valid_direction]}
                          </span>
                        )}
                      </td>
                      {canManage && (
                        <td>
                          <button
                            className="remove-button"
                            onClick={() => handleForget(frame)}
                            disabled={forget.isPending}
                          >
                            Forget
                          </button>
                        </td>
                      )}
                    </tr>
                  ))}
                </tbody>
              ))}
            </table>
          )}
          {frames.length > 0 && (
            <div className="calibration-library-footer">
              <span>
                Showing {visibleFrames.length} of {frames.length} matching frames
              </span>
              {visibleFrames.length < frames.length && (
                <button
                  className="browse-button"
                  onClick={() => setVisibleCount((count) => count + 100)}
                >
                  Show 100 more
                </button>
              )}
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
