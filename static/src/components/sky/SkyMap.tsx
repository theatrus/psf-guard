import { useEffect, useMemo, useRef, useState, type MouseEvent, type PointerEvent } from 'react';
import { Minus, Plus, RotateCcw } from 'lucide-react';
import type { SkyFrame, SkyMode, SkyView } from '../../utils/skyProjection';
import {
  celestialEquatorPoints,
  defaultView,
  eclipticPoints,
  footprintOutline,
  formatDecShort,
  formatRaShort,
  galacticBandQuads,
  galacticEquatorPoints,
  galacticToEquatorial,
  graticule,
  projectedPath,
  projectedPoint,
  tanPixelToSky,
  wrap360,
  type LonLat,
  type PathScale,
} from '../../utils/skyProjection';
import { blendFilterColors, filterColor } from '../../utils/filterColors';
import { BRIGHT_STARS, CONSTELLATION_LINES, CONSTELLATION_NAMES } from '../../data/skyBackdrop';
import { formatHours, formatNight, type ShownTarget } from './skyModel';

export const MAP_WIDTH = 1000;
export const MAP_HEIGHT = 540;
const AT: PathScale = { cx: MAP_WIDTH / 2, cy: MAP_HEIGHT / 2, scale: 236 };
const MAX_ZOOM = 60;
/** Stack previews are drawn once a field is big enough to show one. */
const STACKS_FROM_ZOOM = 3;

interface Props {
  targets: ShownTarget[];
  frame: SkyFrame;
  mode: SkyMode;
  showBackdrop: boolean;
  showStacks: boolean;
  onOpen: (target: ShownTarget) => void;
}

interface View {
  x: number;
  y: number;
  k: number;
}

const HOME: View = { x: 0, y: 0, k: 1 };

function opacityFor(seconds: number, maxSeconds: number): number {
  if (!(maxSeconds > 0)) return 0.6;
  const scaled = Math.log10(1 + seconds / 3600) / Math.log10(1 + maxSeconds / 3600);
  return 0.3 + 0.6 * Math.min(1, Math.max(0, scaled));
}

function clampView(view: View): View {
  const k = Math.min(MAX_ZOOM, Math.max(1, view.k));
  const width = MAP_WIDTH / k;
  const height = MAP_HEIGHT / k;
  return {
    k,
    x: Math.min(MAP_WIDTH - width, Math.max(0, view.x)),
    y: Math.min(MAP_HEIGHT - height, Math.max(0, view.y)),
  };
}

/** The four sky corners of a target's image, image top-left first, clockwise. */
function imageCorners(item: ShownTarget): LonLat[] | null {
  const { target } = item;
  const preview = target.preview;
  if (!preview || target.ra_deg == null || target.dec_deg == null) return null;
  if (preview.wcs) {
    const { width: w, height: h } = preview;
    return [
      tanPixelToSky(preview.wcs, 0, 0),
      tanPixelToSky(preview.wcs, w, 0),
      tanPixelToSky(preview.wcs, w, h),
      tanPixelToSky(preview.wcs, 0, h),
    ];
  }
  if (!target.footprint) return null;
  // North up, east left, turned by the planned rotation: image left is east.
  const corners = footprintOutline(target.ra_deg, target.dec_deg, { ...target.footprint, vertices: null }, 1);
  // footprintOutline's rectangle starts at (west, south) and runs (east, south), (east, north), (west, north).
  const [ws, es, en, wn] = corners;
  return [en, wn, ws, es];
}

/** SVG matrix mapping preview pixels onto the map, fitted to three corners. */
function imageMatrix(corners: LonLat[], width: number, height: number, view: SkyView): string | null {
  const projected = corners.map(([ra, dec]) => projectedPoint(ra, dec, view, AT));
  if (projected.some((point) => !point.visible)) return null;
  const [p0, p1, , p3] = projected;
  const a = (p1.x - p0.x) / width;
  const b = (p1.y - p0.y) / width;
  const c = (p3.x - p0.x) / height;
  const d = (p3.y - p0.y) / height;
  if (![a, b, c, d, p0.x, p0.y].every(Number.isFinite)) return null;
  // A field across the seam would stretch over the whole map.
  if (Math.abs(a * width) > MAP_WIDTH / 3 || Math.abs(c * height) > MAP_WIDTH / 3) return null;
  return `matrix(${a} ${b} ${c} ${d} ${p0.x} ${p0.y})`;
}

