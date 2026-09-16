import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { useNavigate } from 'react-router-dom';
import { Download } from 'lucide-react';
import { useMergedSkyCoverage } from '../../hooks/useDatabases';
import type { SkyFrame } from '../../utils/skyProjection';
import { filterColor } from '../../utils/filterColors';
import { isTauriApp } from '../../utils/tauri';
import SkyMap, { MAP_HEIGHT, MAP_WIDTH } from './SkyMap';
import SkyTimeline from './SkyTimeline';
import {
  formatHours,
  formatNight,
  mergeCoverage,
  shownTargets,
  skyStats,
  timelineLanes,
  type ShownTarget,
  type SkyCut,
} from './skyModel';
import './SkyPage.css';

const REPLAY_STEP_MS = 90;

export default function SkyPage() {
  const navigate = useNavigate();
  const { data: rows, isLoading, isError } = useMergedSkyCoverage();
  const merged = useMemo(() => mergeCoverage(rows), [rows]);

  const [frame, setFrame] = useState<SkyFrame>('equatorial');
  const [acceptedOnly, setAcceptedOnly] = useState(false);
  const [hiddenRigs, setHiddenRigs] = useState<Set<string>>(new Set());
  const [hiddenFilters, setHiddenFilters] = useState<Set<string>>(new Set());
  const [asOfNight, setAsOfNight] = useState<string | null>(null);
  const [playing, setPlaying] = useState(false);
  const replay = useRef<number | null>(null);

  const cut: SkyCut = useMemo(
    () => ({
      rigs: hiddenRigs.size > 0 ? new Set(merged.rigs.map((rig) => rig.db_id).filter((id) => !hiddenRigs.has(id))) : null,
      filters: hiddenFilters.size > 0 ? new Set(merged.filters.filter((filter) => !hiddenFilters.has(filter))) : null,
      acceptedOnly,
      asOfNight,
    }),
    [hiddenRigs, hiddenFilters, acceptedOnly, asOfNight, merged]
  );

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
    const nights = merged.nightKeys;
    if (nights.length < 2) {
      setPlaying(false);
      return;
    }
    let index = asOfNight ? nights.indexOf(asOfNight) : -1;
    if (index >= nights.length - 1) index = -1;
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
  }, [playing, merged.nightKeys]);

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
      canvas.height = MAP_HEIGHT * scale;
      const context = canvas.getContext('2d');
      if (!context) return;
      context.fillStyle = '#05070f';
      context.fillRect(0, 0, canvas.width, canvas.height);
      context.drawImage(image, 0, 0, canvas.width, canvas.height);
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
              {stats.areaDeg2 > 0 ? `${stats.areaDeg2 >= 100 ? Math.round(stats.areaDeg2) : stats.areaDeg2.toFixed(1)} deg²` : '—'}
            </dd>
          </div>
          <div className="sky-stat">
            <dt>Longest night</dt>
            <dd data-stat="longest">
              {stats.longestNight ? (
                <>
                  {formatHours(stats.longestNight.hours)}
                  <small>{formatNight(stats.longestNight.night)}</small>
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
        <label className="sky-check">
          <input type="checkbox" checked={acceptedOnly} onChange={(event) => setAcceptedOnly(event.target.checked)} />
          Accepted frames only
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

      <SkyMap targets={shown} frame={frame} onOpen={open} />

      <SkyTimeline
        lanes={lanes}
        nightKeys={merged.nightKeys}
        asOfNight={asOfNight}
        onAsOf={(night) => {
          setPlaying(false);
          setAsOfNight(night);
        }}
        playing={playing}
        onTogglePlay={() => setPlaying((current) => !current)}
      />
    </div>
  );
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
