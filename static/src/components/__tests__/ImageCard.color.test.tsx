import { act, render, screen } from '@testing-library/react';
import { beforeEach, describe, expect, it } from 'vitest';
import ImageCard from '../ImageCard';
import { setColorPreview } from '../../hooks/useColorPreview';
import type { Image } from '../../api/types';

/**
 * The grid must ask for the rendition the shared preference names.
 *
 * Every view that shows a preview builds its own URL, and four of them were
 * once left out of the colour preference — each silently falling back to a
 * server default, so the grid and the detail view could disagree about the
 * same frame. This pins the grid's half of that.
 */

const image = {
  id: 42,
  project_id: 1,
  project_name: 'Sync Nebula',
  project_display_name: 'Sync Nebula',
  target_id: 1,
  target_name: 'Core',
  acquired_date: 1_750_000_000,
  filter_name: 'OSC',
  grading_status: 0,
  reject_reason: null,
  metadata: {},
  filesystem_path: null,
} as unknown as Image;

function previewSrc(): string {
  const img = screen.getByRole('img', { hidden: true }) as HTMLImageElement;
  return img.getAttribute('src') ?? '';
}

describe('ImageCard colour preference', () => {
  beforeEach(() => {
    setColorPreview(true);
  });

  it('asks for colour while the preference is on', () => {
    render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
      />
    );
    expect(previewSrc()).toContain('color=true');
  });

  it('follows the preference when it is turned off', () => {
    render(
      <ImageCard
        dbId="db"
        image={image}
        isSelected={false}
        onClick={() => {}}
        onDoubleClick={() => {}}
      />
    );
    act(() => setColorPreview(false));
    // Explicit, not merely absent: a missing parameter would take the
    // server's default, which is colour.
    expect(previewSrc()).toContain('color=false');
  });
});