export default function SkyMap({ targets, frame, mode, showBackdrop, showStacks, onOpen }: Props) {
  const [hovered, setHovered] = useState<ShownTarget | null>(null);
  const [pointer, setPointer] = useState<{ x: number; y: number }>({ x: 0, y: 0 });
  const [view, setView] = useState<View>(HOME);
  const [center, setCenter] = useState<{ lon: number; lat: number }>(() => {
    const initial = defaultView(frame, mode);
    return { lon: initial.centerLon, lat: initial.centerLat };
  });
  const wrapper = useRef<HTMLDivElement>(null);
  const svgRef = useRef<SVGSVGElement>(null);
  const drag = useRef<{ x: number; y: number; view: View; center: { lon: number; lat: number }; moved: boolean } | null>(
    null
  );

  const sky: SkyView = useMemo(
    () => ({ frame, mode, centerLon: center.lon, centerLat: center.lat }),
    [frame, mode, center]
  );

  useEffect(() => {
    setView(HOME);
    const initial = defaultView(frame, mode);
    setCenter({ lon: initial.centerLon, lat: initial.centerLat });
  }, [frame, mode]);

  const home = () => {
    setView(HOME);
    const initial = defaultView(frame, mode);
    setCenter({ lon: initial.centerLon, lat: initial.centerLat });
  };

  // React registers wheel listeners passively; zooming must swallow the scroll.
  useEffect(() => {
    const svg = svgRef.current;
    if (!svg) return;
    const onWheel = (event: WheelEvent) => {
      event.preventDefault();
      const rect = svg.getBoundingClientRect();
      setView((current) => {
        const k = Math.min(MAX_ZOOM, Math.max(1, current.k * Math.exp(-event.deltaY * 0.0016)));
        const px = (event.clientX - rect.left) / rect.width;
        const py = (event.clientY - rect.top) / rect.height;
        const sx = current.x + px * (MAP_WIDTH / current.k);
        const sy = current.y + py * (MAP_HEIGHT / current.k);
        return clampView({ k, x: sx - px * (MAP_WIDTH / k), y: sy - py * (MAP_HEIGHT / k) });
      });
    };
    svg.addEventListener('wheel', onWheel, { passive: false });
    return () => svg.removeEventListener('wheel', onWheel);
  }, []);

  const zoomBy = (factor: number) =>
    setView((current) => {
      const k = Math.min(MAX_ZOOM, Math.max(1, current.k * factor));
      const cx = current.x + MAP_WIDTH / current.k / 2;
      const cy = current.y + MAP_HEIGHT / current.k / 2;
      return clampView({ k, x: cx - MAP_WIDTH / k / 2, y: cy - MAP_HEIGHT / k / 2 });
    });

  const onPointerDown = (event: PointerEvent<SVGSVGElement>) => {
    if (event.button !== 0) return;
    drag.current = { x: event.clientX, y: event.clientY, view, center, moved: false };
  };
  const onPointerMove = (event: PointerEvent<SVGSVGElement>) => {
    const start = drag.current;
    if (!start) return;
    const rect = event.currentTarget.getBoundingClientRect();
    const dx = event.clientX - start.x;
    const dy = event.clientY - start.y;
    if (!start.moved && Math.hypot(dx, dy) < 4) return;
    if (!start.moved) {
      // Capture only once this is a drag, so a plain click still reaches
      // the field under the pointer.
      event.currentTarget.setPointerCapture(event.pointerId);
    }
    start.moved = true;
    // On the globe a drag turns it; on the flat map at whole-sky zoom a drag
    // spins the central meridian; zoomed in, it pans the view.
    if (mode === 'globe') {
      setCenter({
        lon: wrap360(start.center.lon + (dx / rect.width) * (200 / start.view.k)),
        lat: Math.max(-90, Math.min(90, start.center.lat + (dy / rect.height) * (120 / start.view.k))),
      });
      return;
    }
    if (start.view.k <= 1) {
      setCenter({ lon: wrap360(start.center.lon + (dx / rect.width) * 360), lat: start.center.lat });
      return;
    }
    setView(
      clampView({
        k: start.view.k,
        x: start.view.x - (dx / rect.width) * (MAP_WIDTH / start.view.k),
        y: start.view.y - (dy / rect.height) * (MAP_HEIGHT / start.view.k),
      })
    );
  };
  const onPointerUp = (event: PointerEvent<SVGSVGElement>) => {
    if (drag.current && event.currentTarget.hasPointerCapture(event.pointerId)) {
      event.currentTarget.releasePointerCapture(event.pointerId);
    }
    // Leave `moved` for the click that follows, then forget the drag.
    window.setTimeout(() => {
      drag.current = null;
    }, 0);
  };
  const open = (item: ShownTarget) => {
    if (drag.current?.moved) return;
    onOpen(item);
  };

  const scenery = useMemo(() => {
    const { frame, centerLon } = sky;
    const inFrame = (line: LonLat[]) => (frame === 'galactic' ? line.map(([l, b]) => galacticToEquatorial(l, b)) : line);
    const grid = graticule(centerLon);
    const meridians = grid.meridians.map((line) => projectedPath(inFrame(line), sky, AT));
    const parallels = grid.parallels.map((line) => projectedPath(inFrame(line), sky, AT));
    const band = galacticBandQuads(12).map((quad) => projectedPath(quad, sky, AT, true)).filter((path) => path && !path.includes('M', 1));
    const core = galacticBandQuads(5).map((quad) => projectedPath(quad, sky, AT, true)).filter((path) => path && !path.includes('M', 1));
    const ecliptic = projectedPath(eclipticPoints(), sky, AT);
    const otherEquator =
      frame === 'galactic' ? projectedPath(celestialEquatorPoints(), sky, AT) : projectedPath(galacticEquatorPoints(), sky, AT);
    const lonLabels: Array<{ x: number; y: number; text: string }> = [];
    for (let lon = 0; lon < 360; lon += 30) {
      if (sky.mode === 'aitoff' && Math.abs(wrap360(lon - centerLon) - 180) < 1) continue;
      const icrs = frame === 'galactic' ? galacticToEquatorial(lon, 0) : ([lon, 0] as LonLat);
      const at = projectedPoint(icrs[0], icrs[1], sky, AT);
      if (!at.visible) continue;
      lonLabels.push({ x: at.x, y: at.y - 4, text: frame === 'galactic' ? `${lon}°` : `${lon / 15}h` });
    }
    const latLabels: Array<{ x: number; y: number; text: string }> = [];
    for (const lat of [-60, -30, 30, 60]) {
      const icrs = frame === 'galactic' ? galacticToEquatorial(centerLon, lat) : ([centerLon, lat] as LonLat);
      const at = projectedPoint(icrs[0], icrs[1], sky, AT);
      if (!at.visible) continue;
      latLabels.push({ x: at.x + 6, y: at.y - 3, text: `${lat > 0 ? '+' : '−'}${Math.abs(lat)}°` });
    }
    const constellations = Object.values(CONSTELLATION_LINES).map((figure) =>
      figure.map((polyline) => projectedPath(polyline, sky, AT)).join('')
    );
    const names = Object.entries(CONSTELLATION_NAMES)
      .map(([id, [name, ra, dec]]) => ({ id, name, ...projectedPoint(ra, dec, sky, AT) }))
      .filter((label) => label.visible);
    const stars = BRIGHT_STARS.map(([ra, dec, mag]) => ({ ...projectedPoint(ra, dec, sky, AT), mag })).filter(
      (star) => star.visible
    );
    return { meridians, parallels, band, core, ecliptic, otherEquator, lonLabels, latLabels, constellations, names, stars };
  }, [sky]);

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
          const at = projectedPoint(ra, dec, sky, AT);
          if (!at.visible) return null;
          if (item.target.footprint) {
            const outline = footprintOutline(ra, dec, item.target.footprint);
            const path = projectedPath(outline, sky, AT, true);
            if (!path) return null;
            const corners = imageCorners(item);
            const matrix =
              corners && item.target.preview
                ? imageMatrix(corners, item.target.preview.width, item.target.preview.height, sky)
                : null;
            return { item, kind: 'field' as const, path, fill, opacity, at, matrix };
          }
          return { item, kind: 'point' as const, path: '', fill, opacity, at, matrix: null };
        })
        .filter((entry) => entry !== null),
    [targets, sky, maxSeconds]
  );

  // Which fields are inside the current view, for lazy stack previews.
  const visibleWidth = MAP_WIDTH / view.k;
  const visibleHeight = MAP_HEIGHT / view.k;
  const inView = (at: { x: number; y: number }) =>
    at.x >= view.x - visibleWidth * 0.2 &&
    at.x <= view.x + visibleWidth * 1.2 &&
    at.y >= view.y - visibleHeight * 0.2 &&
    at.y <= view.y + visibleHeight * 1.2;
  const stacksOn = showStacks && view.k >= STACKS_FROM_ZOOM;

  const track = (event: MouseEvent) => {
    const box = wrapper.current?.getBoundingClientRect();
    if (!box) return;
    setPointer({ x: event.clientX - box.left, y: event.clientY - box.top });
  };

  const k = view.k;
  const textScale = 1 / k;

  return (
    <div className="sky-map-wrapper" ref={wrapper} onMouseMove={track}>
      <svg
        ref={svgRef}
        className={`sky-map${k > 1 ? ' is-zoomed' : ''}`}
        viewBox={`${view.x} ${view.y} ${visibleWidth} ${visibleHeight}`}
        role="img"
        aria-label={`Sky coverage map in ${frame} coordinates with ${drawn.length} targets`}
        data-zoom={k.toFixed(2)}
        data-center={`${center.lon.toFixed(1)},${center.lat.toFixed(1)}`}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onDoubleClick={home}
      >
        <defs>
          <radialGradient id="sky-ground" cx="50%" cy="50%" r="60%">
            <stop offset="0%" stopColor="#141c33" />
            <stop offset="100%" stopColor="#090c18" />
          </radialGradient>
          {drawn
            .filter((entry) => entry.kind === 'field' && entry.matrix)
            .map((entry) => (
              <clipPath key={`clip-${entry.item.target.key}`} id={`sky-clip-${entry.item.target.key}`}>
                <path d={entry.path} />
              </clipPath>
            ))}
        </defs>
        <ellipse
          className="sky-globe"
          cx={AT.cx}
          cy={AT.cy}
          rx={mode === 'globe' ? AT.scale : 2 * AT.scale}
          ry={AT.scale}
          fill="url(#sky-ground)"
        />
        <g className="sky-stars">
          {scenery.stars.map((star, index) => (
            <circle
              key={index}
              className="sky-bright-star"
              cx={star.x}
              cy={star.y}
              r={Math.max(0.3, 2.6 - 0.45 * star.mag) * textScale}
              fillOpacity={Math.min(1, 1.15 - 0.12 * star.mag)}
            />
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
            <path key={`m${index}`} d={path} vectorEffect="non-scaling-stroke" />
          ))}
          {scenery.parallels.map((path, index) => (
            <path key={`p${index}`} d={path} vectorEffect="non-scaling-stroke" />
          ))}
        </g>
        {showBackdrop && (
          <g className="sky-constellations">
            {scenery.constellations.map((path, index) => (
              <path key={index} d={path} vectorEffect="non-scaling-stroke" />
            ))}
          </g>
        )}
        <path className="sky-ecliptic" d={scenery.ecliptic} vectorEffect="non-scaling-stroke" />
        <path className="sky-other-equator" d={scenery.otherEquator} vectorEffect="non-scaling-stroke" />
        <g className="sky-labels" style={{ fontSize: 11 * textScale }}>
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
        {showBackdrop && (
          <g className="sky-constellation-names" style={{ fontSize: 10 * textScale }}>
            {scenery.names.map((label) => (
              <text key={label.id} x={label.x} y={label.y} textAnchor="middle">
                {label.name}
              </text>
            ))}
          </g>
        )}
        <g className="sky-targets">
          {drawn.map(({ item, kind, path, fill, opacity, at, matrix }) => {
            const active = hovered?.target.key === item.target.key;
            const textured = stacksOn && matrix != null && inView(at);
            const shared = {
              className: `sky-target${active ? ' is-active' : ''}${textured ? ' is-textured' : ''}`,
              'data-target': item.target.name,
              vectorEffect: 'non-scaling-stroke' as const,
              onMouseEnter: () => setHovered(item),
              onMouseLeave: () => setHovered((current) => (current?.target.key === item.target.key ? null : current)),
              onClick: () => open(item),
              tabIndex: 0,
              onFocus: () => setHovered(item),
              onBlur: () => setHovered(null),
              onKeyDown: (event: { key: string }) => {
                if (event.key === 'Enter') onOpen(item);
              },
            };
            if (kind === 'field') {
              return (
                <path key={item.target.key} d={path} fill={fill} fillOpacity={textured ? 0 : opacity} stroke={fill} {...shared} />
              );
            }
            return (
              <circle
                key={item.target.key}
                cx={at.x}
                cy={at.y}
                r={(active ? 5 : 3.5) * textScale}
                fill={fill}
                fillOpacity={Math.min(1, opacity + 0.2)}
                stroke={fill}
                {...shared}
              />
            );
          })}
        </g>
        {stacksOn && (
          <g className="sky-stacks" pointerEvents="none">
            {drawn
              .filter((entry) => entry.kind === 'field' && entry.matrix && inView(entry.at))
              .map((entry) => (
                <g key={`stack-${entry.item.target.key}`} clipPath={`url(#sky-clip-${entry.item.target.key})`}>
                  <image
                    href={entry.item.target.preview?.url}
                    x={0}
                    y={0}
                    width={entry.item.target.preview?.width}
                    height={entry.item.target.preview?.height}
                    preserveAspectRatio="none"
                    transform={entry.matrix ?? undefined}
                  />
                </g>
              ))}
          </g>
        )}
      </svg>
      <div className="sky-zoom" role="group" aria-label="Zoom">
        <button type="button" onClick={() => zoomBy(1.6)} aria-label="Zoom in">
          <Plus size={14} />
        </button>
        <button type="button" onClick={() => zoomBy(1 / 1.6)} aria-label="Zoom out" disabled={k <= 1}>
          <Minus size={14} />
        </button>
        <button
          type="button"
          onClick={home}
          aria-label="Whole sky"
          disabled={k <= 1 && center.lon === defaultView(frame, mode).centerLon && center.lat === defaultView(frame, mode).centerLat}
        >
          <RotateCcw size={14} />
        </button>
        <span className="sky-zoom-level">{k >= 10 ? `${Math.round(k)}×` : `${k.toFixed(1)}×`}</span>
      </div>
      {hovered && <TargetCard item={hovered} x={pointer.x} y={pointer.y} width={wrapper.current?.clientWidth ?? 0} />}
    </div>
  );
}

