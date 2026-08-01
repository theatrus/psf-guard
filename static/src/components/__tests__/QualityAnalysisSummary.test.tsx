import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
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

  it('offers a measured region overlay only when one is available', () => {
    const onToggleOverlay = vi.fn();
    render(
      <QualityAnalysisSummary
        quality={{
          ...quality,
          spatial_overlay: {
            grid_cols: 2,
            grid_rows: 2,
            image_width: 2000,
            image_height: 1500,
            low_star_cells: [true, false, false, false],
            extinction_cells: [false, false, false, false],
            star_loss_cells: [false, false, false, false],
            background_rise_cells: [false, false, false, false],
            background_fall_cells: [false, false, false, false],
            glow_cells: [false, false, false, false],
          },
        }}
        overlayVisible={false}
        onToggleOverlay={onToggleOverlay}
      />,
    );

    const toggle = screen.getByRole('button', { name: /show affected regions/i });
    expect(toggle).toHaveAttribute('aria-pressed', 'false');
    expect(screen.getByText('Low star coverage')).toBeInTheDocument();
    fireEvent.click(toggle);
    expect(onToggleOverlay).toHaveBeenCalledOnce();
  });
});
