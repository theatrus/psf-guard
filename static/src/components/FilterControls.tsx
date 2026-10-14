import { useMemo } from 'react';
import {
  parseStatusFilter,
  STATUS_FILTER_OPTIONS,
  statusFilterOf,
  statusWords,
  type StatusFilter,
  type StatusWord,
} from '../utils/statusFilter';
import { flagFilterLabel } from '../utils/flagFilter';
import { CALIBRATION_LEVELS, calibrationFilterLabel } from '../utils/calibrationFilter';
import MultiSelectMenu from './MultiSelectMenu';
import ToolbarPopover from './toolbar/ToolbarPopover';
import type { ImageFilterState } from '../utils/imageFilterCount';

export interface FilterOptions {
  status: StatusFilter;
  /** Filters to keep; none keeps every filter. */
  filterNames: string[];
  dateRange: {
    start: Date | null;
    end: Date | null;
  };
  searchTerm: string;
  /** Quality issue categories to keep, as the API spells them; none keeps
   *  every image. */
  flags: string[];
  /** Calibration levels or finding codes to keep; none keeps every image. */
  calibration: string[];
}

interface FilterControlsProps {
  onFilterChange: (filters: FilterOptions) => void;
  availableFilters: string[];
  /** Quality flags any loaded image carries; the Flag select offers these. */
  availableFlags?: string[];
  currentFilters: ImageFilterState;
}

/** A night as the date field holds it, said short: "Apr 16". */
function shortDate(date: Date): string {
  return new Date(date.getUTCFullYear(), date.getUTCMonth(), date.getUTCDate())
    .toLocaleDateString(undefined, { month: 'short', day: 'numeric' });
}

function dateRangeLabel(start: Date | null, end: Date | null): string {
  if (start && end) return `${shortDate(start)} – ${shortDate(end)}`;
  if (start) return `From ${shortDate(start)}`;
  if (end) return `To ${shortDate(end)}`;
  return 'Any';
}

/**
 * The Images filters, as fields for the shared toolbar: Status, Filter,
 * Flag and Calibration keep any of several values; Dates and Search
 * narrow further.
 */
export default function FilterControls({
  onFilterChange,
  availableFilters,
  availableFlags = [],
  currentFilters,
}: FilterControlsProps) {
  // Convert URL state (strings) to component state (Date objects)
  const filters = useMemo(() => ({
    status: parseStatusFilter(currentFilters.status),
    filterNames: currentFilters.filterNames,
    dateRange: {
      start: currentFilters.dateRange.start ? new Date(currentFilters.dateRange.start) : null,
      end: currentFilters.dateRange.end ? new Date(currentFilters.dateRange.end) : null,
    },
    searchTerm: currentFilters.searchTerm,
    flags: currentFilters.flags ?? [],
    calibration: currentFilters.calibration ?? [],
  }), [currentFilters]);

  const change = (changes: Partial<FilterOptions>) => onFilterChange({ ...filters, ...changes });

  const handleDateChange = (field: 'start' | 'end', value: string) => {
    change({ dateRange: { ...filters.dateRange, [field]: value ? new Date(value) : null } });
  };

  // Every grade shown is All; the boxes start ticked, and unticking the
  // last one goes back to All rather than showing nothing.
  const shownStatuses = filters.status === 'all' ? [] : statusWords(filters.status);
  // A chosen filter or flag stays offered even when the current scope has
  // no image carrying it, so the list keeps showing what the URL asked for.
  const filterOptions = [...availableFilters, ...filters.filterNames.filter((name) => !availableFilters.includes(name))]
    .map((name) => ({ value: name, label: name }));
  const flagOptions = [...filters.flags.filter((flag) => !availableFlags.includes(flag)), ...availableFlags]
    .map((flag) => ({ value: flag, label: flagFilterLabel(flag) }));
  // The two levels, then any finding a coverage report link chose.
  const levels: string[] = CALIBRATION_LEVELS.map((level) => level.value);
  const calibrationOptions = [...levels, ...filters.calibration.filter((value) => !levels.includes(value))]
    .map((value) => ({ value, label: calibrationFilterLabel(value) }));
  const { start, end } = filters.dateRange;

  return (
    <>
      <MultiSelectMenu
        id="image-status-filter"
        label="Status"
        options={STATUS_FILTER_OPTIONS}
        chosen={shownStatuses}
        onChange={(words) => change({ status: statusFilterOf(words as StatusWord[]) })}
        everyIsAll
        tickAllWhenEmpty
        title="Show images with these grades"
      />

      <MultiSelectMenu
        id="image-channel-filter"
        label="Filter"
        options={filterOptions}
        chosen={filters.filterNames}
        onChange={(filterNames) => change({ filterNames })}
        everyIsAll
      />

      <MultiSelectMenu
        id="image-flag-filter"
        label="Flag"
        options={flagOptions}
        chosen={filters.flags}
        onChange={(flags) => change({ flags })}
        title="Keep only images whose quality analysis raised any of these flags"
      />

      <MultiSelectMenu
        id="image-calibration-filter"
        label="Calibration"
        options={calibrationOptions}
        chosen={filters.calibration}
        onChange={(calibration) => change({ calibration })}
        title="Keep only lights the calibration library serves poorly (Poor) or that are worth a look (Check)"
      />

      <ToolbarPopover
        id="image-date-filter"
        label="Dates"
        value={dateRangeLabel(start, end)}
        active={Boolean(start || end)}
        title="Keep only images taken between these dates"
      >
        <div className="toolbar-date-range">
          <label className="toolbar-field">
            <span className="toolbar-caption">From</span>
            <input
              id="image-date-start"
              type="date"
              value={start ? start.toISOString().split('T')[0] : ''}
              onChange={(e) => handleDateChange('start', e.target.value)}
            />
          </label>
          <label className="toolbar-field">
            <span className="toolbar-caption">To</span>
            <input
              id="image-date-end"
              type="date"
              value={end ? end.toISOString().split('T')[0] : ''}
              onChange={(e) => handleDateChange('end', e.target.value)}
            />
          </label>
          <button
            type="button"
            className="tool-button tool-button-quiet"
            disabled={!start && !end}
            onClick={() => change({ dateRange: { start: null, end: null } })}
          >
            Any date
          </button>
        </div>
      </ToolbarPopover>

      <div className="toolbar-field">
        <label className="toolbar-caption" htmlFor="image-search-filter">Search</label>
        <input
          id="image-search-filter"
          type="text"
          placeholder="Target name…"
          value={filters.searchTerm}
          onChange={(e) => change({ searchTerm: e.target.value })}
        />
      </div>
    </>
  );
}
