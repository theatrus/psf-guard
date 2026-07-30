import type { ImageQualityResult } from '../api/types';
import { qualityScoreBasis, qualityScoreDescription } from '../utils/qualityScore';
import { QualityReasonText } from './QualityReasonPopover';

interface QualityAnalysisSummaryProps {
  quality?: ImageQualityResult | null;
}

export default function QualityAnalysisSummary({ quality }: QualityAnalysisSummaryProps) {
  if (!quality || (!quality.regrade_reason && !quality.details)) return null;

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
      <div className="detail-quality-reason">
        <strong>{quality.regrade_reason ? 'Review reason' : 'Score reason'}</strong>
        <QualityReasonText reason={quality.regrade_reason} details={quality.details} />
      </div>
    </div>
  );
}
