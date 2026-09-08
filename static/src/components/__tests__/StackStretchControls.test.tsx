import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackStretchPreview, StackViewProcessingRequest } from '../../api/types';
import StackStretchControls from '../StackStretchControls';

const request: StackViewProcessingRequest = {
  model: { type: 'auto-mtf', target_median: 0.25, shadows_clip: -2.8 },
  color_strategy: 'linked',
  deconvolution: null,
  rc_astro: { steps: [{ tool: 'sxt', parameters: { stars: true } }] },
};
const applied: StackStretchPreview = {
  schema_version: 3, stretch_id: 'selected', stretch_version: '0.1',
  deconvolution_version: null, deconvolution_id: null,
  request, config: { ...request, max_analysis_samples: 100 }, resolved_plan: {},
  source_transfer: 'linear', input_range: null,
  linked_statistics: { min: 0, max: 1, median: 0.2, mad: 0.1, count: 4 },
  channel_statistics: [], luminance_statistics: null, deconvolution: null,
  rc_astro: { cli_version: '2.6.6', has_stars: true, steps: [{
    tool: 'sxt', name: 'StarXTerminator', ml_version: 11, warnings: [],
  }] },
  preview_url: '/selected.png', original_preview_url: '/selected-full.png', fits_url: '/selected.fits',
};

function wrapper() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

describe('StackStretchControls saved processing', () => {
  it('includes the restored RC-Astro chain in a saved view setup', async () => {
    let saved: unknown;
    server.use(http.post('/api/processing-setups', async ({ request }) => {
      saved = await request.json();
      return HttpResponse.json({ success: true, data: saved, error: null });
    }));
    render(<StackStretchControls label="R" channels={1} applied={applied}
      apply={vi.fn().mockResolvedValue(applied)} onApplied={vi.fn()}
      onRevert={vi.fn().mockResolvedValue(undefined)} />, { wrapper: wrapper() });
    fireEvent.click(screen.getByText('View processing'));
    fireEvent.click(screen.getByRole('button', { name: 'Save as…' }));
    fireEvent.change(screen.getByRole('textbox', { name: 'New setup name' }),
      { target: { value: 'Starless view' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save current settings' }));
    await waitFor(() => expect(saved).toEqual({ name: 'Starless view', kind: 'view', settings: request }));
  });
  it('hydrates the full selected request without rerunning tools and keeps tools after stretch edits', async () => {
    const apply = vi.fn().mockResolvedValue(applied);
    const props = { label: 'Test R', channels: 1 as const, apply,
      onApplied: vi.fn(), onRevert: vi.fn().mockResolvedValue(undefined) };
    const { rerender } = render(<StackStretchControls {...props} />, { wrapper: wrapper() });
    rerender(<StackStretchControls {...props} applied={applied} />);
    expect(apply).not.toHaveBeenCalled();
    fireEvent.click(screen.getByText('View processing'));
    expect(screen.getByText('RC-Astro 2.6.6')).toBeInTheDocument();
    fireEvent.change(screen.getByRole('combobox', { name: 'Test R stretch model' }),
      { target: { value: 'identity' } });
    rerender(<StackStretchControls {...props} applied={JSON.parse(JSON.stringify(applied))} />);
    expect(screen.getByRole('combobox', { name: 'Test R stretch model' })).toHaveValue('identity');
    fireEvent.click(screen.getByRole('button', { name: 'Apply processing' }));
    await waitFor(() => expect(apply).toHaveBeenCalledWith(
      { ...request, model: { type: 'identity' } }, expect.any(Function), expect.any(AbortSignal)
    ));
  });

  it('keeps Revert disabled until the applied selection cache has finished updating', async () => {
    let finish!: () => void;
    const onApplied = vi.fn(() => new Promise<void>((resolve) => { finish = resolve; }));
    const onRevert = vi.fn().mockResolvedValue(undefined);
    render(<StackStretchControls label="R" channels={1} applied={applied}
      apply={vi.fn().mockResolvedValue(applied)} onApplied={onApplied} onRevert={onRevert} />,
    { wrapper: wrapper() });
    fireEvent.click(screen.getByText('View processing'));
    fireEvent.click(screen.getByRole('button', { name: 'Apply processing' }));
    await waitFor(() => expect(onApplied).toHaveBeenCalled());
    const revert = screen.getByRole('button', { name: 'Revert processing' });
    expect(revert).toBeDisabled();
    fireEvent.click(revert);
    expect(onRevert).not.toHaveBeenCalled();
    finish();
    await waitFor(() => expect(revert).toBeEnabled());
    fireEvent.click(revert);
    await waitFor(() => expect(onRevert).toHaveBeenCalledOnce());
  });

  it('keeps the selected preview and editable request after a failed durable revert', async () => {
    const apply = vi.fn().mockResolvedValue(applied);
    render(<StackStretchControls label="Test R" channels={1} applied={applied} apply={apply}
      onApplied={vi.fn()} onRevert={vi.fn().mockRejectedValue(new Error('Read-only storage'))} />,
    { wrapper: wrapper() });
    fireEvent.click(screen.getByText('View processing'));
    fireEvent.click(screen.getByRole('button', { name: 'Revert processing' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Read-only storage');
    fireEvent.click(screen.getByRole('button', { name: 'Apply processing' }));
    await waitFor(() => expect(apply).toHaveBeenCalledWith(
      request, expect.any(Function), expect.any(AbortSignal)
    ));
  });

  it('stops client polling and ignores completion after unmount', async () => {
    let finish!: (preview: StackStretchPreview) => void;
    const apply = vi.fn(() => new Promise<StackStretchPreview>((resolve) => { finish = resolve; }));
    const onApplied = vi.fn();
    const { unmount } = render(<StackStretchControls label="R" channels={1} apply={apply}
      onApplied={onApplied} onRevert={vi.fn().mockResolvedValue(undefined)} />,
    { wrapper: wrapper() });
    fireEvent.click(screen.getByText('View processing'));
    fireEvent.click(screen.getByRole('button', { name: 'Apply processing' }));
    const signal = (apply.mock.calls[0] as unknown as [unknown, unknown, AbortSignal])[2];
    unmount();
    expect(signal.aborted).toBe(true);
    finish(applied);
    await Promise.resolve();
    expect(onApplied).not.toHaveBeenCalled();
  });
});
