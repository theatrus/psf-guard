import { useEffect, useRef } from 'react';
import type { Image } from '../api/types';
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';
import PreviewImage from './PreviewImage';
import { ensurePreviewReady } from '../hooks/previewPoll';

interface ImageCardProps {
  dbId: string;
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
  qualityScore?: number;
}

export default function ImageCard({ dbId, image, isSelected, onClick, onDoubleClick, qualityScore }: ImageCardProps) {
  const cardRef = useRef<HTMLDivElement>(null);

  // Scroll into view when selected
  useEffect(() => {
    if (isSelected && cardRef.current) {
      cardRef.current.scrollIntoView({ behavior: 'smooth', block: 'nearest' });
    }
  }, [isSelected]);

  // Preload full size image when selected (for quick detail view opening).
  // Warms the interactive queue so the 'large' preview is generated if needed.
  useEffect(() => {
    if (isSelected && image.id) {
      void ensurePreviewReady(
        dbId,
        apiClient.getPreviewUrl(dbId, image.id, { size: 'large' }),
        { imageId: image.id, kind: 'preview', size: 'large' }
      );
    }
  }, [isSelected, image.id, dbId]);

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
        <PreviewImage
          dbId={dbId}
          src={apiClient.getPreviewUrl(dbId, image.id, { size: 'screen' })}
          descriptor={{ imageId: image.id, kind: 'preview', size: 'screen' }}
          alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
          loading="lazy"
        />
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