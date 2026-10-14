import type { ImageQualityResult } from '../api/types';
import { qualityScoreBasis, qualityScoreDescription } from '../utils/qualityScore';
import { QualityReasonText } from './QualityReasonPopover';
import {
  activeQualityRegionSignals,
  qualityRegionSignalCount,
} from '../utils/qualityRegionOverlay';
import DetailSection from './DetailSection';

interface QualityAnalysisSummaryProps {
  quality?: ImageQualityResult | null;
  statusMessage?: string;
  /** A few words for `statusMessage` on a folded panel's heading. */
  statusSummary?: string;
  overlayVisible?: boolean;
  onToggleOverlay?: () => void;
  /** Folded, the heading says only the score and the review reason. */
  open?: boolean;
  onToggleOpen?: () => void;
}

export default function QualityAnalysisSummary({
  quality,
  statusMessage,
  statusSummary,
  overlayVisible = false,
  onToggleOverlay,
  open = true,
  onToggleOpen,
}: QualityAnalysisSummaryProps) {
  const section = {
    title: 'Quality analysis',
    className: 'detail-quality-analysis',
    open,
    onToggle: onToggleOpen,
  };

  if (!quality) {
    if (!statusMessage) return null;
    return (
      <DetailSection {...section} summary={statusSummary}>
        <p className="detail-quality-status">{statusMessage}</p>
      </DetailSection>
    );
  }

  const regionOverlay = quality.spatial_overlay;
  const regionCount = regionOverlay ? qualityRegionSignalCount(regionOverlay) : 0;
  const regionSignals = regionOverlay ? activeQualityRegionSignals(regionOverlay) : [];
  const score = `${Math.round(quality.quality_score * 100)}%`;

  return (
    <DetailSection
      {...section}
      summary={quality.regrade_reason ? `${score} · ${quality.regrade_reason}` : score}
    >
      <div
        className="detail-quality-summary"
        title={qualityScoreDescription(quality, 'capture_sequence')}
      >
        <strong>{score}</strong>
        <span>{qualityScoreBasis(quality)}</span>
      </div>
      {quality.regrade_reason || quality.details ? (
        <div className="detail-quality-reason">
          <strong>{quality.regrade_reason ? 'Review reason' : 'Score reason'}</strong>
          <QualityReasonText reason={quality.regrade_reason} details={quality.details} />
        </div>
      ) : (
        <p className="detail-quality-status">No specific quality issue was identified.</p>
      )}
      {regionOverlay && regionCount > 0 && onToggleOverlay && (
        <div className="quality-region-controls">
          <button
            type="button"
            className="astrometry-toggle quality-region-toggle"
            aria-pressed={overlayVisible}
            onClick={onToggleOverlay}
          >
            <span className="astrometry-toggle-icon" aria-hidden="true">▦</span>
            <span>{overlayVisible ? 'Hide affected regions' : 'Show affected regions'}</span>
            <span className="astrometry-toggle-count">{regionCount}</span>
          </button>
          <div className="quality-region-legend" aria-label="Quality overlay legend">
            {regionSignals.map((signal) => (
              <span key={signal.mask}>
                <i
                  className={`quality-region-swatch quality-region-${signal.className}`}
                  aria-hidden="true"
                />
                {signal.label}
              </span>
            ))}
          </div>
          <p className="quality-region-note">
            Highlights mark measured scan cells, not a traced object.
          </p>
        </div>
      )}
    </DetailSection>
  );
}
