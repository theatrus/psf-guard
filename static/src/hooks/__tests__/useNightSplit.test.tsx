import type { ReactNode } from 'react';
import { describe, expect, it } from 'vitest';
import { act, renderHook, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import { keepAcrossNightSplit, useNightSplit } from '../useNightSplit';

function wrapper(entry: string) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return (
      <QueryClientProvider client={client}>
        <MemoryRouter initialEntries={[entry]}>{children}</MemoryRouter>
      </QueryClientProvider>
    );
  };
}

const render = (entry = '/grid?db=main') => renderHook(
  () => ({ night: useNightSplit(), search: useLocation().search }),
  { wrapper: wrapper(entry) },
);

describe('the night split', () => {
  it('follows the server setting, and the link over it', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json({
      success: true, error: null, data: { rules: { light_nights: 'catalog' } },
    })));
    const { result } = render();
    await waitFor(() => expect(result.current.night.setting).toBe('catalog'));
    expect(result.current.night.split).toBe('catalog');
    // Keyed on the override alone, so the setting's arrival fetches nothing again.
    expect(result.current.night.queryKey).toBe('setting');

    act(() => result.current.night.setSplit('rig'));
    await waitFor(() => expect(new URLSearchParams(result.current.search).get('nights')).toBe('rig'));
    expect(result.current.night.queryKey).toBe('rig');
    act(() => result.current.night.setSplit('catalog'));
    await waitFor(() => expect(new URLSearchParams(result.current.search).get('nights')).toBeNull());
  });

  it('puts a choice in the link while the setting is unknown', async () => {
    server.use(http.get('/api/settings/calibration', () => HttpResponse.json(
      { success: false, error: 'nope', data: null }, { status: 500 },
    )));
    const { result } = render();
    expect(result.current.night.setting).toBeNull();
    act(() => result.current.night.setSplit('rig'));
    await waitFor(() => expect(new URLSearchParams(result.current.search).get('nights')).toBe('rig'));
  });

  it('keeps a scope\'s frames across a split, not across scopes', () => {
    const keep = keepAcrossNightSplit<number[]>(['db', 'main', 'all-images', 1, 2]);
    expect(keep([1, 2, 3], { queryKey: ['db', 'main', 'all-images', 1, 2, 'setting'] })).toEqual([1, 2, 3]);
    expect(keep([1, 2, 3], { queryKey: ['db', 'main', 'all-images', 1, 9, 'setting'] })).toBeUndefined();
    expect(keep(undefined, undefined)).toBeUndefined();
  });
});
