import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackRcAstroControls from '../StackRcAstroControls';
import type { RcAstroProcessing } from '../../api/types';

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null });
}

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const capabilities = {
  available: true,
  executable: '/usr/local/bin/rc-astro',
  tools: [
    {
      schema_version: 6,
      cli_version: '2.6.6',
      key: 'bxt',
      name: 'RC-Astro BlurXTerminator',
      ml_version: 4,
      licensed: true,
      license_message: 'Permanently licensed',
      parameters: [
        {
          name: 'ss',
          flag: '--ss',
          label: 'Sharpen Stars',
          description: 'Amount of stellar sharpening',
          kind: { type: 'float', default: 0.5, min: 0, max: 0.7 },
        },
      ],
    },
    {
      schema_version: 6,
      cli_version: '2.6.6',
      key: 'sxt',
      name: 'RC-Astro StarXTerminator',
      ml_version: 11,
      licensed: true,
      license_message: 'Permanently licensed',
      parameters: [
        {
          name: 'stars',
          flag: '--stars',
          label: 'Generate Star Image',
          description: 'Also write a stars-only image',
          kind: { type: 'bool', default: false },
        },
        {
          name: 'unscreen',
          flag: '--unscreen',
          label: 'Unscreen Stars',
          description: 'Unscreen stars',
          kind: { type: 'bool', default: false },
        },
        {
          name: 'csep',
          flag: null,
          label: 'Color Separation',
          description: 'GUI only',
          kind: { type: 'bool', default: false },
        },
      ],
    },
  ],
};

describe('StackRcAstroControls', () => {
  it('renders nothing when rc-astro is not installed', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({ available: false, tools: [] })));
    const { container } = render(
      <StackRcAstroControls label="Sh2 157 R" config={null} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    await waitFor(() =>
      expect(container.querySelector('.stack-rc-astro-controls')).toBeNull()
    );
  });

  it('builds controls from the schema and keeps the stars image by default', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const onChange = vi.fn();
    render(
      <StackRcAstroControls label="Sh2 157 R" config={null} disabled={false} onChange={onChange} />,
      { wrapper: wrapper() }
    );

    const sxt = await screen.findByRole('checkbox', { name: 'RC-Astro StarXTerminator' });
    fireEvent.click(sxt);
    // Enabling star removal keeps the stars image so both halves can be
    // stretched independently.
    expect(onChange).toHaveBeenCalledWith({
      steps: [{ tool: 'sxt', parameters: { stars: true } }],
    } satisfies RcAstroProcessing);
  });

  it('shows flagged parameters only, seeded from schema defaults', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const config: RcAstroProcessing = {
      steps: [{ tool: 'sxt', parameters: { stars: true } }],
    };
    render(
      <StackRcAstroControls label="Sh2 157 R" config={config} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );

    expect(await screen.findByRole('checkbox', { name: 'Generate Star Image' })).toBeChecked();
    expect(screen.getByRole('checkbox', { name: 'Unscreen Stars' })).not.toBeChecked();
    // A parameter with no CLI flag cannot be set and is not offered.
    expect(screen.queryByText('Color Separation')).toBeNull();
  });

  it('threads a float edit through onChange', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const onChange = vi.fn();
    const config: RcAstroProcessing = { steps: [{ tool: 'bxt', parameters: {} }] };
    render(
      <StackRcAstroControls label="Sh2 157 R" config={config} disabled={false} onChange={onChange} />,
      { wrapper: wrapper() }
    );

    const field = await screen.findByRole('spinbutton', { name: 'Sharpen Stars' });
    expect(field).toHaveValue(0.5);
    fireEvent.change(field, { target: { value: '0.3' } });
    expect(onChange).toHaveBeenCalledWith({
      steps: [{ tool: 'bxt', parameters: { ss: 0.3 } }],
    } satisfies RcAstroProcessing);
  });
});
