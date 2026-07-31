import type { ImageQualityResult } from '../api/types';
import { qualityScoreBasis, qualityScoreDescription } from '../utils/qualityScore';
import { QualityReasonText } from './QualityReasonPopover';

interface QualityAnalysisSummaryProps {
  quality?: ImageQualityResult | null;
  statusMessage?: string;
}

export default function QualityAnalysisSummary({
  quality,
  statusMessage,
}: QualityAnalysisSummaryProps) {
  if (!quality) {
    if (!statusMessage) return null;
    return (
      <div className="info-section detail-quality-analysis">
        <h3>Quality analysis</h3>
        <p className="detail-quality-status">{statusMessage}</p>
      </div>
    );
  }

  return (
    <div className="info-section detail-quality-analysis">
      <h3>Quality analysis</h3>
      <div
        className="detail-quality-summary"
        title={qualityScoreDescription(quality, 'capture_sequence')}
      >
        <strong>{Math.round(quality.quality_score * 100)}%</strong>
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
    </div>
  );
}
