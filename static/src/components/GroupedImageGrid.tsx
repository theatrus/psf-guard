import { useState, useCallback, useEffect, useMemo } from 'react';
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

type GroupingMode = 'filter' | 'date' | 'both';

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
      project_id: projectId!,
      target_id: targetId || undefined,
      limit: 10000, // Get all images
    }),
    enabled: !!projectId,
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

  // Group images based on selected mode
  const imageGroups = useMemo(() => {
    const groups = new Map<string, Image[]>();
    
    filteredImages.forEach(image => {
      let groupKey: string;
      
      if (groupingMode === 'filter') {
        groupKey = image.filter_name || 'No Filter';
      } else if (groupingMode === 'date') {
        // Group by date (YYYY-MM-DD)
        if (image.acquired_date) {
          const date = new Date(image.acquired_date * 1000);
          groupKey = date.toISOString().split('T')[0];
        } else {
          groupKey = 'Unknown Date';
        }
      } else { // 'both'
        // Group by both filter and date
        const filterName = image.filter_name || 'No Filter';
        let dateStr = 'Unknown Date';
        if (image.acquired_date) {
          const date = new Date(image.acquired_date * 1000);
          dateStr = date.toISOString().split('T')[0];
        }
        groupKey = `${filterName} - ${dateStr}`;
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
    
    // Sort groups
    if (groupingMode === 'date') {
      // Sort by date descending (newest first)
      sorted.sort((a, b) => b.filterName.localeCompare(a.filterName));
    } else {
      // Sort alphabetically
      sorted.sort((a, b) => a.filterName.localeCompare(b.filterName));
    }
    
    return sorted;
  }, [filteredImages, groupingMode]);

  // Initialize expanded groups when imageGroups change
  useEffect(() => {
    if (expandedGroups.size === 0 && imageGroups.length > 0) {
      setExpandedGroups(new Set(imageGroups.map(g => g.filterName)));
    }
  }, [imageGroups, expandedGroups.size]);

  // Reset expanded groups when grouping mode changes
  useEffect(() => {
    // Expand all groups when grouping mode changes
    if (imageGroups.length > 0) {
      setExpandedGroups(new Set(imageGroups.map(g => g.filterName)));
    }
  }, [groupingMode, imageGroups]);

  // Flatten images for navigation
  const flatImages = useMemo(() => {
    const result: { image: Image; groupIndex: number; indexInGroup: number }[] = [];
    imageGroups.forEach((group, groupIndex) => {
      if (expandedGroups.has(group.filterName) || expandedGroups.size === 0) {
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
  useHotkeys('r', () => {
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
      navigateToComparison(selectedArray[0], selectedArray[1]);
    } else if (lastSelectedImageId) {
      // Use current image + next image for comparison
      const currentIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
      if (currentIndex !== -1 && currentIndex < flatImages.length - 1) {
        navigateToComparison(lastSelectedImageId, flatImages[currentIndex + 1].image.id);
      }
    }
  }, [selectedImages, lastSelectedImageId, flatImages, navigateToComparison]);
  
  // Grouping mode shortcuts
  useHotkeys('g', () => {
    // Cycle through grouping modes
    if (groupingMode === 'filter') {
      setGroupingMode('date');
    } else if (groupingMode === 'date') {
      setGroupingMode('both');
    } else {
      setGroupingMode('filter');
    }
  }, [groupingMode, setGroupingMode]);

  if (isLoading) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      <div className="grouped-image-container">
        <div className="image-controls">
          <FilterControls 
            onFilterChange={handleFilterChange}
            availableFilters={availableFilters}
          />
          <div className="control-row">
            <div className="size-control">
              <label>Image Size:</label>
              <input
                type="range"
                min="150"
                max="1200"
                step="50"
                value={imageSize}
                onChange={(e) => setImageSize(Number(e.target.value))}
              />
              <span>{imageSize}px {imageSize >= 1000 ? '(Full Width)' : ''}</span>
            </div>
            <div className="grouping-control">
              <label>Group by:</label>
              <select 
                value={groupingMode} 
                onChange={(e) => setGroupingMode(e.target.value as GroupingMode)}
              >
                <option value="filter">Filter</option>
                <option value="date">Date</option>
                <option value="both">Filter & Date</option>
              </select>
            </div>
          </div>
          
          {/* Undo/Redo Toolbar */}
          <div style={{ display: 'flex', gap: '1rem', alignItems: 'center' }}>
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
                className="toolbar-button compare-button"
                onClick={() => {
                  if (selectedImages.size === 2) {
                    // Use the two selected images for comparison
                    const selectedArray = Array.from(selectedImages);
                    navigateToComparison(selectedArray[0], selectedArray[1]);
                  } else if (lastSelectedImageId) {
                    // Use current image + next image for comparison
                    const currentIndex = flatImages.findIndex(item => item.image.id === lastSelectedImageId);
                    if (currentIndex !== -1 && currentIndex < flatImages.length - 1) {
                      navigateToComparison(lastSelectedImageId, flatImages[currentIndex + 1].image.id);
                    }
                  }
                }}
                disabled={!lastSelectedImageId && selectedImages.size !== 2}
                title={selectedImages.size === 2 ? "Compare selected images (C)" : "Compare images side-by-side (C)"}
              >
                {selectedImages.size === 2 ? "Compare Selected (C)" : "Compare (C)"}
              </button>
            )}
          </div>
          
          <div className="group-stats">
            Total: {filteredImages.length} of {allImages.length} images in {imageGroups.length} groups
            {filters.status !== 'all' && ` (${filters.status})`}
            {filters.filterName !== 'all' && ` (${filters.filterName})`}
            {filters.searchTerm && ` (searching: ${filters.searchTerm})`}
            {selectedImages.size > 0 && ` • ${selectedImages.size} selected`}
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
            <div className="empty-state">No images found</div>
          )}
        </div>
      </div>

    </>
  );
}