import { render, screen } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import type { Image } from '../../api/types';
import { GradingStatus } from '../../api/types';
import ImageCard from '../ImageCard';

const image = {
  id: 7,
  project_id: 1,
  project_name: 'Project',
  project_display_name: 'Project',
  target_id: 1,
  target_name: 'Alpha M44',
  acquired_date: 1_750_000_000,
  filter_name: 'B',
  grading_status: GradingStatus.Rejected,
  reject_reason: 'Clouds over the eastern half of the frame',
  metadata: { HFR: 2.4, DetectedStars: 520 },
  filesystem_path: null,
} as unknown as Image;

const card = (props: Partial<Parameters<typeof ImageCard>[0]> = {}) => render(
  <ImageCard dbId="db" image={image} isSelected={false} onClick={() => {}} onDoubleClick={() => {}}
    selectionEffects={false} qualityPresentation="compact" {...props} />
);

describe('ImageCard text', () => {
  it('puts filter, time, HFR and stars in one row of four cells', () => {
    const { container } = card();
    const cells = [...container.querySelectorAll('.image-facts > span')];
    expect(cells.map(cell => cell.classList[0])).toEqual(['image-filter', 'image-date', 'stat-hfr', 'stat-stars']);
    expect(cells[0]).toHaveTextContent('B');
    expect(cells[2]).toHaveTextContent('HFR 2.40');
    expect(cells[3]).toHaveTextContent('★520');
    // The full date stays one hover away.
    expect(cells[1].getAttribute('title')).toBe(new Date(1_750_000_000 * 1000).toLocaleString());
  });

  it('dates a frame with its year, and marks one from another year', () => {
    const { container } = card();
    const date = container.querySelector('.image-date')!;
    const taken = new Date(1_750_000_000 * 1000);
    expect(date.querySelector('.date-day')).toHaveTextContent(String(taken.getFullYear()));
    expect(date.classList.contains('is-other-year')).toBe(taken.getFullYear() !== new Date().getFullYear());
    const now = Math.floor(Date.now() / 1000);
    const { container: recent } = card({ image: { ...image, acquired_date: now } });
    const today = recent.querySelector('.image-date')!;
    expect(today.classList.contains('is-other-year')).toBe(false);
    expect(today.querySelector('.date-day')).toHaveTextContent(String(new Date().getFullYear()));
  });

  it('keeps the status to one strip, with the whole reason in its title', () => {
    const { container } = card();
    const status = container.querySelector('.image-status')!;
    expect(status).toHaveTextContent('Rejected · Clouds over the eastern half of the frame');
    expect(status).toHaveAttribute('title', 'Rejected: Clouds over the eastern half of the frame');
  });

  it('names the target only when asked to', () => {
    card();
    expect(screen.getByRole('heading', { name: 'Alpha M44' })).toBeInTheDocument();
  });

  it('leaves the target out when the view shows only one', () => {
    card({ showTarget: false });
    expect(screen.queryByRole('heading', { name: 'Alpha M44' })).not.toBeInTheDocument();
  });

  it('draws no empty signals row in the compact card', () => {
    const { container } = card();
    expect(container.querySelector('.image-signals')).toBeEmptyDOMElement();
  });
});
