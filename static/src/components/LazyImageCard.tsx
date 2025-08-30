import { useEffect, useRef, useState } from 'react';
import { useInView } from 'react-intersection-observer';
import type { Image } from '../api/types';
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';

interface LazyImageCardProps {
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
}

export default function LazyImageCard({ image, isSelected, onClick, onDoubleClick }: LazyImageCardProps) {
  const cardRef = useRef<HTMLDivElement>(null);
  const [imageLoaded, setImageLoaded] = useState(false);
  const { ref: inViewRef, inView } = useInView({
    threshold: 0,
    rootMargin: '200px',
    triggerOnce: true, // Only trigger once to avoid reloading
  });

  // Combine refs
  const setRefs = (node: HTMLDivElement | null) => {
    cardRef.current = node;
    inViewRef(node);
  };

  // Scroll into view when selected
  useEffect(() => {
    if (isSelected && cardRef.current) {
      cardRef.current.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  }, [isSelected]);

  // Preload full size image when selected (for quick detail view opening)
  useEffect(() => {
    if (isSelected && image.id) {
      const preloadImg = new Image();
      preloadImg.src = apiClient.getPreviewUrl(image.id, { size: 'large' });
    }
  }, [isSelected, image.id]);

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

  const getStatusText = () => {
    switch (image.grading_status) {
      case GradingStatus.Accepted:
        return 'Accepted';
      case GradingStatus.Rejected:
        return 'Rejected';
      default:
        return 'Pending';
    }
  };

  const formatDate = (timestamp: number | null) => {
    if (!timestamp) return 'Unknown';
    return new Date(timestamp * 1000).toLocaleString();
  };

  // Extract HFR and star count from metadata
  const getImageStats = () => {
    const hfr = image.metadata?.HFR;
    const starCount = image.metadata?.DetectedStars;
    return {
      hfr: typeof hfr === 'number' ? hfr.toFixed(2) : null,
      starCount: typeof starCount === 'number' ? starCount : null,
    };
  };

  const stats = getImageStats();

  return (
    <div
      ref={setRefs}
      className={`image-card ${getStatusClass()} ${isSelected ? 'selected' : ''}`}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <div className="image-preview">
        {inView ? (
          <img
            src={apiClient.getPreviewUrl(image.id, { size: 'screen' })}
            alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
            loading="lazy"
            onLoad={() => setImageLoaded(true)}
            style={{ opacity: imageLoaded ? 1 : 0, transition: 'opacity 0.3s' }}
          />
        ) : (
          <div className="image-placeholder" style={{ 
            width: '100%', 
            paddingBottom: '100%', // Maintain aspect ratio
            background: '#2a2a2a',
            position: 'relative'
          }}>
            <div style={{
              position: 'absolute',
              top: '50%',
              left: '50%',
              transform: 'translate(-50%, -50%)',
              color: '#666',
              fontSize: '0.9rem'
            }}>
              Loading...
            </div>
          </div>
        )}
      </div>
      <div className="image-info">
        <h3>{image.target_name}</h3>
        <p className="image-filter">{image.filter_name || 'No filter'}</p>
        <p className="image-date">{formatDate(image.acquired_date)}</p>
        {(stats.hfr || stats.starCount) && (
          <div className="image-stats">
            {stats.hfr && <span className="stat-hfr">HFR: {stats.hfr}</span>}
            {stats.starCount && <span className="stat-stars">★ {stats.starCount}</span>}
          </div>
        )}
        <div className={`image-status ${getStatusClass()}`}>
          {getStatusText()}
          {image.reject_reason && (
            <span className="reject-reason-inline"> - {image.reject_reason}</span>
          )}
        </div>
      </div>
    </div>
  );
}