import { useState, useEffect, useLayoutEffect, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import { GradingStatus } from '../api/types';
import type { PreviewDescriptor } from '../api/types';
import { useImagePreloader } from '../hooks/useImagePreloader';
import { useImageZoom } from '../hooks/useImageZoom';
import { useAsyncImage } from '../hooks/useAsyncImage';
import { ensurePreviewReady } from '../hooks/previewPoll';
import UndoRedoToolbar from './UndoRedoToolbar';

interface ImageDetailViewProps {
  dbId: string;
  imageId: number;
  onClose: () => void;
  onNext: () => void;
  onPrevious: () => void;
  onGrade: (status: 'accepted' | 'rejected' | 'pending') => void;
  adjacentImageIds?: { next: number[]; previous: number[] };
  // Optional grading system for undo/redo (passed from parent)
  grading?: {
    canUndo: boolean;
    canRedo: boolean;
    isLoading: boolean;
    undoStackSize: number;
    redoStackSize: number;
    undo: () => Promise<boolean>;
    redo: () => Promise<boolean>;
    getLastAction: () => any; // eslint-disable-line @typescript-eslint/no-explicit-any
    getNextRedoAction: () => any; // eslint-disable-line @typescript-eslint/no-explicit-any
  };
}

export default function ImageDetailView({
  dbId,
  imageId,
  onClose,
  onNext,
  onPrevious,
  onGrade,
  adjacentImageIds,
  grading,
}: ImageDetailViewProps) {
  const [showStars, setShowStars] = useState(false);
  const [showPsf, setShowPsf] = useState(false);
  const [psfImageLoading, setPsfImageLoading] = useState(false);
  const [maxStars] = useState(1000);
  const [imageSize, setImageSize] = useState<'screen' | 'large' | 'original'>('large');
  const [isOriginalLoaded, setIsOriginalLoaded] = useState(false);
  const [useOriginalImage, setUseOriginalImage] = useState(false);
  const [imageError, setImageError] = useState(false);

  // State machine to prevent feedback loops
  const imageStateRef = useRef<'large' | 'switching-to-original' | 'original'>('large');
  // Guard: request original-resolution generation at most once per image.
  const originalRequestedRef = useRef(false);
  const mainImageKey = `${dbId}:${imageId}:${showStars ? 'stars' : 'preview'}:${maxStars}`;
  const mainImageKeyRef = useRef(mainImageKey);
  mainImageKeyRef.current = mainImageKey;

  // 'fit': the view follows the image (refit whenever a new one loads).
  // 'user': the user owns the view — zoom percent and position are preserved
  // exactly across arrow-key navigation and preview↔original bitmap swaps,
  // until an explicit Fit/reset returns to 'fit'.
  const viewModeRef = useRef<'fit' | 'user'>('fit');

  // Initialize zoom functionality
  const zoom = useImageZoom({
    minScale: 0.1,
    maxScale: 10.0,
    onViewModeChange: (mode) => {
      viewModeRef.current = mode;
    },
  });

  // Check if image has overflow (is larger than container)
  const hasOverflow = zoom.zoomState.scale > 1 || 
    (zoom.containerRef.current && zoom.imageRef.current && 
     zoom.imageRef.current.naturalWidth && zoom.imageRef.current.naturalHeight &&
     (zoom.imageRef.current.naturalWidth * zoom.zoomState.scale > zoom.containerRef.current.clientWidth ||
      zoom.imageRef.current.naturalHeight * zoom.zoomState.scale > zoom.containerRef.current.clientHeight));

  // Preload adjacent images for smooth navigation
  const nextImageIds = adjacentImageIds ? 
    [...adjacentImageIds.next, ...adjacentImageIds.previous] : [];
  
  useImagePreloader(dbId, imageId, nextImageIds, {
    preloadCount: 2,
    includeAnnotated: showStars,
    includeStarData: showStars,
    imageSize: imageSize,
  });

  // Async loading for the main preview/annotated image (PSF has its own
  // endpoint and is not queued here). On a cache miss the server returns 202
  // and this drives a "Generating…" indicator, then reloads when ready.
  const mainSize: 'large' | 'original' = useOriginalImage ? 'original' : 'large';
  const largeNonPsfSrc = showStars
    ? apiClient.getAnnotatedUrl(dbId, imageId, 'large', maxStars)
    : apiClient.getPreviewUrl(dbId, imageId, { size: 'large' });
  const originalNonPsfSrc = showStars
    ? apiClient.getAnnotatedUrl(dbId, imageId, 'original', maxStars)
    : apiClient.getPreviewUrl(dbId, imageId, { size: 'original' });
  const nonPsfSrc = mainSize === 'original' ? originalNonPsfSrc : largeNonPsfSrc;
  const nonPsfDescriptor: PreviewDescriptor = showStars
    ? { imageId, kind: 'annotated', size: mainSize, maxStars }
    : { imageId, kind: 'preview', size: mainSize };
  const asyncImg = useAsyncImage(dbId, nonPsfSrc, nonPsfDescriptor);
  // `src` is what the <img> renders (may carry a `v=` cache-buster after a
  // generation-triggered reload); `baseSrc` is the stable identity used to
  // decide whether the visible artifact still belongs to this image/size.
  const [visibleMainImage, setVisibleMainImage] = useState<{
    key: string;
    src: string;
    baseSrc: string;
    loadedSrc: string | null;
  }>(() => ({
    key: mainImageKey,
    src: nonPsfSrc,
    baseSrc: nonPsfSrc,
    loadedSrc: null,
  }));
  const visibleMainSrc =
    visibleMainImage.key === mainImageKey ? visibleMainImage.src : nonPsfSrc;
  const visibleMainBaseSrc =
    visibleMainImage.key === mainImageKey ? visibleMainImage.baseSrc : nonPsfSrc;
  const loadedMainSrc =
    visibleMainImage.key === mainImageKey ? visibleMainImage.loadedSrc : null;
  const currentMainSrcIsLoaded =
    visibleMainSrc === loadedMainSrc;
  const currentNonPsfSources = [largeNonPsfSrc, originalNonPsfSrc];
  const visibleMainSrcIsCurrent =
    currentNonPsfSources.includes(visibleMainBaseSrc);

  // Fetch image details
  const { data: image, isLoading } = useQuery({
    queryKey: ['db', dbId, 'image', imageId],
    queryFn: () => apiClient.getImage(dbId, imageId),
    placeholderData: (previousData) => previousData, // Keep showing previous image while loading new one
  });

  // Fetch star detection
  const { data: starData, isLoading: starDataLoading } = useQuery({
    queryKey: ['db', dbId, 'stars', imageId],
    queryFn: () => apiClient.getStarDetection(dbId, imageId),
    enabled: showStars,
  });


  // Keyboard shortcuts
  useHotkeys('escape', onClose, [onClose]);
  useHotkeys('k,right', () => {
    onNext(); // K/Right goes to newer image (higher index in oldest-first sort)
  }, { enableOnFormTags: true }, [onNext]);
  useHotkeys('j,left', () => {
    onPrevious(); // J/Left goes to older image (lower index in oldest-first sort)
  }, { enableOnFormTags: true }, [onPrevious]);
  useHotkeys('a', () => onGrade('accepted'), [onGrade]);
  useHotkeys('x', () => onGrade('rejected'), [onGrade]);
  useHotkeys('u', () => onGrade('pending'), [onGrade]);
  useHotkeys('s', () => {
    setShowStars(s => !s);
    setShowPsf(false); // Turn off PSF when showing stars
  }, [showStars]);
  useHotkeys('p', () => {
    const newPsfState = !showPsf;
    setShowPsf(newPsfState);
    setShowStars(false); // Turn off stars when showing PSF
    if (newPsfState) {
      setPsfImageLoading(true);
    }
  }, [showPsf]);
  useHotkeys('z', () => setImageSize(s => s === 'screen' ? 'large' : 'screen'), []);
  useHotkeys('plus,equal', () => zoom.zoomIn(), [zoom.zoomIn]);
  useHotkeys('minus', () => zoom.zoomOut(), [zoom.zoomOut]);
  useHotkeys('0', () => zoom.resetZoom(), [zoom.resetZoom]);
  useHotkeys('1', () => zoom.zoomTo100(), [zoom.zoomTo100]);
  useHotkeys('f', () => zoom.zoomToFit(), [zoom.zoomToFit]);

  // Load original dimensions from metadata if available
  useEffect(() => {
    // Try different possible field names for image dimensions
    const width = image?.metadata?.ImageWidth || 
                  image?.metadata?.NAXIS1 || 
                  image?.metadata?.ImageSize?.[0];
    const height = image?.metadata?.ImageHeight || 
                   image?.metadata?.NAXIS2 || 
                   image?.metadata?.ImageSize?.[1];
    
    if (width && height && typeof width === 'number' && typeof height === 'number') {
      zoom.setImageDimensions(width, height, true);
    }
  }, [image, zoom]);
  
  // Combined effect for image preloading and switching. Triggers on the RAW
  // scale of the displayed bitmap: scale > 1 means the current bitmap is
  // being magnified past its native pixels (blurry) and the original-
  // resolution artifact is needed, whichever preview size is showing.
  useEffect(() => {
    if (!image || showPsf) return;

    const scale = zoom.zoomState.scale;
    const state = imageStateRef.current;

    // Preload near 1:1. Route through the interactive queue so an uncached
    // 'original' is actually generated (its 202 is not a failure); only flip
    // to it once generation completes.
    if (scale > 0.8 && state === 'large' && !isOriginalLoaded && !originalRequestedRef.current) {
      originalRequestedRef.current = true;
      const originalUrl = showStars
        ? apiClient.getAnnotatedUrl(dbId, imageId, 'original', maxStars)
        : apiClient.getPreviewUrl(dbId, imageId, { size: 'original' });
      const originalDescriptor: PreviewDescriptor = showStars
        ? { imageId, kind: 'annotated', size: 'original', maxStars }
        : { imageId, kind: 'preview', size: 'original' };

      const requestedKey = mainImageKey;
      void ensurePreviewReady(dbId, originalUrl, originalDescriptor).then((ok) => {
        // With zoom persisting across navigation these requests fire on every
        // image; ignore resolutions that arrive after we moved on.
        if (mainImageKeyRef.current !== requestedKey) return;
        if (ok) {
          setIsOriginalLoaded(true);
        } else {
          // Transient generation failure — clear the guard so the next zoom
          // change retries instead of leaving this image stuck on the preview.
          originalRequestedRef.current = false;
        }
      });
    }

    // State machine transitions - only switch to original, never back
    if (state === 'large' && scale >= 1.0 && isOriginalLoaded) {
      // Switch to original
      imageStateRef.current = 'switching-to-original';
      setUseOriginalImage(true);
    }
    // Never switch back from original to large
  }, [zoom, imageId, showStars, showPsf, image, isOriginalLoaded, dbId, maxStars, mainImageKey]);
  
  // Reset preload state when image changes. Deliberately does NOT touch the
  // zoom state: in 'user' view mode the zoom/pan carries over to the next
  // image (reportBitmapLoaded reconciles it against the incoming bitmap).
  useLayoutEffect(() => {
    setIsOriginalLoaded(false);
    setUseOriginalImage(false);
    originalRequestedRef.current = false;
    setVisibleMainImage({
      key: mainImageKey,
      src: largeNonPsfSrc,
      baseSrc: largeNonPsfSrc,
      loadedSrc: null,
    });
    imageStateRef.current = 'large';
    setImageError(false);
  }, [dbId, imageId, showStars, maxStars, mainImageKey, largeNonPsfSrc]);

  // A bitmap finished loading into one of the <img> elements. Hand its
  // dimensions to the zoom hook: fit when the view is in 'fit' mode, keep the
  // view (exactly, or remapped across a size change) when the user owns it or
  // when this load is the large→original swap.
  const reportBitmapLoaded = (img: HTMLImageElement) => {
    const width = img.naturalWidth;
    const height = img.naturalHeight;
    if (!width || !height) return;

    zoom.setImageDimensions(width, height, useOriginalImage);

    const switching = imageStateRef.current === 'switching-to-original';
    zoom.applyBitmapDimensions(
      width,
      height,
      switching || viewModeRef.current === 'user' ? 'preserve' : 'fit'
    );
    if (switching) {
      imageStateRef.current = 'original';
    }
  };

  // Show loading state only on initial load
  if (!image && isLoading) {
    return (
      <div className="image-detail-overlay">
        <div className="image-detail">
          <div className="detail-loading">
            <div className="loading-spinner"></div>
          </div>
        </div>
      </div>
    );
  }

  // If no image data at all, close the modal
  if (!image) {
    onClose();
    return null;
  }

  const getStatusClass = () => {
    switch (image.grading_status) {
      case GradingStatus.Accepted:
        return 'status-accepted';
      case GradingStatus.Rejected:
        return 'status-rejected';
      default:
        return 'status-pending';
    }
  };

  const formatDate = (timestamp: number | null) => {
    if (!timestamp) return 'Unknown';
    return new Date(timestamp * 1000).toLocaleString();
  };

  // Hide until the visible src has actually loaded AND still belongs to this
  // image/size — never show a stale or errored <img> (e.g. one whose original
  // request 202'd while its regenerated copy loads under a `v=` buster).
  const hideMainImage =
    !showPsf && (!visibleMainSrcIsCurrent || !currentMainSrcIsLoaded);

  return (
    <div className="image-detail-overlay" onClick={onClose}>
      <div className="image-detail" onClick={e => e.stopPropagation()}>
        <div className="detail-header">
          <h2>{image.target_name} - {image.filter_name || 'No filter'}</h2>
          <div className={`status-banner ${getStatusClass()}`}>
            {image.grading_status === GradingStatus.Accepted && '✓ ACCEPTED'}
            {image.grading_status === GradingStatus.Rejected && '✗ REJECTED'}
            {image.grading_status === GradingStatus.Pending && '○ PENDING'}
          </div>
          <button className="close-button" onClick={onClose}>×</button>
        </div>

        <div className="detail-content">
          <div className="detail-image">
            <div 
              className={`image-container zoom-container ${hasOverflow ? 'has-overflow' : ''}`}
              ref={zoom.containerRef}
              onWheel={zoom.handleWheel}
              onMouseDown={zoom.handleMouseDown}
              onMouseMove={zoom.handleMouseMove}
              onMouseUp={zoom.handleMouseUp}
              onMouseLeave={zoom.handleMouseUp}
              tabIndex={0}
              onKeyDown={zoom.handleKeyDown}
            >
              {/* Loading overlay for star detection and PSF views */}
              {(showStars && starDataLoading) || (showPsf && psfImageLoading) ? (
                <div className="image-loading-overlay">
                  <div className="loading-content">
                    <div className="loading-spinner"></div>
                    <span className="loading-text">
                      {showStars && starDataLoading ? 'Loading star detection...' : 'Loading PSF analysis...'}
                    </span>
                  </div>
                </div>
              ) : null}
              {imageError || (!showPsf && asyncImg.state === 'error') ? (
                <div className="detail-image-error" style={{
                  width: '100%',
                  height: '100%',
                  minHeight: '500px',
                  display: 'flex',
                  alignItems: 'center',
                  justifyContent: 'center',
                  background: '#1a1a1a',
                  borderRadius: '8px'
                }}>
                  <div style={{
                    textAlign: 'center',
                    color: '#888'
                  }}>
                    <svg width="64" height="64" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" style={{ marginBottom: '16px' }}>
                      <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                      <line x1="9" y1="9" x2="15" y2="15" />
                      <line x1="15" y1="9" x2="9" y2="15" />
                    </svg>
                    <h3 style={{ margin: '0 0 8px 0', fontSize: '1.2rem', fontWeight: 'normal' }}>Image not available</h3>
                    <p style={{ margin: 0, fontSize: '0.9rem', color: '#666' }}>The requested image could not be found on the server</p>
                  </div>
                </div>
              ) : (
                <>
                {!showPsf && visibleMainSrc !== asyncImg.src && asyncImg.state !== 'error' && (
                  <img
                    className="detail-image-preload"
                    src={asyncImg.src}
                    alt=""
                    aria-hidden="true"
                    loading="eager"
                    onError={asyncImg.onError}
                    onLoad={(e) => {
                      asyncImg.onLoad();
                      reportBitmapLoaded(e.currentTarget);
                      setVisibleMainImage({
                        key: mainImageKey,
                        src: asyncImg.src,
                        baseSrc: nonPsfSrc,
                        loadedSrc: asyncImg.src,
                      });
                    }}
                    draggable={false}
                  />
                )}
                <img
                  ref={zoom.imageRef}
                  key={`${imageId}-${showStars ? 'stars' : showPsf ? 'psf' : 'normal'}`}
                  className={[
                    'detail-main-image',
                    hideMainImage ? 'detail-image-hidden' : null,
                  ].filter(Boolean).join(' ')}
                  src={
                    showPsf
                      ? apiClient.getPsfUrl(dbId, imageId, {
                          num_stars: 9,
                          psf_type: 'moffat',
                          sort_by: 'r2',
                          selection: 'top-n',
                        })
                      : visibleMainSrc
                  }
                  alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
                  style={{
                    transform: `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`,
                    cursor: zoom.zoomState.scale > 1 ? 'grab' : 'default',
                    transformOrigin: '0 0',
                  }}
                  onError={
                    showPsf
                      ? () => setImageError(true)
                      : visibleMainSrc === asyncImg.src
                        ? asyncImg.onError
                        : undefined
                  }
                  onLoad={(e) => {
                    // The PSF diagnostic image never feeds the zoom
                    // calibration — its dimensions are unrelated to the
                    // preview/original bitmaps.
                    if (showPsf) {
                      setPsfImageLoading(false);
                      return;
                    }

                    if (visibleMainSrc === asyncImg.src) {
                      asyncImg.onLoad();
                    }
                    if (!visibleMainSrcIsCurrent) {
                      return;
                    }

                    reportBitmapLoaded(e.currentTarget);
                    setVisibleMainImage((current) =>
                      current.key === mainImageKey && current.src === visibleMainSrc
                        ? { ...current, loadedSrc: visibleMainSrc }
                        : current
                    );
                  }}
                  draggable={false}
                />
                </>
              )}
              {!showPsf &&
                (asyncImg.state === 'generating' || asyncImg.state === 'loading') && (
                  <div className="image-loading-overlay">
                    <div className="loading-content">
                      <div className="loading-spinner"></div>
                      <span className="loading-text">
                        {asyncImg.state === 'generating'
                          ? 'Generating preview…'
                          : 'Loading…'}
                      </span>
                    </div>
                  </div>
                )}
            </div>
          </div>

          <div className="detail-info">
            <div className="info-section">
              <h3>Image Information</h3>
              
              {/* Date on its own row */}
              <div className="date-row">
                <span className="date-label">Date:</span>
                <span className="date-value">{formatDate(image.acquired_date)}</span>
              </div>
              
              {/* Camera on its own row */}
              {image.metadata?.Camera !== undefined && (
                <div className="date-row">
                  <span className="date-label">Camera:</span>
                  <span className="date-value">{image.metadata.Camera}</span>
                </div>
              )}
              
              {/* Two-column layout for other metadata */}
              <dl>
                {starData && (
                  <>
                    <dt>Stars:</dt>
                    <dd>{starData.detected_stars}</dd>
                    <dt>Avg HFR:</dt>
                    <dd>{starData.average_hfr.toFixed(2)}</dd>
                    <dt>Avg FWHM:</dt>
                    <dd>{starData.average_fwhm.toFixed(2)}</dd>
                  </>
                )}
                
                {image.metadata?.Min !== undefined && (
                  <>
                    <dt>Min:</dt>
                    <dd>{typeof image.metadata.Min === 'number' ? image.metadata.Min.toFixed(0) : image.metadata.Min}</dd>
                  </>
                )}
                
                {image.metadata?.Mean !== undefined && (
                  <>
                    <dt>Mean:</dt>
                    <dd>{typeof image.metadata.Mean === 'number' ? image.metadata.Mean.toFixed(1) : image.metadata.Mean}</dd>
                  </>
                )}
                
                {image.metadata?.Median !== undefined && (
                  <>
                    <dt>Median:</dt>
                    <dd>{typeof image.metadata.Median === 'number' ? image.metadata.Median.toFixed(1) : image.metadata.Median}</dd>
                  </>
                )}
                
                {image.metadata?.HFR !== undefined && (
                  <>
                    <dt>HFR:</dt>
                    <dd>{typeof image.metadata.HFR === 'number' ? image.metadata.HFR.toFixed(2) : image.metadata.HFR}</dd>
                  </>
                )}
                
                {image.metadata?.DetectedStars !== undefined && (
                  <>
                    <dt>Det. Stars:</dt>
                    <dd>{image.metadata.DetectedStars}</dd>
                  </>
                )}
                
                {image.metadata?.Exposure !== undefined && (
                  <>
                    <dt>Exposure:</dt>
                    <dd>{image.metadata.Exposure}s</dd>
                  </>
                )}
                
                {image.metadata?.Temperature !== undefined && (
                  <>
                    <dt>Temp:</dt>
                    <dd>{image.metadata.Temperature}°C</dd>
                  </>
                )}
                
                {image.metadata?.Gain !== undefined && (
                  <>
                    <dt>Gain:</dt>
                    <dd>{image.metadata.Gain}</dd>
                  </>
                )}
              </dl>
              
              {image.reject_reason && (
                <div className="reject-reason">
                  <strong>Reject Reason:</strong>
                  <p>{image.reject_reason}</p>
                </div>
              )}
            </div>

            <div className="detail-actions">
              <button 
                className="action-button accept" 
                onClick={() => onGrade('accepted')}
              >
                Accept (A)
              </button>
              <button 
                className="action-button reject" 
                onClick={() => onGrade('rejected')}
              >
                Reject (X)
              </button>
              <button 
                className="action-button pending" 
                onClick={() => onGrade('pending')}
              >
                Unmark (U)
              </button>
            </div>

            {/* Undo/Redo Toolbar (only show if grading system is provided) */}
            {grading && (
              <UndoRedoToolbar
                canUndo={grading.canUndo}
                canRedo={grading.canRedo}
                isProcessing={grading.isLoading}
                undoStackSize={grading.undoStackSize}
                redoStackSize={grading.redoStackSize}
                onUndo={grading.undo}
                onRedo={grading.redo}
                getLastAction={grading.getLastAction}
                getNextRedoAction={grading.getNextRedoAction}
                className="compact"
              />
            )}

            <div className="detail-shortcuts">
              <div className="shortcut-grid">
                <span>K/→ Next</span>
                <span>J/← Prev</span>
                <span>A Accept</span>
                <span>X Reject</span>
                <span>U Pending</span>
                <span>S Stars {showStars ? '✓' : ''}</span>
                <span>P PSF {showPsf ? '✓' : ''}</span>
                <span>Z Size</span>
                {grading && <span>⌘Z Undo</span>}
                {grading && <span>⌘Y Redo</span>}
              </div>
            </div>

            {/* Compact Zoom Controls at Bottom */}
            <div className="zoom-section-bottom">
              <div className="zoom-info-compact">
                <span className="zoom-percentage-compact">{zoom.getZoomPercentage()}%</span>
              </div>
              <div className="zoom-buttons-compact">
                <button 
                  className="zoom-btn-compact" 
                  onClick={zoom.zoomOut}
                  title="Zoom Out (-)"
                >
                  -
                </button>
                <button 
                  className="zoom-btn-compact" 
                  onClick={zoom.zoomToFit}
                  title="Fit to Screen (F)"
                >
                  Fit
                </button>
                <button 
                  className="zoom-btn-compact" 
                  onClick={zoom.zoomTo100}
                  title="100% (1)"
                >
                  100%
                </button>
                <button 
                  className="zoom-btn-compact" 
                  onClick={zoom.zoomIn}
                  title="Zoom In (+)"
                >
                  +
                </button>
              </div>
            </div>
          </div>
        </div>
      </div>
    </div>
  );
}
