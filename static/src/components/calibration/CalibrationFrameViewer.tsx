import { useRef, useState } from 'react';
import { apiClient } from '../../api/client';
import type {
  CalibrationFlatFeature,
  CalibrationFrameStretch,
  CalibrationFrameView,
  CalibrationKind,
} from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import { useImageZoom } from '../../hooks/useImageZoom';

const STRETCHES: { value: CalibrationFrameStretch; label: string; flat: string; session: string; dark: string }[] = [
  { value: 'wide', label: 'Wide', flat: '±50%', session: '±10%', dark: '±30σ' },
  { value: 'normal', label: 'Normal', flat: '±15%', session: '±3%', dark: '±6σ' },
  { value: 'tight', label: 'Tight', flat: '±3%', session: '±1%', dark: '±2σ' },
];

/** The deepest spots the overlay draws. */
const OVERLAY_SPOTS = 200;

interface CalibrationFrameViewerProps {
  dbId: string;
  frameUuid: string;
  /** Frames of one session keep the zoom, so flipping compares one place. */
  sessionId: string;
  kind: CalibrationKind;
  label: string;
  view: CalibrationFrameView;
  stretch: CalibrationFrameStretch;
  full: boolean;
  canCompare: boolean;
  /** Measured spots, in the frame's own pixels. */
  spots?: { width: number; height: number; features: CalibrationFlatFeature[] };
  onView: (view: CalibrationFrameView) => void;
  onStretch: (stretch: CalibrationFrameStretch) => void;
  onFull: (full: boolean) => void;
}

/**
 * One calibration frame, drawn for its kind, or a flat against its session.
 * While the next frame is drawn the last one stays up, so flipping through
 * a session reads as a blink rather than a flash.
 */
