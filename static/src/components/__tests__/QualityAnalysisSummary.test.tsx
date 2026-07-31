import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { ImageQualityResult } from '../../api/types';
import QualityAnalysisSummary from '../QualityAnalysisSummary';

const quality: ImageQualityResult = {
  image_id: 42,
  quality_score: 0.36,
  temporal_anomaly_score: 0.8,
  category: 'tracking_issue',
  normalized_metrics: {
    star_count: 0.4,
    hfr: 0.3,
    eccentricity: 0.2,
    snr: null,
    background: null,
  },
  regrade_reason: 'Tracking error: elongated stars',
  details: 'HFR and eccentricity are poor compared with this capture sequence.',
};

describe('QualityAnalysisSummary', () => {
  it('shows the sequence score, review reason, and evidence', () => {
    render(<QualityAnalysisSummary quality={quality} />);

    expect(screen.getByRole('heading', { name: 'Quality analysis' })).toBeInTheDocument();
    expect(screen.getByText('36%')).toBeInTheDocument();
    expect(screen.getByText('Review reason')).toBeInTheDocument();
    expect(screen.getByText(quality.regrade_reason!)).toBeInTheDocument();
    expect(screen.getByText('Evidence')).toBeInTheDocument();
    expect(screen.getByText(quality.details!)).toBeInTheDocument();
  });

  it('says when a scored image has no specific issue', () => {
    render(
      <QualityAnalysisSummary quality={{ ...quality, regrade_reason: undefined, details: null }} />,
    );

    expect(screen.getByText('No specific quality issue was identified.')).toBeInTheDocument();
  });

  it('explains why an image has no score', () => {
    render(<QualityAnalysisSummary statusMessage="Choose a project to load quality." />);

    expect(screen.getByText('Choose a project to load quality.')).toBeInTheDocument();
  });
});
