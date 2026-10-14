import { useEffect, useRef, useState } from 'react';
import { apiClient } from '../../api/client';
import type { CalibrationFrameStretch, FlatSession, FlatSpot } from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import { percent } from './flatGroupLabels';

const STRETCHES: { value: CalibrationFrameStretch; label: string; title: string }[] = [
  { value: 'wide', label: 'Wide', title: '±3%' },
  { value: 'normal', label: 'Normal', title: '±1%' },
  { value: 'tight', label: 'Tight', title: '±0.5%' },
];

/** A circle stays big enough to find on a whole sensor. */
const MIN_RADIUS_FRACTION = 0.012;

const SPOT_KINDS: Record<FlatSpot['kind'], string> = {
  new: 'New',
  gone: 'Gone',
  moved: 'Moved',
};

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
  stretch,
  onStretch,
  onClose,
}: FlatRatioViewerProps) {
  const url = apiClient.getFlatRatioUrl(dbId, { earlier, later, stretch });
  const image = useAsyncImage(dbId, url, { kind: 'flat_ratio', earlier, later, stretch });
  const [showSpots, setShowSpots] = useState(true);
  const sectionRef = useRef<HTMLElement>(null);

  // Opened from a button further down: bring it into sight once per pair.
  useEffect(() => {
    sectionRef.current?.scrollIntoView?.({ block: 'nearest' });
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

  return (
    <section ref={sectionRef} className="flat-ratio-viewer" aria-label="Flat comparison" data-testid="flat-ratio-viewer">
      <div className="flat-ratio-head">
        <div>
          <h3>{title}</h3>
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
        {spots.length > 0 && grid && (
          <button type="button" className="tool-button" aria-pressed={showSpots} onClick={() => setShowSpots((value) => !value)}>
            Spots
          </button>
        )}
        {spots.length > 0 && grid && showSpots && (
          <span className="flat-ratio-legend">
            <span className="flat-ratio-swatch is-new" aria-hidden="true" /> darker: new dust
            <span className="flat-ratio-swatch is-gone" aria-hidden="true" /> brighter: dust gone
          </span>
        )}
      </div>
      <div
        className={`flat-ratio-canvas${grid ? '' : ' is-unsized'}`}
        style={grid ? { aspectRatio: `${grid.width} / ${grid.height}` } : undefined}
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
        {ready && grid && showSpots && spots.length > 0 && (
          <svg
            className="flat-ratio-spots"
            viewBox={`0 0 ${grid.sensorWidth} ${grid.sensorHeight}`}
            preserveAspectRatio="none"
            role="img"
            aria-label={`${spots.length} ${spots.length === 1 ? 'spot' : 'spots'} that changed`}
          >
            {spots.map((spot, index) => (
              <circle
                key={index}
                className={`flat-ratio-spot ${spot.darker ? 'is-new' : 'is-gone'}`}
                cx={spot.x}
                cy={spot.y}
                r={Math.max(spot.radius * 1.5, minRadius)}
                vectorEffect="non-scaling-stroke"
              >
                <title>
                  {`${SPOT_KINDS[spot.kind]}: ${spot.darker ? 'darker' : 'brighter'} by ${percent(spot.peak_pct)} near (${Math.round(spot.x)}, ${Math.round(spot.y)})`}
                </title>
              </circle>
            ))}
          </svg>
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
    </section>
  );
}
