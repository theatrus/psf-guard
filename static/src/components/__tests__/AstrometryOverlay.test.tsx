import { render } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { OverlaySolution } from '@seiza/astro-overlay';
import AstrometryOverlay from '../AstrometryOverlay';

describe('AstrometryOverlay', () => {
  it('renders v4 catalog outlines with Seiza catalog-aware colors', () => {
    const solution: OverlaySolution = {
      image_width: 200,
      image_height: 200,
      objects: [
        {
          stable_id: 'openngc:NGC1',
          name: 'NGC 1',
          common_name: 'Test Nebula',
          kind: 'nebula',
          x: 100,
          y: 100,
          semi_major_px: 25,
          semi_minor_px: 10,
          angle_deg: null,
          outlines: [
            {
              geometry_id: 'openngc:NGC1#outline-1',
              source_record_id: 'openngc:NGC1',
              role: 'brightness-level',
              quality: 'catalog',
              level: '1',
              contours: [
                {
                  closed: true,
                  points: [
                    [90, 110],
                    [100, 85],
                    [115, 110],
                  ],
                },
              ],
            },
          ],
        },
      ],
    };

    const { container } = render(
      <AstrometryOverlay solution={solution} showCenter={false} />
    );
    const outline = container.querySelector('.seiza-overlay__marker--outline');
    const label = container.querySelector('.seiza-overlay__label');

    expect(outline).toHaveAttribute('data-geometry-id', 'openngc:NGC1#outline-1');
    expect(outline).toHaveAttribute('data-outline-level', '1');
    expect(outline).toHaveAttribute('stroke', '#55cfff');
    expect(outline).toHaveAttribute(
      'd',
      'M 90.00 110.00 L 100.00 85.00 L 115.00 110.00 Z'
    );
    expect(label).toHaveAttribute('fill', '#55cfff');
  });
});
