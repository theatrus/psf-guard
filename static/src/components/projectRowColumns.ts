/** The Library's columns, in order. Overview.css gives each one a track. */
export const ROW_COLUMNS = ['fold', 'name', 'db', 'state', 'progress', 'grades', 'dates', 'calibration', 'views', 'plan'] as const;
export type RowColumn = typeof ROW_COLUMNS[number];
