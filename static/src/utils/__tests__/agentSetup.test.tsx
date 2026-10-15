import { renderHook, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import { getServerUrl, isTauriApp } from '../tauri';
import { useMcpUrl } from '../agentSetup';

vi.mock('../tauri', () => ({ getServerUrl: vi.fn(), isTauriApp: vi.fn() }));

describe('MCP endpoint URL', () => {
  it('uses the owned desktop server address', async () => {
    vi.mocked(isTauriApp).mockReturnValue(true);
    vi.mocked(getServerUrl).mockResolvedValue('http://127.0.0.1:3030');
    const { result } = renderHook(() => useMcpUrl());
    await waitFor(() => expect(result.current).toBe('http://127.0.0.1:3030/api/mcp'));
  });

  it('handles a failed native lookup without inventing an endpoint', async () => {
    vi.mocked(isTauriApp).mockReturnValue(true);
    vi.mocked(getServerUrl).mockRejectedValue(new Error('The desktop server is unavailable.'));
    const { result } = renderHook(() => useMcpUrl());
    // Let the native command settle; a missing rejection handler also fails
    // the test run as an unhandled rejection.
    await waitFor(() => expect(getServerUrl).toHaveBeenCalledOnce());
    expect(result.current).toBe('');
  });

  it('uses the page origin in a browser without a native lookup', () => {
    vi.mocked(isTauriApp).mockReturnValue(false);
    const { result } = renderHook(() => useMcpUrl());
    expect(result.current).toBe(`${window.location.origin}/api/mcp`);
    expect(getServerUrl).not.toHaveBeenCalled();
  });
});
