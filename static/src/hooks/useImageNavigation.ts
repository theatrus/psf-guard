import { useMemo, useCallback } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useLocation, useSearchParams } from 'react-router-dom';
import { apiClient } from '../api/client';
import {
  CALIBRATION_FOCUS_PARAM,
  useDbProjectTarget,
  useFilters,
  useGridState,
  useImageOrder,
} from './useUrlState';
import type { Image } from '../api/types';
import {
  groupGridImages,
  groupImagesByPanel,
  imageGroupKey,
  resolveExpandedGroups,
} from '../utils/imageGrouping';
import { useProjectMosaic } from './useProjectMosaic';
import {
  imageDetailClosePath,
  imageDetailPath,
  imageDetailReturnView,
} from '../utils/imageDetailRoutes';
import { matchesStatusFilter } from '../utils/statusFilter';
import { matchesFlagFilter } from '../utils/flagFilter';
import { matchesCalibrationFilter } from '../utils/calibrationFilter';
import { useScopedQuality } from './useSequenceAnalysis';
import { useCalibrationMarks } from './useCalibrationMarks';
import { useNavigateNow } from './useNavigateNow';

/**
 * Hook for navigating between images in the current context
 */
export function useImageNavigation(currentImageId?: number) {
  const location = useLocation();
  // Each move hands the keyboard to the view it opens; see useNavigateNow.
  const navigate = useNavigateNow();
  const [searchParams] = useSearchParams();
  const { dbId, projectId, targetId, mosaic: mosaicScope } = useDbProjectTarget();
  const { filters } = useFilters();
  const { groupingMode, expandedGroups } = useGridState();
  const { order } = useImageOrder();
  // In a mosaic's scope the grid lists frames under their panels; walk
  // them the same way.
  const { data: projectMosaic } = useProjectMosaic(mosaicScope ? dbId : null, mosaicScope ? projectId : null);
  const mosaic = mosaicScope ? projectMosaic ?? null : null;
  // The grid filters by quality flag too, so keyboard navigation reads
  // the same scoped quality results the grid does. Without a flag filter
  // the analysis is not fetched: the detail view must not pay for a
  // sequence analysis it does not use.
  const quality = useScopedQuality(
    dbId,
    projectId,
    targetId,
    filters.filterNames.length === 1 ? filters.filterNames[0] : undefined,
    filters.flags.length > 0,
  );
  // And by calibration, read only when that filter is on.
  const calibration = useCalibrationMarks(dbId, projectId, filters.calibration.length > 0);

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
      if (!matchesStatusFilter(filters.status, image.grading_status)) {
        return false;
      }
      
      // Filter name filter
      if (filters.filterNames.length > 0 && !filters.filterNames.includes(image.filter_name ?? '')) {
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

      // Quality flag filter, once the analysis is in; until then, or if it
      // failed, navigation walks the unfiltered set rather than none.
      if (filters.flags.length > 0 && !quality.isLoading && !quality.error
        && !matchesFlagFilter(filters.flags, quality.qualityByImage.get(image.id))) {
        return false;
      }

      if (filters.calibration.length > 0 && !calibration.isLoading && !calibration.error
        && !matchesCalibrationFilter(filters.calibration, calibration.marks.get(image.id))) {
        return false;
      }
      
      return true;
    });
  }, [
    allImages,
    filters,
    quality.isLoading,
    quality.error,
    quality.qualityByImage,
    calibration.isLoading,
    calibration.error,
    calibration.marks,
  ]);

  // The grid's own groups, in the order it shows them.
  const imageGroups = useMemo(
    () => mosaic
      ? groupImagesByPanel(filteredImages, mosaic, images => groupGridImages(images, groupingMode, order))
      : groupGridImages(filteredImages, groupingMode, order),
    [filteredImages, groupingMode, mosaic, order],
  );

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
      // A chosen calibration finding stays chosen from image to image.
      navigate(
        imageDetailPath(
          imageId,
          searchParams,
          imageDetailReturnView(searchParams),
          searchParams.get(CALIBRATION_FOCUS_PARAM) ?? undefined,
        ),
        { replace: true, state: location.state },
      );
    } else {
      // For comparison, we need both left and right image IDs
      const rightImageId = currentImageId === imageId ? 
        (canGoNext ? flatImages[currentIndex + 1]?.id : imageId) : imageId;
      navigate(`/compare/${imageId}/${rightImageId}?${params}`, { replace: true });
    }
  }, [navigate, searchParams, currentImageId, canGoNext, flatImages, currentIndex, location.state]);

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
    const params = new URLSearchParams(searchParams);
    params.delete(CALIBRATION_FOCUS_PARAM);
    navigate(`/grid?${params}`, { replace: true });
  }, [navigate, searchParams]);

  const closeDetail = useCallback(() => {
    navigate(imageDetailClosePath(searchParams), {
      replace: true,
      state: location.state,
    });
  }, [location.state, navigate, searchParams]);

  return {
    canGoPrevious,
    canGoNext,
    currentIndex,
    totalImages: flatImages.length,
    goToNext,
    goToPrevious,
    goToGrid,
    closeDetail,
    navigateToImageWithContext,
    allImages: flatImages, // Return the grouped/filtered list for compatibility
  };
}
