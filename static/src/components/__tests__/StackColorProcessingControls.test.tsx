import { openProcessingFlow } from './processingFlowTestUtils';
import { render, screen, within, waitFor } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackColorProcessingControls from '../StackColorProcessingControls';
import { defaultColorProcessing, automaticNarrowbandProcessing } from '../stackColorProcessing';
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
  it('uses one H-alpha stretch for a new automatic recipe and saves balance changes', async () => {
    const onApply = vi.fn();
    renderWithQueries(<StackColorProcessingControls label="SHO" roles={['ha', 'oiii', 'sii']}
      automaticNarrowband palette="sho" applied={null} backgrounds={{}} protections={{}} fallbacks={{}}
      deconvolutions={{}} disabled={false} onApply={onApply} />);
    await openProcessingFlow();
    expect(screen.getByRole('combobox', { name: 'Narrowband channel stretch' })).toHaveValue('shared');
    expect(screen.getByRole('checkbox', { name: 'Narrowband balance' })).toBeChecked();
    expect(screen.getAllByText('Uses the stretch resolved from Hα.')).toHaveLength(2);
    const boost = screen.getByRole('spinbutton', { name: 'OIII boost' });
    await userEvent.clear(boost);
    await userEvent.type(boost, '1.5');
    await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      narrowband: expect.objectContaining({ shared_stretch: true, balance: expect.objectContaining({ oiii_boost: 1.5 }) }),
    }));
  });

  it('disables shared H-alpha stretching and balance for Foraxx', async () => {
    renderWithQueries(<StackColorProcessingControls label="Foraxx SHO" roles={['ha', 'oiii', 'sii']}
      palette="foraxx-sho" applied={automaticNarrowbandProcessing(defaultColorProcessing(['ha', 'oiii', 'sii']))}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.getByRole('combobox', { name: 'Narrowband channel stretch' })).toBeDisabled();
    expect(screen.getByRole('checkbox', { name: 'Narrowband balance' })).toBeDisabled();
    expect(screen.getByRole('spinbutton', { name: 'OIII boost' })).toBeDisabled();
    expect(screen.queryByText('Uses the stretch resolved from Hα.')).not.toBeInTheDocument();
    expect(screen.getByText(/Foraxx uses its existing dynamic mapping/)).toBeInTheDocument();
  });

  it("follows each channel card's deconvolution and RC-Astro steps by default", async () => {
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
    await openProcessingFlow();
    const mode = screen.getByRole('radiogroup', { name: 'Deconvolution and RC-Astro steps' });
    expect(within(mode).getByRole('radio', { name: 'From each channel card' })).toBeChecked();
    expect(await screen.findByText(
      'Each input runs the steps its channel card applied (Hα: BlurXTerminator). ' +
      'Changing a card makes this preview out of date.'
    )).toBeInTheDocument();
    // Each lane says what its card applied, in place of the tool editors.
    const ha = screen.getByRole('region', { name: 'Hα input linear processing' });
    expect(within(ha).getByText('From its channel card: BlurXTerminator.')).toBeInTheDocument();
    expect(within(ha).queryByRole('region', { name: 'Hα input deconvolution' })).not.toBeInTheDocument();
    const oiii = screen.getByRole('region', { name: 'OIII input linear processing' });
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
    await openProcessingFlow();
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
    await openProcessingFlow();
    const ha = screen.getByRole('region', { name: 'Hα input linear processing' });
    expect(await within(ha).findByText(
      'From its channel card: BlurXTerminator. ' +
      'The card now applies: BlurXTerminator, NoiseXTerminator.'
    )).toBeInTheDocument();
    // OIII's card is unchanged: the lane shows what ran, with the measured
    // PSF its deconvolution used, read-only.
    const oiii = screen.getByRole('region', { name: 'OIII input linear processing' });
    expect(within(oiii).getByText('From its channel card: deconvolution.')).toBeInTheDocument();
    const summary = within(oiii).getByRole('region', { name: 'OIII input deconvolution' });
    expect(summary).toHaveTextContent('PSF FWHM 3.25 px');
    expect(within(summary).queryByRole('checkbox')).not.toBeInTheDocument();
    const sii = screen.getByRole('region', { name: 'SII input linear processing' });
    expect(within(sii).getByText(
      'Its channel card applied no deconvolution or RC-Astro steps. ' +
      'The card now applies: BlurXTerminator.'
    )).toBeInTheDocument();
  });

  it('says the inputs run no tools when no channel card applied any', async () => {
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={defaultColorProcessing(['red', 'green', 'blue'])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.getByText(
      'No channel card has deconvolution or RC-Astro steps applied, so the inputs run none.'
    )).toBeInTheDocument();
    expect(screen.getAllByText(/Tools from channel cards/).length).toBeGreaterThan(0);
  });

  it('keeps the own settings of processing saved before following the cards existed', async () => {
    const saved = defaultColorProcessing(['red', 'green', 'blue']);
    delete saved.input_processing;
    saved.input_deconvolutions = { red: defaultDeconvolutionConfig() };
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={['red', 'green', 'blue']}
      applied={saved}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      channelProcessing={{ green: { deconvolution: defaultDeconvolutionConfig() } }}
      disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
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
    await openProcessingFlow();
    await user.click(screen.getByRole('radio', { name: 'Set in this build' }));
    const red = screen.getByRole('region', { name: 'R input linear processing' });
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
    await openProcessingFlow();
    await user.click(screen.getByRole('radio', { name: 'Set in this build' }));
    const red = screen.getByRole('region', { name: 'R input linear processing' });
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

    await openProcessingFlow();
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
    await openProcessingFlow();
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
    await openProcessingFlow();
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
    await openProcessingFlow();
    const together = await screen.findByRole('region', { name: 'RC-Astro on the color channels together' });
    const correctOnly = within(together).getByRole('checkbox', { name: 'Correct only' });
    expect(correctOnly).toBeChecked();
    await user.click(correctOnly);
    expect(screen.getByRole('button', { name: 'Apply processing stack' })).toBeEnabled();
    await user.click(correctOnly);
    expect(screen.getByRole('button', { name: 'Apply processing stack' })).toBeDisabled();
  });
});

