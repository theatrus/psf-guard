import { useEffect, useState } from 'react';
import type { CalibrationFacets, CalibrationRigSummary } from '../../api/types';
import PillSelect from '../toolbar/PillSelect';
import ToolbarPopover from '../toolbar/ToolbarPopover';
import ViewToolbar from '../toolbar/ViewToolbar';
import { KIND_LABELS } from './calibrationLabels';
import { ALL_RIGS, type CalibrationFilterState } from './useCalibrationUrlState';

interface CalibrationFiltersProps {
  filters: CalibrationFilterState;
  facets: CalibrationFacets | undefined;
  rigs: CalibrationRigSummary[];
  onChange: (changes: Partial<CalibrationFilterState>) => void;
  onClear: () => void;
}

/** A number typed in full before it narrows the lists. */
function NumberField({ label, value, onCommit }: { label: string; value: number | undefined; onCommit: (value: number | undefined) => void }) {
  const [text, setText] = useState(value === undefined ? '' : String(value));
  useEffect(() => setText(value === undefined ? '' : String(value)), [value]);
  const commit = () => {
    const trimmed = text.trim();
    const parsed = trimmed === '' ? undefined : Number(trimmed);
    if (parsed === undefined || Number.isFinite(parsed)) {
      if (parsed !== value) onCommit(parsed);
    } else {
      setText(value === undefined ? '' : String(value));
    }
  };
  return (
    <label className="toolbar-field">
      <span className="toolbar-caption">{label}</span>
      <input
        type="number"
        inputMode="decimal"
        step="any"
        value={text}
        onChange={(event) => setText(event.target.value)}
        onBlur={commit}
        onKeyDown={(event) => {
          if (event.key === 'Enter') commit();
        }}
      />
    </label>
  );
}

const exposureLabel = (seconds: number) => `${Number(seconds.toFixed(2))}s`;

function rangeLabel(from: string | undefined, to: string | undefined): string {
  if (from && to) return `${from} – ${to}`;
  if (from) return `from ${from}`;
  if (to) return `to ${to}`;
  return 'Any';
}

/** What the session list and the strip keep, as pills in the shared
 *  toolbar: in one row on a wide screen, folded behind its Filters button
 *  on a phone. */
