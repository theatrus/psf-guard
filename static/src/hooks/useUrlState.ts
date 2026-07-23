import { useSearchParams } from 'react-router-dom';
import { useCallback, useMemo, useRef } from 'react';
import { type GroupingMode, DEFAULT_SINGLE_PROJECT_MODE } from '../types/grouping';

/**
 * Hook for managing URL search parameters as state
 */
export function useUrlParams() {
  const [searchParams, setSearchParams] = useSearchParams();
  const setSearchParamsRef = useRef(setSearchParams);
  const searchParamsRef = useRef(searchParams);
  setSearchParamsRef.current = setSearchParams;
  searchParamsRef.current = searchParams;
  
  const getParam = useCallback((key: string): string | null => {
    return searchParams.get(key);
  }, [searchParams]);
  
  const getNumberParam = useCallback((key: string): number | null => {
    const value = searchParams.get(key);
    return value ? parseInt(value, 10) : null;
  }, [searchParams]);
  
  const getBooleanParam = useCallback((key: string): boolean => {
    return searchParams.get(key) === 'true';
  }, [searchParams]);
  
  const getArrayParam = useCallback((key: string): string[] => {
    const value = searchParams.get(key);
    return value
      ? value.split(',').filter(Boolean).map((item) => decodeURIComponent(item))
      : [];
  }, [searchParams]);
  
  const getNumberArrayParam = useCallback((key: string): number[] => {
    const value = searchParams.get(key);
    return value ? value.split(',').map(id => parseInt(id, 10)).filter(id => !isNaN(id)) : [];
  }, [searchParams]);
  
  const updateParams = useCallback((updates: Record<string, string | number | boolean | string[] | number[] | null | undefined>) => {
    const newParams = new URLSearchParams(searchParamsRef.current);

    Object.entries(updates).forEach(([key, value]) => {
      if (value === null || value === undefined || value === '' || value === 'all') {
        newParams.delete(key);
      } else if (Array.isArray(value)) {
        if (value.length === 0) {
          newParams.delete(key);
        } else {
          newParams.set(key, value.map((item) => encodeURIComponent(String(item))).join(','));
        }
      } else {
        newParams.set(key, String(value));
      }
    });

    searchParamsRef.current = newParams;
    setSearchParamsRef.current(newParams, { replace: true });
  }, []);
  
  return {
    getParam,
    getNumberParam,
    getBooleanParam,
    getArrayParam,
    getNumberArrayParam,
    updateParams,
    searchParams
  };
}

/**
 * Hook for reading the (db, project, target) triple from URL state, used by
 * every scoped view (grid, detail, comparison, sequence). The `dbId` is
 * required for any of the per-DB API calls; if it's missing, the view should
 * render an empty/error state rather than fall back to a default.
 *
 * `setDbProjectTarget` is the only way to navigate INTO a scoped view from
 * a list — it sets all three params atomically so the URL is never partially
 * populated.
 */
export function useDbProjectTarget() {
  const { getParam, getNumberParam, updateParams } = useUrlParams();

  const dbId = useMemo(() => getParam('db'), [getParam]);
  const projectId = useMemo(() => getNumberParam('project'), [getNumberParam]);
  const targetId = useMemo(() => getNumberParam('target'), [getNumberParam]);

  const setDbProjectTarget = useCallback(
    (db: string | null, project: number | null, target: number | null) => {
      updateParams({ db, project, target });
    },
    [updateParams]
  );

  const setProjectId = useCallback(
    (id: number | null) => {
      // Reset target when project changes; dbId stays.
      updateParams({ project: id, target: null });
    },
    [updateParams]
  );

  const setTargetId = useCallback(
    (id: number | null) => {
      updateParams({ target: id });
    },
    [updateParams]
  );

  return {
    dbId,
    projectId,
    targetId,
    setDbProjectTarget,
    setProjectId,
    setTargetId,
  };
}

/**
 * Backwards-compatible shim. Same signature as the old `useProjectTarget`
 * but the `dbId` is now part of URL state too — readable via
 * `useDbProjectTarget`. Existing call sites that only need project/target
 * (e.g. selectors) can keep using this.
 */