describe('green reduction without other narrowband options', () => {
  it('retains a standalone green amount for a Foraxx palette', async () => {
    const onApply = vi.fn();
    renderWithQueries(<StackColorProcessingControls label="Foraxx SHO" roles={['ha', 'oiii', 'sii']} palette="foraxx-sho"
      applied={defaultColorProcessing(['ha', 'oiii', 'sii'])} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={onApply} />);
    await openProcessingFlow();
    await userEvent.clear(screen.getByLabelText('Reduce green excess'));
    await userEvent.type(screen.getByLabelText('Reduce green excess'), '0.5');
    await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ narrowband: expect.objectContaining({ green_reduction: 0.5 }) }));
  });
});

describe('RGB star reduction', () => {
  it('saves reduction independently of BXT, balance and shared stretching', async () => {
    const onApply = vi.fn();
    renderWithQueries(<StackColorProcessingControls label="SHO" roles={['ha', 'oiii', 'sii', 'red', 'green', 'blue']} palette="sho" managesStars
      applied={defaultColorProcessing(['ha', 'oiii', 'sii', 'red', 'green', 'blue'])} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={onApply} />);
    await openProcessingFlow();
    await userEvent.clear(screen.getByRole('spinbutton', { name: 'RGB star reduction' }));
    await userEvent.type(screen.getByRole('spinbutton', { name: 'RGB star reduction' }), '0.4');
    await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ narrowband: expect.objectContaining({ star_reduction: 0.4 }) }));
  });
});


