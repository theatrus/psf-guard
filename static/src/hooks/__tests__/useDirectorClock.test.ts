import { act, renderHook } from '@testing-library/react';
import { afterEach, expect, it, vi } from 'vitest';
import { useDirectorClock } from '../useDirectorClock';

afterEach(() => vi.useRealTimers());

it('shares one offline freshness clock and releases it after the last view closes', () => {
  vi.useFakeTimers();
  vi.setSystemTime(1_800_000_000_000);
  const first = renderHook(useDirectorClock);
  const second = renderHook(useDirectorClock);
  expect(vi.getTimerCount()).toBe(1);
  expect(first.result.current).toBe(second.result.current);
  act(() => vi.advanceTimersByTime(5000));
  expect(first.result.current).toBe(1_800_000_005_000);
  expect(second.result.current).toBe(first.result.current);
  first.unmount();
  expect(vi.getTimerCount()).toBe(1);
  second.unmount();
  expect(vi.getTimerCount()).toBe(0);
});
