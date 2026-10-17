import { useMemo, useState, type ReactNode } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../../api/client';
import type { CalibrationKind, LibraryCalibrationMaster } from '../../api/types';
import { CalibrationMasterInspector } from '../CalibrationMasterInspector';
import { KIND_LABELS, plural, serverTime } from './calibrationLabels';
import PillSelect from '../toolbar/PillSelect';
import ViewToolbar from '../toolbar/ViewToolbar';
import type { useCalibrationActions } from './useCalibrationActions';

interface CalibrationMasterListProps {
  dbId: string;
  dbName: string;
  canManage: boolean;
  opened: string | null;
  actions: ReturnType<typeof useCalibrationActions>;
  onOpen: (id: string | null) => void;
  /** Keep only the masters this project's lights calibrate with. */
  project?: number;
  /** The scope's revision: the list is asked again when it changes. */
  scopeRevision?: string;
  /** The project scope's chip, first in the bar. */
  scopeChip?: ReactNode;
  /** Clear removes the scope as well as the filters. */
  onClearScope?: () => void;
}

function sourcesText(master: LibraryCalibrationMaster): string {
  const span = master.sources;
  const frames = plural(master.source_count ?? span?.frames ?? 0, 'frame');
  if (!span) return frames;
  const nights = span.first_night && span.last_night
    ? span.nights > 1 ? `${span.nights} nights, ${span.first_night} to ${span.last_night}` : span.first_night
    : null;
  const temps = span.temp_min_c != null && span.temp_max_c != null
    ? span.temp_min_c === span.temp_max_c
      ? `${span.temp_min_c.toFixed(1)} °C`
      : `${span.temp_min_c.toFixed(1)} to ${span.temp_max_c.toFixed(1)} °C`
    : null;
  return [frames, nights, temps].filter(Boolean).join(' · ');
}

/** What sets a master apart from others of its kind. A dark's filter is
 *  wherever the wheel happened to sit, so only a flat's counts. */
function masterTitle(master: LibraryCalibrationMaster): string {
  if (master.kind === 'flat') return master.filter || 'No filter';
  if (master.exposure_s != null) return `${Number(master.exposure_s.toFixed(2))}s`;
  return master.rig_name || master.label;
}

/** What the flat-set check said about a flat master's flats. */
function flatSetText(master: LibraryCalibrationMaster): string | null {
  const set = master.flat_set;
  if (!set || set.changing_features === undefined) return null;
  return `${set.changing_features} changing, ${set.carried_features ?? 0} carried, ${set.steady_features ?? 0} steady spots`;
}