describe('processing flow', () => {
  it('opens with an ordered compact overview and retains edits in closed steps', async () => {
    const onApply = vi.fn();
    renderWithQueries(<StackColorProcessingControls label="SHO" roles={['ha', 'oiii', 'sii']} automaticNarrowband managesStars stars="automatic" resolvedStars="rgb"
      applied={null} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={onApply} />);
    await userEvent.click(screen.getByText('Processing stack'));
    const flow = screen.getByRole('list', { name: 'Color processing flow' });
    const steps = Array.from(flow.querySelectorAll(':scope > li > details'));
    expect(steps.map((step) => step.getAttribute('aria-label'))).toEqual([
      'Prepare inputs', 'Process linear color', 'Stretch and narrowband balance', 'Combine and restore stars', 'Finish RGB',
    ]);
    expect(steps.every((step) => !(step as HTMLDetailsElement).open)).toBe(true);
    expect(within(flow).getByText('Automatic → RGB stars · RGB reduction 0')).toBeVisible();
    await userEvent.click(screen.getByText('Stretch and narrowband balance'));
    const boost = screen.getByRole('spinbutton', { name: 'OIII boost' });
    await userEvent.clear(boost); await userEvent.type(boost, '1.5');
    await userEvent.click(screen.getByText('Stretch and narrowband balance'));
    expect(boost).not.toBeVisible();
    await userEvent.click(screen.getByText('Combine and restore stars'));
    const reduction = screen.getByRole('spinbutton', { name: 'RGB star reduction' });
    await userEvent.clear(reduction); await userEvent.type(reduction, '0.4');
    await userEvent.click(screen.getByText('Combine and restore stars'));
    await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({ narrowband: expect.objectContaining({
      star_reduction: 0.4, balance: expect.objectContaining({ oiii_boost: 1.5 }),
    }) }));
  });

  it('puts natural emission blending after calibration and omits the narrowband-only setup', async () => {
    renderWithQueries(<StackColorProcessingControls label="RGB + emission" roles={['red', 'green', 'blue', 'ha', 'oiii']} combination="emission"
      applied={null} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.queryByRole('option', { name: 'Automatic narrowband' })).not.toBeInTheDocument();
    const subtraction = screen.getByRole('group', { name: 'Subtract continuum' });
    const linear = screen.getByRole('group', { name: 'Process linear color' });
    expect(within(subtraction).getByRole('combobox', { name: 'Continuum mode' })).toBeVisible();
    expect(within(subtraction).queryByRole('spinbutton', { name: 'Hα emission amount' })).toBeNull();
    expect(within(linear).getByRole('spinbutton', { name: 'Hα emission amount' })).toBeVisible();
    expect(within(linear).getByRole('region', { name: 'Color calibration' })).toBeVisible();
  });

  it('keeps RGB reduction inactive for native narrowband stars without discarding the saved value', async () => {
    const processing = automaticNarrowbandProcessing(defaultColorProcessing(['ha', 'oiii']));
    processing.narrowband!.star_reduction = 0.4;
    renderWithQueries(<StackColorProcessingControls label="HOO" roles={['ha', 'oiii']} managesStars stars="narrowband"
      applied={processing} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.getByRole('spinbutton', { name: 'RGB star reduction' })).toBeDisabled();
    expect(screen.getByRole('spinbutton', { name: 'RGB star reduction' })).toHaveValue(0.4);
    expect(screen.getByText('Narrowband stars · RGB reduction inactive')).toBeVisible();
  });
});


it('separates continuum references from mapped stretches and places structure blending with restored stars', async () => {
  renderWithQueries(<StackColorProcessingControls label="SHO + broadband" roles={['ha', 'oiii', 'sii', 'red', 'green', 'blue']} combination="broadband_structure" managesStars stars="rgb"
    applied={null} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={vi.fn()} />);
  await openProcessingFlow();
  const stretch = screen.getByRole('group', { name: 'Stretch and narrowband balance' });
  expect(within(stretch).getAllByRole('region')).toHaveLength(3);
  expect(within(stretch).queryByRole('region', { name: 'R input stretch stack' })).toBeNull();
  const combine = screen.getByRole('group', { name: 'Combine and restore stars' });
  expect(within(combine).getByRole('spinbutton', { name: 'Broadband structure amount' })).toBeVisible();
  expect(within(screen.getByRole('group', { name: 'Subtract continuum' })).queryByRole('spinbutton', { name: 'Broadband structure amount' })).toBeNull();
});