export function useProjectTarget() {
  const { projectId, targetId, setProjectId, setTargetId } = useDbProjectTarget();
  return { projectId, targetId, setProjectId, setTargetId };
}

/**
 * Hook for managing filter state in URL
 */
export function useFilters() {
  const { getParam, updateParams } = useUrlParams();
  
  const filters = useMemo(() => ({
    status: getParam('status') || 'all',
    filterName: getParam('filter') || 'all', 
    dateRange: {
      start: getParam('dateStart') || null,
      end: getParam('dateEnd') || null,
    },
    searchTerm: getParam('search') || '',
  }), [getParam]);
  
  const updateFilters = useCallback((updates: {
    status?: string;
    filterName?: string;
    dateStart?: string;
    dateEnd?: string;
    searchTerm?: string;
  }) => {
    updateParams({
      status: updates.status,
      filter: updates.filterName,
      dateStart: updates.dateStart,
      dateEnd: updates.dateEnd, 
      search: updates.searchTerm,
    });
  }, [updateParams]);
  
  const clearFilters = useCallback(() => {
    updateParams({
      status: null,
      filter: null,
      dateStart: null,
      dateEnd: null,
      search: null,
    });
  }, [updateParams]);
  
  return {
    filters,
    updateFilters,
    clearFilters,
  };
}

/**
 * Hook for managing grid state in URL
 */
export function useGridState() {
  const { getParam, getNumberParam, getBooleanParam, getArrayParam, getNumberArrayParam, updateParams } = useUrlParams();
  
  const groupingMode = useMemo(() => (getParam('grouping') as GroupingMode) || DEFAULT_SINGLE_PROJECT_MODE, [getParam]);
  const imageSize = useMemo(() => getNumberParam('size') || 300, [getNumberParam]);
  const showStats = useMemo(() => getBooleanParam('stats'), [getBooleanParam]);
  const expandedGroups = useMemo(() => new Set(getArrayParam('expanded')), [getArrayParam]);
  const currentImageId = useMemo(() => getNumberParam('current'), [getNumberParam]);
  const selectedImages = useMemo(() => new Set(getNumberArrayParam('selected')), [getNumberArrayParam]);
  const selectedImagesRef = useRef(selectedImages);
  selectedImagesRef.current = selectedImages;
  
  const setGroupingMode = useCallback((mode: GroupingMode) => {
    updateParams({ grouping: mode, expanded: null });
  }, [updateParams]);
  
  const setImageSize = useCallback((size: number) => {
    updateParams({ size: size === 300 ? null : size });
  }, [updateParams]);
  
  const setShowStats = useCallback((show: boolean) => {
    updateParams({ stats: show ? true : null });
  }, [updateParams]);
  
  const setExpandedGroups = useCallback((groups: Set<string> | ((prev: Set<string>) => Set<string>)) => {
    const newGroups = typeof groups === 'function' ? groups(expandedGroups) : groups;
    updateParams({ expanded: Array.from(newGroups) });
  }, [updateParams, expandedGroups]);
  
  const setSelectedImages = useCallback((images: Set<number> | ((prev: Set<number>) => Set<number>)) => {
    const newImages = typeof images === 'function' ? images(selectedImagesRef.current) : images;
    selectedImagesRef.current = new Set(newImages);
    updateParams({ selected: Array.from(selectedImagesRef.current) });
  }, [updateParams]);

  const setCurrentImageId = useCallback((imageId: number) => {
    updateParams({
      current: imageId,
      groupIndex: null,
      imageIndex: null,
    });
  }, [updateParams]);

  const setCurrentImageSelection = useCallback((imageId: number) => {
    selectedImagesRef.current = new Set([imageId]);
    updateParams({
      current: imageId,
      groupIndex: null,
      imageIndex: null,
      selected: [imageId],
    });
  }, [updateParams]);

  return {
    groupingMode,
    imageSize,
    showStats,
    expandedGroups,
    currentImageId,
    selectedImages,
    setGroupingMode,
    setImageSize,
    setShowStats,
    setExpandedGroups,
    setSelectedImages,
    setCurrentImageId,
    setCurrentImageSelection,
  };
}
