import { useMemo, useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useNavigate, useSearchParams } from 'react-router-dom';
import { apiClient } from '../api/client';
import { useDbProjectTarget, useFilters, useGridState } from './useUrlState';
import type { Image } from '../api/types';
import {
  groupImagesBySession,
  imageGroupKey,
  resolveExpandedGroups,
} from '../utils/imageGrouping';

/**
 * Hook for navigating between images in the current context
 */
export function useImageNavigation(currentImageId?: number) {
  const navigate = useNavigate();
  const [searchParams] = useSearchParams();
  const { dbId, projectId, targetId } = useDbProjectTarget();
  const { filters } = useFilters();
  const { groupingMode, expandedGroups } = useGridState();

  // Fetch all images for navigation context
  const { data: allImages = [] } = useQuery({
    queryKey: ['db', dbId, 'all-images', projectId, targetId],
    queryFn: () =>
      apiClient.getImages(dbId!, {
        project_id: projectId!,
        target_id: targetId || undefined,
        limit: 10000,
      }),
    enabled: !!dbId && !!projectId,
  });

  // Apply the same filtering logic as GroupedImageGrid
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

  // Group and sort images the same way as GroupedImageGrid
  const imageGroups = useMemo(() => {
    if (groupingMode === 'session') {
      return groupImagesBySession(filteredImages);
    }

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
        filterName: groupName,
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

  const visibleExpandedGroups = useMemo(
    () => resolveExpandedGroups(imageGroups, groupingMode, expandedGroups),
    [imageGroups, groupingMode, expandedGroups],
  );

  // Create flat list respecting expanded groups (same as GroupedImageGrid)
  const flatImages = useMemo(() => {
    const result: Image[] = [];
    imageGroups.forEach((group) => {
      if (visibleExpandedGroups.has(imageGroupKey(group))) {
        result.push(...group.images);
      }
    });
    return result;
  }, [imageGroups, visibleExpandedGroups]);

  // Find current image index in the flat list
  const currentIndex = useMemo(() => {
    if (!currentImageId || flatImages.length === 0) return -1;
    return flatImages.findIndex(img => img.id === currentImageId);
  }, [currentImageId, flatImages]);

  const canGoPrevious = currentIndex > 0;
  const canGoNext = currentIndex >= 0 && currentIndex < flatImages.length - 1;

  const navigateToImageWithContext = useCallback((imageId: number, view: 'detail' | 'comparison' = 'detail') => {
    const params = searchParams.toString();
    
    if (view === 'detail') {
      navigate(`/detail/${imageId}?${params}`, { replace: true });
    } else {
      // For comparison, we need both left and right image IDs
      const rightImageId = currentImageId === imageId ? 
        (canGoNext ? flatImages[currentIndex + 1]?.id : imageId) : imageId;
      navigate(`/compare/${imageId}/${rightImageId}?${params}`, { replace: true });
    }
  }, [navigate, searchParams, currentImageId, canGoNext, flatImages, currentIndex]);

  const goToNext = useCallback(() => {
    if (canGoNext && currentIndex >= 0) {
      const nextImage = flatImages[currentIndex + 1];
      navigateToImageWithContext(nextImage.id, 'detail');
    }
  }, [canGoNext, currentIndex, flatImages, navigateToImageWithContext]);

  const goToPrevious = useCallback(() => {
    if (canGoPrevious && currentIndex >= 0) {
      const prevImage = flatImages[currentIndex - 1];
      navigateToImageWithContext(prevImage.id, 'detail');
    }
  }, [canGoPrevious, currentIndex, flatImages, navigateToImageWithContext]);

  const goToGrid = useCallback(() => {
    const params = searchParams.toString();
    navigate(`/grid?${params}`, { replace: true });
  }, [navigate, searchParams]);

  return {
    canGoPrevious,
    canGoNext,
    currentIndex,
    totalImages: flatImages.length,
    goToNext,
    goToPrevious,
    goToGrid,
    navigateToImageWithContext,
    allImages: flatImages, // Return the grouped/filtered list for compatibility
  };
}
