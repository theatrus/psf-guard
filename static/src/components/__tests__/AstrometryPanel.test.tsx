import { fireEvent, render, screen } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import AstrometryPanel from '../AstrometryPanel';
import type { AstrometryAnalysis } from '../../api/types';

const catalogHit = {
  stable_id: 'openngc:NGC224',
  source: 'OpenNGC',
  aliases: ['NGC 224'],
  parent_ids: [],
  alternate_ids: ['messier:M31'],
  alternate_sources: [],
  name: 'M 31',
  common_name: 'Andromeda Galaxy',
  kind: 'galaxy',
  mag: 3.44,
  major_arcmin: 177.8,
  minor_arcmin: 69.7,
  position_angle_deg: 35,
  ra_deg: 10.6848,
  dec_deg: 41.2691,
  center_inside: true,
  extent_only: false,
  distance_from_center_deg: 0.01,
  predicted_prominence: 0.9,
};

const solved: AstrometryAnalysis = {
  image_id: 1,
  status: 'solved',
  mode: 'embedded_wcs',
  solution: {
    center_ra_deg: 10.67,
    center_dec_deg: 41.27,
    pixel_scale_arcsec_per_pixel: 1.375,
    image_width: 3840,
    image_height: 2160,
    wcs: {
      crval: [10.67, 41.27],
      crpix: [1919, 1079],
      cd: [[-0.00038, 0], [0, 0.00038]],
    },
    objects: [{
      stable_id: catalogHit.stable_id,
      source: catalogHit.source,
      name: catalogHit.name,
      common_name: catalogHit.common_name,
      kind: catalogHit.kind,
      x: 1920,
      y: 1080,
      semi_major_px: 500,
      semi_minor_px: 200,
      angle_deg: 35,
      prominence: 0.9,
    }],
  },
  catalog_hits: [catalogHit],
  source_fingerprint: {
    canonical_path: '/images/m31.fits',
    size_bytes: 1,
    modified_unix_seconds: 1,
  },
  computed_at: 1,
};

describe('AstrometryPanel', () => {
  it('shows embedded WCS context, catalog objects, and an overlay toggle', () => {
    const onToggle = vi.fn();
    render(
      <AstrometryPanel
        analysis={solved}
        isLoading={false}
        isSolving={false}
        overlayVisible={true}
        onToggleOverlay={onToggle}
        onSolve={vi.fn()}
      />
    );

    expect(screen.getByText('Embedded FITS WCS')).toBeInTheDocument();
    expect(screen.getByText('M 31')).toBeInTheDocument();
    expect(screen.getByText('Andromeda Galaxy')).toBeInTheDocument();
    expect(screen.getByText('1.375″/px')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: /Sky overlay on\s*1/ }));
    expect(onToggle).toHaveBeenCalledOnce();
  });

  it('labels coordinate-only results and offers an on-demand plate solve', () => {
    const onSolve = vi.fn();
    render(
      <AstrometryPanel
        analysis={{
          ...solved,
          status: 'catalog_only',
          mode: undefined,
          solution: undefined,
          catalog_scope: 'nearby_target',
          catalog_radius_deg: 1,
          hint_source: {
            ra_deg: 10.67,
            dec_deg: 41.27,
            source: 'fits_header',
            header_keywords: ['RA', 'DEC'],
          },
        }}
        isLoading={false}
        isSolving={false}
        overlayVisible={false}
        onToggleOverlay={vi.fn()}
        onSolve={onSolve}
      />
    );

    expect(screen.getByText('Nearby catalog')).toBeInTheDocument();
    expect(screen.getByText('Objects near target')).toBeInTheDocument();
    expect(screen.getByText('Within 1.0° · field size unknown')).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Solve field' }));
    expect(onSolve).toHaveBeenCalledOnce();
  });
});
