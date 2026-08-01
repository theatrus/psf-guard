import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { describe, expect, it, vi } from 'vitest';
import type { Image, ImageQualityResult } from '../../api/types';
import ImageCard from '../ImageCard';

const image = {
  id: 42,
  project_id: 1,
  project_name: 'Project',
  project_display_name: 'Project',
  target_id: 1,
  target_name: 'Target',
  acquired_date: 1_750_000_000,
  filter_name: 'Ha',
  grading_status: 0,
  reject_reason: null,
  metadata: {},
  filesystem_path: null,
} as unknown as Image;

const quality = {
  image_id: 42,
  quality_score: 0.42,
  temporal_anomaly_score: 0,
  category: null,
  flags: [],
  normalized_metrics: {
    star_count: 0.4,
    hfr: null,
    eccentricity: null,
    snr: null,
    background: null,
    spatial_coverage: null,
    transparency: null,
    pointing: null,
  },
  details: 'Star count is below the normal range for matching capture settings.',
} satisfies ImageQualityResult;

describe('ImageCard quality reason', () => {
  it('keeps the score visible without adding an empty reason trigger', () => {
    const { container } = render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
        quality={{ ...quality, details: null }}
        selectionEffects={false}
      />
    );

    expect(container.querySelector('.quality-badge')).toHaveTextContent('42');
    expect(container.querySelector('.sequence-score-basis')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Show quality reason' })).not.toBeInTheDocument();
  });

  it('opens long evidence in a popover without selecting the card', async () => {
    const user = userEvent.setup();
    const onClick = vi.fn();
    render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={onClick}
        onDoubleClick={() => {}}
        quality={quality}
        selectionEffects={false}
      />
    );

    expect(screen.queryByText(quality.details!)).not.toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Show quality reason' }));

    const popover = screen.getByRole('dialog', { name: 'Quality reason' });
    expect(popover).toHaveTextContent(quality.details!);
    expect(onClick).not.toHaveBeenCalled();

    await user.keyboard('{Escape}');
    expect(screen.queryByRole('dialog', { name: 'Quality reason' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Show quality reason' })).toHaveFocus();
  });

  it('keeps compact Grid quality controls out of the card body', () => {
    const { container } = render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
        quality={quality}
        qualityPresentation="compact"
        selectionEffects={false}
      />
    );

    expect(container.querySelector('.quality-badge')).toHaveTextContent('42');
    expect(container.querySelector('.sequence-score-basis')).not.toBeInTheDocument();
    expect(container.querySelector('.image-info .sequence-reason-trigger')).not.toBeInTheDocument();
    expect(container.querySelector('.image-preview .sequence-reason-trigger')).toBeInTheDocument();
  });

  it('does not show a satellite warning for an orbital prediction alone', () => {
    const { container } = render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
        quality={{
          ...quality,
          details: null,
          satellite: {
            predicted_tracks: 2,
            potentially_bright_count: 2,
            high_risk_count: 1,
            maximum_bright_trail_risk: 0.9,
            pixel_alignment_attempted: true,
            pixel_aligned_count: 0,
            pixel_aligned_high_risk_count: 0,
            reject_recommended: false,
            association: 'predicted_pixel_checked',
          },
        }}
        selectionEffects={false}
      />
    );

    expect(container.querySelector('.analysis-signal')).not.toBeInTheDocument();
  });

  it('shows a satellite warning when pixel evidence matches a prediction', () => {
    render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
        quality={{
          ...quality,
          category: 'satellite_trail_detected',
          details: null,
          satellite: {
            predicted_tracks: 2,
            potentially_bright_count: 1,
            high_risk_count: 0,
            maximum_bright_trail_risk: 0.6,
            pixel_alignment_attempted: true,
            pixel_aligned_count: 1,
            pixel_aligned_high_risk_count: 0,
            reject_recommended: false,
            association: 'predicted_with_pixel_alignment',
          },
        }}
        selectionEffects={false}
      />
    );

    expect(screen.getByText('Satellite Trail Detected')).toBeInTheDocument();
    expect(screen.getByText('satellite pixel match')).toBeInTheDocument();
  });
});
