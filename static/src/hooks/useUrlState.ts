import { useSearchParams } from 'react-router-dom';
import { useCallback, useMemo } from 'react';
import { type GroupingMode, DEFAULT_SINGLE_PROJECT_MODE } from '../types/grouping';

/**
 * Hook for managing URL search parameters as state
 */
export function useUrlParams() {
  const [searchParams, setSearchParams] = useSearchParams();
  
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
    return value ? value.split(',').filter(Boolean) : [];
  }, [searchParams]);
  
  const getNumberArrayParam = useCallback((key: string): number[] => {
    const value = searchParams.get(key);
    return value ? value.split(',').map(id => parseInt(id, 10)).filter(id => !isNaN(id)) : [];
  }, [searchParams]);
  
  const updateParams = useCallback((updates: Record<string, string | number | boolean | string[] | number[] | null | undefined>) => {
    setSearchParams(prev => {
      const newParams = new URLSearchParams(prev);
      
      Object.entries(updates).forEach(([key, value]) => {
        if (value === null || value === undefined || value === '' || value === 'all') {
          newParams.delete(key);
        } else if (Array.isArray(value)) {
          if (value.length === 0) {
            newParams.delete(key);
          } else {
            newParams.set(key, value.join(','));
          }
        } else {
          newParams.set(key, String(value));
        }
      });
      
      return newParams;
    }, { replace: true });
  }, [setSearchParams]);
  
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
 * Hook for managing project and target selection in URL
 */
export function useProjectTarget() {
  const { getNumberParam, updateParams } = useUrlParams();
  
  const projectId = useMemo(() => getNumberParam('project'), [getNumberParam]);
  const targetId = useMemo(() => getNumberParam('target'), [getNumberParam]);
  
  const setProjectId = useCallback((id: number | null) => {
    updateParams({ 
      project: id,
      target: null, // Reset target when project changes
    });
  }, [updateParams]);
  
  const setTargetId = useCallback((id: number | null) => {
    updateParams({ target: id });
  }, [updateParams]);
  
  return {
    projectId,
    targetId,
    setProjectId,
    setTargetId,
  };
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
  
  const selectedGroupIndex = useMemo(() => getNumberParam('groupIndex') || 0, [getNumberParam]);
  const selectedImageIndex = useMemo(() => getNumberParam('imageIndex') || 0, [getNumberParam]);
  const groupingMode = useMemo(() => (getParam('grouping') as GroupingMode) || DEFAULT_SINGLE_PROJECT_MODE, [getParam]);
  const imageSize = useMemo(() => getNumberParam('size') || 300, [getNumberParam]);
  const showStats = useMemo(() => getBooleanParam('stats'), [getBooleanParam]);
  const expandedGroups = useMemo(() => new Set(getArrayParam('expanded')), [getArrayParam]);
  const selectedImages = useMemo(() => new Set(getNumberArrayParam('selected')), [getNumberArrayParam]);
  
  const setSelectedGroupIndex = useCallback((index: number) => {
    updateParams({ groupIndex: index });
  }, [updateParams]);
  
  const setSelectedImageIndex = useCallback((index: number) => {
    updateParams({ imageIndex: index });
  }, [updateParams]);
  
  const setGroupingMode = useCallback((mode: GroupingMode) => {
    updateParams({ grouping: mode });
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
    const newImages = typeof images === 'function' ? images(selectedImages) : images;
    updateParams({ selected: Array.from(newImages) });
  }, [updateParams, selectedImages]);

  return {
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
    setShowStats,
    setExpandedGroups,
    setSelectedImages,
  };
}

