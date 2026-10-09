import { useEffect, useMemo, useState } from 'react';
import {
  parseStatusFilter,
  STATUS_FILTER_OPTIONS,
  statusFilterOf,
  statusWords,
  type StatusFilter,
  type StatusWord,
} from '../utils/statusFilter';
import { flagFilterLabel } from '../utils/flagFilter';
import MultiSelectMenu from './MultiSelectMenu';

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
}

interface FilterControlsProps {
  onFilterChange: (filters: FilterOptions) => void;
  availableFilters: string[];
  /** Quality flags any loaded image carries; the Flag select offers these. */
  availableFlags?: string[];
  currentFilters: {
    status: string;
    filterNames: string[];
    dateRange: {
      start: string | null;
      end: string | null;
    };
    searchTerm: string;
    flags?: string[];
  };
}

export default function FilterControls({
  onFilterChange,
  availableFilters,
  availableFlags = [],
  currentFilters,
}: FilterControlsProps) {
  const [showDateFilters, setShowDateFilters] = useState(
    Boolean(currentFilters.dateRange.start || currentFilters.dateRange.end),
  );
  useEffect(() => {
    if (currentFilters.dateRange.start || currentFilters.dateRange.end) {
      setShowDateFilters(true);
    }
  }, [currentFilters.dateRange.end, currentFilters.dateRange.start]);

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
  }), [currentFilters]);

  const handleStatusChange = (status: StatusFilter) => {
    const newFilters = { ...filters, status };
    onFilterChange(newFilters);
  };
  // Every grade ticked is All; unticking the last one goes back to All too,
  // rather than showing nothing.
  const shownStatuses = statusWords(filters.status);
  const toggleStatus = (word: StatusWord) => {
    const next = shownStatuses.includes(word)
      ? shownStatuses.filter((shown) => shown !== word)
      : [...shownStatuses, word];
    handleStatusChange(statusFilterOf(next));
  };

  const handleFilterNamesChange = (filterNames: string[]) => {
    onFilterChange({ ...filters, filterNames });
  };

  const handleDateChange = (field: 'start' | 'end', value: string) => {
    const date = value ? new Date(value) : null;
    const newFilters = {
      ...filters,
      dateRange: {
        ...filters.dateRange,
        [field]: date,
      },
    };
    onFilterChange(newFilters);
  };

  const handleSearchChange = (searchTerm: string) => {
    const newFilters = { ...filters, searchTerm };
    onFilterChange(newFilters);
  };

  const handleFlagsChange = (flags: string[]) => {
    onFilterChange({ ...filters, flags });
  };

  const resetFilters = () => {
    const defaultFilters: FilterOptions = {
      status: 'all',
      filterNames: [],
      dateRange: {
        start: null,
        end: null,
      },
      searchTerm: '',
      flags: [],
    };
    onFilterChange(defaultFilters);
    setShowDateFilters(false);
  };

  const dateFilterCount = Number(filters.dateRange.start !== null)
    + Number(filters.dateRange.end !== null);
  const hasFilters = filters.status !== 'all'
    || filters.filterNames.length > 0
    || dateFilterCount > 0
    || filters.searchTerm !== ''
    || filters.flags.length > 0;
  // A chosen filter or flag stays offered even when the current scope has
  // no image carrying it, so the list keeps showing what the URL asked for.
  const filterOptions = [...availableFilters, ...filters.filterNames.filter((name) => !availableFilters.includes(name))]
    .map((name) => ({ value: name, label: name }));
  const flagOptions = [...filters.flags.filter((flag) => !availableFlags.includes(flag)), ...availableFlags]
    .map((flag) => ({ value: flag, label: flagFilterLabel(flag) }));

  return (
    <div className="filter-controls compact">
      <div className="filter-row compact filter-primary-row">
        <div className="filter-input-group status-filter" role="group" aria-labelledby="image-status-filter">
          <span id="image-status-filter">Status:</span>
          {STATUS_FILTER_OPTIONS.map((option) => (
            <label key={option.value} className="status-choice">
              <input
                type="checkbox"
                checked={shownStatuses.includes(option.value)}
                onChange={() => toggleStatus(option.value)}
              />
              {option.label}
            </label>
          ))}
        </div>

        <MultiSelectMenu
          id="image-channel-filter"
          label="Filter:"
          options={filterOptions}
          chosen={filters.filterNames}
          onChange={handleFilterNamesChange}
          everyIsAll
        />

        <MultiSelectMenu
          id="image-flag-filter"
          label="Flag:"
          options={flagOptions}
          chosen={filters.flags}
          onChange={handleFlagsChange}
          title="Keep only images whose quality analysis raised any of these flags"
        />

        <div className="filter-input-group search-filter">
          <label htmlFor="image-search-filter">Search:</label>
          <input
            id="image-search-filter"
            type="text" 
            placeholder="Target name..."
            value={filters.searchTerm}
            onChange={(e) => handleSearchChange(e.target.value)}
          />
        </div>

        <button
          type="button"
          className={`reset-button compact more-filters-button${dateFilterCount > 0 ? ' active' : ''}`}
          aria-expanded={showDateFilters}
          aria-controls="image-date-filters"
          onClick={() => setShowDateFilters(open => !open)}
        >
          Dates{dateFilterCount > 0 ? ` (${dateFilterCount})` : ''}
        </button>

        <button
          type="button"
          className="reset-button compact"
          onClick={resetFilters}
          disabled={!hasFilters}
        >
          Reset
        </button>
      </div>

      {showDateFilters && (
        <div id="image-date-filters" className="filter-row compact filter-secondary-row">
          <div className="filter-input-group date-range">
            <label htmlFor="image-date-start">Date range:</label>
            <input
              id="image-date-start"
              type="date"
              className="compact-date"
              value={filters.dateRange.start ? filters.dateRange.start.toISOString().split('T')[0] : ''}
              onChange={(e) => handleDateChange('start', e.target.value)}
            />
            <span className="date-separator">to</span>
            <input
              id="image-date-end"
              aria-label="End date"
              type="date"
              className="compact-date"
              value={filters.dateRange.end ? filters.dateRange.end.toISOString().split('T')[0] : ''}
              onChange={(e) => handleDateChange('end', e.target.value)}
            />
          </div>
        </div>
      )}
    </div>
  );
}
