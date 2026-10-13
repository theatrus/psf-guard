import { useEffect, useState } from 'react';
import type { CalibrationFacets, CalibrationRigSummary } from '../../api/types';
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
    <label>
      {label}
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

/** What the session list and the strip keep. */
export default function CalibrationFilters({ filters, facets, rigs, onChange, onClear }: CalibrationFiltersProps) {
  // Open on a wide screen; folded on a phone, where they would fill it.
  const [open, setOpen] = useState(() => !window.matchMedia?.('(max-width: 700px)').matches);
  const active = Object.entries(filters)
    .filter(([key, value]) => key !== 'sort' && value !== undefined && !(key === 'rig' && value === ALL_RIGS))
    .length;
  const fields = (
    <div className="calibration-filter-fields">
      {rigs.length > 1 && (
        <label>
          Rig
          <select value={filters.rig ?? ALL_RIGS} onChange={(event) => onChange({ rig: event.target.value })}>
            <option value={ALL_RIGS}>All</option>
            {rigs.map((rig) => <option key={rig.rig_uuid} value={rig.rig_uuid}>{rig.name}</option>)}
          </select>
        </label>
      )}
      <label>
        Type
        <select
          value={filters.kind ?? ''}
          onChange={(event) => onChange({ kind: (event.target.value || undefined) as CalibrationFilterState['kind'] })}
        >
          <option value="">All</option>
          {(['flat', 'dark_flat', 'dark', 'bias'] as const).map((kind) => (
            <option key={kind} value={kind}>{KIND_LABELS[kind]}</option>
          ))}
        </select>
      </label>
      <label>
        Filter
        <select value={filters.filter ?? ''} onChange={(event) => onChange({ filter: event.target.value || undefined })}>
          <option value="">All</option>
          {(facets?.filters ?? []).map((name) => <option key={name} value={name}>{name}</option>)}
          {filters.filter && !(facets?.filters ?? []).includes(filters.filter) && (
            <option value={filters.filter}>{filters.filter}</option>
          )}
        </select>
      </label>
      <label>
        Exposure
        <select
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
        </select>
      </label>
      <NumberField label="From °C" value={filters.tempMin} onCommit={(tempMin) => onChange({ tempMin })} />
      <NumberField label="To °C" value={filters.tempMax} onCommit={(tempMax) => onChange({ tempMax })} />
      <label>
        From night
        <input
          type="date"
          value={filters.from ?? ''}
          min={facets?.first_night ?? undefined}
          max={facets?.last_night ?? undefined}
          onChange={(event) => onChange({ from: event.target.value || undefined })}
        />
      </label>
      <label>
        To night
        <input
          type="date"
          value={filters.to ?? ''}
          min={facets?.first_night ?? undefined}
          max={facets?.last_night ?? undefined}
          onChange={(event) => onChange({ to: event.target.value || undefined })}
        />
      </label>
      <label>
        Grade
        <select
          value={filters.grade ?? ''}
          onChange={(event) => onChange({ grade: (event.target.value || undefined) as CalibrationFilterState['grade'] })}
        >
          <option value="">All</option>
          <option value="poor">Poor</option>
          <option value="suspect">Suspect</option>
          <option value="ok">OK</option>
          <option value="ungraded">Not graded</option>
        </select>
      </label>
      <label>
        Use
        <select
          value={filters.keptOut === undefined ? '' : String(filters.keptOut)}
          onChange={(event) => onChange({ keptOut: event.target.value === '' ? undefined : event.target.value === 'true' })}
        >
          <option value="">All</option>
          <option value="true">Kept out</option>
          <option value="false">In use</option>
        </select>
      </label>
      <label className="calibration-check">
        <input
          type="checkbox"
          checked={filters.missing ?? false}
          onChange={(event) => onChange({ missing: event.target.checked || undefined })}
        />
        Missing files
      </label>
      {active > 0 && (
        <button type="button" className="calibration-button" onClick={onClear}>
          Clear
        </button>
      )}
    </div>
  );
  return (
    <details
      className="calibration-filters"
      open={open}
      onToggle={(event) => setOpen((event.currentTarget as HTMLDetailsElement).open)}
    >
      <summary>Filters{active > 0 ? ` (${active})` : ''}</summary>
      {fields}
    </details>
  );
}
