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

/**
 * Darks and bias live in their own sections: they stay valid far longer
 * than flats, so their long-lived batches must not interleave with the
 * per-night flat groups. Dark-flats belong with the flats they pair with.
 */
type LibrarySection = 'flats' | 'darks' | 'bias';

const SECTION_LABELS: Record<LibrarySection, string> = {
  flats: 'Flats',
  darks: 'Darks',
  bias: 'Bias',
};

const SECTION_ORDER: LibrarySection[] = ['flats', 'darks', 'bias'];

function sectionOf(kind: CalibrationFrameSummary['kind']): LibrarySection {
  if (kind === 'flat' || kind === 'dark_flat') return 'flats';
  return kind === 'dark' ? 'darks' : 'bias';
}

interface NightGroup {
  key: string;
  night: string;
  frames: CalibrationFrameSummary[];
}

function kindBreakdown(frames: CalibrationFrameSummary[]): string {
  const counts = new Map<CalibrationFrameSummary['kind'], number>();
  for (const frame of frames) {
    counts.set(frame.kind, (counts.get(frame.kind) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([kind, count]) => `${count} ${KIND_LABELS[kind].toLowerCase()}`)
    .join(' · ');
}

/** The group's shared mark, when every frame agrees on one. */
function groupValidity(frames: CalibrationFrameSummary[]): 'forward' | 'backward' | null {
  const first = frames[0]?.valid_direction ?? null;
  if (!first) return null;
  return frames.every((frame) => frame.valid_direction === first) ? first : null;
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
  const forgetNight = useMutation({
    mutationFn: (frameUuids: string[]) => apiClient.forgetCalibrationFrames(dbId, frameUuids),
    onSuccess: refresh,
  });
  const clearMasters = useMutation({
    mutationFn: () => apiClient.clearCalibrationMasters(dbId),
    onSuccess: refresh,
  });
  const [selectedGroups, setSelectedGroups] = useState<Set<string>>(new Set());
  const [expandedGroups, setExpandedGroups] = useState<Set<string>>(new Set());
  const [markDirection, setMarkDirection] = useState<'both' | 'forward' | 'backward'>('forward');
  const markValidity = useMutation({
    mutationFn: (input: { frameUuids: string[]; direction: 'both' | 'forward' | 'backward' }) =>
      apiClient.setCalibrationValidity(dbId, input.frameUuids, input.direction),
    onSuccess: () => {
      setSelectedGroups(new Set());
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
    setSelectedGroups(new Set());
  }, [kind, missingOnly, rig]);

  // Grouped display, collapsed first: flats (with their dark-flats) get one
  // group per imaging night; darks and bias sit in their own sections since
  // their batches stay valid far longer. Groups render as header rows only
  // until expanded, so a large library stays a short list of nights.
  const sections = useMemo(() => {
    const built = new Map<LibrarySection, Map<string, NightGroup>>();
    for (const frame of frames) {
      const section = sectionOf(frame.kind);
      const night = nightKey(frame.captured_at);
      const key = `${section}:${night}`;
      const groups = built.get(section) ?? new Map<string, NightGroup>();
      const group = groups.get(key) ?? { key, night, frames: [] };
      group.frames.push(frame);
      groups.set(key, group);
      built.set(section, groups);
    }
    return SECTION_ORDER.flatMap((section) => {
      const groups = built.get(section);
      if (!groups) return [];
      const ordered = [...groups.values()].sort((left, right) => {
        if (left.night === 'unknown') return 1;
        if (right.night === 'unknown') return -1;
        return right.night.localeCompare(left.night);
      });
      return [{ section, groups: ordered }];
    });
  }, [frames]);

  const toggleSelected = (key: string) => {
    setSelectedGroups((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };
  const toggleExpanded = (key: string) => {
    setExpandedGroups((current) => {
      const next = new Set(current);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };
  const selectedFrames = useMemo(
    () =>
      sections.flatMap(({ groups }) =>
        groups
          .filter((group) => selectedGroups.has(group.key))
          .flatMap((group) => group.frames)
      ),
    [sections, selectedGroups]
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

  const handleForgetNight = (section: LibrarySection, group: NightGroup) => {
    if (
      window.confirm(
        `Forget every ${SECTION_LABELS[section].toLowerCase()} frame from ${nightLabel(group.night)} (${group.frames.length} record${group.frames.length === 1 ? '' : 's'})? PSF Guard removes the catalog records and dependent masters. The FITS files will not be deleted.`
      )
    ) {
      forgetNight.mutate(group.frames.map((frame) => frame.frame_uuid));
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
        {canManage && selectedGroups.size > 0 && (
          <div className="calibration-validity-bar">
            <span>
              {selectedFrames.length} frame{selectedFrames.length === 1 ? '' : 's'} across{' '}
              {selectedGroups.size} group{selectedGroups.size === 1 ? '' : 's'} — usable
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
              onClick={() => setSelectedGroups(new Set())}
              disabled={markValidity.isPending}
            >
              Clear selection
            </button>
          </div>
        )}
        {(forget.error || forgetNight.error || clearMasters.error || markValidity.error) && (
          <div className="calibration-library-error">
            {forget.error?.message ||
              forgetNight.error?.message ||
              clearMasters.error?.message ||
              markValidity.error?.message}
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
              {sections.map(({ section, groups }) => (
                <tbody key={section} className="calibration-section-group">
                  <tr className="calibration-section-row">
                    <td colSpan={canManage ? 5 : 4}>
                      <strong>{SECTION_LABELS[section]}</strong>
                      <span>
                        {groups.reduce((sum, group) => sum + group.frames.length, 0)} frame
                        {groups.reduce((sum, group) => sum + group.frames.length, 0) === 1
                          ? ''
                          : 's'}{' '}
                        · {groups.length} night{groups.length === 1 ? '' : 's'}
                      </span>
                    </td>
                  </tr>
                  {groups.map((group) => {
                    const expanded = expandedGroups.has(group.key);
                    const shared = groupValidity(group.frames);
                    return [
                      <tr key={group.key} className="calibration-night-row">
                        <td colSpan={canManage ? 5 : 4}>
                          <div className="calibration-night-controls">
                            {canManage && (
                              <input
                                type="checkbox"
                                checked={selectedGroups.has(group.key)}
                                onChange={() => toggleSelected(group.key)}
                                aria-label={`Select ${SECTION_LABELS[section]} ${nightLabel(group.night)}`}
                              />
                            )}
                            <button
                              type="button"
                              className="calibration-night-toggle"
                              aria-expanded={expanded}
                              onClick={() => toggleExpanded(group.key)}
                            >
                              <span
                                className={`expand-toggle ${expanded ? 'expanded' : ''}`}
                                aria-hidden="true"
                              >
                                ▶
                              </span>
                              <strong>{nightLabel(group.night)}</strong>
                              <span>{kindBreakdown(group.frames)}</span>
                            </button>
                            {shared && (
                              <span
                                className={`calibration-validity calibration-validity-${shared}`}
                                title={VALIDITY_TITLES[shared]}
                              >
                                {VALIDITY_LABELS[shared]}
                              </span>
                            )}
                            {/* Destructive, so it only appears once the
                                night's checkbox says the user means this
                                group. */}
                            {canManage && selectedGroups.has(group.key) && (
                              <button
                                className="remove-button"
                                onClick={() => handleForgetNight(section, group)}
                                disabled={forgetNight.isPending}
                              >
                                Forget night
                              </button>
                            )}
                          </div>
                        </td>
                      </tr>,
                      ...(expanded ? group.frames : []).map((frame) => (
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
                      )),
                    ];
                  })}
                </tbody>
              ))}
            </table>
          )}
          {frames.length > 0 && (
            <div className="calibration-library-footer">
              <span>
                {frames.length} matching frame{frames.length === 1 ? '' : 's'} in{' '}
                {sections.reduce((sum, { groups }) => sum + groups.length, 0)} night group
                {sections.reduce((sum, { groups }) => sum + groups.length, 0) === 1 ? '' : 's'}
              </span>
            </div>
          )}
        </div>
      </section>
    </div>
  );
}
