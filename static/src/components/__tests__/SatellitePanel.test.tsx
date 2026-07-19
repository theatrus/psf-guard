import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import SatellitePanel from '../SatellitePanel';
import type { SatelliteAnalysisStatus } from '../../api/types';

describe('SatellitePanel', () => {
  it('labels prediction evidence and exposes the high-risk track toggle', () => {
    const onToggleOverlay = vi.fn();
    const status = {
      orbital_elements_cached: true,
      analysis: {
        association: 'predicted_not_pixel_detected',
        catalog: { source: 'cache.json', state: 'cached' },
        exposure: { duration_seconds: 120 },
        tracks: [{
          label: 'STARLINK-1234 · NORAD 54321',
          norad_id: 54321,
          risk_level: 'high',
          bright_trail_risk: 0.81,
        }],
        risk: {
          track_count: 1,
          potentially_bright_count: 1,
          high_risk_count: 1,
          maximum_bright_trail_risk: 0.81,
          reject_recommended: true,
        },
      },
    } as SatelliteAnalysisStatus;

    render(
      <SatellitePanel
        status={status}
        isLoading={false}
        isPredicting={false}
        overlayVisible={true}
        onToggleOverlay={onToggleOverlay}
        onPredict={vi.fn()}
      />
    );

    expect(screen.getByText(/does not claim a trail was detected/i)).toBeInTheDocument();
    expect(screen.getByText('1 high')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: /STARLINK-1234/ })).toHaveAttribute(
      'href',
      'https://www.n2yo.com/satellite/?s=54321'
    );
    expect(screen.getByRole('link', { name: /STARLINK-1234/ })).toHaveAttribute('target', '_blank');
    fireEvent.click(screen.getByRole('button', { name: /Track identifiers on/ }));
    expect(onToggleOverlay).toHaveBeenCalledOnce();
  });

  it('offers on-demand prediction when no cached analysis exists', () => {
    const onPredict = vi.fn();
    render(
      <SatellitePanel
        status={{ orbital_elements_cached: false }}
        isLoading={false}
        isPredicting={false}
        overlayVisible={false}
        onToggleOverlay={vi.fn()}
        onPredict={onPredict}
      />
    );
    fireEvent.click(screen.getByRole('button', { name: /Identify satellite tracks/ }));
    expect(onPredict).toHaveBeenCalledOnce();
  });
});
