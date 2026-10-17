import { render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackColorProcessingControls from '../StackColorProcessingControls';
import { defaultColorProcessing } from '../stackColorProcessing';
import { defaultDeconvolutionConfig } from '../stackDeconvolution';

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
  it("copies each channel card's deconvolution and RC-Astro steps into its input", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [{ key: 'bxt', name: 'BlurXTerminator', licensed: true,
          cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] }],
      }, error: null }))
    );
    const roles = ['ha', 'oiii'] as const;
    const deconvolution = defaultDeconvolutionConfig();
    const applied = defaultColorProcessing([...roles]);
    // OIII already deconvolves in the color input; its card shows nothing,
    // so the copy leaves it alone.
    applied.input_deconvolutions = { oiii: deconvolution };
    renderWithQueries(<StackColorProcessingControls label="HOO" roles={[...roles]}
      applied={applied}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      channelProcessing={{ ha: { rc_astro: { steps: [{ tool: 'bxt', parameters: {} }] } } }}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    expect(await screen.findByText(
      'Copies the deconvolution and RC-Astro steps each channel shows: Hα: BlurXTerminator.'
    )).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: "Use each channel's processing" }));
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      input_rc_astro: { ha: { steps: [{ tool: 'bxt', parameters: {} }] } },
      input_deconvolutions: { oiii: deconvolution },
    }));
  });

  it('offers no copy when no channel card shows pixel processing', async () => {
    const user = userEvent.setup();
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={defaultColorProcessing(['red', 'green', 'blue'])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await user.click(screen.getByText('Processing stack'));
    expect(screen.getByRole('button', { name: "Use each channel's processing" })).toBeDisabled();
    expect(screen.getByText('No channel card shows deconvolution or RC-Astro steps.')).toBeInTheDocument();
  });

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
  it('refuses a sharpening past the tool range beside Apply, and shows a server refusal there too', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [{ key: 'bxt', name: 'RC-Astro BlurXTerminator', licensed: true,
          cli_version: '2.6.6', ml_version: 5, schema_version: 6, license_message: null, parameters: [
            { name: 'ss', flag: '--ss', label: 'Sharpen Stars', description: '',
              kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } },
            { name: 'sn', flag: '--sn', label: 'Sharpen Nonstellar', description: '',
              kind: { type: 'float', default: 0.5, min: 0, max: 1 } },
          ] }],
      }, error: null })),
    );
    const props = {
      label: 'RGB', roles: ['red', 'green', 'blue'] as const,
      applied: defaultColorProcessing(['red', 'green', 'blue']),
      backgrounds: {}, protections: {}, fallbacks: {}, deconvolutions: {},
      disabled: false, onApply,
    };
    const { unmount } = renderWithQueries(
      <StackColorProcessingControls {...props} roles={[...props.roles]} />
    );
    await user.click(screen.getByText('Processing stack'));
    const red = screen.getByRole('region', { name: 'R input stretch stack' });
    await user.click(await within(red).findByRole('checkbox', { name: 'RC-Astro BlurXTerminator' }));
    const stars = within(red).getByRole('spinbutton', { name: 'Sharpen Stars' });
    await user.clear(stars);
    await user.type(stars, '0.9');
    expect(stars).toHaveAttribute('aria-invalid', 'true');
    expect(within(red).getByText('0 to 0.7')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).not.toHaveBeenCalled();
    expect(screen.getByRole('alert')).toHaveTextContent(
      'R: RC-Astro BlurXTerminator: Sharpen Stars must be between 0 and 0.7'
    );

    // A refusal the panel could not foresee shows beside the button as well.
    unmount();
    renderWithQueries(
      <StackColorProcessingControls {...props} roles={[...props.roles]}
        applyError="RC-Astro BlurXTerminator is not licensed on this server" />
    );
    expect(await screen.findByText('RC-Astro BlurXTerminator is not licensed on this server'))
      .toHaveAttribute('role', 'alert');
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
