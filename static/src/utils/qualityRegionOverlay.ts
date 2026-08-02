import type { QualityRegionOverlay } from '../api/types';

export type QualityRegionMaskName = keyof Pick<
  QualityRegionOverlay,
  | 'low_star_cells'
  | 'extinction_cells'
  | 'star_loss_cells'
  | 'background_rise_cells'
  | 'background_fall_cells'
  | 'glow_cells'
>;

export const QUALITY_REGION_SIGNALS: ReadonlyArray<{
  mask: QualityRegionMaskName;
  label: string;
  className: string;
}> = [
  { mask: 'low_star_cells', label: 'Low star coverage', className: 'low-stars' },
  { mask: 'extinction_cells', label: 'Localized dimming', className: 'extinction' },
  { mask: 'star_loss_cells', label: 'Transient star loss', className: 'star-loss' },
  { mask: 'background_rise_cells', label: 'Background rise', className: 'background-rise' },
  { mask: 'background_fall_cells', label: 'Background fall', className: 'background-fall' },
  { mask: 'glow_cells', label: 'Localized glow', className: 'glow' },
];

export const QUALITY_REGION_FILL_SIGNALS = QUALITY_REGION_SIGNALS.filter(
  ({ mask }) => mask !== 'background_rise_cells' && mask !== 'background_fall_cells',
);

export function qualityRegionSignalCount(overlay: QualityRegionOverlay): number {
  const cells = overlay.grid_cols * overlay.grid_rows;
  let count = 0;
  for (let index = 0; index < cells; index += 1) {
    if (QUALITY_REGION_SIGNALS.some(({ mask }) => overlay[mask][index])) count += 1;
  }
  return count;
}

export function activeQualityRegionSignals(overlay: QualityRegionOverlay) {
  return QUALITY_REGION_SIGNALS.filter(({ mask }) => overlay[mask].some(Boolean));
}