function Sparkline({ series }: { series: Array<[string, number]> }) {
  if (series.length < 2) return null;
  const width = 232;
  const height = 26;
  const max = series.reduce((m, [, s]) => Math.max(m, s), 0);
  if (!(max > 0)) return null;
  const step = width / series.length;
  return (
    <svg className="sky-card-spark" viewBox={`0 0 ${width} ${height}`} width={width} height={height} aria-hidden="true">
      {series.map(([night, seconds], index) => {
        const h = Math.max(1, (height - 2) * (seconds / max));
        return <rect key={night} x={index * step + 0.3} y={height - h} width={Math.max(step - 0.6, 0.8)} height={h} />;
      })}
    </svg>
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
  const stack = target.preview
    ? `${target.preview.kind === 'color' ? 'Colour' : target.preview.filter ?? 'Mono'} stack preview${
        target.preview.wcs ? ', placed by its plate solve' : ', placed by the planned rotation'
      }`
    : null;
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
      <Sparkline series={item.perNight} />
      <div className="sky-card-span">
        {formatNight(item.firstNight)}
        {item.firstNight !== item.lastNight ? ` → ${formatNight(item.lastNight)}` : ''}
      </div>
      <div className="sky-card-field">{field}</div>
      {stack && <div className="sky-card-field">{stack}</div>}
      <div className="sky-card-hint">Click to open in Images · scroll to zoom, drag to turn or pan</div>
    </div>
  );
}