export default function CalibrationFrameViewer({
  dbId,
  frameUuid,
  sessionId,
  kind,
  label,
  view,
  stretch,
  full,
  canCompare,
  spots,
  onView,
  onStretch,
  onFull,
}: CalibrationFrameViewerProps) {
  const shownView: CalibrationFrameView = view === 'session' && canCompare ? 'session' : 'frame';
  const options = { view: shownView, size: full ? 'original' : 'screen', stretch } as const;
  const url = apiClient.getCalibrationFramePreviewUrl(dbId, frameUuid, options);
  const image = useAsyncImage(dbId, url, { kind: 'calibration_frame', frameUuid, ...options });
  const identity = `${sessionId}:${shownView}:${full}`;
  const [loaded, setLoaded] = useState<{ src: string; identity: string; view: CalibrationFrameView } | null>(null);
  // Another session or view: its pictures differ in size, so the last one
  // is not shown in its place.
  const shown = loaded && loaded.identity === identity ? loaded : null;
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  const [showSpots, setShowSpots] = useState(false);
  const loadedIdentity = useRef<string | null>(null);
  const zoom = useImageZoom({ minScale: 0.05, maxScale: 20 });

  const current = shown?.src === image.src;
  const busy = image.state !== 'error' && !current;
  const stretchOptions = STRETCHES.map((option) => ({
    ...option,
    title: kind === 'flat' ? (shownView === 'session' ? option.session : option.flat) : option.dark,
  }));
  const features = spots && showSpots
    ? [...spots.features].sort((left, right) => right.depth - left.depth).slice(0, OVERLAY_SPOTS)
    : [];
  const transform = `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`;

  return (
    <section className="calibration-viewer" aria-label="Frame viewer">
      <div className="calibration-viewer-controls">
        <div className="calibration-segmented" role="group" aria-label="Stretch">
          {stretchOptions.map((option) => (
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
        {kind === 'flat' && (
          <button
            type="button"
            className="calibration-toggle"
            aria-pressed={shownView === 'session'}
            disabled={!canCompare}
            title={canCompare ? 'Divide by the median of its session’s flats' : 'Needs three flats in the session'}
            onClick={() => onView(shownView === 'session' ? 'frame' : 'session')}
          >
            Compare with session
          </button>
        )}
        {spots && spots.features.length > 0 && (
          <button
            type="button"
            className="calibration-toggle"
            aria-pressed={showSpots}
            onClick={() => setShowSpots((value) => !value)}
          >
            Spots
          </button>
        )}
        <button
          type="button"
          className="calibration-toggle"
          aria-pressed={full}
          onClick={() => onFull(!full)}
        >
          Full size
        </button>
        <div className="calibration-zoom" role="group" aria-label="Zoom">
          <button type="button" onClick={zoom.zoomOut} title="Zoom out" aria-label="Zoom out">−</button>
          <button type="button" onClick={zoom.zoomToFit} title="Fit">Fit</button>
          <button type="button" onClick={zoom.zoomTo100} title="Actual size">100%</button>
          <button type="button" onClick={zoom.zoomIn} title="Zoom in" aria-label="Zoom in">+</button>
          <span>{zoom.getZoomPercentage()}%</span>
        </div>
      </div>
      <div
        className={`calibration-canvas ${zoom.hasOverflow ? 'has-overflow' : ''}`}
        ref={zoom.containerRef}
        onWheel={zoom.handleWheel}
        onMouseDown={zoom.handleMouseDown}
        onMouseMove={zoom.handleMouseMove}
        onMouseUp={zoom.handleMouseUp}
        onMouseLeave={zoom.handleMouseUp}
        onTouchStart={zoom.handleTouchStart}
        onTouchMove={zoom.handleTouchMove}
        onTouchEnd={zoom.handleTouchEnd}
        onKeyDown={zoom.handleKeyDown}
        tabIndex={0}
        aria-label={`${label}, ${shownView === 'session' ? 'against its session' : 'as shot'}`}
      >
        {/* Fetches the next picture; the one on screen changes only once it
            has arrived. */}
        <img
          className="calibration-probe"
          src={image.src}
          alt=""
          aria-hidden="true"
          onLoad={() => {
            image.onLoad();
            setLoaded({ src: image.src, identity, view: shownView });
          }}
          onError={image.onError}
        />
        {shown && (
          <img
            ref={zoom.imageRef}
            src={shown.src}
            alt={label}
            data-testid="calibration-frame-image"
            data-view={shown.view}
            draggable={false}
            onLoad={(event) => {
              const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
              if (!width || !height) return;
              setDimensions({ width, height });
              zoom.setImageDimensions(width, height, true);
              zoom.applyBitmapDimensions(width, height, loadedIdentity.current === shown.identity ? 'preserve' : 'fit');
              loadedIdentity.current = shown.identity;
            }}
            style={{ transform, transformOrigin: '0 0', cursor: zoom.hasOverflow ? 'grab' : 'default' }}
          />
        )}
        {shown && dimensions && features.length > 0 && spots && (
          <svg
            className="calibration-spots"
            width={dimensions.width}
            height={dimensions.height}
            viewBox={`0 0 ${spots.width} ${spots.height}`}
            preserveAspectRatio="none"
            style={{ transform, transformOrigin: '0 0' }}
            aria-label={`${features.length} measured spots`}
            role="img"
          >
            {features.map((feature, index) => (
              <rect
                key={index}
                x={feature.left - 2}
                y={feature.top - 2}
                width={feature.right - feature.left + 5}
                height={feature.bottom - feature.top + 5}
                vectorEffect="non-scaling-stroke"
              >
                <title>{`${(feature.depth * 100).toFixed(1)}% deep, ${feature.pixels} px`}</title>
              </rect>
            ))}
          </svg>
        )}
        {image.state === 'error' ? (
          <div className="calibration-canvas-note" role="alert">{image.error || 'The picture could not be drawn.'}</div>
        ) : busy ? (
          <div className="calibration-canvas-note" role="status">
            <span className="calibration-spinner" aria-hidden="true" />
            {image.state === 'generating' ? 'Drawing…' : 'Loading…'}
          </div>
        ) : null}
      </div>
    </section>
  );
}
