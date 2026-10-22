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
  it("follows each channel card's deconvolution and RC-Astro steps by default", async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [{ key: 'bxt', name: 'BlurXTerminator', licensed: true,
          cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] }],
      }, error: null }))
    );
    const roles = ['ha', 'oiii'] as const;
    renderWithQueries(<StackColorProcessingControls label="HOO" roles={[...roles]}
      applied={null}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      channelProcessing={{ ha: { rc_astro: { steps: [{ tool: 'bxt', parameters: {} }] } } }}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    const mode = screen.getByRole('radiogroup', { name: 'Deconvolution and RC-Astro steps' });
    expect(within(mode).getByRole('radio', { name: 'From each channel card' })).toBeChecked();
    expect(await screen.findByText(
      'Each input runs the steps its channel card applied (Hα: BlurXTerminator). ' +
      'Changing a card makes this preview out of date.'
    )).toBeInTheDocument();
    // Each lane says what its card applied, in place of the tool editors.
    const ha = screen.getByRole('region', { name: 'Hα input stretch stack' });
    expect(within(ha).getByText('From its channel card: BlurXTerminator.')).toBeInTheDocument();
    expect(within(ha).queryByRole('region', { name: 'Hα input deconvolution' })).not.toBeInTheDocument();
    const oiii = screen.getByRole('region', { name: 'OIII input stretch stack' });
    expect(within(oiii).getByText('Its channel card applied no deconvolution or RC-Astro steps.'))
      .toBeInTheDocument();
  });

  it('starts its own settings from what the channel cards applied', async () => {
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
    // The last build ran OIII's own deconvolution; OIII's card has none now.
    applied.input_deconvolutions = { oiii: deconvolution };
    renderWithQueries(<StackColorProcessingControls label="HOO" roles={[...roles]}
      applied={applied}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      channelProcessing={{ ha: { rc_astro: { steps: [{ tool: 'bxt', parameters: {} }] } } }}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    await user.click(screen.getByRole('radio', { name: 'Set in this build' }));
    expect(screen.getByRole('radio', { name: 'Set in this build' })).toBeChecked();
    expect(screen.getByText('Each input runs the deconvolution and RC-Astro steps set in its lane below.'))
      .toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      input_rc_astro: { ha: { steps: [{ tool: 'bxt', parameters: {} }] } },
      input_deconvolutions: {},
    }));
    // Own settings leave the field out, as the server reads it.
    expect(onApply.mock.calls[0][0]).not.toHaveProperty('input_processing');
  });

  it('shows what a build that followed the cards ran, and what a card applies now', async () => {
    const user = userEvent.setup();
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [
          { key: 'bxt', name: 'BlurXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] },
          { key: 'nxt', name: 'NoiseXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] },
        ],
      }, error: null }))
    );
    const roles = ['ha', 'oiii', 'sii'] as const;
    const deconvolution = { ...defaultDeconvolutionConfig(), psf_fwhm_pixels: 3.25 };
    const bxt = { steps: [{ tool: 'bxt', parameters: {} }] };
    renderWithQueries(<StackColorProcessingControls label="SHO" roles={[...roles]}
      applied={defaultColorProcessing([...roles])}
      followed={{ ha: { rc_astro: bxt }, oiii: { deconvolution }, sii: {} }}
      backgrounds={{}} protections={{}} fallbacks={{}}
      deconvolutions={{ oiii: { config: deconvolution, channels: [] } }}
      channelProcessing={{
        // Hα's card added NoiseXTerminator after the build.
        ha: { rc_astro: { steps: [...bxt.steps, { tool: 'nxt', parameters: {} }] } },
        oiii: { deconvolution },
        // SII's card took BlurXTerminator with other settings.
        sii: { rc_astro: bxt },
      }}
      disabled={false} onApply={vi.fn()} />);
    await user.click(screen.getByText('Processing stack'));
    const ha = screen.getByRole('region', { name: 'Hα input stretch stack' });
    expect(await within(ha).findByText(
      'From its channel card: BlurXTerminator. ' +
      'The card now applies: BlurXTerminator, NoiseXTerminator.'
    )).toBeInTheDocument();
    // OIII's card is unchanged: the lane shows what ran, with the measured
    // PSF its deconvolution used, read-only.
    const oiii = screen.getByRole('region', { name: 'OIII input stretch stack' });
    expect(within(oiii).getByText('From its channel card: deconvolution.')).toBeInTheDocument();
    const summary = within(oiii).getByRole('region', { name: 'OIII input deconvolution' });
    expect(summary).toHaveTextContent('PSF FWHM 3.25 px');
    expect(within(summary).queryByRole('checkbox')).not.toBeInTheDocument();
    const sii = screen.getByRole('region', { name: 'SII input stretch stack' });
    expect(within(sii).getByText(
      'Its channel card applied no deconvolution or RC-Astro steps. ' +
      'The card now applies: BlurXTerminator.'
    )).toBeInTheDocument();
  });

  it('says the inputs run no tools when no channel card applied any', async () => {
    const user = userEvent.setup();
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={defaultColorProcessing(['red', 'green', 'blue'])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await user.click(screen.getByText('Processing stack'));
    expect(screen.getByText(
      'No channel card has deconvolution or RC-Astro steps applied, so the inputs run none.'
    )).toBeInTheDocument();
    expect(screen.getByText(/Tools from channel cards/)).toBeInTheDocument();
  });

  it('keeps the own settings of processing saved before following the cards existed', async () => {
    const user = userEvent.setup();
    const saved = defaultColorProcessing(['red', 'green', 'blue']);
    delete saved.input_processing;
    saved.input_deconvolutions = { red: defaultDeconvolutionConfig() };
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={saved}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      channelProcessing={{ green: { deconvolution: defaultDeconvolutionConfig() } }}
      disabled={false} onApply={vi.fn()} />);
    await user.click(screen.getByText('Processing stack'));
    expect(screen.getByRole('radio', { name: 'Set in this build' })).toBeChecked();
    const red = screen.getByRole('region', { name: 'R input deconvolution' });
    expect(within(red).getByRole('checkbox', { name: 'Deconvolution' })).toBeChecked();
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
    await user.click(screen.getByRole('radio', { name: 'Set in this build' }));
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
    await user.click(screen.getByRole('radio', { name: 'Set in this build' }));
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

  it('runs BlurXTerminator in Correct only mode on the color channels together', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    const parameter = (name: string, type: 'float' | 'bool') => ({
      name, flag: `--${name}`, label: name, description: name,
      kind: type === 'bool'
        ? { type, default: false }
        : { type, default: 0, min: -1, max: 8 },
    });
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [
          { key: 'bxt', name: 'BlurXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6,
            parameters: [
              ...['ss', 'sn', 'ash', 'nsd'].map((name) => parameter(name, 'float')),
              ...['ansp', 'lp'].map((name) => parameter(name, 'bool')),
            ] },
          { key: 'nxt', name: 'NoiseXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] },
          { key: 'sxt', name: 'StarXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6, parameters: [] },
        ],
      }, error: null }))
    );
    const roles = ['red', 'green', 'blue'] as const;
    const applied = defaultColorProcessing([...roles]);
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={[...roles]}
      applied={applied}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    const together = await screen.findByRole('region', { name: 'RC-Astro on the color channels together' });
    expect(within(together).queryByRole('checkbox', { name: 'StarXTerminator' })).toBeNull();

    // Turning a tool on and off again leaves nothing to apply.
    await user.click(within(together).getByRole('checkbox', { name: 'NoiseXTerminator' }));
    await user.click(within(together).getByRole('checkbox', { name: 'NoiseXTerminator' }));
    expect(screen.getByRole('button', { name: 'Apply processing stack' })).toBeDisabled();

    await user.click(within(together).getByRole('checkbox', { name: 'BlurXTerminator' }));
    expect(within(together).getByRole('checkbox', { name: 'Correct only' })).toBeChecked();
    await user.click(within(together).getByRole('checkbox', { name: 'NoiseXTerminator' }));
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      color_rc_astro: { steps: [
        { tool: 'bxt', parameters: { ss: 0, sn: 0, ash: 0, nsd: 0, ansp: true, lp: false } },
        { tool: 'nxt', parameters: {} },
      ] },
    }));
  });

  it('starts BlurXTerminator with its defaults where Correct only cannot be sent', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [
          { key: 'bxt', name: 'BlurXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6,
            parameters: [{ name: 'ss', flag: '--ss', label: 'ss', description: 'ss',
              kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } }] },
        ],
      }, error: null }))
    );
    const roles = ['red', 'green', 'blue'] as const;
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={[...roles]}
      applied={defaultColorProcessing([...roles])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={onApply} />);
    await user.click(screen.getByText('Processing stack'));
    const together = await screen.findByRole('region', { name: 'RC-Astro on the color channels together' });
    await user.click(within(together).getByRole('checkbox', { name: 'BlurXTerminator' }));
    expect(within(together).queryByRole('checkbox', { name: 'Correct only' })).toBeNull();
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      color_rc_astro: { steps: [{ tool: 'bxt', parameters: {} }] },
    }));
  });

  it('counts an applied color-channel chain edited and changed back as unchanged', async () => {
    const user = userEvent.setup();
    const parameter = (name: string, type: 'float' | 'bool') => ({
      name, flag: `--${name}`, label: name, description: name,
      kind: type === 'bool' ? { type, default: false } : { type, default: 0, min: -1, max: 8 },
    });
    server.use(
      http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
        available: true, tools: [
          { key: 'bxt', name: 'BlurXTerminator', licensed: true,
            cli_version: '2.6.6', ml_version: 3, schema_version: 6,
            parameters: [
              ...['ss', 'sn', 'ash', 'nsd'].map((name) => parameter(name, 'float')),
              ...['ansp', 'lp'].map((name) => parameter(name, 'bool')),
            ] },
        ],
      }, error: null }))
    );
    const roles = ['red', 'green', 'blue'] as const;
    // The server writes parameters in name order, and the field after the
    // inputs' own tools.
    const applied = {
      ...defaultColorProcessing([...roles]),
      color_rc_astro: { steps: [{ tool: 'bxt', parameters:
        { ansp: true, ash: 0, lp: false, nsd: 0, sn: 0, ss: 0 } }] },
    };
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={[...roles]}
      applied={applied}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await user.click(screen.getByText('Processing stack'));
    const together = await screen.findByRole('region', { name: 'RC-Astro on the color channels together' });
    const correctOnly = within(together).getByRole('checkbox', { name: 'Correct only' });
    expect(correctOnly).toBeChecked();
    await user.click(correctOnly);
    expect(screen.getByRole('button', { name: 'Apply processing stack' })).toBeEnabled();
    await user.click(correctOnly);
    expect(screen.getByRole('button', { name: 'Apply processing stack' })).toBeDisabled();
  });
});
