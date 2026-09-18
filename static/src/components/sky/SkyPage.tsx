import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Download } from 'lucide-react';
import type { SkyStats } from './skyModel';
import { useMergedSkyCoverage } from '../../hooks/useDatabases';
import type { SkyFrame, SkyMode } from '../../utils/skyProjection';
import { filterColor } from '../../utils/filterColors';
import { isTauriApp } from '../../utils/tauri';
import SkyMap, { MAP_HEIGHT, MAP_WIDTH } from './SkyMap';
import SkyTimeline from './SkyTimeline';
import {
  formatHours,
  formatNight,
  formatPixels,
  mergeCoverage,
  nightKeysFor,
  shownTargets,
  skyStats,
  timelineLanes,
  type ShownTarget,
  type SkyCut,
} from './skyModel';
import { recallSkyView, rememberSkyView } from './skyViewMemory';
import './SkyPage.css';

const REPLAY_STEP_MS = 90;

export default function SkyPage() {
  const navigate = useNavigate();
  const { data: rows, isLoading, isError } = useMergedSkyCoverage();
  const merged = useMemo(() => mergeCoverage(rows), [rows]);

  // Start where the last visit left off, for this browser session.
  const remembered = useMemo(() => recallSkyView(), []);
  const [frame, setFrame] = useState<SkyFrame>(remembered.frame ?? 'equatorial');
  const [mode, setMode] = useState<SkyMode>(remembered.mode ?? 'aitoff');
  const [acceptedOnly, setAcceptedOnly] = useState(remembered.acceptedOnly ?? false);
  const [hiddenRigs, setHiddenRigs] = useState<Set<string>>(new Set(remembered.hiddenRigs ?? []));
  const [hiddenFilters, setHiddenFilters] = useState<Set<string>>(new Set(remembered.hiddenFilters ?? []));
  const [asOfNight, setAsOfNight] = useState<string | null>(remembered.asOfNight ?? null);
  const [fromNight, setFromNight] = useState<string | null>(remembered.fromNight ?? null);
  const [playing, setPlaying] = useState(false);
  const [showBackdrop, setShowBackdrop] = useState(remembered.showBackdrop ?? true);
  const [showStacks, setShowStacks] = useState(remembered.showStacks ?? true);
  const initialZoom = remembered.zoom;
  const initialCenter = useMemo(
    () =>
      remembered.centerLon != null && remembered.centerLat != null
        ? { lon: remembered.centerLon, lat: remembered.centerLat }
        : undefined,
    [remembered]
  );

  useEffect(() => {
    rememberSkyView({
      frame,
      mode,
      acceptedOnly,
      showBackdrop,
      showStacks,
      hiddenRigs: [...hiddenRigs],
      hiddenFilters: [...hiddenFilters],
      fromNight,
      asOfNight,
    });
  }, [frame, mode, acceptedOnly, showBackdrop, showStacks, hiddenRigs, hiddenFilters, fromNight, asOfNight]);

  const rememberView = useCallback((zoom: number, center: { lon: number; lat: number }) => {
    rememberSkyView({ zoom, centerLon: center.lon, centerLat: center.lat });
  }, []);
  const replay = useRef<number | null>(null);

  const cut: SkyCut = useMemo(
    () => ({
      rigs: hiddenRigs.size > 0 ? new Set(merged.rigs.map((rig) => rig.db_id).filter((id) => !hiddenRigs.has(id))) : null,
      filters: hiddenFilters.size > 0 ? new Set(merged.filters.filter((filter) => !hiddenFilters.has(filter))) : null,
      acceptedOnly,
      fromNight,
      asOfNight,
    }),
    [hiddenRigs, hiddenFilters, acceptedOnly, fromNight, asOfNight, merged]
  );

  // The slider runs over the nights the selected rigs and filters captured on.
  const nightKeys = useMemo(() => nightKeysFor(merged, cut), [merged, cut]);
  useEffect(() => {
    if (nightKeys.length === 0) return;
    if (fromNight && !nightKeys.includes(fromNight)) {
      const next = nightKeys.find((night) => night >= fromNight) ?? null;
      setFromNight(next && next !== nightKeys[0] ? next : null);
    }
    if (asOfNight && !nightKeys.includes(asOfNight)) {
      const previous = [...nightKeys].reverse().find((night) => night <= asOfNight) ?? null;
      setAsOfNight(previous && previous !== nightKeys[nightKeys.length - 1] ? previous : null);
    }
  }, [nightKeys, fromNight, asOfNight]);

  const shown = useMemo(() => shownTargets(merged, cut), [merged, cut]);
  const lanes = useMemo(() => timelineLanes(merged, cut), [merged, cut]);
  const stats = useMemo(() => skyStats(shown, lanes, cut), [shown, lanes, cut]);

  // The replay walks the nights in order and lets go at the last one.
  useEffect(() => {
    if (!playing) {
      if (replay.current != null) window.clearInterval(replay.current);
      replay.current = null;
      return;
    }
    const nights = nightKeys;
    if (nights.length < 2) {
      setPlaying(false);
      return;
    }
    // Start where the replay was parked, else at the chosen start night.
    const start = fromNight ? Math.max(0, nights.indexOf(fromNight)) : 0;
    let index = asOfNight ? nights.indexOf(asOfNight) : start - 1;
    if (index >= nights.length - 1) index = start - 1;
    replay.current = window.setInterval(() => {
      index += 1;
      if (index >= nights.length - 1) {
        setAsOfNight(null);
        setPlaying(false);
      } else {
        setAsOfNight(nights[index]);
      }
    }, REPLAY_STEP_MS);
    return () => {
      if (replay.current != null) window.clearInterval(replay.current);
      replay.current = null;
    };
    // The start point is read once when the replay begins.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [playing, nightKeys]);

  const open = useCallback(
    (item: ShownTarget) => {
      const { target } = item;
      navigate(
        `/grid?db=${encodeURIComponent(target.db_id)}&project=${target.project_id}&target=${target.id}`
      );
    },
    [navigate]
  );

  const toggle = (set: Set<string>, value: string, update: (next: Set<string>) => void) => {
    const next = new Set(set);
    if (next.has(value)) next.delete(value);
    else next.add(value);
    update(next);
  };

  const savePng = async () => {
    const svg = document.querySelector<SVGSVGElement>('.sky-map');
    if (!svg) return;
    const styled = inlineMapStyles(svg);
    const blob = new Blob([new XMLSerializer().serializeToString(styled)], { type: 'image/svg+xml' });
    const url = URL.createObjectURL(blob);
    try {
      const image = new Image();
      await new Promise<void>((resolve, reject) => {
        image.onload = () => resolve();
        image.onerror = () => reject(new Error('could not render the map'));
        image.src = url;
      });
      const scale = 2;
      const canvas = document.createElement('canvas');
      canvas.width = MAP_WIDTH * scale;
      canvas.height = (MAP_HEIGHT + POSTER_HEADER + POSTER_FOOTER) * scale;
      const context = canvas.getContext('2d');
      if (!context) return;
      context.fillStyle = '#05070f';
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.drawImage(image, 0, POSTER_HEADER * scale, MAP_WIDTH * scale, MAP_HEIGHT * scale);
      paintPosterText(context, scale, stats, merged.rigs.length, asOfNight);
      const link = document.createElement('a');
      link.download = `psf-guard-sky-${asOfNight ?? 'all'}.png`;
      link.href = canvas.toDataURL('image/png');
      link.click();
    } finally {
      URL.revokeObjectURL(url);
    }
  };

  if (isLoading && rows.length === 0) {
    return <div className="sky-page sky-loading">Reading the catalogs…</div>;
  }
  if (isError && rows.length === 0) {
    return <div className="sky-page sky-loading">The sky coverage could not be loaded.</div>;
  }
  if (merged.targets.length === 0) {
    return (
      <div className="sky-page sky-loading">
        No targets yet. Add a database or import image folders, and this page fills in.
      </div>
    );
  }

  return (
    <div className="sky-page">
      <header className="sky-hero">
        <div className="sky-hero-text">
          <h1>Sky coverage</h1>
          <p>
            Everything {merged.rigs.length === 1 ? 'this rig' : `${merged.rigs.length} rigs`} pointed at
            {stats.firstNight ? ` since ${formatNight(stats.firstNight)}` : ''}
            {asOfNight ? `, as it stood on ${formatNight(asOfNight)}` : ''}.
          </p>
        </div>
        <dl className="sky-stats">
          <div className="sky-stat">
            <dt>Integration</dt>
            <dd data-stat="hours">{formatHours(stats.hours)}</dd>
          </div>
          <div className="sky-stat">
            <dt>Frames</dt>
            <dd data-stat="frames">{stats.frames.toLocaleString()}</dd>
          </div>
          <div className="sky-stat">
            <dt>Targets</dt>
            <dd data-stat="targets">{stats.targets}</dd>
          </div>
          <div className="sky-stat">
            <dt>Nights</dt>
            <dd data-stat="nights">{stats.nights}</dd>
          </div>
          <div className="sky-stat">
            <dt>Sky covered</dt>
            <dd data-stat="area">
              {stats.areaDeg2 > 0 ? `${formatDeg2(stats.areaDeg2)} deg²` : '—'}
              {stats.fieldsDeg2 > stats.areaDeg2 * 1.005 && (
                <small title="The fields added together, overlaps counted every time">
                  {formatDeg2(stats.fieldsDeg2)} deg² of fields
                </small>
              )}
            </dd>
          </div>
          <div className="sky-stat">
            <dt>Pixels on sky</dt>
            <dd data-stat="pixels" title="Each covered patch counted at the finest plate scale that reached it">
              {formatPixels(stats.pixels)}
            </dd>
          </div>
          <div className="sky-stat">
            <dt>Longest night</dt>
            <dd data-stat="longest">
              {stats.longestNight ? (
                <>
                  {formatHours(stats.longestNight.hours)}
                  <small>
                    {formatNight(stats.longestNight.night)}
                    {merged.rigs.length > 1 ? ` · ${stats.longestNight.rig}` : ''}
                  </small>
                </>
              ) : (
                '—'
              )}
            </dd>
          </div>
          <div className="sky-stat">
            <dt>Most loved</dt>
            <dd data-stat="top">
              {stats.topTarget ? (
                <>
                  <span className="sky-stat-name" title={stats.topTarget.name}>
                    {stats.topTarget.name}
                  </span>
                  <small>{formatHours(stats.topTarget.hours)}</small>
                </>
              ) : (
                '—'
              )}
            </dd>
          </div>
        </dl>
      </header>

      <div className="sky-controls">
        <div className="sky-control-group" role="radiogroup" aria-label="Coordinate frame">
          {(['equatorial', 'galactic'] as SkyFrame[]).map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={frame === option}
              className={`sky-chip${frame === option ? ' is-on' : ''}`}
              onClick={() => setFrame(option)}
            >
              {option === 'equatorial' ? 'Equatorial' : 'Galactic'}
            </button>
          ))}
        </div>
        <div className="sky-control-group" role="radiogroup" aria-label="Map shape">
          {(['aitoff', 'globe'] as SkyMode[]).map((option) => (
            <button
              key={option}
              type="button"
              role="radio"
              aria-checked={mode === option}
              className={`sky-chip${mode === option ? ' is-on' : ''}`}
              onClick={() => setMode(option)}
            >
              {option === 'aitoff' ? 'Flat' : 'Globe'}
            </button>
          ))}
        </div>
        <label className="sky-check">
          <input type="checkbox" checked={acceptedOnly} onChange={(event) => setAcceptedOnly(event.target.checked)} />
          Accepted frames only
        </label>
        <label className="sky-check">
          <input type="checkbox" checked={showBackdrop} onChange={(event) => setShowBackdrop(event.target.checked)} />
          Constellations
        </label>
        <label className="sky-check">
          <input type="checkbox" checked={showStacks} onChange={(event) => setShowStacks(event.target.checked)} />
          Stacks when zoomed
        </label>
        {merged.rigs.length > 1 && (
          <div className="sky-control-group" aria-label="Rigs">
            {merged.rigs.map((rig) => (
              <button
                key={rig.db_id}
                type="button"
                aria-pressed={!hiddenRigs.has(rig.db_id)}
                className={`sky-chip${hiddenRigs.has(rig.db_id) ? '' : ' is-on'}`}
                onClick={() => toggle(hiddenRigs, rig.db_id, setHiddenRigs)}
              >
                {rig.db_name}
              </button>
            ))}
          </div>
        )}
        <div className="sky-control-group" aria-label="Filters">
          {merged.filters.map((filter) => (
            <button
              key={filter}
              type="button"
              aria-pressed={!hiddenFilters.has(filter)}
              className={`sky-chip sky-chip-filter${hiddenFilters.has(filter) ? '' : ' is-on'}`}
              style={{ ['--chip-color' as string]: filterColor(filter) }}
              onClick={() => toggle(hiddenFilters, filter, setHiddenFilters)}
            >
              <span className="sky-chip-swatch" />
              {filter}
            </button>
          ))}
        </div>
        {!isTauriApp() && (
          <button type="button" className="sky-chip sky-save" onClick={savePng}>
            <Download size={14} /> Save PNG
          </button>
        )}
      </div>

      <SkyMap
        targets={shown}
        frame={frame}
        mode={mode}
        showBackdrop={showBackdrop}
        showStacks={showStacks}
        initialZoom={initialZoom}
        initialCenter={initialCenter}
        onViewChange={rememberView}
        onOpen={open}
      />

      <SkyTimeline
        lanes={lanes}
        nightKeys={nightKeys}
        fromNight={fromNight}
        asOfNight={asOfNight}
        onRange={(from, to) => {
          setPlaying(false);
          setFromNight(from);
          setAsOfNight(to);
        }}
        playing={playing}
        onTogglePlay={() => setPlaying((current) => !current)}
      />
    </div>
  );
}