export default function CalibrationFilters({ filters, facets, rigs, onChange, onClear }: CalibrationFiltersProps) {
  const active = Object.entries(filters)
    .filter(([key, value]) => key !== 'sort' && value !== undefined && !(key === 'rig' && value === ALL_RIGS))
    .length;
  const celsius = (value: number | undefined) => (value === undefined ? undefined : `${value} °C`);
  const fields = (
    <>
      {rigs.length > 1 && (
        <PillSelect
          label="Rig"
          active={filters.rig !== undefined && filters.rig !== ALL_RIGS}
          value={filters.rig ?? ALL_RIGS}
          onChange={(event) => onChange({ rig: event.target.value })}
        >
          <option value={ALL_RIGS}>All</option>
          {rigs.map((rig) => <option key={rig.rig_uuid} value={rig.rig_uuid}>{rig.name}</option>)}
        </PillSelect>
      )}
      <PillSelect
        label="Type"
        active={filters.kind !== undefined}
        value={filters.kind ?? ''}
        onChange={(event) => onChange({ kind: (event.target.value || undefined) as CalibrationFilterState['kind'] })}
      >
        <option value="">All</option>
        {(['flat', 'dark_flat', 'dark', 'bias'] as const).map((kind) => (
          <option key={kind} value={kind}>{KIND_LABELS[kind]}</option>
        ))}
      </PillSelect>
      <PillSelect
        label="Filter"
        active={filters.filter !== undefined}
        value={filters.filter ?? ''}
        onChange={(event) => onChange({ filter: event.target.value || undefined })}
      >
        <option value="">All</option>
        {(facets?.filters ?? []).map((name) => <option key={name} value={name}>{name}</option>)}
        {filters.filter && !(facets?.filters ?? []).includes(filters.filter) && (
          <option value={filters.filter}>{filters.filter}</option>
        )}
      </PillSelect>
      <PillSelect
        label="Exposure"
        active={filters.exposure !== undefined}
        value={filters.exposure === undefined ? '' : String(filters.exposure)}
        onChange={(event) => onChange({ exposure: event.target.value === '' ? undefined : Number(event.target.value) })}
      >
        <option value="">All</option>
        {(facets?.exposures ?? []).map((seconds) => (
          <option key={seconds} value={String(seconds)}>{exposureLabel(seconds)}</option>
        ))}
        {filters.exposure !== undefined && !(facets?.exposures ?? []).includes(filters.exposure) && (
          <option value={String(filters.exposure)}>{exposureLabel(filters.exposure)}</option>
        )}
      </PillSelect>
      <ToolbarPopover
        id="calibration-temperature"
        label="Temp"
        value={rangeLabel(celsius(filters.tempMin), celsius(filters.tempMax))}
        active={filters.tempMin !== undefined || filters.tempMax !== undefined}
        title="Keep only frames taken between these sensor temperatures"
      >
        <div className="toolbar-range-panel">
          <NumberField label="From °C" value={filters.tempMin} onCommit={(tempMin) => onChange({ tempMin })} />
          <NumberField label="To °C" value={filters.tempMax} onCommit={(tempMax) => onChange({ tempMax })} />
        </div>
      </ToolbarPopover>
      <ToolbarPopover
        id="calibration-nights"
        label="Nights"
        value={rangeLabel(filters.from, filters.to)}
        active={filters.from !== undefined || filters.to !== undefined}
        title="Keep only frames taken between these nights"
      >
        <div className="toolbar-range-panel">
          <label className="toolbar-field">
            <span className="toolbar-caption">From night</span>
            <input
              type="date"
              value={filters.from ?? ''}
              min={facets?.first_night ?? undefined}
              max={facets?.last_night ?? undefined}
              onChange={(event) => onChange({ from: event.target.value || undefined })}
            />
          </label>
          <label className="toolbar-field">
            <span className="toolbar-caption">To night</span>
            <input
              type="date"
              value={filters.to ?? ''}
              min={facets?.first_night ?? undefined}
              max={facets?.last_night ?? undefined}
              onChange={(event) => onChange({ to: event.target.value || undefined })}
            />
          </label>
        </div>
      </ToolbarPopover>
      <PillSelect
        label="Grade"
        active={filters.grade !== undefined}
        value={filters.grade ?? ''}
        onChange={(event) => onChange({ grade: (event.target.value || undefined) as CalibrationFilterState['grade'] })}
      >
        <option value="">All</option>
        <option value="poor">Poor</option>
        <option value="suspect">Suspect</option>
        <option value="ok">OK</option>
        <option value="ungraded">Not graded</option>
      </PillSelect>
      <PillSelect
        label="Use"
        active={filters.keptOut !== undefined}
        value={filters.keptOut === undefined ? '' : String(filters.keptOut)}
        onChange={(event) => onChange({ keptOut: event.target.value === '' ? undefined : event.target.value === 'true' })}
      >
        <option value="">All</option>
        <option value="true">Kept out</option>
        <option value="false">In use</option>
      </PillSelect>
      <label className="toolbar-pill toolbar-pill-check">
        <input
          type="checkbox"
          checked={filters.missing ?? false}
          onChange={(event) => onChange({ missing: event.target.checked || undefined })}
        />
        Missing files
      </label>
      <label className="toolbar-pill toolbar-pill-check" title="Exposures the library records more than once">
        <input
          type="checkbox"
          checked={filters.duplicates ?? false}
          onChange={(event) => onChange({ duplicates: event.target.checked || undefined })}
        />
        Duplicates
      </label>
    </>
  );
  return (
    <ViewToolbar
      label="Calibration filters"
      className="calibration-filters"
      filters={fields}
      activeFilters={active}
      onClearFilters={onClear}
    />
  );
}
