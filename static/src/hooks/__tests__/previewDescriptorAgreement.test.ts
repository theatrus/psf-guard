import { describe, expect, it } from 'vitest';
import { apiClient } from '../../api/client';
import { descriptorKey } from '../previewPoll';
import type { PreviewDescriptor } from '../../api/types';

/**
 * A preview `<img>` and the poll that waits for it must describe the same
 * artifact.
 *
 * The URL and the descriptor are built separately at every call site, and the
 * server keys colour and greyscale renditions apart. Three views have now
 * shipped with `color` on one and not the other; each time the browser waited
 * on an artifact nobody was generating, and only with the preference turned
 * off, which is the case nobody clicks through.
 *
 * The check is that whatever the URL asks for appears in the descriptor too.
 */
function colorInUrl(url: string): boolean | undefined {
  const value = new URL(url, 'http://localhost').searchParams.get('color');
  return value === null ? undefined : value === 'true';
}

describe('preview URL and poll descriptor agreement', () => {
  const cases: Array<{ name: string; color: boolean | undefined }> = [
    { name: 'colour on', color: true },
    { name: 'colour off', color: false },
    { name: 'unstated', color: undefined },
  ];

  for (const { name, color } of cases) {
    it(`carries the same colour choice into both, with ${name}`, () => {
      const url = apiClient.getPreviewUrl('db', 7, { size: 'large', color });
      const descriptor: PreviewDescriptor = {
        imageId: 7,
        kind: 'preview',
        size: 'large',
        color,
      };
      expect(colorInUrl(url)).toBe(descriptor.color);
    });
  }

  it('gives colour and greyscale different descriptor keys', () => {
    // If they collided, a poll for one would report the other ready and the
    // <img> would reload into a 202 forever.
    const base: PreviewDescriptor = { imageId: 7, kind: 'preview', size: 'large' };
    expect(descriptorKey({ ...base, color: true })).not.toBe(
      descriptorKey({ ...base, color: false })
    );
  });

  it('gives colour and greyscale different URLs', () => {
    expect(apiClient.getPreviewUrl('db', 7, { size: 'large', color: true })).not.toBe(
      apiClient.getPreviewUrl('db', 7, { size: 'large', color: false })
    );
  });
});
