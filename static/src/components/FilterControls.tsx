import { useMemo } from 'react';
import { GradingStatus } from '../api/types';

export interface FilterOptions {
  status: GradingStatus | 'all';
  filterName: string | 'all';
  dateRange: {
    start: Date | null;
    end: Date | null;
  };
  searchTerm: string;
}

interface FilterControlsProps {
  onFilterChange: (filters: FilterOptions) => void;
  availableFilters: string[];
  currentFilters: {
    status: string;
    filterName: string;
    dateRange: {
      start: string | null;
      end: string | null;
    };
    searchTerm: string;
  };
}

export default function FilterControls({ onFilterChange, availableFilters, currentFilters }: FilterControlsProps) {
  // Convert URL state (strings) to component state (Date objects)
  const filters = useMemo(() => ({
    status: currentFilters.status as GradingStatus | 'all',
    filterName: currentFilters.filterName,
    dateRange: {
      start: currentFilters.dateRange.start ? new Date(currentFilters.dateRange.start) : null,
      end: currentFilters.dateRange.end ? new Date(currentFilters.dateRange.end) : null,
    },
    searchTerm: currentFilters.searchTerm,
  }), [currentFilters]);

  const handleStatusChange = (status: GradingStatus | 'all') => {
    const newFilters = { ...filters, status };
    onFilterChange(newFilters);
  };

  const handleFilterNameChange = (filterName: string) => {
    const newFilters = { ...filters, filterName };
    onFilterChange(newFilters);
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

  const resetFilters = () => {
    const defaultFilters: FilterOptions = {
      status: 'all',
      filterName: 'all',
      dateRange: {
        start: null,
        end: null,
      },
      searchTerm: '',
    };
    onFilterChange(defaultFilters);
  };

  return (
    <div className="filter-controls compact">
      <div className="filter-row compact">
        <div className="filter-input-group">
          <label>Status:</label>
          <select 
            value={filters.status} 
            onChange={(e) => handleStatusChange(e.target.value as GradingStatus | 'all')}
          >
            <option value="all">All</option>
            <option value={GradingStatus.Accepted}>Accepted</option>
            <option value={GradingStatus.Rejected}>Rejected</option>
            <option value={GradingStatus.Pending}>Pending</option>
          </select>
        </div>

        <div className="filter-input-group">
          <label>Filter:</label>
          <select 
            value={filters.filterName} 
            onChange={(e) => handleFilterNameChange(e.target.value)}
          >
            <option value="all">All</option>
            {availableFilters.map(filter => (
              <option key={filter} value={filter}>{filter}</option>
            ))}
          </select>
        </div>

        <div className="filter-input-group date-range">
          <label>Date Range:</label>
          <input 
            type="date" 
            className="compact-date"
            value={filters.dateRange.start ? filters.dateRange.start.toISOString().split('T')[0] : ''}
            onChange={(e) => handleDateChange('start', e.target.value)}
            placeholder="Start"
          />
          <span className="date-separator">to</span>
          <input 
            type="date" 
            className="compact-date"
            value={filters.dateRange.end ? filters.dateRange.end.toISOString().split('T')[0] : ''}
            onChange={(e) => handleDateChange('end', e.target.value)}
            placeholder="End"
          />
        </div>

        <div className="filter-input-group">
          <label>Search:</label>
          <input 
            type="text" 
            placeholder="Target name..."
            value={filters.searchTerm}
            onChange={(e) => handleSearchChange(e.target.value)}
          />
        </div>

        <button className="reset-button compact" onClick={resetFilters}>
          Reset
        </button>
      </div>
    </div>
  );
}