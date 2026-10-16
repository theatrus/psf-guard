import { useEffect, useRef, useState, type CSSProperties } from 'react';
import { apiClient } from '../../api/client';
import type { CalibrationFrameStretch, FlatSession, FlatSpot, FlatVignettingChange } from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import DustOverlay from './DustOverlay';
import { isLarge, spotMark, spotWords, vignettingChangeMark } from './dustLabels';
import { percent } from './flatGroupLabels';

const STRETCHES: { value: CalibrationFrameStretch; label: string; title: string }[] = [
  { value: 'wide', label: 'Wide', title: '±3%' },
  { value: 'normal', label: 'Normal', title: '±1%' },
  { value: 'tight', label: 'Tight', title: '±0.5%' },
];

/** A spot's mark stays big enough to find on a whole sensor. */
const MIN_RADIUS_FRACTION = 0.012;

interface FlatRatioViewerProps {
  dbId: string;
  earlier: string;
  later: string;
  /** The earlier session: its response grid sizes the spots' overlay. */
  earlierSession?: FlatSession;
  laterSession?: FlatSession;
  /** What the pair shows, in words. */
  title: string;
  spots: FlatSpot[];
  /** Changes of vignetting between the two: not dust. */
  vignetting?: FlatVignettingChange[];
  /** Spots drawn, and nested under it, large dust and vignetting too. */
  showSpots: boolean;
  showDust: boolean;
  onShowSpots: (show: boolean) => void;
  onShowDust: (show: boolean) => void;
  stretch: CalibrationFrameStretch;
  onStretch: (stretch: CalibrationFrameStretch) => void;
  onClose: () => void;
}

/**
 * Two flat sessions set against each other: the later divided by the
 * earlier, grey around 1, with the shadows that came or went circled. The
 * picture rides the shared generation poll while the server draws it.
 */
