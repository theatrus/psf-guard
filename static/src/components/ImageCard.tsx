import { useCallback, useEffect } from 'react';
import { useInView } from 'react-intersection-observer';
import type { Image, ImageCopy, ImageQualityResult } from '../api/types';

/** How far a skewed frame turned, and against what. A rotator slip is
 *  measured against the angle the rotator reported; its own run agrees with
 *  itself, so the segment skew would read near zero. */
function rotationSkew(
  pointing: ImageQualityResult['pointing']
): { degrees: number; against: string } | null {
  if (pointing?.rotator_skew_deg != null) {
    return { degrees: pointing.rotator_skew_deg, against: 'from the angle the rotator reported' };
  }
  if (pointing?.rotation_skew_deg != null) {
    return { degrees: pointing.rotation_skew_deg, against: 'from the rest of the framing segment' };
  }
  return null;
}
import { GradingStatus } from '../api/types';
import { apiClient } from '../api/client';
import PreviewImage from './PreviewImage';
import { useColorPreview } from '../hooks/useColorPreview';
import { useDisplayPreferences } from '../hooks/useDisplayPreferences';
import { ensurePreviewReady } from '../hooks/previewPoll';
import {
  type BasisScores,
  basisChipDescription,
  qualityScoreBasis,
  qualityScoreDescription,
  type QualityScoreScope,
} from '../utils/qualityScore';
import QualityReasonPopover from './QualityReasonPopover';
import { formatCategory } from '../utils/issueCategory';
import { LayersIcon, MoonIcon } from './ScoreChipIcons';

export interface ImageCardProps {
  dbId: string;
  image: Image;
  isSelected: boolean;
  onClick: (event: React.MouseEvent) => void;
  onDoubleClick: () => void;
  quality?: ImageQualityResult;
  qualityScoreScope?: QualityScoreScope;
  /** Every basis score the frame has. Both chips render whenever their
   * score exists and their toggle is on — never hidden for matching the
   * badge, so the same frame keeps the same chips in every view. */
  basisScores?: BasisScores;
  qualityPresentation?: 'full' | 'compact';
  lazyPreview?: boolean;
  selectionEffects?: boolean;
  className?: string;
  /** Name the target on the card. A grid of one target's frames says it
   *  once in its header, not on every card. */
  showTarget?: boolean;
}

