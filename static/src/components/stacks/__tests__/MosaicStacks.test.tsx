import { describe, expect, it } from 'vitest';
import { render, screen } from '@testing-library/react';
import MosaicStacks from '../MosaicStacks';
import type { MosaicPanel } from '../../../api/types';

// A 1000 × 750 preview at 1.8″ a pixel, north up and east left.
const preview = (ra: number) => ({
  url: `/stack-${ra}.png`, width: 1000, height: 750, kind: 'mono' as const, filter: 'L',
  wcs: { crpix1: 500, crpix2: 375, crval1: ra, crval2: 41, cd11: -0.0005, cd12: 0, cd21: 0, cd22: -0.0005 },
});

describe('MosaicStacks', () => {
  it("lays each panel's stack out as the grid and places the solved ones on the sky", () => {
    const panels: MosaicPanel[] = [
      { target_id: 1, target_name: 'M31 1', panel_id: 'r1c1', row: 1, column: 1, preview: preview(10.9) },
      { target_id: 2, target_name: 'M31 2', panel_id: 'r1c2', row: 1, column: 2, preview: preview(10.4) },
      { target_id: 3, target_name: 'M31 3', panel_id: 'r2c1', row: 2, column: 1, preview: null },
    ];
    const { container } = render(<MosaicStacks mosaic={{ project_id: 1, name: 'M31', source: 'director', rows: 2, columns: 2, panels }} />);
    expect(screen.getByRole('heading', { name: 'M31 mosaic (3 panels)' })).toBeInTheDocument();
    const cells = container.querySelectorAll<HTMLElement>('.mosaic-stacks-cell');
    expect([...cells].map(cell => [cell.style.gridRow, cell.style.gridColumn])).toEqual([['1', '1'], ['1', '2'], ['2', '1']]);
    expect(screen.getByText('No stack yet')).toBeInTheDocument();
    const sky = screen.getByRole('img', { name: 'Panels placed on the sky by their plate solves' });
    const images = sky.querySelectorAll('image');
    expect(images).toHaveLength(2);
    // East is left: the panel at the larger right ascension sits further left.
    const shift = (element: Element) => Number(element.getAttribute('transform')!.match(/matrix\(([^)]+)\)/)![1].split(' ')[4]);
    expect(shift(images[0])).toBeLessThan(shift(images[1]));
    expect(screen.getByText(/No stack yet: r2c1/)).toBeInTheDocument();
  });
});
