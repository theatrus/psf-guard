import { act, renderHook } from '@testing-library/react';
import { beforeEach, describe, expect, it, vi } from 'vitest';
import { useAsyncImage } from '../useAsyncImage';
import type { PreviewDescriptor } from '../../api/types';

const { registerPendingMock } = vi.hoisted(() => ({
  registerPendingMock: vi.fn(),
}));

vi.mock('../previewPoll', () => ({ registerPending: registerPendingMock }));

function preview(imageId: number): PreviewDescriptor {
  return { imageId, kind: 'preview', size: 'screen' };
}

describe('useAsyncImage', () => {
  beforeEach(() => {
    registerPendingMock.mockReset();
  });

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

  it('surfaces the sanitized terminal message from the shared poller', () => {
    let reportError: ((message: string) => void) | undefined;
    registerPendingMock.mockImplementation(
      (_dbId, _descriptor, callbacks: { onError: (message: string) => void }) => {
        reportError = callbacks.onError;
        return vi.fn();
      }
    );
    const { result } = renderHook(() =>
      useAsyncImage(
        'test',
        '/api/db/test/images/1/preview?size=screen',
        preview(1)
      )
    );

    act(() => result.current.onError());
    act(() => reportError?.('frame.fits matches multiple capture candidates'));

    expect(result.current.state).toBe('error');
    expect(result.current.error).toBe(
      'frame.fits matches multiple capture candidates'
    );
  });
});
