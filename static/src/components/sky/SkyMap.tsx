import { useMemo, useRef, useState, type MouseEvent } from 'react';
import type { SkyFrame } from '../../utils/skyProjection';
import {
  celestialEquatorPoints,
  eclipticPoints,
  footprintOutline,
  formatDecShort,
  formatRaShort,
  frameCenter,
  galacticBandQuads,
  galacticEquatorPoints,
  galacticToEquatorial,
  graticule,
  projectedPath,
  projectedPoint,
  type PathScale,
} from '../../utils/skyProjection';
import { blendFilterColors, filterColor } from '../../utils/filterColors';
import { formatHours, formatNight, type ShownTarget } from './skyModel';

export const MAP_WIDTH = 1000;
export const MAP_HEIGHT = 540;
const AT: PathScale = { cx: MAP_WIDTH / 2, cy: MAP_HEIGHT / 2, scale: 236 };

interface Props {
  targets: ShownTarget[];
  frame: SkyFrame;
  onOpen: (target: ShownTarget) => void;
}

/** A handful of faint fixed stars so an empty sky still reads as one. */
function backdropStars(count: number): Array<{ x: number; y: number; r: number }> {
  let seed = 7;
  const next = () => {
    seed = (seed * 1103515245 + 12345) % 2147483648;
    return seed / 2147483648;
  };
  const stars = [];
  while (stars.length < count) {
    const x = (next() * 4 - 2) * AT.scale + AT.cx;
    const y = (next() * 2 - 1) * AT.scale + AT.cy;
    const inside = ((x - AT.cx) / (2 * AT.scale)) ** 2 + ((y - AT.cy) / AT.scale) ** 2 <= 0.98;
    if (inside) stars.push({ x, y, r: 0.4 + next() * 0.9 });
  }
  return stars;
}

const STARS = backdropStars(260);

function opacityFor(seconds: number, maxSeconds: number): number {
  if (!(maxSeconds > 0)) return 0.6;
  const scaled = Math.log10(1 + seconds / 3600) / Math.log10(1 + maxSeconds / 3600);
  return 0.3 + 0.6 * Math.min(1, Math.max(0, scaled));
}

export default function SkyMap({ targets, frame, onOpen }: Props) {
  const [hovered, setHovered] = useState<ShownTarget | null>(null);
  const [pointer, setPointer] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const wrapper = useRef<HTMLDivElement>(null);

  const scenery = useMemo(() => {
    const center = frameCenter(frame);
    const grid = graticule(center);
    const framePaths = {
      meridians: grid.meridians.map((line) =>
        projectedPath(
          frame === 'galactic' ? line.map(([l, b]) => galacticToEquatorial(l, b)) : line,
          frame,
          AT
        )
      ),
      parallels: grid.parallels.map((line) =>
        projectedPath(
          frame === 'galactic' ? line.map(([l, b]) => galacticToEquatorial(l, b)) : line,
          frame,
          AT
        )
      ),
    };
    const band = galacticBandQuads(12).map((quad) => projectedPath(quad, frame, AT, true)).filter((path) => !path.includes('M', 1));
    const core = galacticBandQuads(5).map((quad) => projectedPath(quad, frame, AT, true)).filter((path) => !path.includes('M', 1));
    const ecliptic = projectedPath(eclipticPoints(), frame, AT);
    const otherEquator =
      frame === 'galactic'
        ? projectedPath(celestialEquatorPoints(), frame, AT)
        : projectedPath(galacticEquatorPoints(), frame, AT);
    const lonLabels: Array<{ x: number; y: number; text: string }> = [];
    for (let lon = 0; lon < 360; lon += 30) {
      if (lon === (center + 180) % 360) continue;
      const icrs = frame === 'galactic' ? galacticToEquatorial(lon, 0) : ([lon, 0] as [number, number]);
      const at = projectedPoint(icrs[0], icrs[1], frame, AT);
      lonLabels.push({ x: at.x, y: at.y - 4, text: frame === 'galactic' ? `${lon}°` : `${lon / 15}h` });
    }
    const latLabels: Array<{ x: number; y: number; text: string }> = [];
    for (const lat of [-60, -30, 30, 60]) {
      const icrs = frame === 'galactic' ? galacticToEquatorial(center, lat) : ([center, lat] as [number, number]);
      const at = projectedPoint(icrs[0], icrs[1], frame, AT);
      latLabels.push({ x: at.x + 6, y: at.y - 3, text: `${lat > 0 ? '+' : '−'}${Math.abs(lat)}°` });
    }
    return { ...framePaths, band, core, ecliptic, otherEquator, lonLabels, latLabels };
  }, [frame]);

  const maxSeconds = useMemo(() => targets.reduce((max, item) => Math.max(max, item.seconds), 0), [targets]);

  const drawn = useMemo(
    () =>
      targets
        .filter((item) => item.target.ra_deg != null && item.target.dec_deg != null)
        .map((item) => {
          const ra = item.target.ra_deg as number;
          const dec = item.target.dec_deg as number;
          const fill = blendFilterColors(item.byFilter.map(({ filter, seconds }) => ({ filter, weight: seconds })));
          const opacity = opacityFor(item.seconds, maxSeconds);
          if (item.target.footprint) {
            const outline = footprintOutline(ra, dec, item.target.footprint);
            return { item, kind: 'field' as const, path: projectedPath(outline, frame, AT, true), fill, opacity, point: null };
          }
          return { item, kind: 'point' as const, path: '', fill, opacity, point: projectedPoint(ra, dec, frame, AT) };
        }),
    [targets, frame, maxSeconds]
  );

  const track = (event: MouseEvent) => {
    const box = wrapper.current?.getBoundingClientRect();
    if (!box) return;
    setPointer({ x: event.clientX - box.left, y: event.clientY - box.top });
  };

  return (
    <div className="sky-map-wrapper" ref={wrapper} onMouseMove={track}>
      <svg
        className="sky-map"
        viewBox={`0 0 ${MAP_WIDTH} ${MAP_HEIGHT}`}
        role="img"
        aria-label={`Sky coverage map in ${frame} coordinates with ${drawn.length} targets`}
      >
        <defs>
          <radialGradient id="sky-ground" cx="50%" cy="50%" r="60%">
            <stop offset="0%" stopColor="#141c33" />
            <stop offset="100%" stopColor="#090c18" />
          </radialGradient>
        </defs>
        <ellipse className="sky-globe" cx={AT.cx} cy={AT.cy} rx={2 * AT.scale} ry={AT.scale} fill="url(#sky-ground)" />
        <g className="sky-stars">
          {STARS.map((star, index) => (
            <circle key={index} cx={star.x} cy={star.y} r={star.r} />
          ))}
        </g>
        <g className="sky-milky-way">
          {scenery.band.map((path, index) => (
            <path key={`b${index}`} d={path} />
          ))}
          {scenery.core.map((path, index) => (
            <path key={`c${index}`} d={path} />
          ))}
        </g>
        <g className="sky-graticule">
          {scenery.meridians.map((path, index) => (
            <path key={`m${index}`} d={path} />
          ))}
          {scenery.parallels.map((path, index) => (
            <path key={`p${index}`} d={path} />
          ))}
        </g>
        <path className="sky-ecliptic" d={scenery.ecliptic} />
        <path className="sky-other-equator" d={scenery.otherEquator} />
        <g className="sky-labels">
          {scenery.lonLabels.map((label) => (
            <text key={label.text} x={label.x} y={label.y} textAnchor="middle">
              {label.text}
            </text>
          ))}
          {scenery.latLabels.map((label) => (
            <text key={label.text} x={label.x} y={label.y}>
              {label.text}
            </text>
          ))}
        </g>
        <g className="sky-targets">
          {drawn.map(({ item, kind, path, fill, opacity, point }) => {
            const active = hovered?.target.key === item.target.key;
            const shared = {
              className: `sky-target${active ? ' is-active' : ''}`,
              'data-target': item.target.name,
              onMouseEnter: () => setHovered(item),
              onMouseLeave: () => setHovered((current) => (current?.target.key === item.target.key ? null : current)),
              onClick: () => onOpen(item),
              tabIndex: 0,
              onFocus: () => setHovered(item),
              onBlur: () => setHovered(null),
              onKeyDown: (event: { key: string }) => {
                if (event.key === 'Enter') onOpen(item);
              },
            };
            if (kind === 'field') {
              return <path key={item.target.key} d={path} fill={fill} fillOpacity={opacity} stroke={fill} {...shared} />;
            }
            return (
              <circle
                key={item.target.key}
                cx={point?.x}
                cy={point?.y}
                r={active ? 5 : 3.5}
                fill={fill}
                fillOpacity={Math.min(1, opacity + 0.2)}
                stroke={fill}
                {...shared}
              />
            );
          })}
        </g>
      </svg>
      {hovered && (
        <TargetCard item={hovered} x={pointer.x} y={pointer.y} width={wrapper.current?.clientWidth ?? 0} />
      )}
    </div>
  );
}

