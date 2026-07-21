import { useEffect, useState } from 'react';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import type { StackGroupStatus } from '../api/types';
import { useImageZoom } from '../hooks/useImageZoom';

interface StackPreviewInspectorProps {
  dbId: string;
  jobId: string;
  artifactRevision: string;
  group: StackGroupStatus;
  onClose: () => void;
}

export default function StackPreviewInspector({
  dbId,
  jobId,
  artifactRevision,
  group,
  onClose,
}: StackPreviewInspectorProps) {
  const [loaded, setLoaded] = useState(false);
  const [error, setError] = useState(false);
  const [dimensions, setDimensions] = useState<{ width: number; height: number } | null>(null);
  const zoom = useImageZoom({ minScale: 0.05, maxScale: 10 });
  const imageUrl = apiClient.getStackPreviewUrl(
    dbId,
    jobId,
    group.index,
    artifactRevision,
    'original'
  );

  useHotkeys('escape', onClose, { enableOnFormTags: true }, [onClose]);
  useHotkeys('plus,equal', zoom.zoomIn, [zoom.zoomIn]);
  useHotkeys('minus', zoom.zoomOut, [zoom.zoomOut]);
  useHotkeys('0,f', zoom.zoomToFit, [zoom.zoomToFit]);
  useHotkeys('1', zoom.zoomTo100, [zoom.zoomTo100]);

  useEffect(() => {
    zoom.containerRef.current?.focus();
  }, [zoom.containerRef]);

  return (
    <div className="stack-inspector-overlay" role="presentation" onClick={onClose}>
      <section
        className="stack-inspector"
        role="dialog"
        aria-modal="true"
        aria-labelledby="stack-inspector-title"
        onClick={(event) => event.stopPropagation()}
      >
        <header className="stack-inspector-header">
          <div>
            <div className="stack-preview-eyebrow">Full-resolution integration</div>
            <h2 id="stack-inspector-title">
              {group.target_name} <span>{group.filter_name || 'No filter'}</span>
            </h2>
          </div>
          <div className="stack-inspector-summary">
            <span>{group.accepted_frames} frames</span>
            <span>{Math.round(group.total_exposure_seconds)} s</span>
            {dimensions && <span>{dimensions.width} × {dimensions.height}</span>}
          </div>
          <button className="close-button" type="button" onClick={onClose} aria-label="Close stack inspector">
            ×
          </button>
        </header>

        <div
          className={`stack-inspector-canvas zoom-container ${zoom.hasOverflow ? 'has-overflow' : ''}`}
          ref={zoom.containerRef}
          onWheel={zoom.handleWheel}
          onMouseDown={zoom.handleMouseDown}
          onMouseMove={zoom.handleMouseMove}
          onMouseUp={zoom.handleMouseUp}
          onMouseLeave={zoom.handleMouseUp}
          onKeyDown={zoom.handleKeyDown}
          tabIndex={0}
        >
          {!loaded && !error && (
            <div className="stack-inspector-loading">
              <span className="stack-preview-spinner" aria-hidden="true" />
              Loading full-resolution stack…
            </div>
          )}
          {error ? (
            <div className="stack-inspector-loading error" role="alert">
              The full-resolution stack could not be loaded.
            </div>
          ) : (
            <img
              ref={zoom.imageRef}
              src={imageUrl}
              alt={`Full-resolution stack for ${group.target_name} ${group.filter_name || 'No filter'}`}
              data-testid="stack-inspector-image"
              draggable={false}
              onError={() => setError(true)}
              onLoad={(event) => {
                const { naturalWidth: width, naturalHeight: height } = event.currentTarget;
                if (!width || !height) return;
                setDimensions({ width, height });
                zoom.setImageDimensions(width, height, true);
                zoom.applyBitmapDimensions(width, height, 'fit');
                setLoaded(true);
              }}
              style={{
                visibility: loaded ? 'visible' : 'hidden',
                transform: `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`,
                transformOrigin: '0 0',
                cursor: zoom.hasOverflow ? 'grab' : 'default',
              }}
            />
          )}
        </div>

        <footer className="stack-inspector-toolbar">
          <div className="stack-inspector-hint">Wheel to zoom · drag to pan · F fit · 1 actual size</div>
          <a
            className="stack-preview-download"
            href={apiClient.getStackFitsUrl(dbId, jobId, group.index, artifactRevision)}
            download
          >
            Download linear FITS
          </a>
          <div className="zoom-info-compact">
            <span className="zoom-percentage-compact">{zoom.getZoomPercentage()}%</span>
          </div>
          <div className="zoom-buttons-compact stack-inspector-zoom-buttons">
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomOut} title="Zoom Out (-)">−</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomToFit} title="Fit to Screen (F)">Fit</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomTo100} title="100% (1)">100%</button>
            <button className="zoom-btn-compact" type="button" onClick={zoom.zoomIn} title="Zoom In (+)">+</button>
          </div>
        </footer>
      </section>
    </div>
  );
}
