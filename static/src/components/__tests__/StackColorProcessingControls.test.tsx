import { render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackColorProcessingControls from '../StackColorProcessingControls';
import { defaultColorProcessing } from '../stackColorProcessing';

// The setups bar inside the editor reads the global setups list.
function renderWithQueries(element: ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(
    <QueryClientProvider client={queryClient}>{element}</QueryClientProvider>
  );
}

describe('StackColorProcessingControls', () => {
  it('edits and saves RC-Astro steps for the selected input channel', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    let saved: unknown;
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [{ key: 'nxt', name: 'NoiseXTerminator', licensed: true,
          cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] }],
      }, error: null })),
      http.post('/api/processing-setups', async ({ request }) => {
        saved = await request.json();
        return HttpResponse.json({ success: true, data: saved, error: null });
      })
    );
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={defaultColorProcessing(['red', 'green', 'blue'])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    const red = screen.getByRole('region', { name: 'R input stretch stack' });
    await user.click(await within(red).findByRole('checkbox', { name: 'NoiseXTerminator' }));
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      input_rc_astro: { red: { steps: [{ tool: 'nxt', parameters: {} }] } },
    }));
    await user.click(screen.getByRole('button', { name: 'Save as…' }));
    await user.type(screen.getByRole('textbox', { name: 'New setup name' }), 'RC color');
    await user.click(screen.getByRole('button', { name: 'Save current settings' }));
    await waitFor(() => expect(saved).toEqual(expect.objectContaining({ name: 'RC color', kind: 'color',
      settings: expect.objectContaining({ input_rc_astro: { red: {
        steps: [{ tool: 'nxt', parameters: {} }],
      } } }),
    })));
  });
  it('lets the user disable catalog background protection', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    renderWithQueries(
      <StackColorProcessingControls
        label="Test RGB"
        roles={['red', 'green', 'blue']}
        applied={defaultColorProcessing(['red', 'green', 'blue'])}
        backgrounds={{}}
        protections={{}}
        fallbacks={{}}
        deconvolutions={{}}
        disabled={false}
        onApply={onApply}
      />
    );

    await user.click(screen.getByText('Processing stack'));
    const protection = screen.getByRole('checkbox', {
      name: 'Protect catalog emission from background fitting',
    });
    expect(protection).toBeChecked();

    await user.click(protection);
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));

    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      background_extraction: expect.objectContaining({
        protect_catalog_emission: false,
      }),
    }));
  });
});