export default function ImageCard({
  dbId,
  image,
  isSelected,
  onClick,
  onDoubleClick,
  quality,
  qualityScoreScope = 'capture_sequence',
  basisScores,
  qualityPresentation = 'full',
  lazyPreview = false,
  selectionEffects = true,
  className = '',
  showTarget = true,
}: ImageCardProps) {
  const color = useColorPreview();
  const { showNightChip, showAllChip } = useDisplayPreferences();
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

  // Preload the full-size image for quick detail-view opening. Callers must
  // enable this only for the keyboard-cursor card: preloading every card in
  // a multi-selection would enqueue a large-preview generation per selected
  // image (select-all on a big grid meant thousands of queued jobs).
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
  // Day, hour and minute, and seconds, as separate pieces so a narrow card
  // can drop the day and the seconds; the full date is the cell's title.
  const shortDate = (timestamp: number | null) => {
    if (!timestamp) return null;
    const date = new Date(timestamp * 1000);
    return {
      day: date.toLocaleDateString(undefined, { month: 'numeric', day: 'numeric' }),
      time: date.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit', hour12: false }),
      seconds: `:${String(date.getSeconds()).padStart(2, '0')}`,
    };
  };
  const when = shortDate(image.acquired_date);

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
        <CopyMark copies={image.copies} />
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
        {quality && basisScores && (showNightChip || showAllChip) && (
          <div className="score-chip-stack">
            {showNightChip && (
              <div
                className="score-chip"
                style={{
                  backgroundColor: qualityColor(basisScores.night),
                }}
                title={basisChipDescription(basisScores.night, 'capture_sequence')}
              >
                <MoonIcon />
                {(basisScores.night * 100).toFixed(0)}
              </div>
            )}
            {showAllChip && basisScores.all != null && (
              <div
                className="score-chip"
                style={{
                  backgroundColor: qualityColor(basisScores.all),
                }}
                title={basisChipDescription(basisScores.all, 'target_filter')}
              >
                <LayersIcon />
                {(basisScores.all * 100).toFixed(0)}
              </div>
            )}
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
        {showTarget && <h3 className="image-target" title={image.target_name}>{image.target_name}</h3>}
        {/* One row of fixed columns, the same on every card, so nothing
            wraps and the values line up from card to card. */}
        <div className="image-facts">
          <span className="image-filter" title={`Filter: ${image.filter_name || 'none'}`}>{image.filter_name || 'No filter'}</span>
          <span className="image-date" title={formatDate(image.acquired_date)}>
            {when ? <><span className="date-day">{when.day} </span>{when.time}<span className="date-seconds">{when.seconds}</span></> : '—'}
          </span>
          <span className="stat-hfr" title="Half-flux radius">{stats.hfr && <><span className="fact-label">HFR </span>{stats.hfr}</>}</span>
          <span className="stat-stars" title="Detected stars">{stats.starCount != null && `★${stats.starCount}`}</span>
        </div>
        {qualityPresentation === 'full' && quality && (
          <span
            className="sequence-score-basis"
            title={qualityScoreDescription(quality, qualityScoreScope)}
          >
            {qualityScoreBasis(quality)}
          </span>
        )}
        <div
          className={`image-status ${getStatusClass()}`}
          title={image.reject_reason ? `${getStatusText()}: ${image.reject_reason}` : getStatusText()}
        >
          {getStatusText()}
          {image.reject_reason && (
            <span className="reject-reason-inline"> · {image.reject_reason}</span>
          )}
        </div>
        <div className="image-signals">
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
        {qualityPresentation === 'full'
          && (quality?.flags ?? []).includes('rotation_skew')
          && rotationSkew(quality?.pointing) && (() => {
            const skew = rotationSkew(quality?.pointing)!;
            const signed = `${skew.degrees > 0 ? '+' : ''}${skew.degrees.toFixed(1)}°`;
            return (
              <span
                className="analysis-signal danger"
                title={`Solved field rotation ${quality?.pointing?.field_rotation_deg?.toFixed(1) ?? '?'}°, ${signed} ${skew.against}`}
              >
                rotation {signed}
              </span>
            );
          })()}
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
        </div>
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

/** A small mark, always in the same corner, when other software left
 *  calibrated or registered copies of this light, or when the light's own
 *  file is such a copy. The detail view switches between them. */
function CopyMark({ copies }: { copies?: ImageCopy[] }) {
  if (!copies || copies.length === 0) return null;
  const own = copies.find((copy) => copy.primary);
  const others = copies.filter((copy) => !copy.primary);
  const lines = [
    own && `From a calibrated copy (${own.label}); no raw frame yet`,
    others.length > 0 && `Copies: ${others.map((copy) => copy.label).join(', ')}`,
  ].filter(Boolean);
  return (
    <span
      className={`copy-mark${own ? ' copy-mark-own' : ''}`}
      title={lines.join('. ')}
      aria-label={lines.join('. ')}
      role="img"
    >
      <svg width="12" height="12" viewBox="0 0 12 12" aria-hidden="true">
        <rect x="0.5" y="3.5" width="8" height="8" rx="1" fill={own ? 'currentColor' : 'none'} stroke="currentColor" />
        <path d="M3.5 3.5V1.5a1 1 0 0 1 1-1h6a1 1 0 0 1 1 1v6a1 1 0 0 1-1 1H8.5" fill="none" stroke="currentColor" />
      </svg>
    </span>
  );
}

function qualityColor(score: number): string {
  if (score >= 0.7) return 'var(--color-success)';
  if (score >= 0.5) return 'var(--color-warning)';
  return 'var(--color-error)';
}
