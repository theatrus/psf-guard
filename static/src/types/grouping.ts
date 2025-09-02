/**
 * Grouping modes for organizing images in the grid view
 */
export type GroupingMode = 
  | 'filter' 
  | 'date' 
  | 'both' 
  | 'project' 
  | 'project+filter' 
  | 'project+date' 
  | 'project+date+filter';

/**
 * Grouping modes available for single-project view
 */
export const SINGLE_PROJECT_MODES: readonly GroupingMode[] = [
  'filter',
  'date', 
  'both'
] as const;

/**
 * Grouping modes available for multi-project view  
 */
export const MULTI_PROJECT_MODES: readonly GroupingMode[] = [
  'project',
  'project+filter',
  'project+date', 
  'project+date+filter'
] as const;

/**
 * All available grouping modes
 */
export const ALL_GROUPING_MODES: readonly GroupingMode[] = [
  ...SINGLE_PROJECT_MODES,
  ...MULTI_PROJECT_MODES
] as const;

/**
 * Default grouping mode for single-project view
 */
export const DEFAULT_SINGLE_PROJECT_MODE: GroupingMode = 'filter';

/**
 * Default grouping mode for multi-project view
 */
export const DEFAULT_MULTI_PROJECT_MODE: GroupingMode = 'project';

/**
 * Human-readable labels for grouping modes
 */
export const GROUPING_MODE_LABELS: Record<GroupingMode, string> = {
  'filter': 'Filter',
  'date': 'Date', 
  'both': 'Filter + Date',
  'project': 'Project',
  'project+filter': 'Project + Filter',
  'project+date': 'Project + Date',
  'project+date+filter': 'Project + Date + Filter'
} as const;

/**
 * Check if a grouping mode is valid for single-project view
 */
export function isSingleProjectMode(mode: GroupingMode): mode is typeof SINGLE_PROJECT_MODES[number] {
  return SINGLE_PROJECT_MODES.includes(mode);
}

/**
 * Check if a grouping mode is valid for multi-project view
 */
export function isMultiProjectMode(mode: GroupingMode): mode is typeof MULTI_PROJECT_MODES[number] {
  return MULTI_PROJECT_MODES.includes(mode);
}

/**
 * Get the next grouping mode in cycle for single-project view
 */
export function getNextSingleProjectMode(current: GroupingMode): GroupingMode {
  const currentIndex = SINGLE_PROJECT_MODES.indexOf(current as any);
  const nextIndex = currentIndex === -1 ? 0 : (currentIndex + 1) % SINGLE_PROJECT_MODES.length;
  return SINGLE_PROJECT_MODES[nextIndex];
}

/**
 * Get the next grouping mode in cycle for multi-project view  
 */
export function getNextMultiProjectMode(current: GroupingMode): GroupingMode {
  const currentIndex = MULTI_PROJECT_MODES.indexOf(current as any);
  const nextIndex = currentIndex === -1 ? 0 : (currentIndex + 1) % MULTI_PROJECT_MODES.length;
  return MULTI_PROJECT_MODES[nextIndex];
}