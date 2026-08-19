import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import StackSnrCurve from '../StackSnrCurve';
import type { ProgressiveSnr, SnrPoint } from '../../api/types';

function points(noiseAt: (frames: number) => number, depths: number[]): SnrPoint[] {
  return depths.map((frames) => {
    const noise = noiseAt(frames);
    return {
      frames,
      exposure_seconds: frames * 300,
      noise,
      background: 1000,
      signal: 500,
      snr: 500 / noise,
    };
  });
}

function curve(overrides: Partial<ProgressiveSnr> = {}): ProgressiveSnr {
  return {
    order: 'capture',
    points: points((n) => 20 / Math.sqrt(n), [1, 2, 4, 8, 16, 32]),
    analysis: {
      measured_frames: 32,
      measured_seconds: 9600,
      best_snr: 141.4,
      final_noise: 3.54,
      noise_exponent: -0.5,
      overall_noise_exponent: -0.5,
      fit_r_squared: 1,
      ideal_exponent: -0.5,
      efficiency: 1,
      frames_for_90_percent: 32,
      seconds_for_90_percent: 9600,
      frames_for_95_percent: 32,
      seconds_for_95_percent: 9600,
      projections: [{ gain: 1.1, extra_frames: 7, extra_seconds: 2100 }],
      regressions: [],
      verdict: 'improving',
      summary: 'Noise is still falling at 100% of the ideal rate.',
    },
    ...overrides,
  };
}

describe('StackSnrCurve', () => {
  it('shows the verdict, the fitted exponent and what more frames would buy', () => {
    render(<StackSnrCurve curve={curve()} label="M31 Ha" />);

    expect(screen.getByText('Still improving')).toBeInTheDocument();
    expect(screen.getByText('-0.50')).toBeInTheDocument();
    expect(screen.getByText('100%')).toBeInTheDocument();
    expect(screen.getByText('+7')).toBeInTheDocument();
    expect(screen.getByText(/frames \(0.6 h\) for 10% more/)).toBeInTheDocument();
    expect(
      screen.getByRole('img', { name: /M31 Ha: signal-to-noise ratio against frame count/ }),
    ).toBeInTheDocument();
  });

  it('draws the ideal square-root line beside the measured one', () => {
    const { container } = render(<StackSnrCurve curve={curve()} label="M31 Ha" />);

    // Both lines are needed: the reading is the gap between them.
    expect(container.querySelector('.stack-snr-measured')).not.toBeNull();
    expect(container.querySelector('.stack-snr-ideal')).not.toBeNull();
    expect(container.querySelectorAll('.stack-snr-chart circle')).toHaveLength(6);
  });

  it('names the frames that made the stack noisier, and what that means in quality order', () => {
    render(
      <StackSnrCurve
        curve={curve({
          order: 'quality',
          analysis: {
            ...curve().analysis!,
            verdict: 'degrading',
            regressions: [{ from_frames: 16, to_frames: 32, noise_increase: 0.2 }],
          },
        })}
        label="M31 Ha"
      />,
    );

    expect(screen.getByText('Getting worse')).toBeInTheDocument();
    expect(screen.getByText(/16→32 frames \(\+20%\)/)).toBeInTheDocument();
    expect(
      screen.getByText(/weaker frames stop paying for themselves/),
    ).toBeInTheDocument();
  });

  it('says a trend cannot be read yet instead of inventing one', () => {
    render(
      <StackSnrCurve
        curve={curve({ points: points((n) => 20 / Math.sqrt(n), [1, 2]), analysis: null })}
        label="M31 Ha"
      />,
    );

    expect(screen.getByText(/Three are needed/)).toBeInTheDocument();
    expect(screen.queryByText('Still improving')).not.toBeInTheDocument();
  });

  it('renders nothing before a second depth exists', () => {
    const { container } = render(
      <StackSnrCurve
        curve={curve({ points: points((n) => 20 / Math.sqrt(n), [1]), analysis: null })}
        label="M31 Ha"
      />,
    );

    expect(container.firstChild).toBeNull();
  });
});
