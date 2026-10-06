import { renderHook, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../../api/client';
import type { DirectorCutoutRequest } from '../../../api/directorTypes';
import { useSurveyCutout } from '../useSurveyCutout';

const main: DirectorCutoutRequest = { survey: 'dss2_color', ra: 10, dec: 20, fov: 4, width: 256, height: 192, rotation: 0 };
const wider = [16, 64, 180].map(fov => ({ ...main, fov }));

describe('survey cutouts', () => {
  beforeEach(() => { vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => 'blob:tile'), revokeObjectURL: vi.fn() })); });
  afterEach(() => { vi.restoreAllMocks(); vi.unstubAllGlobals(); });

  it('asks for each wider tile once, however many land meanwhile', async () => {
    const asked: string[] = [];
    vi.spyOn(apiClient, 'fetchDirectorCutout').mockImplementation(async request => {
      asked.push(JSON.stringify(request));
      await new Promise(resolve => setTimeout(resolve, 5));
      return { state: 'ready', blob: new Blob([]) };
    });
    const { result } = renderHook(() => useSurveyCutout(main, 0, wider));
    await waitFor(() => expect(result.current.tiles).toHaveLength(4));
    // Each landed tile used to start the prefetch over, so a tile still on
    // its way was asked for again.
    expect(asked).toEqual([main, ...wider].map(request => JSON.stringify(request)));
  });

  it('drops a request the view has moved on from', async () => {
    const signals: AbortSignal[] = [];
    vi.spyOn(apiClient, 'fetchDirectorCutout').mockImplementation((_request, signal) => {
      signals.push(signal!);
      return new Promise((_resolve, reject) => signal!.addEventListener('abort', () => reject(new Error('aborted'))));
    });
    const { rerender, unmount } = renderHook(({ request }) => useSurveyCutout(request, 0), { initialProps: { request: main } });
    await waitFor(() => expect(signals).toHaveLength(1));
    rerender({ request: { ...main, ra: 11 } });
    await waitFor(() => expect(signals).toHaveLength(2));
    expect(signals[0].aborted).toBe(true);
    expect(signals[1].aborted).toBe(false);
    unmount();
    expect(signals[1].aborted).toBe(true);
  });
});
