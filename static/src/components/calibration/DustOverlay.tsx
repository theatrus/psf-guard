import type { CSSProperties, KeyboardEvent } from 'react';
import type { VignettingEdge } from '../../api/types';
import type { DustMark, VignettingMark } from './dustLabels';
import { isLarge, isRound } from './dustLabels';

/** Labels drawn at most: the largest and deepest first. */
const MAX_LABELS = 12;
/** Label text, in CSS pixels whatever the zoom. */
const LABEL_PX = 12;
/** Below this drawn width, labels shorten to the depth. */
const NARROW_PX = 640;
/** A compact spot's box is widened by this many CSS pixels so it is seen. */
const SPOT_PAD_PX = 3;

/**
 * Where a vignetting shadow lies, from the edge it hangs from and the area it
 * covers: a quarter disc in its corner, or a band along its edge. A shadow
 * hangs from the border, so it is drawn there rather than as a circle round
 * its centroid, which reaches into the frame where it is not.
 */
function shadowShape(mark: VignettingMark, width: number, height: number) {
  const corners: Partial<Record<VignettingEdge, [number, number, number, number]>> = {
    top_left: [0, 0, 1, 1],
    top_right: [width, 0, -1, 1],
    bottom_left: [0, height, 1, -1],
    bottom_right: [width, height, -1, -1],
  };
  const corner = corners[mark.edge];
  if (corner) {
    const [cx, cy, sx, sy] = corner;
    const r = Math.min(Math.sqrt((4 * mark.area) / Math.PI), width, height);
    const sweep = sx * sy > 0 ? 1 : 0;
    return {
      d: `M ${cx} ${cy} L ${cx + sx * r} ${cy} A ${r} ${r} 0 0 ${sweep} ${cx} ${cy + sy * r} Z`,
      label: { x: cx + sx * r * 0.5, y: cy + sy * r * 0.5 },
    };
  }
  const along = mark.edge === 'top' || mark.edge === 'bottom' ? width : height;
  const depth = Math.min(Math.max(mark.area / Math.max(along, 1), 1), along === width ? height : width);
  const bands: Partial<Record<VignettingEdge, [number, number, number, number]>> = {
    top: [0, 0, width, depth],
    bottom: [0, height - depth, width, depth],
    left: [0, 0, depth, height],
    right: [width - depth, 0, depth, height],
  };
  const band = bands[mark.edge] ?? [0, 0, 0, 0];
  const [x, y, w, h] = band;
  // The band's own place along the edge is where its centroid lies.
  return {
    d: `M ${x} ${y} h ${w} v ${h} h ${-w} Z`,
    label: {
      x: mark.edge === 'top' || mark.edge === 'bottom' ? mark.x : x + w / 2,
      y: mark.edge === 'left' || mark.edge === 'right' ? mark.y : y + h / 2,
    },
  };
}

interface DustOverlayProps {
  /** The sensor's size: marks are in its pixels. */
  width: number;
  height: number;
  marks: DustMark[];
  vignetting: VignettingMark[];
  /** CSS pixels per sensor pixel as drawn, so labels and strokes keep their
   *  size at every zoom. */
  pixelScale: number;
  picked: string | null;
  onPick: (key: string | null) => void;
  className: string;
  /** The drawn size, when the overlay is positioned by its parent. */
  svgWidth?: number;
  svgHeight?: number;
  style?: CSSProperties;
  testId?: string;
}

/**
 * Dust over a flat at its true shape and size: a disk's rim, a donut's ring
 * and hole, a spot's or a patch's box, a vignetting shadow at its edge. Large marks carry a label; any mark tapped or focused says its
 * numbers through `onPick`.
 */
