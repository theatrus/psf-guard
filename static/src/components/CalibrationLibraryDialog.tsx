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
  useEffect(() => setVisibleCount(100), [kind, missingOnly, rig]);
  const visibleFrames = frames.slice(0, visibleCount);

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
        {(forget.error || clearMasters.error) && (
          <div className="calibration-library-error">
            {forget.error?.message || clearMasters.error?.message}
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
              <tbody>
                {visibleFrames.map((frame) => (
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
