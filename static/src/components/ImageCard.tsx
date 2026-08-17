import { useCallback, useEffect } from 'react';
import { useInView } from 'react-intersection-observer';
import type { Image, ImageQualityResult } from '../api/types';
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';
import PreviewImage from './PreviewImage';
import { useColorPreview } from '../hooks/useColorPreview';
import { ensurePreviewReady } from '../hooks/previewPoll';
import {
  qualityScoreBasis,
  qualityScoreDescription,
  type QualityScoreScope,
  secondaryScoreDescription,
} from '../utils/qualityScore';
import QualityReasonPopover from './QualityReasonPopover';

export interface ImageCardProps {
  dbId: string;
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
  quality?: ImageQualityResult;
  qualityScoreScope?: QualityScoreScope;
  /** The same frame's score under the OTHER comparison basis, when it
   * exists — session-relative when the badge is all-sessions, and the
   * reverse. Rendered as a smaller chip when the rounded values differ. */
  secondaryScore?: { score: number; scope: QualityScoreScope };
  qualityPresentation?: 'full' | 'compact';
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
  qualityScoreScope = 'capture_sequence',
  secondaryScore,
  qualityPresentation = 'full',
  lazyPreview = false,
  selectionEffects = true,
  className = '',
}: ImageCardProps) {
  const color = useColorPreview();
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
        apiClient.getPreviewUrl(dbId, image.id, { size: 'large', color }),
        { imageId: image.id, kind: 'preview', size: 'large', color }
      );
    }
  }, [isSelected, image.id, dbId, selectionEffects, color]);

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
      data-card-image-id={image.id}
      className={`image-card ${getStatusClass()} ${isSelected ? 'selected' : ''} ${className}`.trim()}
      onClick={onClick}
      onDoubleClick={onDoubleClick}
    >
      <div className="image-preview">
        {shouldLoadPreview ? (
          <PreviewImage
            dbId={dbId}
            src={apiClient.getPreviewUrl(dbId, image.id, { size: 'screen', color })}
            descriptor={{ imageId: image.id, kind: 'preview', size: 'screen', color }}
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
            title={qualityScoreDescription(quality, qualityScoreScope)}
          >
            {(quality.quality_score * 100).toFixed(0)}
          </div>
        )}
        {quality &&
          secondaryScore &&
          Math.round(secondaryScore.score * 100) !==
            Math.round(quality.quality_score * 100) && (
            <div
              className="quality-badge quality-badge-secondary"
              title={secondaryScoreDescription(secondaryScore.score, secondaryScore.scope)}
            >
              {secondaryScore.scope === 'capture_sequence' ? 'night' : 'all'}{' '}
              {(secondaryScore.score * 100).toFixed(0)}
            </div>
          )}
        {quality?.category && (
          <div className="category-label">
            {formatCategory(quality.category)}
          </div>
        )}
        {qualityPresentation === 'compact'
          && (quality?.regrade_reason || quality?.details) && (
          <div className="card-quality-reason-overlay">
            <QualityReasonPopover
              reason={quality.regrade_reason}
              details={quality.details}
            />
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
        {qualityPresentation === 'full' && quality && (
          <span
            className="sequence-score-basis"
            title={qualityScoreDescription(quality, qualityScoreScope)}
          >
            {qualityScoreBasis(quality)}
          </span>
        )}
        <div className={`image-status ${getStatusClass()}`}>
          {getStatusText()}
          {image.reject_reason && (
            <span className="reject-reason-inline"> - {image.reject_reason}</span>
          )}
        </div>
        {qualityPresentation === 'full'
          && quality?.normalized_metrics.spatial_coverage != null
          && quality.normalized_metrics.spatial_coverage < 0.9 && (
          <span className="sequence-image-coverage" title="Spatial star coverage (1.0 = stars across the whole frame)">
            coverage {quality.normalized_metrics.spatial_coverage.toFixed(2)}
          </span>
        )}
        {qualityPresentation === 'full'
          && quality?.pointing?.field_fraction_offset != null && (
          <span
            className={quality.regrade_reason ? 'analysis-signal danger' : 'analysis-signal'}
            title={`Solved target offset: ${quality.pointing.separation_arcsec?.toFixed(0) ?? '?'} arcsec`}
          >
            offset {(quality.pointing.field_fraction_offset * 100).toFixed(0)}% field
          </span>
        )}
        {qualityPresentation === 'full' && quality?.pointing?.solve_failed && (
          <span
            className="analysis-signal warning"
            title={quality.pointing.error || (quality.pointing.image_quality_evidence
              ? 'Pixels did not match a field'
              : 'Plate solver could not make a quality determination')}
          >
            {quality.pointing.image_quality_evidence ? 'unsolved' : 'solve unavailable'}
          </span>
        )}
        {qualityPresentation === 'full'
          && quality?.satellite
          && quality.satellite.pixel_aligned_count > 0 && (
          <span
            className={quality.satellite.pixel_aligned_high_risk_count > 0
              ? 'analysis-signal danger'
              : 'analysis-signal warning'}
            title="Pixel corridor evidence matches an orbital candidate"
          >
            satellite {quality.satellite.pixel_aligned_high_risk_count > 0
              ? 'trail matched'
              : 'pixel match'}
          </span>
        )}
        {qualityPresentation === 'full'
          && (quality?.regrade_reason || quality?.details) && (
          <QualityReasonPopover
            reason={quality.regrade_reason}
            details={quality.details}
          />
        )}
      </div>
    </div>
  );
}

function formatCategory(category: string): string {
  if (category === 'satellite_trail_risk') return 'Satellite Trail Detected';
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
