import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type { StarDetectionResponse } from '../api/types';
import { useColorPreview } from '../hooks/useColorPreview';
import { analyzeCells, tiltSummary, type CellStats } from '../utils/tiltAnalysis';
import Dialog from './Dialog';
import './TiltInspector.css';

interface TiltInspectorProps {
  open: boolean;
  dbId: string;
  imageId: number;
  onClose: () => void;
}

/** Side of one mosaic pane, in CSS pixels. */
const PANE_SIZE = 200;
/** Fraction of the frame's short side each pane crops at 1:1 scale. */
const CROP_FRACTION = 1 / 6;

function cellLabel(row: number, col: number): string {
  const vertical = ['Top', 'Middle', 'Bottom'][row];
  const horizontal = ['left', 'center', 'right'][col];
  return row === 1 && col === 1 ? 'Center' : `${vertical} ${horizontal}`;
}

/** Color a cell by how much softer it is than the sharpest cell. */
function hfrColor(hfr: number | null, bestHfr: number | null): string {
  if (hfr === null || bestHfr === null || bestHfr <= 0) {
    return 'var(--color-border, #555)';
  }
  const excess = hfr / bestHfr - 1;
  if (excess < 0.1) return 'var(--color-success)';
  if (excess < 0.25) return 'var(--color-warning)';
  return 'var(--color-error)';
}

/**
 * Sensor tilt and aberration inspection for one frame: a 3x3 mosaic of 1:1
 * crops (the corner view PixInsight's aberration inspector gives),
 * per-region star statistics with elongation direction, and ASTAP-style
 * corner-HFD tilt numbers. A tilted sensor focuses one side of the frame
 * ahead of the other, so its signature is a soft corner opposite a sharp
 * one; evenly soft corners are field curvature instead.
 */
export default function TiltInspector({
  open,
  dbId,
  imageId,
  onClose,
}: TiltInspectorProps) {
  const color = useColorPreview();
  const detection = useQuery<StarDetectionResponse>({
    queryKey: ['db', dbId, 'stars', imageId],
    queryFn: () => apiClient.getStarDetection(dbId, imageId),
    enabled: open,
    staleTime: Infinity,
  });

  const data = detection.data;
  const width = data?.width ?? null;
  const height = data?.height ?? null;
  const cells = useMemo<CellStats[] | null>(() => {
    if (!data || width === null || height === null) return null;
    return analyzeCells(data.stars, width, height);
  }, [data, width, height]);
  const summary = useMemo(() => (cells ? tiltSummary(cells) : null), [cells]);
  const bestCellHfr = useMemo(() => {
    if (!cells) return null;
    const values = cells
      .map((cell) => cell.medianHfr)
      .filter((hfr): hfr is number => hfr !== null);
    return values.length > 0 ? Math.min(...values) : null;
  }, [cells]);

  // 1:1 crops from the "large" preview via background positioning. The
  // preview is capped at 2000px on the long side, so scale accordingly.
  const previewUrl = apiClient.getPreviewUrl(dbId, imageId, {
    size: 'large',
    color,
  });
  const paneStyle = (row: number, col: number): React.CSSProperties => {
    if (width === null || height === null) return {};
    const previewScale = Math.min(1, 2000 / Math.max(width, height));
    const previewWidth = width * previewScale;
    const previewHeight = height * previewScale;
    const cropSide = Math.min(width, height) * CROP_FRACTION * previewScale;
    const scale = PANE_SIZE / cropSide;
    const centerX = ((col + 0.5) * previewWidth) / 3;
    const centerY = ((row + 0.5) * previewHeight) / 3;
    return {
      backgroundImage: `url(${JSON.stringify(previewUrl)})`,
      backgroundSize: `${previewWidth * scale}px ${previewHeight * scale}px`,
      backgroundPosition: `${-(centerX * scale - PANE_SIZE / 2)}px ${-(
        centerY * scale -
        PANE_SIZE / 2
      )}px`,
    };
  };

  return (
    <Dialog open={open} onClose={onClose} title="Sensor tilt and aberration inspection">
      <div className="tilt-inspector">
        {detection.isLoading && (
          <p className="tilt-inspector-muted">Detecting stars…</p>
        )}
        {detection.error && (
          <p className="tilt-inspector-error">
            {detection.error instanceof Error
              ? detection.error.message
              : String(detection.error)}
          </p>
        )}
        {data && (width === null || height === null) && (
          <p className="tilt-inspector-muted">
            This frame's star analysis predates region support. Run Analyze
            Quality again, or reopen after the star cache refreshes.
          </p>
        )}
        {cells && summary && (
          <>
            <div className="tilt-mosaic">
              {cells.map((cell) => (
                <div
                  key={`${cell.row}:${cell.col}`}
                  className="tilt-pane"
                  style={{
                    width: PANE_SIZE,
                    height: PANE_SIZE,
                    borderColor: hfrColor(cell.medianHfr, bestCellHfr),
                    ...paneStyle(cell.row, cell.col),
                  }}
                  title={cellLabel(cell.row, cell.col)}
                >
                  {cell.meanTheta !== null && cell.thetaCoherence > 0.25 && (
                    <svg
                      className="tilt-direction"
                      viewBox="-100 -100 200 200"
                      aria-hidden="true"
                    >
                      <line
                        x1={-30 * Math.cos(cell.meanTheta)}
                        y1={-30 * Math.sin(cell.meanTheta)}
                        x2={30 * Math.cos(cell.meanTheta)}
                        y2={30 * Math.sin(cell.meanTheta)}
                        stroke="currentColor"
                        strokeWidth={1.5 + 2.5 * cell.thetaCoherence}
                        strokeLinecap="round"
                        opacity={0.8}
                      />
                    </svg>
                  )}
                  <div className="tilt-pane-stats">
                    {cell.medianHfr !== null ? (
                      <>
                        <span>HFR {cell.medianHfr.toFixed(2)}</span>
                        <span>e {cell.medianEccentricity?.toFixed(2) ?? '—'}</span>
                        <span>{cell.starCount}★</span>
                      </>
                    ) : (
                      <span>no stars</span>
                    )}
                  </div>
                </div>
              ))}
            </div>

            <div className="tilt-summary">
              <div>
                <strong>Tilt</strong>
                <span>
                  {summary.tiltPercent !== null
                    ? `${summary.tiltPercent.toFixed(0)}% — softest ${summary.worstCorner}, sharpest ${summary.bestCorner}`
                    : 'needs stars in all four corners'}
                </span>
              </div>
              <div>
                <strong>Field curvature</strong>
                <span>
                  {summary.curvaturePercent !== null
                    ? `corners ${summary.curvaturePercent >= 0 ? '+' : ''}${summary.curvaturePercent.toFixed(0)}% vs center`
                    : 'needs center and corner stars'}
                </span>
              </div>
              <div>
                <strong>Center HFR</strong>
                <span>{summary.centerHfr?.toFixed(2) ?? '—'}</span>
              </div>
            </div>

            <p className="tilt-inspector-muted">
              Panes crop the frame 1:1 at each region's center; borders color
              by softness against the sharpest region. A line through a pane
              is the region's mean star-elongation direction (thicker = more
              aligned). The same direction in every region — center included —
              is guiding or wind, not optics; directions that only align near
              the edges point at sensor tilt or astigmatism; soft corners in
              every direction with a sharp center indicate field curvature.
              One frame's seeing can mimic any of these — confirm on several
              frames.
            </p>
          </>
        )}
      </div>
    </Dialog>
  );
}
