import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { QualityRegionOverlay as QualityRegionOverlayData } from '../../api/types';
import QualityRegionOverlay from '../QualityRegionOverlay';

const overlay: QualityRegionOverlayData = {
  grid_cols: 2,
  grid_rows: 2,
  image_width: 2000,
  image_height: 1500,
  low_star_cells: [true, false, false, false],
  extinction_cells: [false, true, false, false],
  star_loss_cells: [false, false, false, false],
  background_rise_cells: [false, false, true, false],
  background_fall_cells: [false, false, true, false],
  glow_cells: [false, false, false, false],
};

describe('QualityRegionOverlay', () => {
  it('draws each measured cell over the analysis grid', () => {
    const { container } = render(<QualityRegionOverlay overlay={overlay} />);

    expect(screen.getByRole('img', { name: '3 quality analysis regions' })).toBeInTheDocument();
    expect(container.querySelectorAll('[data-cell-index]')).toHaveLength(3);
    expect(container.querySelector('.quality-region-low-stars')).toBeInTheDocument();
    expect(container.querySelector('.quality-region-extinction')).toBeInTheDocument();
    expect(container.querySelector('.quality-region-background-rise')).toBeInTheDocument();
    expect(container.querySelector('.quality-region-background-fall')).toBeInTheDocument();
  });

  it('draws nothing when no cell supports the finding', () => {
    const empty = Object.fromEntries(
      Object.entries(overlay).map(([key, value]) => [
        key,
        Array.isArray(value) ? value.map(() => false) : value,
      ]),
    ) as unknown as QualityRegionOverlayData;
    const { container } = render(<QualityRegionOverlay overlay={empty} />);
    expect(container).toBeEmptyDOMElement();
  });
});
