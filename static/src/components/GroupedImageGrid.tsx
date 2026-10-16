import { useState, useCallback, useEffect, useMemo, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useLocation, useNavigate, useSearchParams } from 'react-router-dom';
import { useHotkeys } from 'react-hotkeys-hook';
import { ChartColumn, Check, Columns2, FolderInput, Layers, RotateCcw, ScanSearch, X } from 'lucide-react';
import { apiClient } from '../api/client';
import type { Image, ProjectMosaic } from '../api/types';
import { GradingStatus } from '../api/types';
import { useGrading } from '../hooks/useGrading';
import { useSpatialScan } from '../hooks/useSpatialScan';
import { useStableScrollAnchor } from '../hooks/useStableScrollAnchor';
import { useDbProjectTarget, useGridState, useFilters, useImageOrder, useUrlParams } from '../hooks/useUrlState';
import { cardIsOnScreen, useReorderAnchor, viewIsAtRest } from '../hooks/useReorderAnchor';
import type { ImageOrder } from '../utils/imageOrder';
import { useNewFrameIds } from '../hooks/useNewFrames';
import ImageCard from './ImageCard';
import LazyImageCard from './LazyImageCard';
import FilterControls, { type FilterOptions } from './FilterControls';
import { imageFilterCount } from '../utils/imageFilterCount';
import ViewToolbar from './toolbar/ViewToolbar';
import ScopeChip from './toolbar/ScopeChip';
import RescanImageFolders from './toolbar/RescanImageFolders';
import SelectionBar from './toolbar/SelectionBar';
import ImageOrderToggle from './toolbar/ImageOrderToggle';
import { availableFlags, matchesFlagFilter } from '../utils/flagFilter';
import StatsDashboard from './StatsDashboard';
import UndoRedoToolbar from './UndoRedoToolbar';
import ProjectExposureGrouping from './ProjectExposureGrouping';
import ThumbnailSizeControl from './ThumbnailSizeControl';
import { QualityScanButton } from './QualityScanControl';
import { 
  type GroupingMode, 
  SINGLE_PROJECT_MODES,
  MULTI_PROJECT_MODES,
  GROUPING_MODE_LABELS,
  getNextSingleProjectMode,
  getNextMultiProjectMode 
} from '../types/grouping';
import {
  imageGroupKey,
  NO_EXPANDED_GROUPS,
  resolveExpandedGroups,
} from '../utils/imageGrouping';
import { imageDetailPath } from '../utils/imageDetailRoutes';
import {
  findGridNavigationIndex,
  type GridNavigationDirection,
} from '../utils/gridNavigation';
import { thumbnailCardHeightEstimate, thumbnailGridColumns } from '../utils/thumbnailSizing';
import { useScopedQuality } from '../hooks/useSequenceAnalysis';
import { useGridGroups } from '../hooks/useGridGroups';
import { mosaicLabel, useProjectMosaic } from '../hooks/useProjectMosaic';
import { useDisplayPreferences } from '../hooks/useDisplayPreferences';
import SecondaryScoreToggle from './SecondaryScoreToggle';
import { useCalibrationMarks } from '../hooks/useCalibrationMarks';
import { useNavigateNow } from '../hooks/useNavigateNow';
import { chipFocus } from '../utils/calibrationFocus';
import { calibrationFilterLabel, matchesCalibrationFilter } from '../utils/calibrationFilter';
import { useFlatGroupLights } from '../hooks/useFlatGroupLights';
import OrganizationDialog, { type OrganizationScope } from './OrganizationDialog';
import ScoringChangedNote from './ScoringChangedNote';
import { matchesStatusFilter, statusFilterLabel } from '../utils/statusFilter';

interface GroupedImageGridProps {
  useLazyImages?: boolean;
}

/** A date filter bound as the URL keeps it: the day alone when it falls on
 *  midnight UTC, as the date inputs give it, else the full instant, as a
 *  link to one night's lights carries it. Both read back as the same
 *  instant, so changing another filter keeps the night's window. */
function urlDate(date: Date | null): string {
  if (!date || Number.isNaN(date.getTime())) return '';
  const instant = date.toISOString();
  return instant.endsWith('T00:00:00.000Z') ? instant.slice(0, 10) : instant;
}

