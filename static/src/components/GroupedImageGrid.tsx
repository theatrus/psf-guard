import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import type { Image } from '../api/types';
import { GradingStatus } from '../api/types';
import { useGrading } from '../hooks/useGrading';
import { useProjectTarget, useGridState, useFilters, useUrlParams } from '../hooks/useUrlState';
import ImageCard from './ImageCard';
import LazyImageCard from './LazyImageCard';
import FilterControls, { type FilterOptions } from './FilterControls';
import StatsDashboard from './StatsDashboard';
import UndoRedoToolbar from './UndoRedoToolbar';
import { 
  type GroupingMode, 
  SINGLE_PROJECT_MODES,
  MULTI_PROJECT_MODES,
  DEFAULT_SINGLE_PROJECT_MODE,
  DEFAULT_MULTI_PROJECT_MODE,
  GROUPING_MODE_LABELS,
  getNextSingleProjectMode,
  getNextMultiProjectMode 
} from '../types/grouping';

interface GroupedImageGridProps {
  useLazyImages?: boolean;
}

export default function GroupedImageGrid({ useLazyImages = false }: GroupedImageGridProps) {
  // Get state from URL hooks
  const navigate = useNavigate();
  const { projectId, targetId } = useProjectTarget();
  const { updateParams } = useUrlParams();
  const {
    selectedGroupIndex,
    selectedImageIndex,
    groupingMode,
    imageSize,
    showStats,
    expandedGroups,
    selectedImages,
    setSelectedGroupIndex,
    setSelectedImageIndex,
    setGroupingMode,
    setImageSize,
    setExpandedGroups,
    setSelectedImages,
  } = useGridState();
  const { filters, updateFilters } = useFilters();
  
  // Adapter function to convert between FilterOptions and URL state
  const handleFilterChange = useCallback((filterOptions: FilterOptions) => {
    updateFilters({
      status: filterOptions.status === 'all' ? 'all' : String(filterOptions.status),
      filterName: filterOptions.filterName,
      dateStart: filterOptions.dateRange.start?.toISOString().split('T')[0] || '',
      dateEnd: filterOptions.dateRange.end?.toISOString().split('T')[0] || '',
      searchTerm: filterOptions.searchTerm,
    });
  }, [updateFilters]);

  // Initialize grading system with undo/redo
  const grading = useGrading();
  const [lastSelectedImageId, setLastSelectedImageId] = useState<number | null>(null);

  // Navigation helpers
  const [searchParams] = useSearchParams();
  
  const navigateToDetail = useCallback((imageId: number) => {
    const params = searchParams.toString();
    navigate(`/detail/${imageId}?${params}`);
  }, [navigate, searchParams]);
  
  const navigateToComparison = useCallback((leftId: number, rightId: number) => {
    const params = searchParams.toString();
    navigate(`/compare/${leftId}/${rightId}?${params}`);
  }, [navigate, searchParams]);
  
  // Legacy setters for compatibility
  const setSelectedImageId = (id: number) => {
    setLastSelectedImageId(id);
  };

  // Fetch ALL images (no pagination for grouping) with periodic refresh
  const { data: allImages = [], isLoading } = useQuery({
    queryKey: ['all-images', projectId, targetId],
    queryFn: () => apiClient.getImages({
      project_id: projectId || undefined, // null becomes undefined for API
      target_id: targetId || undefined,
      limit: 10000, // Get all images
    }),
    enabled: projectId !== undefined, // Enable for both specific projects and null (all projects)
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });

  // Filter images based on current filters
  const filteredImages = useMemo(() => {
    return allImages.filter(image => {
      // Status filter
      if (filters.status !== 'all') {
        const statusMap: { [key: string]: number } = { 'pending': 0, 'accepted': 1, 'rejected': 2 };
        if (statusMap[filters.status] !== image.grading_status) {
          return false;
        }
      }
      
      // Filter name filter
      if (filters.filterName !== 'all' && image.filter_name !== filters.filterName) {
        return false;
      }
      
      // Date range filter
      if (filters.dateRange.start && image.acquired_date) {
        const imageDate = new Date(image.acquired_date * 1000);
        const startDate = new Date(filters.dateRange.start);
        if (imageDate < startDate) return false;
      }
      if (filters.dateRange.end && image.acquired_date) {
        const imageDate = new Date(image.acquired_date * 1000);
        const endDate = new Date(filters.dateRange.end);
        if (imageDate > endDate) return false;
      }
      
      // Search filter
      if (filters.searchTerm) {
        const searchLower = filters.searchTerm.toLowerCase();
        if (!image.target_name.toLowerCase().includes(searchLower)) {
          return false;
        }
      }
      
      return true;
    });
  }, [allImages, filters]);
  
  // Get available filter names from all images (not just filtered)
  const availableFilters = useMemo(() => {
    const filterSet = new Set<string>();
    allImages.forEach(img => {
      if (img.filter_name) filterSet.add(img.filter_name);
    });
    return Array.from(filterSet).sort();
  }, [allImages]);

  // Determine if we're in multi-project mode
  const isMultiProjectMode = projectId === null;
  
  // Group images based on selected mode
  const imageGroups = useMemo(() => {
    const groups = new Map<string, Image[]>();
    
    filteredImages.forEach(image => {
      let groupKey: string;
      
      // Helper functions for building group keys
      const getProjectPart = () => image.project_display_name || 'Unknown Project';
      const getFilterPart = () => image.filter_name || 'No Filter';
      const getDatePart = () => {
        if (image.acquired_date) {
          const date = new Date(image.acquired_date * 1000);
          return date.toISOString().split('T')[0];
        }
        return 'Unknown Date';
      };
      
      // Build group key based on mode
      switch (groupingMode) {
        case 'filter':
          groupKey = getFilterPart();
          break;
        case 'date':
          groupKey = getDatePart();
          break;
        case 'both':
          groupKey = `${getFilterPart()} - ${getDatePart()}`;
          break;
        case 'project':
          groupKey = getProjectPart();
          break;
        case 'project+filter':
          groupKey = `${getProjectPart()} - ${getFilterPart()}`;
          break;
        case 'project+date':
          groupKey = `${getProjectPart()} - ${getDatePart()}`;
          break;
        case 'project+date+filter':
          groupKey = `${getProjectPart()} - ${getDatePart()} - ${getFilterPart()}`;
          break;
        default:
          groupKey = getFilterPart();
      }
      
      if (!groups.has(groupKey)) {
        groups.set(groupKey, []);
      }
      groups.get(groupKey)!.push(image);
    });

    // Convert to array and sort
    const sorted = Array.from(groups.entries())
      .map(([groupName, images]) => ({ 
        filterName: groupName, // Keep property name for compatibility
        images: images.sort((a, b) => {
          // Within each group, sort by acquired date (oldest first - chronological order)
          const dateA = a.acquired_date || 0;
          const dateB = b.acquired_date || 0;
          return dateA - dateB; // Oldest first
        })
      }));
    
    // Sort groups based on mode
    if (groupingMode === 'date' || groupingMode.includes('date')) {
      // Sort by group name descending for date-based grouping (newest first)
      sorted.sort((a, b) => b.filterName.localeCompare(a.filterName));
    } else {
      // Sort alphabetically for other modes
      sorted.sort((a, b) => a.filterName.localeCompare(b.filterName));
    }
    
    return sorted;
  }, [filteredImages, groupingMode]);

  // Initialize expanded groups ONLY on very first load (no URL params, never initialized before)
  const hasInitialized = useRef(false);
  const initialLoad = useRef(true);
  useEffect(() => {
    // Only auto-expand on the very first page load when there's no URL state and no previous user action
    if (!hasInitialized.current && initialLoad.current && expandedGroups.size === 0 && imageGroups.length > 0) {
      setExpandedGroups(new Set(imageGroups.map(g => g.filterName)));
      hasInitialized.current = true;
    }
    initialLoad.current = false;
  }, [imageGroups.length, expandedGroups.size, setExpandedGroups]);

  // Reset expanded groups only when grouping mode actually changes
  const prevGroupingMode = useRef(groupingMode);
  useEffect(() => {
    if (prevGroupingMode.current !== groupingMode) {
      // Expand all groups when grouping mode changes
      if (imageGroups.length > 0) {
        setExpandedGroups(new Set(imageGroups.map(g => g.filterName)));
      }
      prevGroupingMode.current = groupingMode;
    }
  }, [groupingMode, imageGroups.length, setExpandedGroups]); // Depend on length, not content

  // Flatten images for navigation
  const flatImages = useMemo(() => {
    const result: { image: Image; groupIndex: number; indexInGroup: number }[] = [];
    imageGroups.forEach((group, groupIndex) => {
      // Only include images from expanded groups
      if (expandedGroups.has(group.filterName)) {
        group.images.forEach((image, indexInGroup) => {
          result.push({ image, groupIndex, indexInGroup });
        });
      }
    });
    return result;
  }, [imageGroups, expandedGroups]);

  // Compute current selected image ID
  const selectedImageId = useMemo(() => {
    const currentFlat = flatImages.find(
      item => item.groupIndex === selectedGroupIndex && item.indexInGroup === selectedImageIndex
    );
    return currentFlat?.image.id || null;
  }, [selectedGroupIndex, selectedImageIndex, flatImages]);

  // Update lastSelectedImageId when selectedImageId changes
  useEffect(() => {
    if (selectedImageId) {
      setLastSelectedImageId(selectedImageId);
    }
  }, [selectedImageId]);

  // Initialize lastSelectedImageId from URL state on mount (for returning from detail/comparison views)
  useEffect(() => {
    if (!lastSelectedImageId) {
      // Try to get from selectedImageId (URL groupIndex/imageIndex)
      if (selectedImageId) {
        setLastSelectedImageId(selectedImageId);
      }
      // Fallback to single selected image
      else if (selectedImages.size === 1) {
        const singleSelectedId = Array.from(selectedImages)[0];
        setLastSelectedImageId(singleSelectedId);
      }
    }
  }, [selectedImageId, selectedImages, lastSelectedImageId]);

  // Grading is now handled by the useGrading hook

  const navigateImages = useCallback((direction: 'next' | 'prev') => {
    const currentIndex = flatImages.findIndex(
      item => item.groupIndex === selectedGroupIndex && item.indexInGroup === selectedImageIndex
    );

    if (currentIndex === -1) return;

    const newIndex = direction === 'next' 
      ? Math.min(currentIndex + 1, flatImages.length - 1)
      : Math.max(currentIndex - 1, 0);

    const newItem = flatImages[newIndex];
    setSelectedGroupIndex(newItem.groupIndex);
    setSelectedImageIndex(newItem.indexInGroup);
    setSelectedImageId(newItem.image.id); // Set immediately, don't wait for useEffect
  }, [flatImages, selectedGroupIndex, selectedImageIndex]);

  const gradeImage = useCallback(async (status: 'accepted' | 'rejected' | 'pending') => {
    if (!selectedImageId) return;

    try {
      await grading.gradeImage(selectedImageId, status);
      // Auto-advance to next image
      setTimeout(() => navigateImages('next'), 100);
    } catch (error) {
      console.error('Failed to grade image:', error);
    }
  }, [selectedImageId, grading, navigateImages]);

  const toggleGroup = useCallback((filterName: string) => {
    setExpandedGroups((prev: Set<string>) => {
      const next = new Set(prev);
      if (next.has(filterName)) {
        next.delete(filterName);
      } else {
        next.add(filterName);
      }
      return next;
    });
  }, [setExpandedGroups]);

  // Handle image selection with shift+click support
  const handleImageSelection = useCallback((imageId: number, groupIndex: number, indexInGroup: number, event: React.MouseEvent) => {
    if (event.shiftKey && lastSelectedImageId) {
      // Shift+click: select range
      const startIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
      const endIndex = flatImages.findIndex(item => item.image.id === imageId);
      
      if (startIndex !== -1 && endIndex !== -1) {
        const [minIndex, maxIndex] = [Math.min(startIndex, endIndex), Math.max(startIndex, endIndex)];
        const newSelections = new Set<number>();
        
        for (let i = minIndex; i <= maxIndex; i++) {
          newSelections.add(flatImages[i].image.id);
        }
        
        setSelectedImages((prev: Set<number>) => {
          const next = new Set(prev);
          newSelections.forEach(id => next.add(id));
          return next;
        });
      }
    } else if (event.ctrlKey || event.metaKey) {
      // Ctrl+click: toggle selection
      setSelectedImages((prev: Set<number>) => {
        const next = new Set(prev);
        if (next.has(imageId)) {
          next.delete(imageId);
        } else {
          next.add(imageId);
        }
        return next;
      });
      setLastSelectedImageId(imageId);
    } else {
      // Regular click: single selection for navigation
      // Batch URL updates to prevent race conditions
      updateParams({
        groupIndex: groupIndex,
        imageIndex: indexInGroup,
        selected: [imageId]
      });
      
      setSelectedImageId(imageId);
      setLastSelectedImageId(imageId);
    }
  }, [flatImages, lastSelectedImageId, updateParams]);

  // Batch grading functions
  const gradeBatch = useCallback(async (status: 'accepted' | 'rejected' | 'pending') => {
    if (selectedImages.size === 0) return;

    try {
      await grading.gradeBatch(Array.from(selectedImages), status);
      
      // Clear selection after batch operation
      setSelectedImages(new Set());
      setLastSelectedImageId(null);
    } catch (error) {
      console.error('Batch grading failed:', error);
    }
  }, [selectedImages, grading]);

  // Switch to appropriate grouping mode when changing between single/multi-project
  const prevProjectId = useRef(projectId);
  useEffect(() => {
    if (prevProjectId.current !== projectId) {
      const wasMultiProject = prevProjectId.current === null;
      const isNowMultiProject = projectId === null;
      
      if (wasMultiProject !== isNowMultiProject) {
        // Switching between single and multi-project modes
        if (isNowMultiProject) {
          // Switched to multi-project, set appropriate default grouping
          setGroupingMode(DEFAULT_MULTI_PROJECT_MODE);
        } else {
          // Switched to single project, set appropriate default grouping
          setGroupingMode(DEFAULT_SINGLE_PROJECT_MODE);
        }
      }
      prevProjectId.current = projectId;
    }
  }, [projectId, setGroupingMode]);
  
  // Clear date filters when project/target changes to avoid "no results" scenarios
  useEffect(() => {
    // Only clear dates if they exist, to avoid infinite loops
    if (filters.dateRange.start || filters.dateRange.end) {
      updateFilters({
        dateStart: '',
        dateEnd: '',
      });
    }
  }, [projectId, targetId]); // Only trigger when project/target changes

  // Smart date suggestion: Show info if no images are visible due to date filtering
  const hasDateFilters = filters.dateRange.start || filters.dateRange.end;
  const shouldShowDateHint = hasDateFilters && filteredImages.length === 0 && allImages.length > 0;

  // Clear selection when actual filter values change (not when other URL params change)
  useEffect(() => {
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
  }, [filters.status, filters.filterName, filters.dateRange.start, filters.dateRange.end, filters.searchTerm]);

  // Keyboard shortcuts
  useHotkeys('k', () => navigateImages('next'), [navigateImages]);
  useHotkeys('j', () => navigateImages('prev'), [navigateImages]);
  useHotkeys('a', () => {
    if (selectedImages.size > 1) {
      gradeBatch('accepted');
    } else {
      gradeImage('accepted');
    }
  }, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('x', () => {
    if (selectedImages.size > 1) {
      gradeBatch('rejected');
    } else {
      gradeImage('rejected');
    }
  }, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('u', () => {
    if (selectedImages.size > 1) {
      gradeBatch('pending');
    } else {
      gradeImage('pending');
    }
  }, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('enter', () => {
    if (lastSelectedImageId) {
      navigateToDetail(lastSelectedImageId);
    }
  }, [lastSelectedImageId, navigateToDetail]);
  useHotkeys('escape', () => {
    // Just clear selection on escape (we're already in grid view)
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
  }, [setSelectedImages]);
  
  // Add comparison keyboard shortcut
  useHotkeys('c', () => {
    // Only allow comparison from grid view
    if (selectedImages.size === 2) {
      // Use the two selected images for comparison
      const selectedArray = Array.from(selectedImages);
      // Prevent comparing same image with itself
      if (selectedArray[0] !== selectedArray[1]) {
        navigateToComparison(selectedArray[0], selectedArray[1]);
      }
    } else if (lastSelectedImageId) {
      // Use current image + next different image for comparison
      const currentIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
      if (currentIndex !== -1) {
        // Find the next image that is different from the current one
        for (let i = currentIndex + 1; i < flatImages.length; i++) {
          const nextImageId = flatImages[i].image.id;
          if (lastSelectedImageId !== nextImageId) {
            navigateToComparison(lastSelectedImageId, nextImageId);
            break;
          }
        }
      }
    }
  }, [selectedImages, lastSelectedImageId, flatImages, navigateToComparison]);
  
  // Grouping mode shortcuts
  useHotkeys('g', () => {
    // Cycle through grouping modes based on mode (single project vs multi-project)
    if (isMultiProjectMode) {
      setGroupingMode(getNextMultiProjectMode(groupingMode));
    } else {
      setGroupingMode(getNextSingleProjectMode(groupingMode));
    }
  }, [groupingMode, isMultiProjectMode, setGroupingMode]);

  if (isLoading) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      <div className="grouped-image-container">
        <div className="image-controls sticky">
          {/* Combined Controls Row */}
          <div className="controls-row-combined">
            <FilterControls 
              onFilterChange={handleFilterChange}
              availableFilters={availableFilters}
              currentFilters={filters}
            />
            
            <div className="controls-section">
              <div className="size-control compact">
                <label>Size:</label>
                <input
                  type="range"
                  min="150"
                  max="1200"
                  step="50"
                  value={imageSize}
                  onChange={(e) => setImageSize(Number(e.target.value))}
                />
                <span className="size-value">{imageSize}px</span>
              </div>
              
              <div className="grouping-control compact">
                <label>Group:</label>
                <select 
                  value={groupingMode} 
                  onChange={(e) => setGroupingMode(e.target.value as GroupingMode)}
                >
                  {isMultiProjectMode 
                    ? MULTI_PROJECT_MODES.map(mode => (
                        <option key={mode} value={mode}>
                          {GROUPING_MODE_LABELS[mode]}
                        </option>
                      ))
                    : SINGLE_PROJECT_MODES.map(mode => (
                        <option key={mode} value={mode}>
                          {GROUPING_MODE_LABELS[mode]}
                        </option>
                      ))
                  }
                </select>
              </div>
            </div>
            
            {/* Undo/Redo Toolbar */}
            <div className="toolbar-section">
            <UndoRedoToolbar
              canUndo={grading.canUndo}
              canRedo={grading.canRedo}
              isProcessing={grading.isLoading}
              undoStackSize={grading.undoStackSize}
              redoStackSize={grading.redoStackSize}
              onUndo={grading.undo}
              onRedo={grading.redo}
              getLastAction={grading.getLastAction}
              getNextRedoAction={grading.getNextRedoAction}
              className="compact"
            />
              {(lastSelectedImageId || selectedImages.size === 2) && (
                <button
                  className="toolbar-button compare-button compact"
                onClick={() => {
                  if (selectedImages.size === 2) {
                    // Use the two selected images for comparison
                    const selectedArray = Array.from(selectedImages);
                    // Prevent comparing same image with itself
                    if (selectedArray[0] !== selectedArray[1]) {
                      navigateToComparison(selectedArray[0], selectedArray[1]);
                    }
                  } else if (lastSelectedImageId) {
                    // Use current image + next different image for comparison
                    const currentIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
                    if (currentIndex !== -1) {
                      // Find the next image that is different from the current one
                      for (let i = currentIndex + 1; i < flatImages.length; i++) {
                        const nextImageId = flatImages[i].image.id;
                        if (lastSelectedImageId !== nextImageId) {
                          navigateToComparison(lastSelectedImageId, nextImageId);
                          break;
                        }
                      }
                    }
                  }
                }}
                disabled={(() => {
                  if (selectedImages.size === 2) {
                    const selectedArray = Array.from(selectedImages);
                    return selectedArray[0] === selectedArray[1]; // Same image selected twice
                  }
                  if (lastSelectedImageId) {
                    const currentIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
                    if (currentIndex === -1) return true; // Image not found
                    
                    // Check if there's any different image after current position
                    for (let i = currentIndex + 1; i < flatImages.length; i++) {
                      if (flatImages[i].image.id !== lastSelectedImageId) return false; // Found different image
                    }
                    return true; // No different image found
                  }
                  return true; // No valid selection
                })()}
                title={selectedImages.size === 2 ? "Compare selected images (C)" : "Compare images side-by-side (C)"}
              >
                  {selectedImages.size === 2 ? "Compare" : "Compare"}
                </button>
              )}
            </div>
            
            <div className="stats-section">
              {filteredImages.length} of {allImages.length} images • {imageGroups.length} groups
              {filters.status !== 'all' && ` • ${filters.status}`}
              {filters.filterName !== 'all' && ` • ${filters.filterName}`}
              {filters.searchTerm && ` • "${filters.searchTerm}"`}
              {selectedImages.size > 0 && ` • ${selectedImages.size} selected`}
            </div>
          </div>
          
          {selectedImages.size > 1 && (
            <div className="batch-actions">
              <div className="batch-info">
                {selectedImages.size} images selected - Use keyboard shortcuts (A/R/U) or click buttons below:
              </div>
              <div className="batch-buttons">
                <button 
                  className="action-button accept"
                  onClick={() => gradeBatch('accepted')}
                >
                  Accept Selected ({selectedImages.size})
                </button>
                <button 
                  className="action-button reject"
                  onClick={() => gradeBatch('rejected')}
                >
                  Reject Selected ({selectedImages.size})
                </button>
                <button 
                  className="action-button pending"
                  onClick={() => gradeBatch('pending')}
                >
                  Mark Pending ({selectedImages.size})
                </button>
                <button 
                  className="action-button"
                  onClick={() => {
                    setSelectedImages(new Set());
                    setLastSelectedImageId(null);
                  }}
                >
                  Clear Selection
                </button>
              </div>
            </div>
          )}
        </div>

        {showStats && (
          <StatsDashboard images={filteredImages} />
        )}

        <div className="image-groups">
          {imageGroups.map((group, groupIndex) => {
            const isExpanded = expandedGroups.has(group.filterName);
            const stats = {
              total: group.images.length,
              accepted: group.images.filter(img => img.grading_status === GradingStatus.Accepted).length,
              rejected: group.images.filter(img => img.grading_status === GradingStatus.Rejected).length,
              pending: group.images.filter(img => img.grading_status === GradingStatus.Pending).length,
            };

            return (
              <div key={group.filterName} className="filter-group">
                <div 
                  className="filter-header"
                  onClick={() => toggleGroup(group.filterName)}
                >
                  <span className="filter-toggle">{isExpanded ? '▼' : '▶'}</span>
                  <h3>{group.filterName}</h3>
                  <div className="filter-stats">
                    <span className="stat-total">{stats.total} images</span>
                    <span className="stat-accepted">{stats.accepted} accepted</span>
                    <span className="stat-rejected">{stats.rejected} rejected</span>
                    <span className="stat-pending">{stats.pending} pending</span>
                  </div>
                </div>
                
                {isExpanded && (
                  <div 
                    className="filter-images"
                    style={{
                      gridTemplateColumns: `repeat(auto-fill, minmax(${imageSize}px, 1fr))`,
                    }}
                  >
                    {group.images.map((image, indexInGroup) => {
                      const CardComponent = useLazyImages ? LazyImageCard : ImageCard;
                      return (
                        <div
                          key={image.id}
                          data-image-id={image.id}
                          className={`image-card-wrapper ${
                            selectedImages.has(image.id) ? 'multi-selected' : ''
                          } ${
                            selectedGroupIndex === groupIndex && 
                            selectedImageIndex === indexInGroup ? 'current-selection' : ''
                          }`}
                        >
                          <CardComponent
                            image={image}
                            isSelected={
                              selectedImages.has(image.id) || 
                              (selectedGroupIndex === groupIndex && 
                               selectedImageIndex === indexInGroup)
                            }
                            onClick={(event) => handleImageSelection(image.id, groupIndex, indexInGroup, event)}
                            onDoubleClick={() => navigateToDetail(image.id)}
                          />
                        </div>
                      );
                    })}
                  </div>
                )}
              </div>
            );
          })}
          
          {imageGroups.length === 0 && (
            <div className="empty-state">
              {shouldShowDateHint ? (
                <div>
                  <p>No images found in the selected date range.</p>
                  <p>Try clearing the date filters or selecting a broader range.</p>
                  <button 
                    onClick={() => updateFilters({ dateStart: '', dateEnd: '' })}
                    style={{ 
                      marginTop: '0.5rem', 
                      padding: '0.5rem 1rem', 
                      backgroundColor: '#61dafb', 
                      color: '#000',
                      border: 'none',
                      borderRadius: '4px',
                      cursor: 'pointer'
                    }}
                  >
                    Clear Date Filters
                  </button>
                </div>
              ) : (
                "No images found"
              )}
            </div>
          )}
        </div>
      </div>

    </>
  );
}