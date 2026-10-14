import type { QueryClient } from '@tanstack/react-query';

/** What each database's views read that the calibration settings decide. */
const DATABASE_VIEWS = new Set(['image-calibration', 'calibration-health', 'calibration-marks', 'calibrations']);
/** What a project's calibration report reads. */
const PROJECT_VIEWS = new Set(['calibration-health', 'calibration-report']);

/**
 * Read again what a change of calibration settings decides: a light's
 * account, the Library's and a project's health, the Grid's marks and the
 * calibration library's grades. The server judges them again in the
 * background, and these views show its answer as it lands.
 */
export function invalidateCalibrationViews(queryClient: QueryClient): void {
  void queryClient.invalidateQueries({
    predicate: ({ queryKey }) =>
      queryKey[0] === 'db' &&
      (DATABASE_VIEWS.has(String(queryKey[2])) ||
        (queryKey[2] === 'project' && PROJECT_VIEWS.has(String(queryKey[4])))),
  });
}