function formatDeg2(value: number): string {
  return value >= 100 ? String(Math.round(value)) : value.toFixed(1);
}

const POSTER_HEADER = 150;
const POSTER_FOOTER = 40;

/** Title, subtitle, and the numbers above the map, and a footer below it. */
function paintPosterText(
  context: CanvasRenderingContext2D,
  scale: number,
  stats: SkyStats,
  rigs: number,
  asOfNight: string | null
) {
  const px = (value: number) => value * scale;
  context.textBaseline = 'alphabetic';
  context.fillStyle = '#e8ecf8';
  context.font = `600 ${px(34)}px system-ui, sans-serif`;
  context.fillText('Sky coverage', px(40), px(58));
  context.fillStyle = '#9aa4c4';
  context.font = `${px(15)}px system-ui, sans-serif`;
  const when = asOfNight ? `as it stood on ${formatNight(asOfNight)}` : `${formatNight(stats.firstNight)} to ${formatNight(stats.lastNight)}`;
  context.fillText(`Everything ${rigs === 1 ? 'one rig' : `${rigs} rigs`} pointed at, ${when}`, px(40), px(84));
  const numbers: Array<[string, string]> = [
    [formatHours(stats.hours), 'integration'],
    [stats.frames.toLocaleString(), 'frames'],
    [String(stats.targets), 'targets'],
    [String(stats.nights), 'nights'],
    [`${formatDeg2(stats.areaDeg2)} deg²`, 'of sky'],
    [formatPixels(stats.pixels), 'on sky'],
  ];
  let x = px(40);
  for (const [value, label] of numbers) {
    context.fillStyle = '#ffffff';
    context.font = `600 ${px(24)}px system-ui, sans-serif`;
    context.fillText(value, x, px(128));
    const width = context.measureText(value).width;
    context.fillStyle = '#7f89aa';
    context.font = `${px(12)}px system-ui, sans-serif`;
    context.fillText(label, x + width + px(6), px(128));
    x += width + context.measureText(label).width + px(34);
  }
  context.fillStyle = '#5c6584';
  context.font = `${px(11)}px system-ui, sans-serif`;
  context.textAlign = 'right';
  context.fillText(`PSF Guard · ${new Date().toISOString().slice(0, 10)}`, px(MAP_WIDTH - 40), px(POSTER_HEADER + MAP_HEIGHT + 26));
  context.textAlign = 'left';
}

/**
 * The map's look lives in the stylesheet; a serialized SVG drops it. Copy
 * the computed strokes and fills onto the elements of a clone so the saved
 * picture matches the screen.
 */
function inlineMapStyles(svg: SVGSVGElement): SVGSVGElement {
  const clone = svg.cloneNode(true) as SVGSVGElement;
  const sources = svg.querySelectorAll<SVGElement>('*');
  const targets = clone.querySelectorAll<SVGElement>('*');
  const keep = ['fill', 'fill-opacity', 'stroke', 'stroke-width', 'stroke-opacity', 'stroke-dasharray', 'font-size', 'font-family', 'opacity'];
  sources.forEach((source, index) => {
    const computed = window.getComputedStyle(source);
    const target = targets[index];
    if (!target) return;
    for (const property of keep) {
      const value = computed.getPropertyValue(property);
      if (value) target.style.setProperty(property, value);
    }
  });
  clone.setAttribute('xmlns', 'http://www.w3.org/2000/svg');
  clone.setAttribute('width', String(MAP_WIDTH));
  clone.setAttribute('height', String(MAP_HEIGHT));
  return clone;
}
