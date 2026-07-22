import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import type { Image } from '../api/types';
import { GradingStatus } from '../api/types';
import { useGrading } from '../hooks/useGrading';
import { useStableScrollAnchor } from '../hooks/useStableScrollAnchor';
import { useDbProjectTarget, useGridState, useFilters } from '../hooks/useUrlState';
import ImageCard from './ImageCard';
import LazyImageCard from './LazyImageCard';
import FilterControls, { type FilterOptions } from './FilterControls';
import StatsDashboard from './StatsDashboard';
import UndoRedoToolbar from './UndoRedoToolbar';
import StackPreviewPanel from './StackPreviewPanel';
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
import {
  groupImagesBySession,
  imageGroupKey,
  NO_EXPANDED_GROUPS,
  resolveExpandedGroups,
} from '../utils/imageGrouping';

interface GroupedImageGridProps {
  useLazyImages?: boolean;
}

type GridNavigationDirection = 'next' | 'prev' | 'up' | 'down';

export default function GroupedImageGrid({ useLazyImages = false }: GroupedImageGridProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  // Get state from URL hooks
  const location = useLocation();
  const navigate = useNavigate();
  const { dbId, projectId, targetId } = useDbProjectTarget();
  const {
    groupingMode,
    imageSize,
    showStats,
    expandedGroups,
    currentImageId: urlCurrentImageId,
    selectedImages,
    setGroupingMode,
    setImageSize,
    setExpandedGroups,
    setSelectedImages,
    setCurrentImageId,
    setCurrentImageSelection,
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
  const grading = useGrading(dbId!);
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
  
  // Fetch ALL images (no pagination for grouping) with periodic refresh
  const { data: allImages = [], isLoading } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId || undefined, // null becomes undefined for API
        target_id: targetId || undefined,
        limit: 10000, // Get all images
      }),
    enabled: !!dbId && projectId !== undefined, // Need dbId and a project (or null=all-in-this-db)
    refetchInterval: 30000, // Refresh every 30 seconds
    refetchIntervalInBackground: true,
  });
  useStableScrollAnchor(containerRef, !isLoading);

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

  // A multi-selection is an explicit stacking set. A single highlighted image
  // is normal grid navigation, so fall back to the complete visible set.
  const stackCandidates = useMemo(() => {
    const selectedVisible = filteredImages.filter((image) => selectedImages.has(image.id));
    if (selectedVisible.length >= 2) {
      return { images: selectedVisible, source: 'selected' as const };
    }
    return { images: filteredImages, source: 'visible' as const };
  }, [filteredImages, selectedImages]);
  
  // Group images based on selected mode
  const imageGroups = useMemo(() => {
    if (groupingMode === 'session') {
      return groupImagesBySession(filteredImages);
    }

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

  const visibleExpandedGroups = useMemo(
    () => resolveExpandedGroups(imageGroups, groupingMode, expandedGroups),
    [imageGroups, groupingMode, expandedGroups],
  );

  // Flatten images for navigation
  const flatImages = useMemo(() => {
    const result: Image[] = [];
    imageGroups.forEach((group) => {
      // Only include images from expanded groups
      if (visibleExpandedGroups.has(imageGroupKey(group))) {
        result.push(...group.images);
      }
    });
    return result;
  }, [imageGroups, visibleExpandedGroups]);

  // Resolve the keyboard cursor by image ID. Selection is separate so arrow
  // movement can preserve a set built with the Space key.
  const activeImageId = useMemo(() => {
    if (urlCurrentImageId && flatImages.some(image => image.id === urlCurrentImageId)) {
      return urlCurrentImageId;
    }

    if (lastSelectedImageId && flatImages.some(image => image.id === lastSelectedImageId)) {
      return lastSelectedImageId;
    }

    const selectedId = Array.from(selectedImages).find(id =>
      flatImages.some(image => image.id === id)
    );
    return selectedId ?? flatImages[0]?.id ?? null;
  }, [flatImages, lastSelectedImageId, selectedImages, urlCurrentImageId]);
  const activeImageIdRef = useRef(activeImageId);
  activeImageIdRef.current = activeImageId;

  // Keep the cursor on an image ID. Group positions can move when new images
  // arrive or the grouping mode changes.
  useEffect(() => {
    if (activeImageId !== lastSelectedImageId) {
      setLastSelectedImageId(activeImageId);
    }
    if (activeImageId !== null && activeImageId !== urlCurrentImageId) {
      setCurrentImageId(activeImageId);
    }
  }, [activeImageId, lastSelectedImageId, setCurrentImageId, urlCurrentImageId]);

  // Grading is now handled by the useGrading hook

  const navigateImages = useCallback((direction: GridNavigationDirection) => {
    const currentImageId = activeImageIdRef.current;
    const currentIndex = currentImageId == null
      ? -1
      : flatImages.findIndex(image => image.id === currentImageId);

    if (currentIndex === -1) return;

    let newIndex = currentIndex;
    if (direction === 'next') {
      newIndex = Math.min(currentIndex + 1, flatImages.length - 1);
    } else if (direction === 'prev') {
      newIndex = Math.max(currentIndex - 1, 0);
    } else {
      const currentId = flatImages[currentIndex].id;
      const currentNode = containerRef.current?.querySelector<HTMLElement>(
        `[data-image-id="${currentId}"]`,
      );
      if (!currentNode) return;

      const currentRect = currentNode.getBoundingClientRect();
      const currentCenter = currentRect.left + currentRect.width / 2;
      const sign = direction === 'down' ? 1 : -1;
      let bestVerticalDistance = Number.POSITIVE_INFINITY;
      let bestHorizontalDistance = Number.POSITIVE_INFINITY;

      flatImages.forEach((image, index) => {
        if (index === currentIndex) return;
        const node = containerRef.current?.querySelector<HTMLElement>(
          `[data-image-id="${image.id}"]`,
        );
        if (!node) return;

        const rect = node.getBoundingClientRect();
        const verticalDistance = (rect.top - currentRect.top) * sign;
        if (verticalDistance <= 4) return;
        const horizontalDistance = Math.abs((rect.left + rect.width / 2) - currentCenter);

        const isCloserRow = verticalDistance < bestVerticalDistance - 4;
        const isCloserColumn = Math.abs(verticalDistance - bestVerticalDistance) <= 4
          && horizontalDistance < bestHorizontalDistance;
        if (isCloserRow || isCloserColumn) {
          newIndex = index;
          bestVerticalDistance = verticalDistance;
          bestHorizontalDistance = horizontalDistance;
        }
      });
    }

    if (newIndex === currentIndex) return;

    const newImage = flatImages[newIndex];
    activeImageIdRef.current = newImage.id;
    setCurrentImageId(newImage.id);
    requestAnimationFrame(() => {
      containerRef.current?.querySelector(`[data-image-id="${newImage.id}"]`)
        ?.scrollIntoView({ block: 'nearest' });
    });
  }, [
    flatImages,
    setCurrentImageId,
  ]);

  const toggleCurrentImageSelection = useCallback(() => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId === null) return;
    setSelectedImages((selected) => {
      const next = new Set(selected);
      if (next.has(currentImageId)) {
        next.delete(currentImageId);
      } else {
        next.add(currentImageId);
      }
      return next;
    });
  }, [setSelectedImages]);

  const gradeImage = useCallback(async (status: 'accepted' | 'rejected' | 'pending') => {
    const currentImageId = activeImageIdRef.current;
    if (!currentImageId) return;

    try {
      await grading.gradeImage(currentImageId, status);
      // Auto-advance to next image
      setTimeout(() => navigateImages('next'), 100);
    } catch (error) {
      console.error('Failed to grade image:', error);
    }
  }, [grading, navigateImages]);

  const toggleGroup = useCallback((filterName: string) => {
    setExpandedGroups(() => {
      const next = new Set(visibleExpandedGroups);
      if (next.has(filterName)) {
        next.delete(filterName);
      } else {
        next.add(filterName);
      }
      return next.size > 0 ? next : new Set([NO_EXPANDED_GROUPS]);
    });
  }, [setExpandedGroups, visibleExpandedGroups]);

  // Handle image selection with shift+click support
  const handleImageSelection = useCallback((imageId: number, event: React.MouseEvent) => {
    if (event.shiftKey && lastSelectedImageId) {
      // Shift+click: select range
      const startIndex = flatImages.findIndex(image => image.id === lastSelectedImageId);
      const endIndex = flatImages.findIndex(image => image.id === imageId);
      
      if (startIndex !== -1 && endIndex !== -1) {
        const [minIndex, maxIndex] = [Math.min(startIndex, endIndex), Math.max(startIndex, endIndex)];
        const newSelections = new Set<number>();
        
        for (let i = minIndex; i <= maxIndex; i++) {
          newSelections.add(flatImages[i].id);
        }
        
        setSelectedImages((prev: Set<number>) => {
          const next = new Set(prev);
          newSelections.forEach(id => next.add(id));
          return next;
        });
      }
      activeImageIdRef.current = imageId;
      setCurrentImageId(imageId);
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
      activeImageIdRef.current = imageId;
      setCurrentImageId(imageId);
      setLastSelectedImageId(imageId);
    } else {
      // Regular click: single selection for navigation
      activeImageIdRef.current = imageId;
      setCurrentImageSelection(imageId);
      setLastSelectedImageId(imageId);
    }
  }, [
    flatImages,
    lastSelectedImageId,
    setCurrentImageId,
    setCurrentImageSelection,
    setSelectedImages,
  ]);

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
  }, [selectedImages, grading, setSelectedImages]);

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
  const filtersRef = useRef(filters);
  const updateFiltersRef = useRef(updateFilters);
  useEffect(() => {
    filtersRef.current = filters;
    updateFiltersRef.current = updateFilters;
  }, [filters, updateFilters]);

  useEffect(() => {
    // Only clear dates if they exist, to avoid infinite loops
    if (filtersRef.current.dateRange.start || filtersRef.current.dateRange.end) {
      updateFiltersRef.current({
        dateStart: '',
        dateEnd: '',
      });
    }
  }, [projectId, targetId]); // Only trigger when project/target changes

  // Smart date suggestion: Show info if no images are visible due to date filtering
  const hasDateFilters = filters.dateRange.start || filters.dateRange.end;
  const shouldShowDateHint = hasDateFilters && filteredImages.length === 0 && allImages.length > 0;

  // Clear selection when actual filter values change, but preserve URL-backed
  // selection on mount and when unrelated URL state changes.
  const selectionFilterKey = [
    filters.status,
    filters.filterName,
    filters.dateRange.start ?? '',
    filters.dateRange.end ?? '',
    filters.searchTerm,
  ].join('\u0000');
  const previousSelectionFilterKey = useRef(selectionFilterKey);
  useEffect(() => {
    if (previousSelectionFilterKey.current === selectionFilterKey) return;
    previousSelectionFilterKey.current = selectionFilterKey;
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
  }, [selectionFilterKey, setSelectedImages]);

  // Keyboard shortcuts
  const isGridRoute = location.pathname === '/grid';
  const gridHotkeyOptions = useMemo(() => ({ enabled: isGridRoute }), [isGridRoute]);
  const gridArrowHotkeyOptions = useMemo(
    () => ({ enabled: isGridRoute, preventDefault: true }),
    [isGridRoute],
  );

  useHotkeys('k', () => navigateImages('next'), gridHotkeyOptions, [navigateImages]);
  useHotkeys('j', () => navigateImages('prev'), gridHotkeyOptions, [navigateImages]);
  useHotkeys('right', () => navigateImages('next'), gridArrowHotkeyOptions, [navigateImages]);
  useHotkeys('left', () => navigateImages('prev'), gridArrowHotkeyOptions, [navigateImages]);
  useHotkeys('down', () => navigateImages('down'), gridArrowHotkeyOptions, [navigateImages]);
  useHotkeys('up', () => navigateImages('up'), gridArrowHotkeyOptions, [navigateImages]);
  useHotkeys('space', toggleCurrentImageSelection, gridArrowHotkeyOptions, [toggleCurrentImageSelection]);
  useHotkeys('a', () => {
    if (selectedImages.size > 1) {
      gradeBatch('accepted');
    } else {
      gradeImage('accepted');
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('x', () => {
    if (selectedImages.size > 1) {
      gradeBatch('rejected');
    } else {
      gradeImage('rejected');
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('u', () => {
    if (selectedImages.size > 1) {
      gradeBatch('pending');
    } else {
      gradeImage('pending');
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size]);
  useHotkeys('enter', () => {
    if (lastSelectedImageId) {
      navigateToDetail(lastSelectedImageId);
    }
  }, gridHotkeyOptions, [lastSelectedImageId, navigateToDetail]);
  useHotkeys('escape', () => {
    // Just clear selection on escape (we're already in grid view)
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
  }, gridHotkeyOptions, [setSelectedImages]);
  
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
      const currentIndex = flatImages.findIndex(image => image.id === lastSelectedImageId);
      if (currentIndex !== -1) {
        // Find the next image that is different from the current one
        for (let i = currentIndex + 1; i < flatImages.length; i++) {
          const nextImageId = flatImages[i].id;
          if (lastSelectedImageId !== nextImageId) {
            navigateToComparison(lastSelectedImageId, nextImageId);
            break;
          }
        }
      }
    }
  }, gridHotkeyOptions, [selectedImages, lastSelectedImageId, flatImages, navigateToComparison]);
  
  // Grouping mode shortcuts
  useHotkeys('g', () => {
    // Cycle through grouping modes based on mode (single project vs multi-project)
    if (isMultiProjectMode) {
      setGroupingMode(getNextMultiProjectMode(groupingMode));
    } else {
      setGroupingMode(getNextSingleProjectMode(groupingMode));
    }
  }, gridHotkeyOptions, [groupingMode, isMultiProjectMode, setGroupingMode]);

  if (isLoading) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      <div ref={containerRef} className="grouped-image-container">
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
                    const currentIndex = flatImages.findIndex(image => image.id === lastSelectedImageId);
                    if (currentIndex !== -1) {
                      // Find the next image that is different from the current one
                      for (let i = currentIndex + 1; i < flatImages.length; i++) {
                        const nextImageId = flatImages[i].id;
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
                    const currentIndex = flatImages.findIndex(image => image.id === lastSelectedImageId);
                    if (currentIndex === -1) return true; // Image not found
                    
                    // Check if there's any different image after current position
                    for (let i = currentIndex + 1; i < flatImages.length; i++) {
                      if (flatImages[i].id !== lastSelectedImageId) return false; // Found different image
                    }
                    return true; // No different image found
                  }
                  return true; // No valid selection
                })()}
                title={selectedImages.size === 2 ? "Compare selected images (C)" : "Compare images side-by-side (C)"}
              >
                Compare
              </button>
            </div>
            
            <div className="stats-section">
              <div className="grid-stats">
                {filteredImages.length} of {allImages.length} images • {imageGroups.length} groups
                {filters.status !== 'all' && ` • ${filters.status}`}
                {filters.filterName !== 'all' && ` • ${filters.filterName}`}
                {filters.searchTerm && ` • "${filters.searchTerm}"`}
              </div>
              <div
                className={`selection-action-bar ${selectedImages.size > 1 ? 'active' : ''}`}
                aria-label="Selected image actions"
                aria-hidden={selectedImages.size <= 1}
              >
                <div className="selection-count">
                  {selectedImages.size} selected
                </div>
                <div className="batch-buttons">
                  <button
                    className="action-button accept"
                    disabled={selectedImages.size <= 1}
                    onClick={() => gradeBatch('accepted')}
                  >
                    Accept
                  </button>
                  <button
                    className="action-button reject"
                    disabled={selectedImages.size <= 1}
                    onClick={() => gradeBatch('rejected')}
                  >
                    Reject
                  </button>
                  <button
                    className="action-button pending"
                    disabled={selectedImages.size <= 1}
                    onClick={() => gradeBatch('pending')}
                  >
                    Pending
                  </button>
                  <button
                    className="action-button"
                    disabled={selectedImages.size <= 1}
                    onClick={() => {
                      setSelectedImages(new Set());
                      setLastSelectedImageId(null);
                    }}
                  >
                    Clear
                  </button>
                </div>
              </div>
            </div>
          </div>
        </div>

        {dbId && projectId !== null && projectId !== undefined && (
          <StackPreviewPanel
            dbId={dbId}
            projectId={projectId}
            images={stackCandidates.images}
            selectionSource={stackCandidates.source}
          />
        )}

        {showStats && (
          <StatsDashboard images={filteredImages} />
        )}

        <div className="image-groups">
          {imageGroups.map((group) => {
            const groupKey = imageGroupKey(group);
            const isExpanded = visibleExpandedGroups.has(groupKey);
            const stats = {
              total: group.images.length,
              accepted: group.images.filter(img => img.grading_status === GradingStatus.Accepted).length,
              rejected: group.images.filter(img => img.grading_status === GradingStatus.Rejected).length,
              pending: group.images.filter(img => img.grading_status === GradingStatus.Pending).length,
            };

            return (
              <div key={groupKey} className="filter-group" data-group-key={groupKey}>
                <div 
                  className="filter-header"
                  data-scroll-anchor={`group:${groupKey}`}
                  onClick={() => toggleGroup(groupKey)}
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
                    {group.images.map((image) => {
                      const CardComponent = useLazyImages ? LazyImageCard : ImageCard;
                      return (
                        <div
                          key={image.id}
                          data-image-id={image.id}
                          data-scroll-anchor={`image:${image.id}`}
                          className={`image-card-wrapper ${
                            selectedImages.has(image.id) ? 'multi-selected' : ''
                          } ${
                            image.id === activeImageId ? 'current-selection' : ''
                          }`}
                        >
                          <CardComponent
                            dbId={dbId!}
                            image={image}
                            isSelected={
                              selectedImages.has(image.id) ||
                              image.id === activeImageId
                            }
                            onClick={(event) => handleImageSelection(image.id, event)}
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