function TargetCard({ item, x, y, width }: { item: ShownTarget; x: number; y: number; width: number }) {
  const { target } = item;
  const total = item.byFilter.reduce((sum, entry) => sum + entry.seconds, 0);
  const flip = width > 0 && x > width * 0.6;
  const style = flip ? { right: width - x + 14, top: y + 14 } : { left: x + 14, top: y + 14 };
  const where =
    target.ra_deg != null && target.dec_deg != null
      ? `${formatRaShort(target.ra_deg)}  ${formatDecShort(target.dec_deg)}`
      : 'no coordinates';
  const field = target.footprint
    ? `${target.footprint.width_deg.toFixed(2)}° × ${target.footprint.height_deg.toFixed(2)}° field, ${
        target.footprint.source === 'solved' ? 'from a plate solve' : 'from the frame header'
      }`
    : 'field size unknown';
  return (
    <div className="sky-card" style={style} role="tooltip">
      <div className="sky-card-title">{target.name}</div>
      <div className="sky-card-sub">
        {target.db_name} · {target.project_name}
      </div>
      <div className="sky-card-where">{where}</div>
      <div className="sky-card-total">
        <strong>{formatHours(item.seconds / 3600)}</strong> in {item.frames.toLocaleString()} frames over {item.nights}{' '}
        {item.nights === 1 ? 'night' : 'nights'}
      </div>
      <div className="sky-card-bars">
        {item.byFilter.map(({ filter, seconds }) => (
          <div className="sky-card-bar" key={filter}>
            <span className="sky-card-bar-name">{filter}</span>
            <span className="sky-card-bar-track">
              <span
                className="sky-card-bar-fill"
                style={{ width: `${total > 0 ? (100 * seconds) / total : 0}%`, background: filterColor(filter) }}
              />
            </span>
            <span className="sky-card-bar-value">{formatHours(seconds / 3600)}</span>
          </div>
        ))}
      </div>
      <div className="sky-card-span">
        {formatNight(item.firstNight)}
        {item.firstNight !== item.lastNight ? ` → ${formatNight(item.lastNight)}` : ''}
      </div>
      <div className="sky-card-field">{field}</div>
      <div className="sky-card-hint">Click to open in Images</div>
    </div>
  );
}