export default function FlatRatioViewer({
  dbId,
  earlier,
  later,
  earlierSession,
  laterSession,
  title,
  spots,
  vignetting = [],
  showSpots,
  showDust,
  onShowSpots,
  onShowDust,
  stretch,
  onStretch,
  onClose,
}: FlatRatioViewerProps) {
  const url = apiClient.getFlatRatioUrl(dbId, { earlier, later, stretch });
  const image = useAsyncImage(dbId, url, { kind: 'flat_ratio', earlier, later, stretch });
  const [picked, setPicked] = useState<string | null>(null);
  const [canvasWidth, setCanvasWidth] = useState(0);
  const canvasRef = useRef<HTMLDivElement>(null);
  const sectionRef = useRef<HTMLElement>(null);

  // The picture's drawn width sizes the marks' labels.
  useEffect(() => {
    const element = canvasRef.current;
    if (!element) return;
    const measure = () => setCanvasWidth(element.getBoundingClientRect().width);
    measure();
    if (typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, []);
  const headingRef = useRef<HTMLHeadingElement>(null);

  // Opened from a button elsewhere: bring it into sight and move focus to
  // it once per pair, so the keyboard carries on from here.
  useEffect(() => {
    sectionRef.current?.scrollIntoView?.({ block: 'nearest' });
    headingRef.current?.focus({ preventScroll: true });
  }, [earlier, later]);

  const grid = earlierSession && earlierSession.bin && earlierSession.width && earlierSession.height
    ? {
        width: earlierSession.width,
        height: earlierSession.height,
        sensorWidth: earlierSession.width * earlierSession.bin,
        sensorHeight: earlierSession.height * earlierSession.bin,
      }
    : null;
  const minRadius = grid ? Math.max(grid.sensorWidth, grid.sensorHeight) * MIN_RADIUS_FRACTION : 0;
  const nights = earlierSession && laterSession ? `${laterSession.night} ÷ ${earlierSession.night}` : 'Later ÷ earlier';
  const ready = image.state === 'ready';
  const hasLarge = spots.some((spot) => isLarge(spot.shape)) || vignetting.length > 0;
  const hasAny = spots.length > 0 || vignetting.length > 0;
  // A compact spot's mark is widened so it can be found on a whole sensor;
  // disks and donuts are drawn at their size.
  const marks = showSpots
    ? spots
        .filter((spot) => showDust || !isLarge(spot.shape))
        .map((spot, index) => {
          const mark = spotMark(spot, `spot-${index}`);
          if (isLarge(spot.shape)) return mark;
          const reach = Math.max(spot.radius * 1.5, minRadius);
          return { ...mark, bounds: [spot.x - reach, spot.y - reach, spot.x + reach, spot.y + reach] as [number, number, number, number] };
        })
    : [];
  const shadows = showSpots && showDust
    ? vignetting.map((change, index) => vignettingChangeMark(change, `vignetting-${index}`))
    : [];
  const pickedMark = [...marks, ...shadows].find((mark) => mark.key === picked);

  return (
    <section ref={sectionRef} className="flat-ratio-viewer" aria-label="Flat comparison" data-testid="flat-ratio-viewer">
      <div className="flat-ratio-head">
        <div>
          <h3 ref={headingRef} tabIndex={-1}>{title}</h3>
          <p className="calibration-muted">{nights}: the later flats divided by the earlier, grey where they agree</p>
        </div>
        <button type="button" className="tool-button" onClick={onClose}>Close</button>
      </div>
      <div className="flat-ratio-controls">
        <div className="tool-segmented" role="group" aria-label="Stretch">
          {STRETCHES.map((option) => (
            <button
              key={option.value}
              type="button"
              aria-pressed={stretch === option.value}
              title={option.title}
              onClick={() => onStretch(option.value)}
            >
              {option.label}
            </button>
          ))}
        </div>
        {hasAny && grid && (
          <div className="calibration-spots-toggle" role="group" aria-label="Spots">
            <button type="button" className="tool-button" aria-pressed={showSpots} onClick={() => {
              setPicked(null);
              onShowSpots(!showSpots);
            }}>
              Spots
            </button>
            {showSpots && hasLarge && (
              <button
                type="button"
                className="tool-button calibration-subtoggle"
                aria-pressed={showDust}
                title="Out-of-focus disks and donuts, and vignetting, at their shape and size"
                onClick={() => {
                  setPicked(null);
                  onShowDust(!showDust);
                }}
              >
                Large dust
              </button>
            )}
          </div>
        )}
        {hasAny && grid && showSpots && (
          <span className="flat-ratio-legend">
            <span className="flat-ratio-swatch is-new" aria-hidden="true" /> new
            <span className="flat-ratio-swatch is-gone" aria-hidden="true" /> gone
            <span className="flat-ratio-swatch is-moved" aria-hidden="true" /> moved
            <span className="flat-ratio-swatch is-tolerance" aria-hidden="true" /> within focus travel
            {showDust && vignetting.length > 0 && (
              <><span className="flat-ratio-swatch is-vignetting" aria-hidden="true" /> vignetting</>
            )}
          </span>
        )}
      </div>
      <div
        ref={canvasRef}
        className={`flat-ratio-canvas${grid ? '' : ' is-unsized'}`}
        style={grid
          ? ({ aspectRatio: `${grid.width} / ${grid.height}`, '--flat-ratio-aspect': grid.width / grid.height } as CSSProperties)
          : undefined}
      >
        <img
          className={ready ? undefined : 'is-pending'}
          src={image.src}
          alt={`${title}: later flats divided by earlier`}
          data-testid="flat-ratio-image"
          data-stretch={stretch}
          onLoad={image.onLoad}
          onError={image.onError}
        />
        {ready && grid && marks.length + shadows.length > 0 && (
          <DustOverlay
            className="flat-ratio-spots"
            width={grid.sensorWidth}
            height={grid.sensorHeight}
            marks={marks}
            vignetting={shadows}
            pixelScale={canvasWidth > 0 ? canvasWidth / grid.sensorWidth : 0.15}
            picked={picked}
            onPick={setPicked}
            testId="flat-ratio-spots"
          />
        )}
        {image.state === 'error' ? (
          <div className="calibration-canvas-note" role="alert">{image.error || 'The picture could not be drawn.'}</div>
        ) : !ready ? (
          <div className="calibration-canvas-note" role="status">
            <span className="calibration-spinner" aria-hidden="true" />
            {image.state === 'generating' ? 'Drawing…' : 'Loading…'}
          </div>
        ) : null}
      </div>
      {pickedMark && (
        <p className="calibration-dust-pick" role="status" data-testid="flat-ratio-dust-pick">{pickedMark.detail}</p>
      )}
      {hasAny && (
        <ul className="flat-ratio-spot-list" aria-label="Spots that changed">
          {spots.map((spot, index) => (
            <li key={index}>
              <span className={`flat-ratio-swatch ${spot.within_tolerance ? 'is-tolerance' : `is-${spot.kind}`}`} aria-hidden="true" />
              {`${spotWords(spot)}: ${spot.darker ? 'darker' : 'brighter'} by ${percent(spot.peak_pct)} near (${Math.round(spot.x)}, ${Math.round(spot.y)})`}
            </li>
          ))}
          {vignetting.map((change, index) => (
            <li key={`v${index}`}>
              <span className="flat-ratio-swatch is-vignetting" aria-hidden="true" />
              {vignettingChangeMark(change, '').detail}
            </li>
          ))}
        </ul>
      )}
    </section>
  );
}