/** The masters the library recorded, and an inspector for any of them. */
export default function CalibrationMasterList({
  dbId, dbName, canManage, opened, actions, onOpen, project, scopeRevision, scopeChip, onClearScope,
}: CalibrationMasterListProps) {
  const [kind, setKind] = useState<CalibrationKind | ''>('');
  const [onDisk, setOnDisk] = useState(false);
  const catalog = useQuery({
    queryKey: ['db', dbId, 'calibrations', 'masters', project ?? 'all', scopeRevision],
    queryFn: () => apiClient.getLibraryCalibrationMasters(dbId, project),
  });
  const masters = useMemo(
    () => (catalog.data?.masters ?? []).filter(
      (master) => (!kind || master.kind === kind) && (!onDisk || master.available)
    ),
    [catalog.data?.masters, kind, onDisk]
  );
  const clear = () => {
    if (window.confirm('Clear every generated master of this database? Raw frames and their records stay; stacks build masters again when they need them.')) {
      actions.clearMasters.mutate();
    }
  };
  return (
    <section className="calibration-masters" aria-label="Masters">
      <ViewToolbar
        label="Master filters"
        scope={scopeChip}
        filters={(
          <>
            <PillSelect
              label="Type"
              active={kind !== ''}
              value={kind}
              onChange={(event) => setKind(event.target.value as CalibrationKind | '')}
            >
              <option value="">All</option>
              {(['flat', 'dark_flat', 'dark', 'bias'] as const).map((value) => (
                <option key={value} value={value}>{KIND_LABELS[value]}</option>
              ))}
            </PillSelect>
            <label className="toolbar-pill toolbar-pill-check">
              <input type="checkbox" checked={onDisk} onChange={(event) => setOnDisk(event.target.checked)} />
              Files on disk only
            </label>
          </>
        )}
        activeFilters={Number(kind !== '') + Number(onDisk)}
        onClearFilters={() => {
          setKind('');
          setOnDisk(false);
          onClearScope?.();
        }}
        menu={canManage && (catalog.data?.masters.length ?? 0) > 0 ? (
          <button type="button" className="toolbar-menu-item toolbar-menu-item-danger" disabled={actions.pending} onClick={clear}>
            Clear generated masters
          </button>
        ) : undefined}
        summary={plural(masters.length, 'master')}
      />
      {actions.error && <p className="calibration-error" role="alert">{actions.error}</p>}
      {catalog.isLoading && <p className="calibration-muted">Loading…</p>}
      {catalog.error && <p className="calibration-error">Could not read the masters: {catalog.error.message}</p>}
      {catalog.data && masters.length === 0 && (
        <p className="calibration-empty">
          {catalog.data.masters.length
            ? 'No masters match.'
            : project !== undefined
              ? 'No master this project’s lights calibrate with yet. A stack builds them.'
              : 'No masters yet. A stack builds them.'}
        </p>
      )}
      {masters.length > 0 && (
        <ul className="calibration-master-cards">
          {masters.map((master) => {
            const flatSet = flatSetText(master);
            const verdict = master.flat_set?.master?.grade;
            return (
              <li key={master.id} className="calibration-master-card">
                <div className="calibration-master-card-head">
                  <span className={`calibration-kind calibration-kind-${master.kind}`}>{KIND_LABELS[master.kind]}</span>
                  <strong>{masterTitle(master)}</strong>
                  <button type="button" className="tool-button" disabled={!master.available}
                    onClick={() => onOpen(master.id)} aria-label={`View ${KIND_LABELS[master.kind]} master ${master.label}`}>
                    View
                  </button>
                </div>
                <p className="calibration-muted">{sourcesText(master)}</p>
                <p className="calibration-muted">
                  {[master.rig_name, `built ${serverTime(master.created_at)}`].filter(Boolean).join(' · ')}
                </p>
                <div className="calibration-badges">
                  {master.thin && <span className="calibration-badge calibration-badge-poor" title="This master has too few frames and can add noise during calibration">Too few frames</span>}
                  {verdict && verdict !== 'ok' && (
                    <span className={`calibration-badge calibration-badge-${verdict}`} title={flatSet ?? undefined}>
                      {verdict === 'poor' ? 'Poor' : 'Suspect'} · spots
                    </span>
                  )}
                  {master.set_aside && <span className="calibration-badge calibration-badge-suspect">Set aside</span>}
                  {master.external && <span className="calibration-badge">Other software</span>}
                  {!master.available && <span className="calibration-badge calibration-badge-poor" title={master.unavailable_reason ?? undefined}>Missing file</span>}
                </div>
                {flatSet && <p className="calibration-muted">{flatSet}</p>}
                {master.flat_note && <p className="calibration-muted">{master.flat_note}</p>}
                {master.used_by.length > 0 && (
                  <p className="calibration-muted">
                    Used by {master.used_by.map((use) => use.name ?? `project ${use.project_id}`).join(', ')}
                  </p>
                )}
              </li>
            );
          })}
        </ul>
      )}
      {opened && (
        <CalibrationMasterInspector
          dbId={dbId}
          source={{ kind: 'library' }}
          title={dbName}
          label="Library"
          initialMasterId={opened}
          onClose={() => onOpen(null)}
        />
      )}
    </section>
  );
}
