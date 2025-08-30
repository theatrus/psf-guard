import { useState } from 'react';
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
}

export default function FilterControls({ onFilterChange, availableFilters }: FilterControlsProps) {
  const [filters, setFilters] = useState<FilterOptions>({
    status: 'all',
    filterName: 'all',
    dateRange: {
      start: null,
      end: null,
    },
    searchTerm: '',
  });

  const handleStatusChange = (status: GradingStatus | 'all') => {
    const newFilters = { ...filters, status };
    setFilters(newFilters);
    onFilterChange(newFilters);
  };

  const handleFilterNameChange = (filterName: string) => {
    const newFilters = { ...filters, filterName };
    setFilters(newFilters);
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
    setFilters(newFilters);
    onFilterChange(newFilters);
  };

  const handleSearchChange = (searchTerm: string) => {
    const newFilters = { ...filters, searchTerm };
    setFilters(newFilters);
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
    setFilters(defaultFilters);
    onFilterChange(defaultFilters);
  };

  return (
    <div className="filter-controls">
      <div className="filter-row">
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
            <option value="all">All Filters</option>
            {availableFilters.map(filter => (
              <option key={filter} value={filter}>{filter}</option>
            ))}
          </select>
        </div>

        <div className="filter-input-group">
          <label>Start Date:</label>
          <input 
            type="date" 
            value={filters.dateRange.start ? filters.dateRange.start.toISOString().split('T')[0] : ''}
            onChange={(e) => handleDateChange('start', e.target.value)}
          />
        </div>

        <div className="filter-input-group">
          <label>End Date:</label>
          <input 
            type="date" 
            value={filters.dateRange.end ? filters.dateRange.end.toISOString().split('T')[0] : ''}
            onChange={(e) => handleDateChange('end', e.target.value)}
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

        <button className="reset-button" onClick={resetFilters}>
          Reset Filters
        </button>
      </div>
    </div>
  );
}