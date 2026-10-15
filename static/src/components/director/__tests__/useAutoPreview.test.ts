import { act, renderHook } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { useAutoPreview } from '../useAutoPreview';

/** A fetch whose answers the test hands out one at a time. */
function controlled() {
  const calls: Array<{ key: string; resolve: (value: string) => void; reject: (error: Error) => void }> = [];
  let running = 0;
  let most = 0;
  const fetchFor = (key: string) => () => new Promise<string>((resolve, reject) => {
    running++;
    most = Math.max(most, running);
    calls.push({ key, resolve: value => { running--; resolve(value); }, reject: error => { running--; reject(error); } });
  });
  return { calls, fetchFor, most: () => most };
}

type Props = { inputs: string; enabled: boolean };

function mount(api: ReturnType<typeof controlled>, initial: Props = { inputs: 'a', enabled: true }) {
  return renderHook(({ inputs, enabled }: Props) => useAutoPreview({ key: inputs, enabled, fetch: api.fetchFor(inputs), delay: 300 }), { initialProps: initial });
}

describe('useAutoPreview', () => {
  beforeEach(() => { vi.useFakeTimers(); });
  afterEach(() => { vi.useRealTimers(); });

  it('previews at once when it opens, and the answer is fresh', async () => {
    const api = controlled();
    const { result } = mount(api);
    expect(result.current.loading).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(api.calls.map(call => call.key)).toEqual(['a']);
    expect(result.current.fresh).toBe(false);
    await act(async () => { api.calls[0].resolve('preview a'); });
    expect(result.current).toMatchObject({ result: 'preview a', fresh: true, loading: false, error: null });
  });

  it('waits for the inputs to rest, then previews once', async () => {
    const api = controlled();
    const { result, rerender } = mount(api);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); api.calls[0].resolve('preview a'); });
    for (const inputs of ['b', 'c', 'd']) {
      rerender({ inputs, enabled: true });
      await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    }
    // Outdated at once: Activate must wait.
    expect(result.current.fresh).toBe(false);
    expect(result.current.loading).toBe(true);
    expect(api.calls).toHaveLength(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(300); });
    expect(api.calls.map(call => call.key)).toEqual(['a', 'd']);
    await act(async () => { api.calls[1].resolve('preview d'); });
    expect(result.current).toMatchObject({ result: 'preview d', fresh: true });
  });

  it('runs one preview at a time and drops an answer the inputs have outrun', async () => {
    const api = controlled();
    const { result, rerender } = mount(api);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    rerender({ inputs: 'b', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    // The first is still out: the second waits for it.
    expect(api.calls).toHaveLength(1);
    await act(async () => { api.calls[0].resolve('preview a'); });
    // Its answer is for inputs that changed: dropped, and the next one runs.
    expect(result.current.result).toBeNull();
    expect(result.current.fresh).toBe(false);
    expect(api.calls.map(call => call.key)).toEqual(['a', 'b']);
    await act(async () => { api.calls[1].resolve('preview b'); });
    expect(result.current).toMatchObject({ result: 'preview b', fresh: true });
    expect(api.most()).toBe(1);
  });

  it('keeps the rest after the last change though a stale answer lands first', async () => {
    const api = controlled();
    const { result, rerender } = mount(api);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    rerender({ inputs: 'b', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(400); });
    rerender({ inputs: 'c', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(250); });
    // 'a' lands 250 ms after the change to 'c': dropped, and 'c' waits.
    await act(async () => { api.calls[0].resolve('preview a'); });
    expect(api.calls.map(call => call.key)).toEqual(['a']);
    expect(result.current.loading).toBe(true);
    await act(async () => { await vi.advanceTimersByTimeAsync(49); });
    expect(api.calls).toHaveLength(1);
    await act(async () => { await vi.advanceTimersByTimeAsync(1); });
    expect(api.calls.map(call => call.key)).toEqual(['a', 'c']);
  });

  it('starts no answer twice when the inputs come back to the ones running', async () => {
    const api = controlled();
    const { result, rerender } = mount(api);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    rerender({ inputs: 'b', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(400); });
    rerender({ inputs: 'a', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(400); });
    await act(async () => { api.calls[0].resolve('preview a'); });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.calls.map(call => call.key)).toEqual(['a']);
    expect(result.current).toMatchObject({ result: 'preview a', fresh: true, loading: false });
    // A change and back before its rest is over asks nothing either.
    rerender({ inputs: 'b', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(100); });
    rerender({ inputs: 'a', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.calls).toHaveLength(1);
    expect(result.current.fresh).toBe(true);
  });

  it('runs again on refresh, never by itself, and keeps a result put in its place', async () => {
    const api = controlled();
    const { result, rerender } = mount(api);
    await act(async () => { await vi.advanceTimersByTimeAsync(0); api.calls[0].reject(new Error('locked')); });
    expect(result.current.error).toEqual(new Error('locked'));
    expect(result.current.fresh).toBe(false);
    // No polling and no retry of its own.
    await act(async () => { await vi.advanceTimersByTimeAsync(10_000); });
    expect(api.calls).toHaveLength(1);
    await act(async () => { result.current.refresh(); });
    await act(async () => { api.calls[1].resolve('preview a'); });
    expect(result.current).toMatchObject({ result: 'preview a', fresh: true, error: null });
    act(() => { result.current.replace('activated a'); });
    // Switching off and on, as an Apply does, does not replace it.
    rerender({ inputs: 'a', enabled: false });
    rerender({ inputs: 'a', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.calls).toHaveLength(2);
    expect(result.current).toMatchObject({ result: 'activated a', fresh: true });
  });

  it('previews nothing while off', async () => {
    const api = controlled();
    const { result, rerender } = mount(api, { inputs: 'a', enabled: false });
    await act(async () => { await vi.advanceTimersByTimeAsync(1000); });
    expect(api.calls).toHaveLength(0);
    expect(result.current.loading).toBe(false);
    rerender({ inputs: 'a', enabled: true });
    await act(async () => { await vi.advanceTimersByTimeAsync(0); });
    expect(api.calls).toHaveLength(1);
  });
});
