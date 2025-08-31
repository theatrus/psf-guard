import { useState, useCallback, useEffect, useRef } from 'react';
import { useQuery } from '@tanstack/react-query';
import { useHotkeys } from 'react-hotkeys-hook';
import { apiClient } from '../api/client';
import { GradingStatus, type Image } from '../api/types';
import { useImageZoom } from '../hooks/useImageZoom';

interface ImageComparisonViewProps {
  leftImageId: number;
  rightImageId: number | null;
  onClose: () => void;
  onSelectRightImage: () => void;
  onGradeLeft: (status: 'accepted' | 'rejected' | 'pending') => void;
  onGradeRight: (status: 'accepted' | 'rejected' | 'pending') => void;
  onNavigateRightNext: () => void;
  onNavigateRightPrev: () => void;
  onSwapImages: () => void;
}

export default function ImageComparisonView({
  leftImageId,
  rightImageId,
  onClose,
  onSelectRightImage,
  onGradeLeft,
  onGradeRight,
  onNavigateRightNext,
  onNavigateRightPrev,
  onSwapImages,
}: ImageComparisonViewProps) {
  const [showStars, setShowStars] = useState(false);
  const [syncZoom, setSyncZoom] = useState(true);
  
  // Initialize zoom for both images
  const leftZoom = useImageZoom({ minScale: 0.1, maxScale: 10.0 });
  const rightZoom = useImageZoom({ minScale: 0.1, maxScale: 10.0 });
  
  // Track image loading
  const [leftImageLoaded, setLeftImageLoaded] = useState(false);
  const [rightImageLoaded, setRightImageLoaded] = useState(false);
  const [leftImageError, setLeftImageError] = useState(false);
  const [rightImageError, setRightImageError] = useState(false);
  
  // Track original image preloading
  const [leftOriginalLoaded, setLeftOriginalLoaded] = useState(false);
  const [rightOriginalLoaded, setRightOriginalLoaded] = useState(false);
  const leftOriginalRef = useRef<HTMLImageElement | null>(null);
  const rightOriginalRef = useRef<HTMLImageElement | null>(null);
  
  // Track whether to use original images
  const [useLeftOriginal, setUseLeftOriginal] = useState(false);
  const [useRightOriginal, setUseRightOriginal] = useState(false);
  const leftDimensionsRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 });
  const rightDimensionsRef = useRef<{ width: number; height: number }>({ width: 0, height: 0 });
  
  // State machine to prevent feedback loops
  const leftImageStateRef = useRef<'large' | 'switching-to-original' | 'original'>('large');
  const rightImageStateRef = useRef<'large' | 'switching-to-original' | 'original'>('large');

  // Fetch left image
  const { data: leftImage } = useQuery({
    queryKey: ['image', leftImageId],
    queryFn: () => apiClient.getImage(leftImageId),
  });

  // Fetch right image (if selected)
  const { data: rightImage, isFetching: isRightImageFetching } = useQuery({
    queryKey: ['image', rightImageId],
    queryFn: () => rightImageId ? apiClient.getImage(rightImageId) : Promise.resolve(null),
    enabled: !!rightImageId,
    placeholderData: (previousData) => previousData, // Keep showing previous image while loading new one
  });

  // Keyboard shortcuts
  useHotkeys('escape', onClose, [onClose]);
  useHotkeys('s', () => setShowStars(s => !s), []);
  useHotkeys('z', () => setSyncZoom(s => !s), []);
  
  // Grade left image
  useHotkeys('1', () => onGradeLeft('accepted'), [onGradeLeft]);
  useHotkeys('2', () => onGradeLeft('rejected'), [onGradeLeft]);
  useHotkeys('3', () => onGradeLeft('pending'), [onGradeLeft]);
  
  // Grade right image
  useHotkeys('7', () => rightImageId && onGradeRight('accepted'), [rightImageId, onGradeRight]);
  useHotkeys('8', () => rightImageId && onGradeRight('rejected'), [rightImageId, onGradeRight]);
  useHotkeys('9', () => rightImageId && onGradeRight('pending'), [rightImageId, onGradeRight]);
  
  // Navigate right image
  useHotkeys('right,k', onNavigateRightNext, [onNavigateRightNext]);
  useHotkeys('left,j', onNavigateRightPrev, [onNavigateRightPrev]);
  
  // Swap images (prevent swapping identical images)
  useHotkeys('w', () => rightImageId && leftImageId !== rightImageId && onSwapImages(), [rightImageId, leftImageId, onSwapImages]);

  // Reset loaded state when images change
  useEffect(() => {
    setLeftImageLoaded(false);
    setLeftImageError(false);
    leftZoom.resetInitialization();
  }, [leftImageId, showStars, leftZoom]);
  
  useEffect(() => {
    setRightImageLoaded(false);
    setRightImageError(false);
    rightZoom.resetInitialization();
  }, [rightImageId, showStars, rightZoom]);


  // Store the target zoom level when user zooms
  const targetRightZoomRef = useRef<number | null>(null);
  const targetLeftZoomRef = useRef<number | null>(null);
  
  // Capture zoom intent before image change
  useEffect(() => {
    const visualScale = rightZoom.getVisualScale();
    console.log('[Zoom Capture] Right zoom state changed, visual scale:', visualScale);
    if (visualScale > 1.2 || visualScale < 0.8) {
      targetRightZoomRef.current = visualScale;
      console.log('[Zoom Capture] Stored target zoom for right:', visualScale);
    }
  }, [rightZoom.zoomState.scale, rightZoom]);
  
  useEffect(() => {
    const visualScale = leftZoom.getVisualScale();
    if (visualScale > 1.2 || visualScale < 0.8) {
      targetLeftZoomRef.current = visualScale;
    }
  }, [leftZoom.zoomState.scale, leftZoom]);
  
  // Fit images when they load
  useEffect(() => {
    if (leftImageLoaded) {
      if (targetLeftZoomRef.current === null) {
        leftZoom.zoomToFit();
      }
      // Otherwise preserve existing zoom
    }
  }, [leftImageLoaded, leftZoom]);
  
  useEffect(() => {
    if (rightImageLoaded) {
      console.log('[Auto-fit Effect] Right image loaded, targetZoom:', targetRightZoomRef.current);
      if (targetRightZoomRef.current === null) {
        console.log('[Auto-fit Effect] No target zoom, calling zoomToFit');
        rightZoom.zoomToFit();
      } else {
        console.log('[Auto-fit Effect] Preserving zoom, not calling zoomToFit');
      }
    }
  }, [rightImageLoaded, rightZoom]);
  
  // Force switch to original when image loads with high zoom
  useEffect(() => {
    console.log('[Force Switch Effect] Checking right:', {
      rightImageLoaded,
      rightOriginalLoaded,
      useRightOriginal,
      targetZoom: targetRightZoomRef.current,
      useLeftOriginal
    });
    if (rightImageLoaded && rightOriginalLoaded && !useRightOriginal) {
      // Use stored target zoom instead of current zoom which might be reset
      // OR if left is using original, right should match
      if ((targetRightZoomRef.current && targetRightZoomRef.current > 1.0) || useLeftOriginal) {
        console.log('[Force Switch Effect] Switching right to original! (left using original:', useLeftOriginal, ')');
        // We have high zoom and original is ready but not being used - switch now
        setUseRightOriginal(true);
        rightImageStateRef.current = 'original';
      }
    }
  }, [rightImageLoaded, rightOriginalLoaded, useRightOriginal, useLeftOriginal]);
  
  useEffect(() => {
    if (leftImageLoaded && leftOriginalLoaded && !useLeftOriginal) {
      // Use stored target zoom instead of current zoom which might be reset
      if (targetLeftZoomRef.current && targetLeftZoomRef.current > 1.0) {
        // We have high zoom and original is ready but not being used - switch now
        setUseLeftOriginal(true);
        leftImageStateRef.current = 'original';
      }
    }
  }, [leftImageLoaded, leftOriginalLoaded, useLeftOriginal]);
  
  // Load original dimensions from metadata if available
  useEffect(() => {
    // Try different possible field names for image dimensions
    const width = leftImage?.metadata?.ImageWidth || 
                  leftImage?.metadata?.NAXIS1 || 
                  leftImage?.metadata?.ImageSize?.[0];
    const height = leftImage?.metadata?.ImageHeight || 
                   leftImage?.metadata?.NAXIS2 || 
                   leftImage?.metadata?.ImageSize?.[1];
    
    if (width && height && typeof width === 'number' && typeof height === 'number') {
      leftZoom.setImageDimensions(width, height, true);
    }
  }, [leftImage, leftZoom]);
  
  // Combined effect for left image preloading and switching
  useEffect(() => {
    if (!leftImage) return;
    
    const visualScale = leftZoom.getVisualScale();
    const state = leftImageStateRef.current;
    
    // Preload when visual zoom > 80%, or immediately if zoom > 100% (preserved from previous image)
    // Also check stored target zoom in case current zoom is reset
    const shouldPreload = (visualScale > 0.8 && state === 'large') || 
                         (visualScale > 1.0 && state === 'large') ||
                         (targetLeftZoomRef.current && targetLeftZoomRef.current > 0.8 && state === 'large');
    
    if (shouldPreload && !leftOriginalLoaded && !leftOriginalRef.current) {
      const originalUrl = showStars 
        ? apiClient.getAnnotatedUrl(leftImageId, 'original')
        : apiClient.getPreviewUrl(leftImageId, { size: 'original' });
      
      const img = new Image();
      img.src = originalUrl;
      leftOriginalRef.current = img;
      
      img.onload = () => {
        setLeftOriginalLoaded(true);
        
        // If we were waiting for original due to high zoom, switch immediately
        const currentVisualScale = leftZoom.getVisualScale();
        const currentState = leftImageStateRef.current;
        if (currentState === 'large' && currentVisualScale > 1.0) {
          leftImageStateRef.current = 'switching-to-original';
          setUseLeftOriginal(true);
          // Delay state update to allow render
          setTimeout(() => {
            if (leftImageStateRef.current === 'switching-to-original') {
              leftImageStateRef.current = 'original';
            }
          }, 300);
        }
      };
    }
    
    // State machine transitions based on visual scale - only switch to original, never back
    if (state === 'large' && visualScale > 1.0 && leftOriginalLoaded) {
      // Switch to original
      leftImageStateRef.current = 'switching-to-original';
      setUseLeftOriginal(true);
      // Delay state update to allow render
      setTimeout(() => {
        if (leftImageStateRef.current === 'switching-to-original') {
          leftImageStateRef.current = 'original';
        }
      }, 300);
    }
    // Never switch back from original to large
  }, [leftZoom, leftImageId, showStars, leftImage, leftOriginalLoaded]);
  
  // Load original dimensions from metadata if available
  useEffect(() => {
    // Try different possible field names for image dimensions
    const width = rightImage?.metadata?.ImageWidth || 
                  rightImage?.metadata?.NAXIS1 || 
                  rightImage?.metadata?.ImageSize?.[0];
    const height = rightImage?.metadata?.ImageHeight || 
                   rightImage?.metadata?.NAXIS2 || 
                   rightImage?.metadata?.ImageSize?.[1];
    
    if (width && height && typeof width === 'number' && typeof height === 'number') {
      rightZoom.setImageDimensions(width, height, true);
    }
  }, [rightImage, rightZoom]);
  
  // Combined effect for right image preloading and switching
  useEffect(() => {
    if (!rightImage || !rightImageId) return;
    
    const visualScale = rightZoom.getVisualScale();
    const state = rightImageStateRef.current;
    
    console.log('[Preload Effect] Right image check:', {
      imageId: rightImageId,
      visualScale,
      state,
      targetZoom: targetRightZoomRef.current,
      rightOriginalLoaded,
      hasOriginalRef: !!rightOriginalRef.current
    });
    
    // Preload when visual zoom > 80%, or immediately if zoom > 100% (preserved from previous image)
    // Also check stored target zoom in case current zoom is reset
    // Most importantly: if left is using original, right should too
    const shouldPreload = (visualScale > 0.8 && state === 'large') || 
                         (visualScale > 1.0 && state === 'large') ||
                         (targetRightZoomRef.current && targetRightZoomRef.current > 0.8 && state === 'large') ||
                         (useLeftOriginal && state === 'large');
    
    console.log('[Preload Effect] Should preload?', shouldPreload, 'useLeftOriginal:', useLeftOriginal);
    
    if (shouldPreload && !rightOriginalLoaded && !rightOriginalRef.current) {
      console.log('[Preload Effect] Starting original preload for right image');
      const originalUrl = showStars 
        ? apiClient.getAnnotatedUrl(rightImageId, 'original')
        : apiClient.getPreviewUrl(rightImageId, { size: 'original' });
      
      const img = new Image();
      img.src = originalUrl;
      rightOriginalRef.current = img;
      
      img.onload = () => {
        setRightOriginalLoaded(true);
        
        // If we were waiting for original due to high zoom OR left is using original, switch immediately
        const currentVisualScale = rightZoom.getVisualScale();
        const currentState = rightImageStateRef.current;
        if (currentState === 'large' && (currentVisualScale > 1.0 || useLeftOriginal)) {
          rightImageStateRef.current = 'switching-to-original';
          setUseRightOriginal(true);
          // Delay state update to allow render
          setTimeout(() => {
            if (rightImageStateRef.current === 'switching-to-original') {
              rightImageStateRef.current = 'original';
            }
          }, 300);
        }
      };
    }
    
    // State machine transitions based on visual scale OR left image state
    if (state === 'large' && ((visualScale > 1.0 && rightOriginalLoaded) || (useLeftOriginal && rightOriginalLoaded))) {
      // Switch to original
      rightImageStateRef.current = 'switching-to-original';
      setUseRightOriginal(true);
      // Delay state update to allow render
      setTimeout(() => {
        if (rightImageStateRef.current === 'switching-to-original') {
          rightImageStateRef.current = 'original';
        }
      }, 300);
    }
    // Never switch back from original to large
  }, [rightZoom, rightImageId, showStars, rightImage, rightOriginalLoaded, useLeftOriginal]);
  
  // Effect to make right image follow left image's resolution choice
  useEffect(() => {
    if (useLeftOriginal && !useRightOriginal && rightOriginalLoaded && rightImageStateRef.current !== 'switching-to-original') {
      console.log('[Follow Left Effect] Left is using original, switching right to match');
      rightImageStateRef.current = 'switching-to-original';
      setUseRightOriginal(true);
      setTimeout(() => {
        if (rightImageStateRef.current === 'switching-to-original') {
          rightImageStateRef.current = 'original';
        }
      }, 300);
    }
  }, [useLeftOriginal, useRightOriginal, rightOriginalLoaded]);

  // Reset preload state when images change
  useEffect(() => {
    setLeftOriginalLoaded(false);
    leftOriginalRef.current = null;
    setUseLeftOriginal(false);
    leftDimensionsRef.current = { width: 0, height: 0 };
    leftImageStateRef.current = 'large';
  }, [leftImageId, showStars]);
  
  useEffect(() => {
    console.log('[Reset Effect] Right image changing to:', rightImageId);
    console.log('[Reset Effect] Current targetRightZoomRef:', targetRightZoomRef.current);
    setRightOriginalLoaded(false);
    rightOriginalRef.current = null;
    setUseRightOriginal(false);
    rightDimensionsRef.current = { width: 0, height: 0 };
    rightImageStateRef.current = 'large';
  }, [rightImageId, showStars]);

  // Sync zoom states when syncZoom is enabled
  useEffect(() => {
    if (syncZoom) {
      // Sync right zoom to match left zoom whenever left changes
      rightZoom.setZoomState(leftZoom.zoomState);
    }
  }, [syncZoom, leftZoom.zoomState, rightZoom]);

  // Handle zoom events
  const handleLeftZoom = useCallback((e: React.WheelEvent) => {
    e.preventDefault();
    e.stopPropagation();
    leftZoom.handleWheel(e);
  }, [leftZoom]);

  const handleRightZoom = useCallback((e: React.WheelEvent) => {
    e.preventDefault();
    e.stopPropagation();
    if (!syncZoom) {
      rightZoom.handleWheel(e);
    } else {
      // When synced, translate coordinates and apply to left image
      const leftContainer = leftZoom.containerRef.current;
      const rightContainer = rightZoom.containerRef.current;
      if (leftContainer && rightContainer) {
        const rightRect = rightContainer.getBoundingClientRect();
        const leftRect = leftContainer.getBoundingClientRect();
        
        // Create a new wheel event with translated coordinates
        const adjustedEvent = Object.create(e);
        adjustedEvent.clientX = e.clientX - rightRect.left + leftRect.left;
        adjustedEvent.clientY = e.clientY - rightRect.top + leftRect.top;
        
        leftZoom.handleWheel(adjustedEvent);
      }
    }
  }, [leftZoom, rightZoom, syncZoom]);
  
  // Handle mouse move for panning
  const handleLeftMouseMove = useCallback((e: React.MouseEvent) => {
    leftZoom.handleMouseMove(e);
  }, [leftZoom]);
  
  const handleRightMouseMove = useCallback((e: React.MouseEvent) => {
    if (!syncZoom) {
      rightZoom.handleMouseMove(e);
    } else {
      // When synced, translate coordinates and apply to left image
      const leftContainer = leftZoom.containerRef.current;
      const rightContainer = rightZoom.containerRef.current;
      if (leftContainer && rightContainer) {
        const rightRect = rightContainer.getBoundingClientRect();
        const leftRect = leftContainer.getBoundingClientRect();
        
        // Create a new mouse event with translated coordinates
        const adjustedEvent = Object.create(e);
        adjustedEvent.clientX = e.clientX - rightRect.left + leftRect.left;
        adjustedEvent.clientY = e.clientY - rightRect.top + leftRect.top;
        
        leftZoom.handleMouseMove(adjustedEvent);
      }
    }
  }, [leftZoom, rightZoom, syncZoom]);

  const getStatusClass = (image: Image | null | undefined) => {
    if (!image) return '';
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

  if (!leftImage) {
    return <div className="comparison-overlay"><div className="loading">Loading...</div></div>;
  }

  return (
    <div className="comparison-overlay">
      <div className="comparison-container">
        <div className="comparison-header">
          <h2>Image Comparison</h2>
          <div className="comparison-controls">
            <label>
              <input
                type="checkbox"
                checked={syncZoom}
                onChange={(e) => setSyncZoom(e.target.checked)}
              />
              Sync Zoom
            </label>
            <label>
              <input
                type="checkbox"
                checked={showStars}
                onChange={(e) => setShowStars(e.target.checked)}
              />
              Show Stars
            </label>
            {rightImageId && (
              <button
                className="swap-button"
                onClick={onSwapImages}
                disabled={leftImageId === rightImageId}
                title={leftImageId === rightImageId ? "Cannot swap identical images" : "Swap left and right images (W)"}
              >
                ↔ Swap
              </button>
            )}
          </div>
          <button className="close-button" onClick={onClose}>×</button>
        </div>

        <div className="comparison-content">
          {/* Left Image */}
          <div className="comparison-panel">
            <div className="panel-header">
              <div className="panel-title-row">
                <h3>Left Image</h3>
                <div className={`status-banner ${getStatusClass(leftImage)}`}>
                  {leftImage.grading_status === GradingStatus.Accepted && '✓ ACCEPTED'}
                  {leftImage.grading_status === GradingStatus.Rejected && '✗ REJECTED'}
                  {leftImage.grading_status === GradingStatus.Pending && '○ PENDING'}
                </div>
              </div>
            </div>
            
            <div className="panel-image">
              <div 
                className={`zoom-container ${leftZoom.hasOverflow ? 'zoomed' : ''}`}
                ref={leftZoom.containerRef}
                onWheel={handleLeftZoom}
                onMouseDown={leftZoom.handleMouseDown}
                onMouseMove={handleLeftMouseMove}
                onMouseUp={leftZoom.handleMouseUp}
                onMouseLeave={leftZoom.handleMouseUp}
              >
                {leftImageError ? (
                  <div className="comparison-image-error" style={{
                    width: '100%',
                    height: '100%',
                    minHeight: '400px',
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
                      <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" style={{ marginBottom: '12px' }}>
                        <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                        <line x1="9" y1="9" x2="15" y2="15" />
                        <line x1="15" y1="9" x2="9" y2="15" />
                      </svg>
                      <p style={{ margin: 0, fontSize: '0.9rem', color: '#666' }}>Image not available</p>
                    </div>
                  </div>
                ) : (
                  <img
                    ref={leftZoom.imageRef}
                    src={showStars 
                      ? apiClient.getAnnotatedUrl(leftImageId, (useLeftOriginal || (leftOriginalLoaded && targetLeftZoomRef.current && targetLeftZoomRef.current > 1.0)) ? 'original' : 'large')
                      : apiClient.getPreviewUrl(leftImageId, { size: (useLeftOriginal || (leftOriginalLoaded && targetLeftZoomRef.current && targetLeftZoomRef.current > 1.0)) ? 'original' : 'large' })
                    }
                    alt={`${leftImage.target_name} - ${leftImage.filter_name || 'No filter'}`}
                    onError={() => setLeftImageError(true)}
                    onLoad={(e) => {
                    setLeftImageLoaded(true);
                    
                    const img = e.currentTarget;
                    const newWidth = img.naturalWidth;
                    const newHeight = img.naturalHeight;
                    const oldWidth = leftDimensionsRef.current.width;
                    const oldHeight = leftDimensionsRef.current.height;
                    
                    // Update image dimensions in zoom hook
                    const isShowingOriginal = useLeftOriginal || (leftOriginalLoaded && targetLeftZoomRef.current !== null && targetLeftZoomRef.current > 1.0);
                    leftZoom.setImageDimensions(newWidth, newHeight, isShowingOriginal);
                    
                    // Check if dimensions actually changed
                    const dimensionsChanged = oldWidth > 0 && (Math.abs(newWidth - oldWidth) > 10 || Math.abs(newHeight - oldHeight) > 10);
                    
                    if (dimensionsChanged) {
                      const currentVisualScale = leftZoom.getVisualScale();
                      
                      // Adjust zoom in two cases:
                      // 1. Switching to original image (preserve visual scale)
                      // 2. Loading new regular image with preserved zoom from previous image
                      if (leftImageStateRef.current === 'switching-to-original' || 
                          (leftImageStateRef.current === 'large' && currentVisualScale > 1.2)) {
                        // Adjust zoom to maintain visual continuity
                        leftZoom.adjustZoomForNewImage(oldWidth, oldHeight, newWidth, newHeight);
                      }
                    }
                    
                    // Update stored dimensions
                    leftDimensionsRef.current = { width: newWidth, height: newHeight };
                  }}
                  style={{
                    transform: `translate(${leftZoom.zoomState.offsetX}px, ${leftZoom.zoomState.offsetY}px) scale(${leftZoom.zoomState.scale})`,
                    cursor: leftZoom.zoomState.scale > 1 ? 'grab' : 'default',
                  }}
                  draggable={false}
                  />
                )}
              </div>
            </div>

            <div className="panel-info">
              <div className="info-row">
                <strong>Target:</strong> {leftImage.target_name}
              </div>
              <div className="info-row">
                <strong>Filter:</strong> {leftImage.filter_name || 'None'}
              </div>
              <div className="info-row">
                <strong>Date:</strong> {formatDate(leftImage.acquired_date)}
              </div>
              {leftImage.metadata?.HFR && (
                <div className="info-row">
                  <strong>HFR:</strong> {leftImage.metadata.HFR.toFixed(2)}
                </div>
              )}
              {leftImage.metadata?.DetectedStars !== undefined && (
                <div className="info-row">
                  <strong>Stars:</strong> {leftImage.metadata.DetectedStars}
                </div>
              )}
            </div>

            <div className="panel-actions">
              <button onClick={() => onGradeLeft('accepted')} className="action-button accept">
                Accept (1)
              </button>
              <button onClick={() => onGradeLeft('rejected')} className="action-button reject">
                Reject (2)
              </button>
              <button onClick={() => onGradeLeft('pending')} className="action-button pending">
                Unmark (3)
              </button>
            </div>
          </div>

          {/* Right Image */}
          <div className="comparison-panel">
            {rightImage ? (
              <>
                <div className="panel-header">
                  <div className="panel-title-row">
                    <h3>Right Image {isRightImageFetching && '(Loading...)'}</h3>
                    <div className={`status-banner ${getStatusClass(rightImage)}`}>
                      {rightImage.grading_status === GradingStatus.Accepted && '✓ ACCEPTED'}
                      {rightImage.grading_status === GradingStatus.Rejected && '✗ REJECTED'}
                      {rightImage.grading_status === GradingStatus.Pending && '○ PENDING'}
                    </div>
                  </div>
                </div>
                
                <div className="panel-image">
                  <div 
                    className={`zoom-container ${rightZoom.hasOverflow ? 'zoomed' : ''}`}
                    ref={rightZoom.containerRef}
                    onWheel={handleRightZoom}
                    onMouseDown={(e) => {
                      if (!syncZoom) {
                        rightZoom.handleMouseDown(e);
                      } else {
                        // When synced, translate coordinates and apply to left image
                        const leftContainer = leftZoom.containerRef.current;
                        const rightContainer = rightZoom.containerRef.current;
                        if (leftContainer && rightContainer) {
                          const rightRect = rightContainer.getBoundingClientRect();
                          const leftRect = leftContainer.getBoundingClientRect();
                          
                          // Create a new mouse event with translated coordinates
                          const adjustedEvent = Object.create(e);
                          adjustedEvent.clientX = e.clientX - rightRect.left + leftRect.left;
                          adjustedEvent.clientY = e.clientY - rightRect.top + leftRect.top;
                          
                          leftZoom.handleMouseDown(adjustedEvent);
                        }
                      }
                    }}
                    onMouseMove={handleRightMouseMove}
                    onMouseUp={syncZoom ? leftZoom.handleMouseUp : rightZoom.handleMouseUp}
                    onMouseLeave={syncZoom ? leftZoom.handleMouseUp : rightZoom.handleMouseUp}
                  >
                    {rightImageError ? (
                      <div className="comparison-image-error" style={{
                        width: '100%',
                        height: '100%',
                        minHeight: '400px',
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
                          <svg width="48" height="48" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" style={{ marginBottom: '12px' }}>
                            <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                            <line x1="9" y1="9" x2="15" y2="15" />
                            <line x1="15" y1="9" x2="9" y2="15" />
                          </svg>
                          <p style={{ margin: 0, fontSize: '0.9rem', color: '#666' }}>Image not available</p>
                        </div>
                      </div>
                    ) : (() => {
                      // Match the left image's size for consistency
                      const shouldUseOriginal = useRightOriginal || useLeftOriginal;
                      const imageSize = shouldUseOriginal ? 'original' : 'large';
                      console.log('[Image Render] Right image decision:', {
                        imageId: rightImageId,
                        useRightOriginal,
                        useLeftOriginal,
                        rightOriginalLoaded,
                        targetZoom: targetRightZoomRef.current,
                        shouldUseOriginal,
                        imageSize
                      });
                      
                      return (
                        <img
                          ref={rightZoom.imageRef}
                          src={showStars 
                            ? apiClient.getAnnotatedUrl(rightImageId!, imageSize)
                            : apiClient.getPreviewUrl(rightImageId!, { size: imageSize })
                          }
                        alt={`${rightImage.target_name} - ${rightImage.filter_name || 'No filter'}`}
                        onError={() => setRightImageError(true)}
                        onLoad={(e) => {
                        setRightImageLoaded(true);
                        
                        const img = e.currentTarget;
                        const newWidth = img.naturalWidth;
                        const newHeight = img.naturalHeight;
                        const oldWidth = rightDimensionsRef.current.width;
                        const oldHeight = rightDimensionsRef.current.height;
                        
                        // Update image dimensions in zoom hook
                        const isShowingOriginal = useRightOriginal || (rightOriginalLoaded && targetRightZoomRef.current !== null && targetRightZoomRef.current > 1.0);
                        rightZoom.setImageDimensions(newWidth, newHeight, isShowingOriginal);
                        
                        // Check if dimensions actually changed
                        const dimensionsChanged = oldWidth > 0 && (Math.abs(newWidth - oldWidth) > 10 || Math.abs(newHeight - oldHeight) > 10);
                        
                        if (dimensionsChanged) {
                          const currentVisualScale = rightZoom.getVisualScale();
                          
                          // Adjust zoom in two cases:
                          // 1. Switching to original image (preserve visual scale)
                          // 2. Loading new regular image with preserved zoom from previous image
                          if (rightImageStateRef.current === 'switching-to-original' || 
                              (rightImageStateRef.current === 'large' && currentVisualScale > 1.2)) {
                            // Adjust zoom to maintain visual continuity
                            rightZoom.adjustZoomForNewImage(oldWidth, oldHeight, newWidth, newHeight);
                          }
                        }
                        
                        // Update stored dimensions
                        rightDimensionsRef.current = { width: newWidth, height: newHeight };
                          }}
                          style={{
                            transform: `translate(${rightZoom.zoomState.offsetX}px, ${rightZoom.zoomState.offsetY}px) scale(${rightZoom.zoomState.scale})`,
                            cursor: rightZoom.zoomState.scale > 1 ? 'grab' : 'default',
                          }}
                          draggable={false}
                        />
                      );
                    })()}
                  </div>
                </div>

                <div className="panel-info">
                  <div className="info-row">
                    <strong>Target:</strong> {rightImage.target_name}
                  </div>
                  <div className="info-row">
                    <strong>Filter:</strong> {rightImage.filter_name || 'None'}
                  </div>
                  <div className="info-row">
                    <strong>Date:</strong> {formatDate(rightImage.acquired_date)}
                  </div>
                  {rightImage.metadata?.HFR && (
                    <div className="info-row">
                      <strong>HFR:</strong> {rightImage.metadata.HFR.toFixed(2)}
                    </div>
                  )}
                  {rightImage.metadata?.DetectedStars !== undefined && (
                    <div className="info-row">
                      <strong>Stars:</strong> {rightImage.metadata.DetectedStars}
                    </div>
                  )}
                </div>

                <div className="panel-actions">
                  <button onClick={() => onGradeRight('accepted')} className="action-button accept">
                    Accept (7)
                  </button>
                  <button onClick={() => onGradeRight('rejected')} className="action-button reject">
                    Reject (8)
                  </button>
                  <button onClick={() => onGradeRight('pending')} className="action-button pending">
                    Unmark (9)
                  </button>
                </div>
              </>
            ) : (
              <div className="panel-empty">
                <p>No image selected for comparison</p>
                <button onClick={onSelectRightImage} className="select-button">
                  Select Image
                </button>
              </div>
            )}
          </div>
        </div>

        <div className="comparison-shortcuts">
          <div className="shortcut-section">
            <h4>Navigation</h4>
            <span>ESC Close</span>
            <span>S Toggle Stars</span>
            <span>Z Toggle Sync Zoom</span>
            <span>W Swap Images</span>
            <span>→/K Next Right</span>
            <span>←/J Prev Right</span>
          </div>
          <div className="shortcut-section">
            <h4>Left Image</h4>
            <span>1 Accept</span>
            <span>2 Reject</span>
            <span>3 Unmark</span>
          </div>
          <div className="shortcut-section">
            <h4>Right Image</h4>
            <span>7 Accept</span>
            <span>8 Reject</span>
            <span>9 Unmark</span>
          </div>
        </div>
      </div>
    </div>
  );
}