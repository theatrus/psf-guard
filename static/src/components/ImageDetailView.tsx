import { useState, useEffect, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import { GradingStatus } from '../api/types';
import { useImagePreloader } from '../hooks/useImagePreloader';
import { useImageZoom } from '../hooks/useImageZoom';
import UndoRedoToolbar from './UndoRedoToolbar';

interface ImageDetailViewProps {
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
  const [imageSize, setImageSize] = useState<'screen' | 'large' | 'original'>('large');
  const [isOriginalLoaded, setIsOriginalLoaded] = useState(false);
  const preloadedOriginalRef = useRef<HTMLImageElement | null>(null);
  const [useOriginalImage, setUseOriginalImage] = useState(false);
  const imageDimensionsRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 });
  const [imageError, setImageError] = useState(false);
  
  // State machine to prevent feedback loops
  const imageStateRef = useRef<'large' | 'switching-to-original' | 'original'>('large');

  // Initialize zoom functionality
  const zoom = useImageZoom({
    minScale: 0.1,
    maxScale: 10.0,
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
  
  useImagePreloader(imageId, nextImageIds, {
    preloadCount: 2,
    includeAnnotated: showStars,
    includeStarData: showStars,
    imageSize: imageSize,
  });

  // Fetch image details
  const { data: image, isLoading, isFetching } = useQuery({
    queryKey: ['image', imageId],
    queryFn: () => apiClient.getImage(imageId),
    placeholderData: (previousData) => previousData, // Keep showing previous image while loading new one
  });

  // Fetch star detection
  const { data: starData, isLoading: starDataLoading } = useQuery({
    queryKey: ['stars', imageId],
    queryFn: () => apiClient.getStarDetection(imageId),
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

  // Track zoom state to detect user interactions
  const previousZoomState = useRef(zoom.zoomState);
  const lastUserZoomRef = useRef<number>(0);
  const lastImageIdRef = useRef<number>(imageId);
  const USER_ZOOM_COOLDOWN = 2000; // 2 seconds
  
  // Track user zoom interactions by monitoring zoom state changes
  useEffect(() => {
    const currentZoom = zoom.zoomState;
    const prevZoom = previousZoomState.current;
    
    // Check if zoom changed and it wasn't from an image change
    const zoomChanged = currentZoom.scale !== prevZoom.scale || 
                       currentZoom.offsetX !== prevZoom.offsetX || 
                       currentZoom.offsetY !== prevZoom.offsetY;
    
    const imageChanged = imageId !== lastImageIdRef.current;
    
    // If zoom changed but image didn't change, it was a user interaction
    if (zoomChanged && !imageChanged) {
      lastUserZoomRef.current = Date.now();
    }
    
    previousZoomState.current = currentZoom;
    lastImageIdRef.current = imageId;
  }, [zoom.zoomState, imageId]);

  // Reset zoom only when image ID changes and user hasn't zoomed recently
  useEffect(() => {
    const timeSinceUserZoom = Date.now() - lastUserZoomRef.current;
    
    // Only auto-fit if user hasn't zoomed recently
    if (timeSinceUserZoom > USER_ZOOM_COOLDOWN) {
      const timer = setTimeout(() => {
        zoom.zoomToFit();
      }, 300);
      
      return () => clearTimeout(timer);
    }
  }, [imageId, zoom.zoomToFit]); // eslint-disable-line react-hooks/exhaustive-deps
  
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
  
  // Combined effect for image preloading and switching
  useEffect(() => {
    if (!image || showPsf) return;
    
    const visualScale = zoom.getVisualScale();
    const state = imageStateRef.current;
    
    // Preload when visual zoom > 80%
    if (visualScale > 0.8 && !isOriginalLoaded && state === 'large') {
      const originalUrl = showStars 
        ? apiClient.getAnnotatedUrl(imageId, 'original')
        : apiClient.getPreviewUrl(imageId, { size: 'original' });
      
      const img = new Image();
      img.src = originalUrl;
      preloadedOriginalRef.current = img;
      
      img.onload = () => {
        setIsOriginalLoaded(true);
      };
    }
    
    // State machine transitions based on visual scale - only switch to original, never back
    if (state === 'large' && visualScale > 1.0 && isOriginalLoaded) {
      // Switch to original
      imageStateRef.current = 'switching-to-original';
      setUseOriginalImage(true);
      // Delay state update to allow render
      setTimeout(() => {
        if (imageStateRef.current === 'switching-to-original') {
          imageStateRef.current = 'original';
        }
      }, 300);
    }
    // Never switch back from original to large
  }, [zoom, imageId, showStars, showPsf, image, isOriginalLoaded]);
  
  // Reset preload state when image changes
  useEffect(() => {
    setIsOriginalLoaded(false);
    preloadedOriginalRef.current = null;
    setUseOriginalImage(false);
    imageDimensionsRef.current = { width: 0, height: 0 };
    imageStateRef.current = 'large';
    setImageError(false);
  }, [imageId, showStars]);

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
              {imageError ? (
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
                <img
                  ref={zoom.imageRef}
                  key={`${imageId}-${showStars ? 'stars' : showPsf ? 'psf' : 'normal'}`}
                  className={isFetching ? 'loading' : ''}
                  src={
                    showPsf
                      ? apiClient.getPsfUrl(imageId, { 
                          num_stars: 9,
                          psf_type: 'moffat',
                          sort_by: 'r2',
                          selection: 'top-n'
                        })
                      : showStars 
                        ? apiClient.getAnnotatedUrl(imageId, useOriginalImage ? 'original' : 'large')
                        : apiClient.getPreviewUrl(imageId, { size: useOriginalImage ? 'original' : 'large' })
                  }
                  alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
                  style={{
                    transform: `translate(${zoom.zoomState.offsetX}px, ${zoom.zoomState.offsetY}px) scale(${zoom.zoomState.scale})`,
                    cursor: zoom.zoomState.scale > 1 ? 'grab' : 'default',
                    transformOrigin: '0 0',
                  }}
                  onError={() => setImageError(true)}
                  onLoad={(e) => {
                  // Remove loading class when image loads
                  e.currentTarget.classList.remove('loading');
                  
                  // Clear PSF loading state when PSF image loads
                  if (showPsf) {
                    setPsfImageLoading(false);
                  }
                  
                  const img = e.currentTarget;
                  const newWidth = img.naturalWidth;
                  const newHeight = img.naturalHeight;
                  const oldWidth = imageDimensionsRef.current.width;
                  const oldHeight = imageDimensionsRef.current.height;
                  
                  // Update image dimensions in zoom hook
                  zoom.setImageDimensions(newWidth, newHeight, useOriginalImage);
                  
                  // Check if dimensions actually changed (indicating size switch)
                  const dimensionsChanged = oldWidth > 0 && (Math.abs(newWidth - oldWidth) > 10 || Math.abs(newHeight - oldHeight) > 10);
                  
                  if (dimensionsChanged && imageStateRef.current === 'switching-to-original') {
                    // Adjust zoom to maintain visual continuity
                    zoom.adjustZoomForNewImage(oldWidth, oldHeight, newWidth, newHeight);
                  } else if (imageDimensionsRef.current.width === 0) {
                    // Only zoom to fit on initial load
                    setTimeout(() => {
                      zoom.zoomToFit();
                    }, 50);
                  }
                  
                  // Update stored dimensions
                  imageDimensionsRef.current = { width: newWidth, height: newHeight };
                  }}
                  draggable={false}
                />
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