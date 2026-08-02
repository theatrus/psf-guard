import type { CSSProperties } from 'react';
import type { QualityRegionOverlay as QualityRegionOverlayData } from '../api/types';
import {
  QUALITY_REGION_FILL_SIGNALS,
  qualityRegionSignalCount,
} from '../utils/qualityRegionOverlay';

interface QualityRegionOverlayProps {
  overlay: QualityRegionOverlayData;
  className?: string;
  style?: CSSProperties;
}

export default function QualityRegionOverlay({
  overlay,
  className,
  style,
}: QualityRegionOverlayProps) {
  const cells = overlay.grid_cols * overlay.grid_rows;
  const affected = qualityRegionSignalCount(overlay);
  if (cells <= 0 || affected === 0) return null;

  return (
    <svg
      className={className}
      style={style}
      viewBox={`0 0 ${overlay.grid_cols} ${overlay.grid_rows}`}
      preserveAspectRatio="none"
      role="img"
      aria-label={`${affected} quality analysis region${affected === 1 ? '' : 's'}`}
    >
      {Array.from({ length: cells }, (_, index) => {
        const x = index % overlay.grid_cols;
        const y = Math.floor(index / overlay.grid_cols);
        const fillSignal = QUALITY_REGION_FILL_SIGNALS.find(
          ({ mask }) => overlay[mask][index],
        );
        const backgroundRise = overlay.background_rise_cells[index];
        const backgroundFall = overlay.background_fall_cells[index];
        if (!fillSignal && !backgroundRise && !backgroundFall) return null;

        return (
          <g key={index} data-cell-index={index}>
            {fillSignal && (
              <rect
                x={x}
                y={y}
                width="1"
                height="1"
                className={`quality-region-cell quality-region-${fillSignal.className}`}
              />
            )}
            {backgroundRise && (
              <rect
                x={x + 0.04}
                y={y + 0.04}
                width="0.92"
                height="0.92"
                className="quality-region-border quality-region-background-rise"
              />
            )}
            {backgroundFall && (
              <rect
                x={x + 0.1}
                y={y + 0.1}
                width="0.8"
                height="0.8"
                className="quality-region-border quality-region-background-fall"
              />
            )}
          </g>
        );
      })}
    </svg>
  );
}
