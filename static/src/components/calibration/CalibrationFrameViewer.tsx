import { useRef, useState } from 'react';
import { apiClient } from '../../api/client';
import type {
  CalibrationFlatFeature,
  CalibrationFrameStretch,
  CalibrationFrameView,
  CalibrationKind,
  CalibrationSessionDust,
  CalibrationVignetting,
} from '../../api/types';
import { useAsyncImage } from '../../hooks/useAsyncImage';
import { useImageZoom } from '../../hooks/useImageZoom';
import DustOverlay from './DustOverlay';
import {
  featureMark,
  isLarge,
  vignettingMark,
  type DustMark,
  type VignettingMark,
} from './dustLabels';

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
  /** Measured spots and large dust, in the frame's own pixels. */
  spots?: {
    width: number;
    height: number;
    features: CalibrationFlatFeature[];
    vignetting?: CalibrationVignetting[];
  };
  /** The session's dust, found on its stack: drawn against the session. */
  sessionDust?: CalibrationSessionDust;
  /** Spots drawn, and nested under it, large dust and vignetting too. */
  showSpots: boolean;
  showDust: boolean;
  onShowSpots: (show: boolean) => void;
  onShowDust: (show: boolean) => void;
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
  sessionDust,
  showSpots,
  showDust,
  onShowSpots,
  onShowDust,
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
  // A mark picked on one frame says nothing of the next: the pick is kept
  // with its frame.
  const [pickedOn, setPickedOn] = useState<{ frame: string; key: string } | null>(null);
  const picked = pickedOn?.frame === frameUuid ? pickedOn.key : null;
  const setPicked = (key: string | null) => setPickedOn(key ? { frame: frameUuid, key } : null);
  const loadedIdentity = useRef<string | null>(null);
  const zoom = useImageZoom({ minScale: 0.05, maxScale: 20 });

  const current = shown?.src === image.src;
  const busy = image.state !== 'error' && !current;
  const stretchOptions = STRETCHES.map((option) => ({
    ...option,
    title: kind === 'flat' ? (shownView === 'session' ? option.session : option.flat) : option.dark,
  }));
  // What the overlay draws: against the session, the session's dust found
  // on its stack (dimmed where this flat does not show it) and what this
  // flat alone holds; as shot, the flat's own. Large dust and vignetting
  // only while their toggle, nested under Spots, is on.
  const againstSession = shownView === 'session' && !!sessionDust;
  const ownFeatures = spots?.features ?? [];
  const ownVignetting = spots?.vignetting ?? [];
  const hasLarge = againstSession
    ? sessionDust!.features.some((f) => isLarge(f.shape)) || sessionDust!.vignetting.length > 0
    : ownFeatures.some((f) => isLarge(f.shape)) || ownVignetting.length > 0;
  const hasAny = againstSession
    ? sessionDust!.features.length + sessionDust!.transients.length + sessionDust!.vignetting.length > 0
    : ownFeatures.length + ownVignetting.length > 0;
  const marks: DustMark[] = [];
  const shadows: VignettingMark[] = [];
  if (spots && showSpots) {
    const keep = (feature: CalibrationFlatFeature) => showDust || !isLarge(feature.shape);
    if (againstSession) {
      sessionDust!.features.forEach((feature, index) => {
        if (keep(feature)) {
          marks.push(featureMark(feature, `session-${index}`, { unseen: !sessionDust!.seen.includes(index) }));
        }
      });
      sessionDust!.transients.forEach((feature, index) => {
        if (keep(feature)) marks.push(featureMark(feature, `transient-${index}`, { transient: true }));
      });
      if (showDust) sessionDust!.vignetting.forEach((v, index) => shadows.push(vignettingMark(v, `vignetting-${index}`)));
    } else {
      const compact = ownFeatures
        .filter((feature) => !isLarge(feature.shape))
        .sort((left, right) => right.depth - left.depth)
        .slice(0, OVERLAY_SPOTS);
      const large = showDust ? ownFeatures.filter((feature) => isLarge(feature.shape)) : [];
      [...large, ...compact].forEach((feature, index) => marks.push(featureMark(feature, `own-${index}`)));
      if (showDust) ownVignetting.forEach((v, index) => shadows.push(vignettingMark(v, `vignetting-${index}`)));
    }
  }
  const pickedMark = [...marks, ...shadows].find((mark) => mark.key === picked);
  const transform = `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`;

  return (
    <section className="calibration-viewer" aria-label="Frame viewer">
      <div className="calibration-viewer-controls">
        <div className="tool-segmented" role="group" aria-label="Stretch">
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
            className="tool-button"
            aria-pressed={shownView === 'session'}
            disabled={!canCompare}
            title={canCompare ? 'Divide by the median of its session’s flats' : 'Needs three flats in the session'}
            onClick={() => onView(shownView === 'session' ? 'frame' : 'session')}
          >
            Compare with session
          </button>
        )}
        {spots && hasAny && (
          <div className="calibration-spots-toggle" role="group" aria-label="Spots">
            <button
              type="button"
              className="tool-button"
              aria-pressed={showSpots}
              onClick={() => {
                setPicked(null);
                onShowSpots(!showSpots);
              }}
            >
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
        <button
          type="button"
          className="tool-button"
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
        {/* Only over the bitmap of this frame: while the next one loads,
            the last stays up without the new frame's marks on it. */}
        {shown && current && dimensions && spots && marks.length + shadows.length > 0 && (
          <DustOverlay
            className="calibration-spots"
            width={spots.width}
            height={spots.height}
            svgWidth={dimensions.width}
            svgHeight={dimensions.height}
            marks={marks}
            vignetting={shadows}
            pixelScale={(dimensions.width / spots.width) * zoom.zoomState.scale}
            picked={picked}
            onPick={setPicked}
            style={{ transform, transformOrigin: '0 0' }}
            testId="calibration-dust-overlay"
          />
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
      {pickedMark && (
        <p className="calibration-dust-pick" role="status" data-testid="calibration-dust-pick">
          {pickedMark.detail}
        </p>
      )}
    </section>
  );
}