export default function GroupedImageGrid({ useLazyImages = false }: GroupedImageGridProps) {
  const containerRef = useRef<HTMLDivElement>(null);
  // Get state from URL hooks
  const location = useLocation();
  const navigate = useNavigate();
  const navigateNow = useNavigateNow();
  const { dbId, projectId, targetId, mosaic: mosaicScope } = useDbProjectTarget();
  const { data: projectMosaic } = useProjectMosaic(mosaicScope ? dbId : null, mosaicScope ? projectId : null);
  // In a mosaic's scope the grid shows each panel's frames under its panel.
  const mosaic: ProjectMosaic | null = mosaicScope ? projectMosaic ?? null : null;
  const {
    groupingMode,
    imageSize,
    showStats,
    setShowStats,
    expandedGroups,
    currentImageId: urlCurrentImageId,
    selectedImages,
    setGroupingMode,
    resetGroupingMode,
    setImageSize,
    setExpandedGroups,
    setSelectedImages,
    setCurrentImageId,
    setCurrentImageSelection,
  } = useGridState();
  const { filters, updateFilters, clearFilters } = useFilters();
  const { order, setOrder } = useImageOrder();
  
  // Adapter function to convert between FilterOptions and URL state
  const handleFilterChange = useCallback((filterOptions: FilterOptions) => {
    updateFilters({
      status: filterOptions.status === 'all' ? 'all' : String(filterOptions.status),
      filterNames: filterOptions.filterNames,
      dateStart: urlDate(filterOptions.dateRange.start),
      dateEnd: urlDate(filterOptions.dateRange.end),
      searchTerm: filterOptions.searchTerm,
      flags: filterOptions.flags,
      calibration: filterOptions.calibration,
    });
  }, [updateFilters]);

  // Initialize grading system with undo/redo
  const grading = useGrading(dbId!);
  const { advanceOnGrade, showCalibrationChip } = useDisplayPreferences();
  const [lastSelectedImageId, setLastSelectedImageId] = useState<number | null>(null);
  const [organizationScope, setOrganizationScope] = useState<OrganizationScope | null>(null);
  const gridScopeKey = JSON.stringify([dbId, projectId, targetId]);
  const organizationNavigationFromRef = useRef<string | null>(null);
  const selectionAnchorIdRef = useRef<number | null>(null);
  const selectionBaseIdsRef = useRef<Set<number>>(new Set());

  // Navigation helpers
  const [searchParams] = useSearchParams();
  
  const navigateToDetail = useCallback((imageId: number, calibrationFocus?: string) => {
    navigateNow(imageDetailPath(imageId, searchParams, 'grid', calibrationFocus));
  }, [navigateNow, searchParams]);
  
  const navigateToComparison = useCallback((leftId: number, rightId: number) => {
    const params = searchParams.toString();
    navigateNow(`/compare/${leftId}/${rightId}?${params}`);
  }, [navigateNow, searchParams]);
  
  // Fetch ALL images (no pagination for grouping) with periodic refresh
  const { data: allImages = [], isLoading: imagesLoading } = useQuery({
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

  const imagesToMove = allImages.filter(image => selectedImages.has(image.id));
  const canMoveImages = imagesToMove.length > 0
    && imagesToMove.length === selectedImages.size
    && imagesToMove.every(image => image.target_id === imagesToMove[0].target_id);

  // One filter chosen narrows the server's reads to it; with several, all
  // are read and narrowed here.
  const onlyFilter = filters.filterNames.length === 1 ? filters.filterNames[0] : undefined;
  const quality = useScopedQuality(
    dbId,
    projectId,
    targetId,
    onlyFilter,
  );

  const flagFilterActive = filters.flags.length > 0;
  const flagFilterReady = !quality.isLoading && !quality.error;
  const calibrationFilterActive = filters.calibration.length > 0;
  const calibration = useCalibrationMarks(dbId, projectId, calibrationFilterActive || showCalibrationChip);
  const calibrationFilterReady = !calibration.isLoading && !calibration.error;
  // One flat group's lights, from the Calibration page's group detail.
  const flatGroup = useFlatGroupLights(dbId ?? null, filters.flatGroup);
  const { updateParams } = useUrlParams();
  const clearFlatGroup = useCallback(() => updateParams({ flatGroup: null }), [updateParams]);

  // Filter images based on current filters
  const filteredImages = useMemo(() => {
    return allImages.filter(image => {
      // Status filter
      if (!matchesStatusFilter(filters.status, image.grading_status)) {
        return false;
      }
      
      // Filter name filter: any of the chosen filters.
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

      // Quality flag filter. While the analysis is still loading, or could
      // not load, nothing is hidden: an empty grid would read as "no such
      // frames" rather than "not known yet".
      if (flagFilterActive && flagFilterReady
        && !matchesFlagFilter(filters.flags, quality.qualityByImage.get(image.id))) {
        return false;
      }

      // Calibration filter, the same way: nothing is hidden until the
      // marks are in.
      if (calibrationFilterActive && calibrationFilterReady
        && !matchesCalibrationFilter(filters.calibration, calibration.marks.get(image.id))) {
        return false;
      }

      // Flat group filter: none until its lights are known, so nothing
      // the bulk actions reach is outside the group.
      if (flatGroup.active && (!flatGroup.ready || !flatGroup.ids.has(image.id))) {
        return false;
      }
      
      return true;
    });
  }, [
    allImages,
    filters,
    flagFilterActive,
    flagFilterReady,
    quality.qualityByImage,
    calibrationFilterActive,
    calibrationFilterReady,
    calibration.marks,
    flatGroup.active,
    flatGroup.ready,
    flatGroup.ids,
  ]);

  const availableFlagsForScope = useMemo(
    () => availableFlags(quality.qualityByImage.values()),
    [quality.qualityByImage]
  );
  
  // Get available filter names from all images (not just filtered)
  const availableFilters = useMemo(() => {
    const filterSet = new Set<string>();
    allImages.forEach(img => {
      if (img.filter_name) filterSet.add(img.filter_name);
    });
    return Array.from(filterSet).sort();
  }, [allImages]);

  const spatialScan = useSpatialScan(
    dbId,
    targetId ?? undefined,
    onlyFilter,
  );
  let qualityStatus = {
    label: 'Quality: unscored',
    title: 'No sequence score is available for these images. Open a target Sequence view to inspect the evidence or run Analyze Quality.',
  };
  // One target on screen: the scope already names it, so the cards don't.
  const severalTargets = useMemo(
    () => new Set(filteredImages.map(image => image.target_id)).size > 1,
    [filteredImages]
  );
  const scoredImageCount = filteredImages.reduce(
    (count, image) => count + Number(quality.qualityByImage.has(image.id)),
    0,
  );
  const unscoredImageCount = filteredImages.length - scoredImageCount;
  if (quality.isLoading) {
    qualityStatus = {
      label: 'Quality: loading',
      title: 'Loading sequence scores and reasons from stored metadata and cached evidence.',
    };
  } else if (quality.error) {
    qualityStatus = {
      label: 'Quality: unavailable',
      title: 'Sequence quality could not be loaded for this scope.',
    };
  } else if (scoredImageCount > 0 && unscoredImageCount > 0) {
    qualityStatus = {
      label: `Quality: ${scoredImageCount} scored · ${unscoredImageCount} unscored`,
      title: 'Unscored images lack a comparable sequence or enough evidence. Open their target Sequence view to inspect them or run Analyze Quality.',
    };
  } else if (scoredImageCount > 0) {
    qualityStatus = {
      label: `Quality: ${scoredImageCount} scored`,
      title: 'Sequence scores and reasons are available for all visible images.',
    };
  } else if (unscoredImageCount > 0) {
    qualityStatus = {
      label: `Quality: ${unscoredImageCount} unscored`,
      title: 'These images lack a comparable sequence or enough evidence. Open a target Sequence view to inspect them or run Analyze Quality.',
    };
  }

  // Determine if we're in multi-project mode
  const isMultiProjectMode = projectId === null;

  
  // Group images based on selected mode, under their panels in a mosaic,
  // exactly as the detail view's next and previous walk them. Date groups
  // are observing nights, split where this catalog's nights split; they
  // wait for the boundary rather than open on UTC dates and move.
  const { groupsIn, ready: groupsReady } = useGridGroups(dbId, filteredImages, groupingMode, mosaic);
  const isLoading = imagesLoading || !groupsReady;
  useStableScrollAnchor(containerRef, !isLoading);
  const imageGroups = useMemo(() => groupsIn(order), [groupsIn, order]);

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

  // Whether the person has moved the cursor in this scope, by keys or a
  // click. Until then the view is as it opened.
  const touchedScopeRef = useRef<string | null>(null);
  const markTouched = useCallback(() => {
    touchedScopeRef.current = gridScopeKey;
  }, [gridScopeKey]);

  // Turning the order around. At rest, the page stays put and the cursor
  // moves to the new first card. Once the person has moved, the cursor's
  // image, the selection and the cursor's card's place on screen stay.
  const cardOf = useCallback((imageId: number | null) =>
    containerRef.current?.querySelector<HTMLElement>(`[data-image-id="${imageId}"]`) ?? null, []);
  // The order can also change by itself: a signed-in user's saved order
  // arriving after the frames drew, or another tab's choice. A view still
  // at rest then treats it as a flip at rest.
  const captureReorder = useReorderAnchor(order, () => cardOf(activeImageIdRef.current), {
    firstId: flatImages[0]?.id ?? null,
    decide: (previousFirstId) => {
      const nextFirst = flatImages[0]?.id ?? null;
      if (touchedScopeRef.current === gridScopeKey || previousFirstId === null || nextFirst === null
        || activeImageIdRef.current !== previousFirstId || nextFirst === previousFirstId) {
        return 'anchor';
      }
      activeImageIdRef.current = nextFirst;
      selectionAnchorIdRef.current = nextFirst;
      setLastSelectedImageId(nextFirst);
      setCurrentImageId(nextFirst);
      return 'rest';
    },
  });
  const chooseOrder = useCallback((next: ImageOrder) => {
    if (next === order) return;
    const firstId = flatImages[0]?.id ?? null;
    const atRest = viewIsAtRest({
      currentId: activeImageIdRef.current,
      firstId,
      touched: touchedScopeRef.current === gridScopeKey,
      firstOnScreen: cardIsOnScreen(cardOf(firstId)),
    });
    if (!atRest) {
      captureReorder('anchor');
      setOrder(next);
      return;
    }
    const groups = groupsIn(next);
    const shown = resolveExpandedGroups(groups, groupingMode, expandedGroups);
    const nextFirst = groups.find(group => shown.has(imageGroupKey(group)))?.images[0]?.id ?? null;
    captureReorder('rest');
    if (nextFirst === null) {
      setOrder(next);
      return;
    }
    activeImageIdRef.current = nextFirst;
    selectionAnchorIdRef.current = nextFirst;
    setLastSelectedImageId(nextFirst);
    setOrder(next, { current: nextFirst, groupIndex: null, imageIndex: null });
  }, [captureReorder, cardOf, expandedGroups, flatImages, gridScopeKey, groupingMode, groupsIn, order, setOrder]);

  // Frames captured since the last visit from the Library.
  const newImageIds = useNewFrameIds(dbId, allImages);

  // Keep the cursor on an image ID. Group positions can move when new images
  // arrive or the grouping mode changes.
  useEffect(() => {
    // A post-move source refresh must not overwrite navigation to the destination
    // while the router is still committing that transition.
    if (organizationScope || organizationNavigationFromRef.current === gridScopeKey) return;
    organizationNavigationFromRef.current = null;
    if (selectionAnchorIdRef.current === null && activeImageId !== null) {
      selectionAnchorIdRef.current = activeImageId;
      const base = new Set(selectedImages);
      base.delete(activeImageId);
      selectionBaseIdsRef.current = base;
    }
    if (activeImageId !== lastSelectedImageId) {
      setLastSelectedImageId(activeImageId);
    }
    if (activeImageId !== null && activeImageId !== urlCurrentImageId) {
      setCurrentImageId(activeImageId);
    }
  }, [
    activeImageId,
    gridScopeKey,
    lastSelectedImageId,
    organizationScope,
    selectedImages,
    setCurrentImageId,
    urlCurrentImageId,
  ]);

  // Grading is now handled by the useGrading hook

  const navigateImages = useCallback((direction: GridNavigationDirection) => {
    const currentImageId = activeImageIdRef.current;
    const currentIndex = currentImageId == null
      ? -1
      : flatImages.findIndex(image => image.id === currentImageId);

    if (currentIndex === -1) return;

    const newIndex = findGridNavigationIndex(
      flatImages,
      currentIndex,
      direction,
      image => containerRef.current
        ?.querySelector<HTMLElement>(`[data-image-id="${image.id}"]`)
        ?.getBoundingClientRect() ?? null,
    );

    if (newIndex === currentIndex) return;

    const newImage = flatImages[newIndex];
    markTouched();
    activeImageIdRef.current = newImage.id;
    setCurrentImageId(newImage.id);
    requestAnimationFrame(() => {
      containerRef.current?.querySelector(`[data-image-id="${newImage.id}"]`)
        ?.scrollIntoView({ block: 'nearest' });
    });
  }, [
    flatImages,
    markTouched,
    setCurrentImageId,
  ]);

  // Shift+arrow: move the cursor and select everything between the anchor
  // and the new cursor, exactly like shift+click on the destination card.
  const extendSelection = useCallback((direction: GridNavigationDirection) => {
    const currentImageId = activeImageIdRef.current;
    const currentIndex = currentImageId == null
      ? -1
      : flatImages.findIndex(image => image.id === currentImageId);
    if (currentIndex === -1) return;

    const newIndex = findGridNavigationIndex(
      flatImages,
      currentIndex,
      direction,
      image => containerRef.current
        ?.querySelector<HTMLElement>(`[data-image-id="${image.id}"]`)
        ?.getBoundingClientRect() ?? null,
    );
    if (newIndex === currentIndex) return;
    markTouched();

    const storedAnchorId = selectionAnchorIdRef.current;
    const anchorId = storedAnchorId !== null && flatImages.some(
      image => image.id === storedAnchorId,
    ) ? storedAnchorId : currentImageId;
    selectionAnchorIdRef.current = anchorId;
    const anchorIndex = flatImages.findIndex(image => image.id === anchorId);
    const [minIndex, maxIndex] = [
      Math.min(anchorIndex, newIndex),
      Math.max(anchorIndex, newIndex),
    ];
    const next = new Set(selectionBaseIdsRef.current);
    for (let i = minIndex; i <= maxIndex; i++) {
      next.add(flatImages[i].id);
    }
    setSelectedImages(next);

    const newImage = flatImages[newIndex];
    activeImageIdRef.current = newImage.id;
    setCurrentImageId(newImage.id);
    requestAnimationFrame(() => {
      containerRef.current?.querySelector(`[data-image-id="${newImage.id}"]`)
        ?.scrollIntoView({ block: 'nearest' });
    });
  }, [flatImages, markTouched, setCurrentImageId, setSelectedImages]);

  const toggleCurrentImageSelection = useCallback(() => {
    const currentImageId = activeImageIdRef.current;
    if (currentImageId === null) return;
    markTouched();
    selectionAnchorIdRef.current = currentImageId;
    setSelectedImages((selected) => {
      const next = new Set(selected);
      if (next.has(currentImageId)) {
        next.delete(currentImageId);
      } else {
        next.add(currentImageId);
      }
      const base = new Set(next);
      base.delete(currentImageId);
      selectionBaseIdsRef.current = base;
      return next;
    });
  }, [markTouched, setSelectedImages]);

  const gradeImage = useCallback(async (
    status: 'accepted' | 'rejected' | 'pending',
    shiftHeld = false,
  ) => {
    const currentImageId = activeImageIdRef.current;
    if (!currentImageId || !grading.canWrite) return;

    try {
      await grading.gradeImage(currentImageId, status);
      // Advance per preference; Shift does the opposite for this grade.
      const advance = shiftHeld ? !advanceOnGrade : advanceOnGrade;
      if (advance) setTimeout(() => navigateImages('next'), 100);
    } catch (error) {
      console.error('Failed to grade image:', error);
    }
  }, [grading, navigateImages, advanceOnGrade]);

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
    markTouched();
    const storedAnchorId = selectionAnchorIdRef.current;
    const selectionAnchorId = storedAnchorId !== null && flatImages.some(
      image => image.id === storedAnchorId,
    ) ? storedAnchorId : activeImageIdRef.current;
    if (event.shiftKey && selectionAnchorId !== null) {
      // Shift+click: select range
      const startIndex = flatImages.findIndex(image => image.id === selectionAnchorId);
      const endIndex = flatImages.findIndex(image => image.id === imageId);
      
      if (startIndex !== -1 && endIndex !== -1) {
        const [minIndex, maxIndex] = [Math.min(startIndex, endIndex), Math.max(startIndex, endIndex)];
        const newSelections = new Set<number>();
        
        for (let i = minIndex; i <= maxIndex; i++) {
          newSelections.add(flatImages[i].id);
        }
        
        const next = new Set(selectionBaseIdsRef.current);
        newSelections.forEach(id => next.add(id));
        setSelectedImages(next);
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
        const base = new Set(next);
        base.delete(imageId);
        selectionBaseIdsRef.current = base;
        return next;
      });
      activeImageIdRef.current = imageId;
      setCurrentImageId(imageId);
      setLastSelectedImageId(imageId);
      selectionAnchorIdRef.current = imageId;
    } else {
      // Regular click: single selection for navigation
      activeImageIdRef.current = imageId;
      setCurrentImageSelection(imageId);
      setLastSelectedImageId(imageId);
      selectionAnchorIdRef.current = imageId;
      selectionBaseIdsRef.current = new Set();
    }
  }, [
    flatImages,
    markTouched,
    setCurrentImageId,
    setCurrentImageSelection,
    setSelectedImages,
  ]);

  // Batch grading functions
  const gradeBatch = useCallback(async (status: 'accepted' | 'rejected' | 'pending') => {
    if (selectedImages.size === 0 || !grading.canWrite) return;

    try {
      await grading.gradeBatch(Array.from(selectedImages), status);
      
      // Clear selection after batch operation
      setSelectedImages(new Set());
      setLastSelectedImageId(null);
      selectionAnchorIdRef.current = null;
      selectionBaseIdsRef.current = new Set();
    } catch (error) {
      console.error('Batch grading failed:', error);
    }
  }, [selectedImages, grading, setSelectedImages]);

  const clearSelection = useCallback(() => {
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
  }, [setSelectedImages]);

  // What Compare opens: the two selected images, or the cursor's image and
  // the next one after it.
  const comparePair = useMemo((): [number, number] | null => {
    if (selectedImages.size === 2) {
      const [left, right] = Array.from(selectedImages);
      return left !== right ? [left, right] : null;
    }
    if (lastSelectedImageId === null) return null;
    const currentIndex = flatImages.findIndex(image => image.id === lastSelectedImageId);
    if (currentIndex === -1) return null;
    const next = flatImages.slice(currentIndex + 1).find(image => image.id !== lastSelectedImageId);
    return next ? [lastSelectedImageId, next.id] : null;
  }, [flatImages, lastSelectedImageId, selectedImages]);

  // Switch to appropriate grouping mode when changing between single/multi-project
  const prevProjectId = useRef(projectId);
  useEffect(() => {
    if (prevProjectId.current !== projectId) {
      const wasMultiProject = prevProjectId.current === null;
      const isNowMultiProject = projectId === null;
      
      if (wasMultiProject !== isNowMultiProject) {
        // Between one project and All projects the groupings differ: drop
        // the link's, so the saved choice for the new scope applies.
        resetGroupingMode();
      }
      prevProjectId.current = projectId;
    }
  }, [projectId, resetGroupingMode]);
  
  // Clear date filters when project/target changes to avoid "no results" scenarios
  const filtersRef = useRef(filters);
  const updateFiltersRef = useRef(updateFilters);
  useEffect(() => {
    filtersRef.current = filters;
    updateFiltersRef.current = updateFilters;
  }, [filters, updateFilters]);

  // Not on mount: dates a link or a reload brought, such as one night's
  // window from a coverage report, belong to this scope.
  const datesScope = useRef(`${projectId}:${targetId}`);
  useEffect(() => {
    const scope = `${projectId}:${targetId}`;
    if (datesScope.current === scope) return;
    datesScope.current = scope;
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
    filters.filterNames,
    filters.dateRange.start ?? '',
    filters.dateRange.end ?? '',
    filters.searchTerm,
    filters.flags,
    filters.calibration,
    filters.flatGroup ?? '',
  ].join('\u0000');
  const previousSelectionFilterKey = useRef(selectionFilterKey);
  useEffect(() => {
    if (previousSelectionFilterKey.current === selectionFilterKey) return;
    previousSelectionFilterKey.current = selectionFilterKey;
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
  }, [selectionFilterKey, setSelectedImages]);

  // Keyboard shortcuts
  const isGridRoute = location.pathname === '/grid';
  const gridHotkeyOptions = useMemo(() => ({ enabled: isGridRoute }), [isGridRoute]);
  const gridArrowHotkeyOptions = useMemo(
    () => ({ enabled: isGridRoute, preventDefault: true }),
    [isGridRoute],
  );
  // React Router updates the hash before this component rerenders. Opening
  // detail or Compare draws them in the same step (useNavigateNow), but other
  // route changes, such as Back, do not. Guard each callback against the live
  // hash too, so a key pressed in that gap cannot reach a stale grid listener.
  const isLiveGridRoute = useCallback(
    () => window.location.hash.slice(1).split('?')[0] === '/grid',
    [],
  );
  const onLiveGridRoute = useCallback((action: () => void) => {
    if (isLiveGridRoute()) action();
  }, [isLiveGridRoute]);

  useHotkeys(
    'k',
    () => onLiveGridRoute(() => navigateImages('next')),
    gridHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'j',
    () => onLiveGridRoute(() => navigateImages('prev')),
    gridHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'right',
    () => onLiveGridRoute(() => navigateImages('next')),
    gridArrowHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'left',
    () => onLiveGridRoute(() => navigateImages('prev')),
    gridArrowHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'down',
    () => onLiveGridRoute(() => navigateImages('down')),
    gridArrowHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'up',
    () => onLiveGridRoute(() => navigateImages('up')),
    gridArrowHotkeyOptions,
    [navigateImages, onLiveGridRoute],
  );
  useHotkeys(
    'shift+right',
    () => onLiveGridRoute(() => extendSelection('next')),
    gridArrowHotkeyOptions,
    [extendSelection, onLiveGridRoute],
  );
  useHotkeys(
    'shift+left',
    () => onLiveGridRoute(() => extendSelection('prev')),
    gridArrowHotkeyOptions,
    [extendSelection, onLiveGridRoute],
  );
  useHotkeys(
    'shift+down',
    () => onLiveGridRoute(() => extendSelection('down')),
    gridArrowHotkeyOptions,
    [extendSelection, onLiveGridRoute],
  );
  useHotkeys(
    'shift+up',
    () => onLiveGridRoute(() => extendSelection('up')),
    gridArrowHotkeyOptions,
    [extendSelection, onLiveGridRoute],
  );
  useHotkeys(
    'space',
    () => onLiveGridRoute(toggleCurrentImageSelection),
    gridArrowHotkeyOptions,
    [toggleCurrentImageSelection, onLiveGridRoute],
  );
  useHotkeys('a,shift+a', (event) => {
    if (!isLiveGridRoute()) return;
    if (selectedImages.size > 1) {
      gradeBatch('accepted');
    } else {
      gradeImage('accepted', event.shiftKey);
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size, isLiveGridRoute]);
  useHotkeys('x,shift+x', (event) => {
    if (!isLiveGridRoute()) return;
    if (selectedImages.size > 1) {
      gradeBatch('rejected');
    } else {
      gradeImage('rejected', event.shiftKey);
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size, isLiveGridRoute]);
  useHotkeys('u,shift+u', (event) => {
    if (!isLiveGridRoute()) return;
    if (selectedImages.size > 1) {
      gradeBatch('pending');
    } else {
      gradeImage('pending', event.shiftKey);
    }
  }, gridHotkeyOptions, [gradeImage, gradeBatch, selectedImages.size, isLiveGridRoute]);
  useHotkeys('enter', () => {
    if (!isLiveGridRoute()) return;
    if (lastSelectedImageId) {
      navigateToDetail(lastSelectedImageId);
    }
  }, gridHotkeyOptions, [lastSelectedImageId, navigateToDetail, isLiveGridRoute]);
  useHotkeys('escape', () => {
    if (!isLiveGridRoute()) return;
    // Just clear selection on escape (we're already in grid view)
    setSelectedImages(new Set());
    setLastSelectedImageId(null);
    selectionAnchorIdRef.current = null;
    selectionBaseIdsRef.current = new Set();
  }, gridHotkeyOptions, [setSelectedImages, isLiveGridRoute]);
  
  // Add comparison keyboard shortcut
  useHotkeys('c', () => {
    if (!isLiveGridRoute()) return;
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
  }, gridHotkeyOptions, [selectedImages, lastSelectedImageId, flatImages, navigateToComparison, isLiveGridRoute]);
  
  // Grouping mode shortcuts
  useHotkeys('g', () => {
    if (!isLiveGridRoute()) return;
    // Cycle through grouping modes based on mode (single project vs multi-project)
    if (isMultiProjectMode) {
      setGroupingMode(getNextMultiProjectMode(groupingMode));
    } else {
      setGroupingMode(getNextSingleProjectMode(groupingMode));
    }
  }, gridHotkeyOptions, [groupingMode, isMultiProjectMode, setGroupingMode, isLiveGridRoute]);

  if (isLoading) {
    return <div className="loading">Loading images...</div>;
  }

  return (
    <>
      <div ref={containerRef} className="grouped-image-container">
        <ViewToolbar
          label="Image tools"
          className="image-controls"
          sticky
          filters={(
            <FilterControls
              onFilterChange={handleFilterChange}
              availableFilters={availableFilters}
              availableFlags={availableFlagsForScope}
              currentFilters={filters}
            />
          )}
          activeFilters={imageFilterCount(filters, dbId)}
          scope={flatGroup.active ? (
            <ScopeChip
              name={flatGroup.flagged ? 'Flagged lights of a flat group' : 'Lights of a flat group'}
              removeLabel="Show every light, not only the flat group's"
              onRemove={clearFlatGroup}
            />
          ) : undefined}
          onClearFilters={clearFilters}
          controls={(
            <ThumbnailSizeControl
              id="grid-thumbnail-size"
              value={imageSize}
              onChange={setImageSize}
              icons
            />
          )}
          viewNote={order === 'newest' ? 'newest first' : undefined}
          view={(
            <>
              <div className="toolbar-field">
                <label className="toolbar-caption" htmlFor="grid-grouping">Group</label>
                <select
                  id="grid-grouping"
                  value={groupingMode}
                  onChange={(e) => setGroupingMode(e.target.value as GroupingMode)}
                >
                  {(isMultiProjectMode ? MULTI_PROJECT_MODES : SINGLE_PROJECT_MODES).map(mode => (
                    <option key={mode} value={mode}>
                      {GROUPING_MODE_LABELS[mode]}
                    </option>
                  ))}
                </select>
              </div>
              <ImageOrderToggle
                order={order}
                onChange={chooseOrder}
                title="The frames in each group by capture time. Date and Session groups always start with the newest night."
              />
              <div className="toolbar-field">
                <span className="toolbar-caption">Chips</span>
                <SecondaryScoreToggle />
              </div>
              {dbId && projectId != null && <ProjectExposureGrouping
                key={`${dbId}:${projectId}`}
                dbId={dbId}
                projectId={projectId}
                canManage={grading.canWrite}
              />}
            </>
          )}
          tools={(
            <>
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
                type="button"
                className="tool-button icon-at-1600 compare-button"
                onClick={() => {
                  if (comparePair) navigateToComparison(comparePair[0], comparePair[1]);
                }}
                disabled={!comparePair}
                title={selectedImages.size === 2 ? "Compare selected images (C)" : "Compare images side-by-side (C)"}
              >
                <Columns2 size={14} aria-hidden="true" />
                <span className="tool-button-label">Compare</span>
              </button>
              {/* The statistics open below the toolbar. */}
              <button
                type="button"
                className="tool-button icon-at-1600 stats-toggle"
                aria-pressed={showStats}
                aria-controls={showStats ? 'grid-stats-dashboard' : undefined}
                title={showStats ? 'Hide image statistics' : 'Show image statistics'}
                onClick={() => setShowStats(!showStats)}
              >
                <ChartColumn size={14} aria-hidden="true" />
                <span className="tool-button-label">Stats</span>
              </button>
              <QualityScanButton
                scan={spatialScan}
                targetId={targetId}
                canWrite={grading.canWrite}
                className="tool-button icon-at-1360 quality-scan-button"
                icon={<ScanSearch size={14} aria-hidden="true" />}
              />
            </>
          )}
          menu={grading.canWrite ? (
            <>
            <button
              type="button"
              className="toolbar-menu-item organization-action"
              disabled={!canMoveImages}
              title={canMoveImages ? 'Move selected exposures to another target or project' : 'Select exposures from one target to move'}
              onClick={() => {
                if (!dbId || !canMoveImages) return;
                const source = imagesToMove[0];
                setOrganizationScope({
                  kind: 'move_images',
                  dbId,
                  sourceTargetId: source.target_id,
                  sourceTargetName: source.target_name,
                  sourceProjectId: source.project_id,
                  imageIds: imagesToMove.map(image => image.id),
                });
              }}
            >
              <FolderInput size={14} aria-hidden="true" /> Move exposures
            </button>
            <RescanImageFolders dbId={dbId ?? null} />
            </>
          ) : undefined}
          summary={(
            <>
            <span className="grid-stats">
              {mosaic && <>{mosaicLabel(mosaic)} • </>}
              {filteredImages.length} of {allImages.length} images • {imageGroups.length} groups
              {filters.status !== 'all' && ` • ${statusFilterLabel(filters.status)}`}
              {filters.filterNames.length > 0 && ` • ${filters.filterNames.join(', ')}`}
              {filters.searchTerm && ` • "${filters.searchTerm}"`}
              {calibrationFilterActive && ` • ${filters.calibration.map(calibrationFilterLabel).join(', ')}`}
              {calibrationFilterActive && calibration.updating && ' (updating)'}
              {flatGroup.active && ` • ${flatGroupSummary(flatGroup)}`}
              {' • '}
              <span className="grid-quality-state" title={qualityStatus.title}>
                {qualityStatus.label}
              </span>
            </span>
            {/* The badges and Flagged follow scoring set in Sequence. */}
            <ScoringChangedNote />
            </>
          )}
          selection={(
            <SelectionBar
              count={selectedImages.size}
              minimum={2}
              hint="Ctrl- or ⌘-click to add images, Shift-click for a range"
              onClear={clearSelection}
            >
              <button
                type="button"
                className="tool-button tool-button-accept"
                disabled={!grading.canWrite}
                title="Accept the selected images (A)"
                onClick={() => gradeBatch('accepted')}
              >
                <Check size={14} aria-hidden="true" />
                <span className="tool-button-label">Accept</span>
              </button>
              <button
                type="button"
                className="tool-button tool-button-reject"
                disabled={!grading.canWrite}
                title="Reject the selected images (X)"
                onClick={() => gradeBatch('rejected')}
              >
                <X size={14} aria-hidden="true" />
                <span className="tool-button-label">Reject</span>
              </button>
              <button
                type="button"
                className="tool-button"
                disabled={!grading.canWrite}
                title="Set the selected images back to Pending (U)"
                onClick={() => gradeBatch('pending')}
              >
                <RotateCcw size={14} aria-hidden="true" />
                <span className="tool-button-label">Pending</span>
              </button>
              <span className="selection-actions-divider" aria-hidden="true" />
              {/* Stacks lives beside Images; a selection travels there
                  as the set to stack. */}
              <button
                type="button"
                className="tool-button"
                disabled={projectId == null}
                title="Stack the selected images"
                onClick={() =>
                  navigate(`/stacks?${searchParams.toString()}`, {
                    state: { stackImageIds: [...selectedImages] },
                  })
                }
              >
                <Layers size={14} aria-hidden="true" />
                <span className="tool-button-label">Stack selected</span>
              </button>
            </SelectionBar>
          )}
        />


        {showStats && (
          <div id="grid-stats-dashboard">
            <StatsDashboard images={filteredImages} />
          </div>
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
                {group.panelHeading && <h2 className="mosaic-panel-heading">{group.panelHeading}</h2>}
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
                      gridTemplateColumns: thumbnailGridColumns(imageSize),
                      ['--card-height-estimate' as string]: `${thumbnailCardHeightEstimate(imageSize)}px`,
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
                            quality={quality.qualityByImage.get(image.id)}
                            qualityScoreScope={quality.scopeByImage.get(image.id)}
                            basisScores={quality.basisScoresByImage.get(image.id)}
                            calibration={calibration.marks.get(image.id)}
                            onCalibrationClick={(mark) =>
                              navigateToDetail(image.id, chipFocus(mark.codes, filters.calibration))}
                            qualityPresentation="compact"
                            showTarget={severalTargets}
                            isNew={newImageIds.has(image.id)}
                            isSelected={
                              selectedImages.has(image.id) ||
                              image.id === activeImageId
                            }
                            selectionEffects={image.id === activeImageId}
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
              {flatGroup.active && !flatGroup.ready ? (
                <div data-testid="flat-group-filter-state" role="status">
                  <p>{flatGroupEmptyText(flatGroup)}</p>
                  {flatGroup.state !== 'finding' && (
                    <button type="button" className="empty-state-action" onClick={clearFlatGroup}>
                      Show every light
                    </button>
                  )}
                </div>
              ) : shouldShowDateHint ? (
                <div>
                  <p>No images found in the selected date range.</p>
                  <p>Try clearing the date filters or selecting a broader range.</p>
                  <button
                    type="button"
                    className="empty-state-action"
                    onClick={() => updateFilters({ dateStart: '', dateEnd: '' })}
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

      {organizationScope && (
        <OrganizationDialog
          scope={organizationScope}
          onClose={() => setOrganizationScope(null)}
          onApplied={result => {
            organizationNavigationFromRef.current = gridScopeKey;
            const params = new URLSearchParams({
              db: organizationScope.dbId,
              project: String(result.project_id),
              target: String(result.target_id),
            });
            navigate(`/grid?${params}`);
          }}
        />
      )}
    </>
  );
}

/** The flat-group filter in the grid's summary line. */
function flatGroupSummary(flatGroup: ReturnType<typeof useFlatGroupLights>): string {
  const what = flatGroup.flagged ? 'Flagged lights of one flat group' : 'Lights of one flat group';
  switch (flatGroup.state) {
    case 'missing':
      return 'This flat group no longer exists';
    case 'failed':
      return `${what}: could not be found`;
    case 'finding':
      return `${what} (finding them…)`;
    default:
      return flatGroup.updating ? `${what} (updating)` : what;
  }
}

/** What the grid says while the flat-group filter shows nothing. */
function flatGroupEmptyText(flatGroup: ReturnType<typeof useFlatGroupLights>): string {
  switch (flatGroup.state) {
    case 'missing':
      return 'This flat group no longer exists: its nights were grouped again. Open Calibration › Groups to find the group now.';
    case 'failed':
      return `Could not find the group's lights: ${flatGroup.error}. Trying again in a while.`;
    default:
      return "Finding the group's lights…";
  }
}
