import { useEffect, useRef, useState } from 'react';
import type { Image } from '../api/types';
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';

interface ImageCardProps {
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
  qualityScore?: number;
}

export default function ImageCard({ image, isSelected, onClick, onDoubleClick, qualityScore }: ImageCardProps) {
  const cardRef = useRef<HTMLDivElement>(null);
  const imgRef = useRef<HTMLImageElement>(null);
  const [imageError, setImageError] = useState(false);

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
      ref={cardRef}
      className={`image-card ${getStatusClass()} ${isSelected ? 'selected' : ''}`}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <div className="image-preview">
        {imageError ? (
          <div className="image-error-placeholder" style={{ 
            width: '100%', 
            paddingBottom: '100%', // Maintain aspect ratio
            background: '#1a1a1a',
            position: 'relative',
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'center'
          }}>
            <div style={{
              position: 'absolute',
              top: '50%',
              left: '50%',
              transform: 'translate(-50%, -50%)',
              textAlign: 'center',
              color: '#666'
            }}>
              <svg width="32" height="32" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.5" style={{ marginBottom: '8px' }}>
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                <line x1="9" y1="9" x2="15" y2="15" />
                <line x1="15" y1="9" x2="9" y2="15" />
              </svg>
              <div style={{ fontSize: '0.8rem' }}>Image not found</div>
            </div>
          </div>
        ) : (
          <img
            ref={imgRef}
            src={apiClient.getPreviewUrl(image.id, { size: 'screen' })}
            alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
            loading="lazy"
            onError={() => setImageError(true)}
          />
        )}
        {qualityScore !== undefined && (
          <div
            className="quality-dot"
            style={{
              backgroundColor: qualityScore >= 0.7 ? 'var(--color-success)' : qualityScore >= 0.5 ? 'var(--color-warning)' : 'var(--color-error)',
            }}
            title={`Quality: ${(qualityScore * 100).toFixed(0)}%`}
          />
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