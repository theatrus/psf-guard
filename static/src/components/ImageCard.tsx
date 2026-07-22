import { useCallback, useEffect } from 'react';
import { useInView } from 'react-intersection-observer';
import type { Image, ImageQualityResult } from '../api/types';
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';
import PreviewImage from './PreviewImage';
import { ensurePreviewReady } from '../hooks/previewPoll';

export interface ImageCardProps {
  dbId: string;
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
  quality?: ImageQualityResult;
  lazyPreview?: boolean;
  selectionEffects?: boolean;
  className?: string;
}

export default function ImageCard({
  dbId,
  image,
  isSelected,
  onClick,
  onDoubleClick,
  quality,
  lazyPreview = false,
  selectionEffects = true,
  className = '',
}: ImageCardProps) {
  const shouldDeferPreview = lazyPreview && typeof IntersectionObserver !== 'undefined';
  const { ref: inViewRef, inView } = useInView({
    threshold: 0,
    rootMargin: '600px 0px',
    triggerOnce: true,
    initialInView: !shouldDeferPreview,
    skip: !shouldDeferPreview,
  });
  const setCardRef = useCallback((node: HTMLDivElement | null) => {
    inViewRef(node);
  }, [inViewRef]);

  // Preload full size image when selected (for quick detail view opening).
  // Warms the interactive queue so the 'large' preview is generated if needed.
  useEffect(() => {
    if (selectionEffects && isSelected && image.id) {
      void ensurePreviewReady(
        dbId,
        apiClient.getPreviewUrl(dbId, image.id, { size: 'large' }),
        { imageId: image.id, kind: 'preview', size: 'large' }
      );
    }
  }, [isSelected, image.id, dbId, selectionEffects]);

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
  const shouldLoadPreview = !shouldDeferPreview || inView;

  return (
    <div
      ref={setCardRef}
      className={`image-card ${getStatusClass()} ${isSelected ? 'selected' : ''} ${className}`.trim()}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <div className="image-preview">
        {shouldLoadPreview ? (
          <PreviewImage
            dbId={dbId}
            src={apiClient.getPreviewUrl(dbId, image.id, { size: 'screen' })}
            descriptor={{ imageId: image.id, kind: 'preview', size: 'screen' }}
            alt={`${image.target_name} - ${image.filter_name || 'No filter'}`}
            loading="lazy"
          />
        ) : (
          <div className="image-preview-deferred" aria-hidden="true" />
        )}
        {quality && (
          <div
            className="quality-badge"
            style={{
              backgroundColor: qualityColor(quality.quality_score),
            }}
            title={`Quality: ${(quality.quality_score * 100).toFixed(0)}%`}
          >
            {(quality.quality_score * 100).toFixed(0)}
          </div>
        )}
        {quality?.category && (
          <div className="category-label">
            {formatCategory(quality.category)}
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
        {quality?.normalized_metrics.spatial_coverage != null
          && quality.normalized_metrics.spatial_coverage < 0.9 && (
          <span className="sequence-image-coverage" title="Spatial star coverage (1.0 = stars across the whole frame)">
            coverage {quality.normalized_metrics.spatial_coverage.toFixed(2)}
          </span>
        )}
        {quality?.pointing?.field_fraction_offset != null && (
          <span
            className={quality.regrade_reason ? 'analysis-signal danger' : 'analysis-signal'}
            title={`Solved target offset: ${quality.pointing.separation_arcsec?.toFixed(0) ?? '?'} arcsec`}
          >
            offset {(quality.pointing.field_fraction_offset * 100).toFixed(0)}% field
          </span>
        )}
        {quality?.pointing?.solve_failed && (
          <span
            className="analysis-signal warning"
            title={quality.pointing.error || (quality.pointing.image_quality_evidence
              ? 'Pixels did not match a field'
              : 'Plate solver could not make a quality determination')}
          >
            {quality.pointing.image_quality_evidence ? 'unsolved' : 'solve unavailable'}
          </span>
        )}
        {quality?.satellite && (quality.satellite.potentially_bright_count > 0
          || quality.satellite.pixel_aligned_count > 0) && (
          <span
            className={quality.satellite.pixel_aligned_high_risk_count > 0
              ? 'analysis-signal danger'
              : 'analysis-signal warning'}
            title={quality.satellite.pixel_aligned_count > 0
              ? 'Pixel corridor evidence matches an orbital candidate'
              : 'Orbital prediction only; no matching pixel trail was found'}
          >
            satellite {quality.satellite.pixel_aligned_high_risk_count > 0
              ? 'trail matched'
              : quality.satellite.pixel_aligned_count > 0
                ? 'pixel match'
                : quality.satellite.high_risk_count > 0
                  ? 'high prediction'
                  : 'possible'}
          </span>
        )}
        {quality?.details && (
          <span className="sequence-image-details" title={quality.details}>
            {quality.details}
          </span>
        )}
      </div>
    </div>
  );
}

function formatCategory(category: string): string {
  return category
    .split('_')
    .map(word => word.charAt(0).toUpperCase() + word.slice(1))
    .join(' ');
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-error)';
}
