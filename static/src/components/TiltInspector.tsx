import { useMemo } from 'react';
import { useQuery } from '@tanstack/react-query';
import { apiClient } from '../api/client';
import type {
  StarDetectionResponse,
  TiltCornerName,
  TiltSummaryInfo,
} from '../api/types';
import { useColorPreview } from '../hooks/useColorPreview';
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

/** Where each corner's vertex points, on the unit square's diagonals. */
const CORNER_DIRECTIONS: Record<TiltCornerName, [number, number]> = {
  'top-left': [-1, -1],
  'top-right': [1, -1],
  'bottom-left': [-1, 1],
  'bottom-right': [1, 1],
};
/** Draw order that walks the quadrilateral's perimeter, not its diagonals. */
const CORNER_PERIMETER: TiltCornerName[] = [
  'top-left',
  'top-right',
  'bottom-right',
  'bottom-left',
];

/**
 * ASTAP's tilt figure: each corner's HFD becomes a vertex distance from
 * center, so a flat field draws a square and a tilted sensor a skewed
 * quadrilateral leaning toward its soft corner. The dashed reference square
 * is every corner at the mean — the shape a perfect sensor would draw.
 */
function TiltDiagram({ tilt }: { tilt: TiltSummaryInfo }) {
  const mean = tilt.mean_hfr;
  const corners = tilt.corners;
  if (mean === null || mean <= 0 || corners.some((corner) => corner.hfr === null)) {
    return null;
  }
  // Half-diagonal of the reference square in viewBox units; vertices scale
  // by hfr/mean around it, clamped so an extreme corner stays in frame.
  const base = 52;
  const point = (name: TiltCornerName, hfr: number): [number, number] => {
    const [dx, dy] = CORNER_DIRECTIONS[name];
    const radius = base * Math.min(1.6, Math.max(0.4, hfr / mean));
    return [dx * radius, dy * radius];
  };
  const measured = CORNER_PERIMETER.map((name) => {
    const corner = corners.find((candidate) => candidate.corner === name)!;
    return { name, hfr: corner.hfr!, point: point(name, corner.hfr!) };
  });
  const polygon = measured
    .map(({ point: [x, y] }) => `${x.toFixed(1)},${y.toFixed(1)}`)
    .join(' ');
  const reference = CORNER_PERIMETER.map((name) => {
    const [dx, dy] = CORNER_DIRECTIONS[name];
    return `${dx * base},${dy * base}`;
  }).join(' ');

  return (
    <svg
      className="tilt-diagram"
      viewBox="-100 -100 200 200"
      role="img"
      aria-label="Corner HFD tilt figure"
    >
      <polygon
        points={reference}
        fill="none"
        stroke="currentColor"
        strokeDasharray="4 4"
        opacity={0.35}
      />
      <polygon
        points={polygon}
        fill="var(--color-primary, #4a9eff)"
        fillOpacity={0.18}
        stroke="var(--color-primary, #4a9eff)"
        strokeWidth={1.5}
      />
      {measured.map(({ name, hfr, point: [x, y] }) => (
        <g key={name}>
          <circle
            cx={x}
            cy={y}
            r={3}
            fill={
              name === tilt.worst_corner
                ? 'var(--color-error)'
                : name === tilt.best_corner
                  ? 'var(--color-success)'
                  : 'currentColor'
            }
          />
          <text
            x={x * 1.28 + (x < 0 ? -2 : 2)}
            y={y * 1.28 + (y < 0 ? -2 : 8)}
            textAnchor={x < 0 ? 'start' : 'end'}
            className="tilt-diagram-label"
          >
            {hfr.toFixed(2)}
          </text>
        </g>
      ))}
    </svg>
  );
}

/**
 * Sensor tilt and aberration inspection for one frame: a 3x3 mosaic of 1:1
 * crops (the corner view PixInsight's aberration inspector gives),
 * per-region star statistics with elongation direction, ASTAP-style
 * corner-HFD tilt numbers, and ASTAP's tilt figure — the corner HFDs drawn
 * as a quadrilateral against the flat-field reference square. The region
 * statistics and verdict come from the server (seiza-stars), so every
 * consumer reads the same numbers this dialog shows.
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
  const cells = data?.cells && data.cells.length > 0 ? data.cells : null;
  const summary = data?.tilt ?? null;
  const bestCellHfr = useMemo(() => {
    if (!cells) return null;
    const values = cells
      .map((cell) => cell.median_hfr)
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
        {data && !cells && (
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
                    borderColor: hfrColor(cell.median_hfr, bestCellHfr),
                    ...paneStyle(cell.row, cell.col),
                  }}
                  title={cellLabel(cell.row, cell.col)}
                >
                  {cell.mean_theta !== null && cell.theta_coherence > 0.25 && (
                    <svg
                      className="tilt-direction"
                      viewBox="-100 -100 200 200"
                      aria-hidden="true"
                    >
                      <line
                        x1={-30 * Math.cos(cell.mean_theta)}
                        y1={-30 * Math.sin(cell.mean_theta)}
                        x2={30 * Math.cos(cell.mean_theta)}
                        y2={30 * Math.sin(cell.mean_theta)}
                        stroke="currentColor"
                        strokeWidth={1.5 + 2.5 * cell.theta_coherence}
                        strokeLinecap="round"
                        opacity={0.8}
                      />
                    </svg>
                  )}
                  <div className="tilt-pane-stats">
                    {cell.median_hfr !== null ? (
                      <>
                        <span>HFR {cell.median_hfr.toFixed(2)}</span>
                        <span>e {cell.median_eccentricity?.toFixed(2) ?? '—'}</span>
                        <span>{cell.star_count}★</span>
                      </>
                    ) : (
                      <span>no stars</span>
                    )}
                  </div>
                </div>
              ))}
            </div>

            <div className="tilt-verdict">
              <TiltDiagram tilt={summary} />
              <div className="tilt-summary">
                <div>
                  <strong>Tilt</strong>
                  <span>
                    {summary.tilt_percent !== null
                      ? `${summary.tilt_percent.toFixed(0)}% — softest ${summary.worst_corner}, sharpest ${summary.best_corner}`
                      : 'needs stars in all four corners'}
                  </span>
                </div>
                <div>
                  <strong>Field curvature</strong>
                  <span>
                    {summary.curvature_percent !== null
                      ? `corners ${summary.curvature_percent >= 0 ? '+' : ''}${summary.curvature_percent.toFixed(0)}% vs center`
                      : 'needs center and corner stars'}
                  </span>
                </div>
                <div>
                  <strong>Center HFR</strong>
                  <span>{summary.center_hfr?.toFixed(2) ?? '—'}</span>
                </div>
              </div>
            </div>

            <p className="tilt-inspector-muted">
              Panes crop the frame 1:1 at each region's center; borders color
              by softness against the sharpest region. A line through a pane
              is the region's mean star-elongation direction (thicker = more
              aligned). The tilt figure draws each corner's HFD as a vertex
              distance: a flat field is the dashed square, a tilted sensor
              leans toward its soft corner, and evenly bulging corners are
              field curvature. The same elongation direction in every region —
              center included — is guiding or wind, not optics. One frame's
              seeing can mimic any of these — confirm on several frames.
            </p>
          </>
        )}
      </div>
    </Dialog>
  );
}
