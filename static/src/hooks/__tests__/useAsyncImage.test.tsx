import { act, renderHook } from '@testing-library/react';
import { describe, expect, it } from 'vitest';
import { useAsyncImage } from '../useAsyncImage';
import type { PreviewDescriptor } from '../../api/types';

function preview(imageId: number): PreviewDescriptor {
  return { imageId, kind: 'preview', size: 'screen' };
}

describe('useAsyncImage', () => {
  it('does not carry ready state across source changes', () => {
    const firstSrc = '/api/db/test/images/1/preview?size=screen';
    const secondSrc = '/api/db/test/images/2/preview?size=screen';
    const { result, rerender } = renderHook(
      ({ src, descriptor }) => useAsyncImage('test', src, descriptor),
      {
        initialProps: {
          src: firstSrc,
          descriptor: preview(1),
        },
      }
    );

    act(() => result.current.onLoad());
    expect(result.current.state).toBe('ready');

    rerender({ src: secondSrc, descriptor: preview(2) });

    expect(result.current.src).toBe(secondSrc);
    expect(result.current.state).toBe('loading');
  });
});