export default function DustOverlay({
  width,
  height,
  marks,
  vignetting,
  pixelScale,
  picked,
  onPick,
  className,
  svgWidth,
  svgHeight,
  style,
  testId,
}: DustOverlayProps) {
  const unit = 1 / Math.max(pixelScale, 1e-6);
  const labelled = new Set(
    [...marks]
      // A move within focus travel is drawn but not counted, and left unlabelled.
      .filter((mark) => mark.tone !== 'tolerance')
      .filter((mark) => isLarge(mark.shape) || mark.tone === 'new' || mark.tone === 'gone' || mark.tone === 'moved')
      .sort((a, b) => b.radius - a.radius)
      .slice(0, MAX_LABELS)
      .map((mark) => mark.key)
  );
  const pick = (key: string) => onPick(picked === key ? null : key);
  const keys = (key: string) => (event: KeyboardEvent) => {
    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      pick(key);
    }
  };
  const fontSize = LABEL_PX * unit;
  // A label's start beside a mark, on the side toward the frame's middle.
  const toward = (x: number, reach: number, gap: number) =>
    x < width / 2 ? x + reach + gap : x - reach - gap;
  // Drawn narrow, as on a phone, a mark says only its depth: a tap gives
  // the rest.
  const narrow = width * pixelScale < NARROW_PX;

  return (
    <svg
      className={`dust-overlay ${className}`}
      width={svgWidth}
      height={svgHeight}
      viewBox={`0 0 ${width} ${height}`}
      preserveAspectRatio="none"
      style={style}
      role="group"
      aria-label={`${marks.length + vignetting.length} dust marks`}
      data-testid={testId}
    >
      {vignetting.map((mark) => {
        const shape = shadowShape(mark, width, height);
        return (
          <g
            key={mark.key}
            className={`dust-mark is-vignetting`}
            role="button"
            tabIndex={0}
            aria-label={mark.detail}
            aria-pressed={picked === mark.key}
            onClick={(event) => {
              event.stopPropagation();
              pick(mark.key);
            }}
            onKeyDown={keys(mark.key)}
            data-shape="vignetting"
          >
            <title>{mark.detail}</title>
            <path d={shape.d} vectorEffect="non-scaling-stroke" />
            <text
              x={shape.label.x}
              y={shape.label.y}
              fontSize={fontSize}
              strokeWidth={3 * unit}
              // Toward the middle of the frame, so a corner's label stays on it.
              textAnchor={mark.x < width / 2 ? 'start' : 'end'}
            >
              {narrow ? mark.short : mark.label}
            </text>
          </g>
        );
      })}
      {marks.map((mark) => {
        const pad = SPOT_PAD_PX * unit;
        const [left, top, right, bottom] = mark.bounds;
        const large = isRound(mark.shape);
        return (
          <g
            key={mark.key}
            className={`dust-mark is-${mark.tone}${mark.unseen ? ' is-unseen' : ''}`}
            role="button"
            tabIndex={0}
            aria-label={mark.detail}
            aria-pressed={picked === mark.key}
            onClick={(event) => {
              event.stopPropagation();
              pick(mark.key);
            }}
            onKeyDown={keys(mark.key)}
            data-shape={mark.shape}
          >
            <title>{mark.detail}</title>
            {large ? (
              <circle cx={mark.x} cy={mark.y} r={mark.radius} vectorEffect="non-scaling-stroke" />
            ) : (
              <rect
                x={left - pad}
                y={top - pad}
                width={right - left + 1 + 2 * pad}
                height={bottom - top + 1 + 2 * pad}
                vectorEffect="non-scaling-stroke"
              />
            )}
            {mark.shape === 'donut' && mark.hole ? (
              <circle
                className="dust-hole"
                cx={mark.x}
                cy={mark.y}
                r={mark.radius * mark.hole}
                vectorEffect="non-scaling-stroke"
              />
            ) : null}
            {labelled.has(mark.key) && (
              <text
                // Toward the middle of the frame, so a label near the right or
                // top edge stays on it.
                x={toward(mark.x, large ? mark.radius * 0.72 : (right - left) / 2 + pad, 4 * unit)}
                y={Math.max(mark.y - (large ? mark.radius * 0.72 : 0), fontSize)}
                textAnchor={mark.x < width / 2 ? 'start' : 'end'}
                fontSize={fontSize}
                strokeWidth={3 * unit}
              >
                {narrow ? mark.short : mark.label}
              </text>
            )}
          </g>
        );
      })}
    </svg>
  );
}
