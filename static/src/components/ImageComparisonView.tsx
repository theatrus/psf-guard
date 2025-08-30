import { useState, useCallback, useEffect } from 'react';
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
}

export default function ImageComparisonView({
  leftImageId,
  rightImageId,
  onClose,
  onSelectRightImage,
  onGradeLeft,
  onGradeRight,
}: ImageComparisonViewProps) {
  const [showStars, setShowStars] = useState(false);
  const [syncZoom, setSyncZoom] = useState(true);
  
  // Initialize zoom for both images
  const leftZoom = useImageZoom({ minScale: 0.1, maxScale: 10.0 });
  const rightZoom = useImageZoom({ minScale: 0.1, maxScale: 10.0 });
  
  // Track image loading
  const [leftImageLoaded, setLeftImageLoaded] = useState(false);
  const [rightImageLoaded, setRightImageLoaded] = useState(false);

  // Fetch left image
  const { data: leftImage } = useQuery({
    queryKey: ['image', leftImageId],
    queryFn: () => apiClient.getImage(leftImageId),
  });

  // Fetch right image (if selected)
  const { data: rightImage } = useQuery({
    queryKey: ['image', rightImageId],
    queryFn: () => rightImageId ? apiClient.getImage(rightImageId) : Promise.resolve(null),
    enabled: !!rightImageId,
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

  // Reset loaded state when images change
  useEffect(() => {
    setLeftImageLoaded(false);
    leftZoom.resetInitialization();
  }, [leftImageId, showStars, leftZoom]);
  
  useEffect(() => {
    setRightImageLoaded(false);
    rightZoom.resetInitialization();
  }, [rightImageId, showStars, rightZoom]);

  // Fit images when they load
  useEffect(() => {
    if (leftImageLoaded) {
      leftZoom.zoomToFit();
    }
  }, [leftImageLoaded, leftZoom]);
  
  useEffect(() => {
    if (rightImageLoaded) {
      rightZoom.zoomToFit();
    }
  }, [rightImageLoaded, rightZoom]);

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
      // When synced, apply zoom to left image and it will sync to right via effect
      leftZoom.handleWheel(e);
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
      // When synced, move left image and it will sync to right via effect
      // Calculate the equivalent mouse position for the left image
      const leftContainer = leftZoom.containerRef.current;
      const rightContainer = rightZoom.containerRef.current;
      if (leftContainer && rightContainer) {
        const rightRect = rightContainer.getBoundingClientRect();
        const leftRect = leftContainer.getBoundingClientRect();
        const adjustedEvent = {
          ...e,
          clientX: e.clientX - rightRect.left + leftRect.left,
          clientY: e.clientY - rightRect.top + leftRect.top,
        } as React.MouseEvent;
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
          </div>
          <button className="close-button" onClick={onClose}>×</button>
        </div>

        <div className="comparison-content">
          {/* Left Image */}
          <div className="comparison-panel">
            <div className="panel-header">
              <h3>Left Image</h3>
              <div className={`status-badge ${getStatusClass(leftImage)}`}>
                {leftImage.grading_status === GradingStatus.Accepted && '✓ ACCEPTED'}
                {leftImage.grading_status === GradingStatus.Rejected && '✗ REJECTED'}
                {leftImage.grading_status === GradingStatus.Pending && '○ PENDING'}
              </div>
            </div>
            
            <div className="panel-image">
              <div 
                className={`zoom-container ${leftZoom.zoomState.scale >= 0.95 ? 'zoomed' : ''}`}
                ref={leftZoom.containerRef}
                onWheel={handleLeftZoom}
                onMouseDown={leftZoom.handleMouseDown}
                onMouseMove={handleLeftMouseMove}
                onMouseUp={leftZoom.handleMouseUp}
                onMouseLeave={leftZoom.handleMouseUp}
              >
                <img
                  ref={leftZoom.imageRef}
                  src={showStars 
                    ? apiClient.getAnnotatedUrl(leftImageId, leftZoom.zoomState.scale > 1 ? 'original' : 'large')
                    : apiClient.getPreviewUrl(leftImageId, { size: leftZoom.zoomState.scale > 1 ? 'original' : 'large' })
                  }
                  alt={`${leftImage.target_name} - ${leftImage.filter_name || 'No filter'}`}
                  onLoad={() => setLeftImageLoaded(true)}
                  style={{
                    transform: `translate(${leftZoom.zoomState.offsetX}px, ${leftZoom.zoomState.offsetY}px) scale(${leftZoom.zoomState.scale})`,
                    cursor: leftZoom.zoomState.scale > 1 ? 'grab' : 'default',
                  }}
                  draggable={false}
                />
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
                  <h3>Right Image</h3>
                  <div className={`status-badge ${getStatusClass(rightImage)}`}>
                    {rightImage.grading_status === GradingStatus.Accepted && '✓ ACCEPTED'}
                    {rightImage.grading_status === GradingStatus.Rejected && '✗ REJECTED'}
                    {rightImage.grading_status === GradingStatus.Pending && '○ PENDING'}
                  </div>
                </div>
                
                <div className="panel-image">
                  <div 
                    className={`zoom-container ${rightZoom.zoomState.scale >= 0.95 ? 'zoomed' : ''}`}
                    ref={rightZoom.containerRef}
                    onWheel={handleRightZoom}
                    onMouseDown={syncZoom ? leftZoom.handleMouseDown : rightZoom.handleMouseDown}
                    onMouseMove={handleRightMouseMove}
                    onMouseUp={syncZoom ? leftZoom.handleMouseUp : rightZoom.handleMouseUp}
                    onMouseLeave={syncZoom ? leftZoom.handleMouseUp : rightZoom.handleMouseUp}
                  >
                    <img
                      ref={rightZoom.imageRef}
                      src={showStars 
                        ? apiClient.getAnnotatedUrl(rightImageId!, rightZoom.zoomState.scale > 1 ? 'original' : 'large')
                        : apiClient.getPreviewUrl(rightImageId!, { size: rightZoom.zoomState.scale > 1 ? 'original' : 'large' })
                      }
                      alt={`${rightImage.target_name} - ${rightImage.filter_name || 'No filter'}`}
                      onLoad={() => setRightImageLoaded(true)}
                      style={{
                        transform: `translate(${rightZoom.zoomState.offsetX}px, ${rightZoom.zoomState.offsetY}px) scale(${rightZoom.zoomState.scale})`,
                        cursor: rightZoom.zoomState.scale > 1 ? 'grab' : 'default',
                      }}
                      draggable={false}
                    />
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