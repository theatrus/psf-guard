import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import SatelliteTrackOverlay from '../SatelliteTrackOverlay';
import type { SatelliteAnalysis } from '../../api/types';

const analysis = {
  tracks: [
    {
      label: 'ISS (ZARYA) · NORAD 25544',
      norad_id: 25544,
      risk_level: 'high',
      clipped_segments: [
        [[10, 20], [100, 120]],
        [[100, 120], [180, 140]],
      ],
    },
  ],
} as SatelliteAnalysis;

describe('SatelliteTrackOverlay', () => {
  it('draws clipped segments and a catalog identifier in sensor coordinates', () => {
    const { container, getByText } = render(
      <SatelliteTrackOverlay
        analysis={analysis}
        imageWidth={300}
        imageHeight={200}
      />
    );

    expect(container.querySelector('svg')).toHaveAttribute('viewBox', '0 0 300 200');
    expect(container.querySelectorAll('line')).toHaveLength(2);
    expect(container.querySelector('line')).toHaveAttribute('stroke', '#ff4d5a');
    expect(getByText('ISS (ZARYA) · NORAD 25544')).toBeInTheDocument();
  });
});
