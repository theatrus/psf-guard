/** The frame-sites job's status query, which the header's job line and the
 *  Calibration page share. */
export const frameSitesKey = (dbId: string) => ['db', dbId, 'calibrations', 'frame-sites'] as const;
