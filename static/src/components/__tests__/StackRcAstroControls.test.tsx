import { describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, within } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import StackRcAstroControls from '../StackRcAstroControls';
import { OPEN_SETTINGS_EVENT } from '../../utils/settingsIntent';
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


const nxt = {
  schema_version: 6,
  cli_version: '2.6.6',
  key: 'nxt',
  name: 'RC-Astro NoiseXTerminator',
  ml_version: 3,
  licensed: true,
  license_message: 'Permanently licensed',
  parameters: [
    {
      name: 'dn', flag: '--dn', label: 'Denoise', description: 'Amount of denoising',
      kind: { type: 'float', default: 0.9, min: 0, max: 1 },
    },
  ],
};

const noiseVersion = 'noise-v1-d33 cpu f32, seiza-model 0.1.0, band scale 5';

const noiseSchema = {
  schema_version: 1,
  cli_version: `${noiseVersion}, card abc`,
  key: 'noise-model',
  name: 'Seiza noise model',
  ml_version: null,
  licensed: true,
  license_message: null,
  parameters: [
    { name: 'grain', flag: '--grain', label: 'Grain', description: 'Grain removed', kind: { type: 'float', default: 0.7, min: 0, max: 1 } },
    { name: 'mottle', flag: '--mottle', label: 'Mottle', description: 'Mottle removed', kind: { type: 'float', default: 0.9, min: 0, max: 1 } },
  ],
};

const [bxt, sxt] = capabilities.tools;

function provider(category: string, schema: { key: string; name: string; cli_version: string }, extra: object = {}) {
  return {
    category, provider: schema.key, name: schema.name, state: 'ready',
    version: schema.cli_version, schema, this_server: true, workers: [], ...extra,
  };
}

const noiseModelMissing = {
  category: 'denoise', provider: 'noise-model', name: 'Seiza noise model', state: 'not_installed',
  reason: 'The Seiza noise model is not downloaded.', remedy: 'download_noise_model',
  this_server: false, workers: [],
};

/** A server with the provider list: all three RC-Astro tools and the noise model. */
const listed = {
  available: true,
  tools: [bxt, nxt, sxt, noiseSchema],
  this_server: true, workers: [], tool_workers: {},
  providers: [
    provider('sharpen', bxt),
    provider('denoise', nxt),
    provider('denoise', noiseSchema, { version: noiseVersion }),
    provider('star_removal', sxt),
  ],
};

const options = (select: HTMLElement) =>
  within(select).getAllByRole('option').map((option) => option.textContent);

describe('StackRcAstroControls', () => {
  it('renders nothing when the server lists no provider', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({ available: false, tools: [] })));
    const { container } = render(
      <StackRcAstroControls label="Sh2 157 R" config={null} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(container).toBeEmptyDOMElement();
  });

  it('shows one picker for each category, in run order', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(listed)));
    render(
      <StackRcAstroControls label="Sh2 157 R" config={null} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    const card = await screen.findByRole('region', { name: 'Sh2 157 R processing tools' });
    expect(within(card).getByText('Processing tools')).toBeInTheDocument();
    expect(within(card).getAllByRole('combobox').map((select) => select.getAttribute('aria-label')))
      .toEqual(['Sharpening', 'Noise reduction', 'Star removal']);
    expect(options(screen.getByRole('combobox', { name: 'Sharpening' })))
      .toEqual(['Off', 'BlurXTerminator']);
    // Noise reduction has two providers.
    expect(options(screen.getByRole('combobox', { name: 'Noise reduction' })))
      .toEqual(['Off', 'NoiseXTerminator', 'Seiza noise model']);
    expect(screen.getByRole('combobox', { name: 'Star removal' })).toHaveValue('');
  });

  it('replaces one noise reduction provider with the other, never both', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(listed)));
    const onChange = vi.fn();
    const sharpen = { tool: 'bxt', parameters: {} };
    const starless = { tool: 'sxt', parameters: { stars: true } };
    const props = { label: 'R', disabled: false, onChange };
    const { rerender } = render(
      <StackRcAstroControls {...props} config={{ steps: [sharpen, starless] }} />,
      { wrapper: wrapper() }
    );
    const denoise = await screen.findByRole('combobox', { name: 'Noise reduction' });
    fireEvent.change(denoise, { target: { value: 'nxt' } });
    // The step lands between sharpening and star removal.
    expect(onChange).toHaveBeenLastCalledWith({
      steps: [sharpen, { tool: 'nxt', parameters: {} }, starless],
    });

    rerender(<StackRcAstroControls {...props}
      config={{ steps: [sharpen, { tool: 'nxt', parameters: { dn: 0.5 } }, starless] }} />);
    expect(screen.getByRole('spinbutton', { name: 'Denoise' })).toHaveValue(0.5);
    fireEvent.change(denoise, { target: { value: 'noise-model' } });
    expect(onChange).toHaveBeenLastCalledWith({
      steps: [sharpen, { tool: 'noise-model', parameters: {} }, starless],
    });

    rerender(<StackRcAstroControls {...props}
      config={{ steps: [sharpen, { tool: 'noise-model', parameters: { grain: 0.5 } }, starless] }} />);
    // The picked provider's controls replace the other's.
    expect(screen.queryByRole('spinbutton', { name: 'Denoise' })).toBeNull();
    expect(screen.getByRole('spinbutton', { name: 'Grain' })).toHaveValue(0.5);
    const mottle = screen.getByRole('spinbutton', { name: 'Mottle' });
    expect(mottle).toHaveValue(0.9);
    fireEvent.change(mottle, { target: { value: '0.6' } });
    expect(onChange).toHaveBeenLastCalledWith({
      steps: [sharpen, { tool: 'noise-model', parameters: { grain: 0.5, mottle: 0.6 } }, starless],
    });
  });

  it('removes a step when its category is set to Off', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(listed)));
    const onChange = vi.fn();
    const sharpen = { tool: 'bxt', parameters: {} };
    const { rerender } = render(
      <StackRcAstroControls label="R" disabled={false} onChange={onChange}
        config={{ steps: [sharpen, { tool: 'noise-model', parameters: {} }] }} />,
      { wrapper: wrapper() }
    );
    fireEvent.change(await screen.findByRole('combobox', { name: 'Noise reduction' }),
      { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith({ steps: [sharpen] });
    rerender(<StackRcAstroControls label="R" disabled={false} onChange={onChange}
      config={{ steps: [sharpen] }} />);
    fireEvent.change(screen.getByRole('combobox', { name: 'Sharpening' }), { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith(null);
  });

  it('lists a provider that is not installed, with its reason and the way to Settings', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({
      ...listed,
      tools: [bxt, nxt, sxt],
      providers: [listed.providers[0], listed.providers[1], noiseModelMissing, listed.providers[3]],
    })));
    const opened = vi.fn();
    window.addEventListener(OPEN_SETTINGS_EVENT, opened);
    render(
      <StackRcAstroControls label="M31 L" config={null} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    const denoise = await screen.findByRole('combobox', { name: 'Noise reduction' });
    expect(options(denoise)).toEqual(['Off', 'NoiseXTerminator', 'Seiza noise model (not installed)']);
    // It cannot be picked until it is installed.
    expect(within(denoise).getByRole('option', { name: 'Seiza noise model (not installed)' })).toBeDisabled();
    expect(screen.getByText(/Seiza noise model: The Seiza noise model is not downloaded\./)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Download in Settings' }));
    window.removeEventListener(OPEN_SETTINGS_EVENT, opened);
    expect((opened.mock.calls[0][0] as CustomEvent).detail)
      .toEqual({ intent: 'stacking', field: 'processing_tools' });
  });

  it('keeps a saved step of a provider that is not installed, with its reason', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({
      ...listed, tools: [bxt, nxt, sxt],
      providers: [listed.providers[1], noiseModelMissing],
    })));
    const onChange = vi.fn();
    render(
      <StackRcAstroControls label="M31 L" disabled={false} onChange={onChange}
        config={{ steps: [{ tool: 'noise-model', parameters: { grain: 0.4 } }] }} />,
      { wrapper: wrapper() }
    );
    const denoise = await screen.findByRole('combobox', { name: 'Noise reduction' });
    expect(denoise).toHaveValue('noise-model');
    expect(await screen.findByText(/^The Seiza noise model is not downloaded\./)).toBeInTheDocument();
    expect(screen.getByText('grain: 0.4')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Download in Settings' })).toBeInTheDocument();
    fireEvent.change(denoise, { target: { value: '' } });
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it('shows the reason alone for an RC-Astro tool that is not licensed', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({
      ...listed,
      providers: [provider('sharpen', bxt, {
        state: 'not_licensed', remedy: 'install_rc_astro',
        reason: 'RC-Astro BlurXTerminator is not licensed on this server (Trial expired)',
      })],
    })));
    render(
      <StackRcAstroControls label="R" config={null} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    const sharpen = await screen.findByRole('combobox', { name: 'Sharpening' });
    expect(options(sharpen)).toEqual(['Off', 'BlurXTerminator (not licensed)']);
    expect(screen.getByText(/is not licensed on this server \(Trial expired\)/)).toBeInTheDocument();
    expect(screen.queryByRole('button')).toBeNull();
  });

  it('builds the pickers from the tools of a server that sends no provider list', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const onChange = vi.fn();
    render(
      <StackRcAstroControls label="Sh2 157 R" config={null} disabled={false} onChange={onChange} />,
      { wrapper: wrapper() }
    );

    const starRemoval = await screen.findByRole('combobox', { name: 'Star removal' });
    expect(options(starRemoval)).toEqual(['Off', 'StarXTerminator']);
    expect(options(screen.getByRole('combobox', { name: 'Sharpening' }))).toEqual(['Off', 'BlurXTerminator']);
    // No tool for noise reduction, so no picker for it.
    expect(screen.queryByRole('combobox', { name: 'Noise reduction' })).toBeNull();
    fireEvent.change(starRemoval, { target: { value: 'sxt' } });
    // Picking star removal keeps the stars image so both halves can be
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

  it('does not reorder the processing chain when editing a parameter', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const onChange = vi.fn();
    const config: RcAstroProcessing = { steps: [
      { tool: 'sxt', parameters: { stars: true } },
      { tool: 'bxt', parameters: {} },
    ] };
    render(<StackRcAstroControls label="R" config={config} disabled={false} onChange={onChange} />,
      { wrapper: wrapper() });
    fireEvent.change(await screen.findByRole('spinbutton', { name: 'Sharpen Stars' }),
      { target: { value: '0.3' } });
    expect(onChange).toHaveBeenCalledWith({ steps: [
      config.steps[0], { tool: 'bxt', parameters: { ss: 0.3 } },
    ] });
  });

  it('stores the steps in run order regardless of the order they are picked', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    const onChange = vi.fn();
    const props = { label: 'R', disabled: false, onChange };
    const { rerender } = render(<StackRcAstroControls {...props} config={null} />,
      { wrapper: wrapper() });
    fireEvent.change(await screen.findByRole('combobox', { name: 'Star removal' }),
      { target: { value: 'sxt' } });
    const starless = { tool: 'sxt', parameters: { stars: true } };
    expect(onChange).toHaveBeenLastCalledWith({ steps: [starless] });
    rerender(<StackRcAstroControls {...props} config={{ steps: [starless] }} />);
    fireEvent.change(screen.getByRole('combobox', { name: 'Sharpening' }), { target: { value: 'bxt' } });
    expect(onChange).toHaveBeenLastCalledWith({ steps: [
      { tool: 'bxt', parameters: {} }, starless,
    ] });
    expect(screen.getByRole('region', { name: 'R processing tools' }))
      .toHaveTextContent('1 step(s) · linear, before stretch');
  });

  it('keeps a saved step visible and removable when the server lists no provider for it', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({ available: false, tools: [] })));
    const onChange = vi.fn();
    const custom = { tool: 'gradient-x', parameters: { amount: 2 } };
    render(<StackRcAstroControls label="R" disabled={false} onChange={onChange}
      config={{ steps: [{ tool: 'sxt', parameters: { stars: true } }, custom] }} />,
      { wrapper: wrapper() });
    // A step of a known category keeps its picker.
    const starRemoval = await screen.findByRole('combobox', { name: 'Star removal' });
    expect(starRemoval).toHaveValue('sxt');
    expect(await screen.findByText(/Your saved step is unchanged\./)).toHaveTextContent('stars: true');
    expect(options(starRemoval)).toEqual(['Off', 'sxt (unavailable)']);
    fireEvent.change(starRemoval, { target: { value: '' } });
    expect(onChange).toHaveBeenLastCalledWith({ steps: [custom] });
    // A step PSF Guard cannot place has a Remove button.
    expect(screen.getByText(/gradient-x \(unavailable\) amount: 2/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Remove gradient-x' }));
    expect(onChange).toHaveBeenLastCalledWith({ steps: [{ tool: 'sxt', parameters: { stars: true } }] });
  });

  it('offers to remove a second step saved in one category', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(listed)));
    const onChange = vi.fn();
    const both = [{ tool: 'nxt', parameters: {} }, { tool: 'noise-model', parameters: {} }];
    render(<StackRcAstroControls label="R" disabled={false} onChange={onChange}
      config={{ steps: both }} />, { wrapper: wrapper() });
    expect(await screen.findByText(/This chain also runs Seiza noise model\./)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Remove Seiza noise model' }));
    expect(onChange).toHaveBeenCalledWith({ steps: [both[0]] });
  });

  it('shows what ran an RC-Astro step', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({ available: false, tools: [] })));
    render(<StackRcAstroControls label="R" config={{ steps: [{ tool: 'sxt', parameters: { stars: true } }] }}
      result={{ cli_version: '2.6.6', has_stars: true, steps: [{ tool: 'sxt',
        name: 'StarXTerminator', ml_version: 11, device: 'CUDA', warnings: ['Ran on the CPU'] }] }}
      disabled={false} onChange={() => {}} />, { wrapper: wrapper() });
    expect(await screen.findByText('Made with RC-Astro 2.6.6')).toBeInTheDocument();
    expect(screen.getByText(/StarXTerminator · model 11 · CUDA/)).toBeInTheDocument();
    expect(screen.getByText('Ran on the CPU')).toBeInTheDocument();
  });

  it('shows what ran a noise model step, with no RC-Astro version', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({
      ...listed,
      providers: listed.providers.map((entry) =>
        entry.provider === 'noise-model' ? { ...entry, workers: ['studio'] } : entry),
    })));
    render(<StackRcAstroControls label="M31 L" disabled={false} onChange={() => {}}
      config={{ steps: [{ tool: 'noise-model', parameters: {} }] }}
      result={{ cli_version: '', has_stars: false, steps: [
        { tool: 'noise-model', name: 'Seiza noise model', ml_version: null, version: noiseVersion, device: 'cpu', warnings: [] },
      ] }} />, { wrapper: wrapper() });

    const card = await screen.findByRole('region', { name: 'M31 L processing tools' });
    await screen.findByRole('spinbutton', { name: 'Grain' });
    expect(card).toHaveTextContent(`Seiza noise model · ${noiseVersion} · cpu`);
    expect(card).not.toHaveTextContent(/RC-Astro\s*$/);
    expect(within(card).queryByText(/^RC-Astro/)).toBeNull();
    // Where each provider runs.
    expect(card).toHaveTextContent(
      'BlurXTerminator on this server · NoiseXTerminator on this server · Seiza noise model on studio · ' +
      'StarXTerminator on this server. This server runs a tool while no worker that takes it is connected.'
    );
  });

  it('allows removing a configured tool whose license is unavailable', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok({ ...capabilities,
      tools: capabilities.tools.map((tool) => ({ ...tool, licensed: false })),
    })));
    const onChange = vi.fn();
    render(<StackRcAstroControls label="R" config={{ steps: [{ tool: 'bxt', parameters: {} }] }}
      disabled={false} onChange={onChange} />, { wrapper: wrapper() });
    const sharpen = await screen.findByRole('combobox', { name: 'Sharpening' });
    expect(sharpen).toBeEnabled();
    expect(sharpen).toHaveValue('bxt');
    // Its fields are locked, and an unlicensed tool cannot be picked.
    expect(await screen.findByRole('spinbutton', { name: 'Sharpen Stars' })).toBeDisabled();
    expect(within(screen.getByRole('combobox', { name: 'Star removal' }))
      .getByRole('option', { name: 'StarXTerminator (not licensed)' })).toBeDisabled();
    fireEvent.change(sharpen, { target: { value: '' } });
    expect(onChange).toHaveBeenCalledWith(null);
  });

  it('fixes the picker on the configured provider where the step is required', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(listed)));
    render(<StackRcAstroControls label="M31 separation" heading="Star separation" required
      categories={['star_removal']} excludedParameters={['stars']}
      config={{ steps: [{ tool: 'sxt', parameters: {} }] }} disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() });
    const card = await screen.findByRole('region', { name: 'M31 separation processing tools' });
    expect(within(card).getByText('Star separation')).toBeInTheDocument();
    const picker = await within(card).findByRole('combobox', { name: 'Star removal' });
    expect(picker).toBeDisabled();
    expect(picker).toHaveValue('sxt');
    // Only the offered provider's category shows, and its fields stay editable.
    expect(within(card).getAllByRole('combobox')).toHaveLength(1);
    expect(within(card).getByRole('checkbox', { name: 'Unscreen Stars' })).toBeEnabled();
    expect(within(card).queryByRole('checkbox', { name: 'Generate Star Image' })).toBeNull();
  });

  it('sets the pinned values for Correct only and locks those fields', async () => {
    const float = (name: string, label: string, max: number, fallback: number) => ({
      name, flag: `--${name}`, label, description: label,
      kind: { type: 'float' as const, default: fallback, min: name === 'ash' ? -0.5 : 0, max },
    });
    const flag = (name: string, label: string, fallback: boolean) => ({
      name, flag: `--${name}`, label, description: label,
      kind: { type: 'bool' as const, default: fallback },
    });
    server.use(http.get('/api/tools/rc-astro', () => ok({
      ...capabilities,
      tools: [{
        ...capabilities.tools[0],
        parameters: [
          float('ss', 'Sharpen Stars', 0.7, 0.5),
          float('ash', 'Adjust Star Halos', 0.5, 0),
          float('nsd', 'Nonstellar PSF Diameter', 8, 0),
          flag('ansp', 'Auto Nonstellar PSF', true),
          float('sn', 'Sharpen Nonstellar', 1, 0.5),
          flag('lp', 'Lunar/Planetary', false),
          float('overlap', 'Tile Overlap', 0.5, 0.2),
        ],
      }, capabilities.tools[1]],
    })));
    const onChange = vi.fn();
    const config: RcAstroProcessing = { steps: [{ tool: 'bxt', parameters: { overlap: 0.3 } }] };
    const { rerender } = render(
      <StackRcAstroControls label="Color channels" config={config} disabled={false}
        categories={['sharpen', 'denoise']} onChange={onChange} />,
      { wrapper: wrapper() }
    );

    // Star removal is not offered where the caller leaves it out.
    expect(await screen.findByRole('combobox', { name: 'Sharpening' })).toHaveValue('bxt');
    expect(screen.queryByRole('combobox', { name: 'Star removal' })).toBeNull();

    const correctOnly = await screen.findByRole('checkbox', { name: 'Correct only' });
    expect(correctOnly).not.toBeChecked();
    fireEvent.click(correctOnly);
    const pinned = { ss: 0, sn: 0, ash: 0, nsd: 0, ansp: true, lp: false };
    expect(onChange).toHaveBeenLastCalledWith({
      steps: [{ tool: 'bxt', parameters: { overlap: 0.3, ...pinned } }],
    } satisfies RcAstroProcessing);

    rerender(
      <StackRcAstroControls label="Color channels"
        config={{ steps: [{ tool: 'bxt', parameters: { overlap: 0.3, ...pinned } }] }}
        disabled={false} categories={['sharpen', 'denoise']} onChange={onChange} />
    );
    expect(screen.getByRole('checkbox', { name: 'Correct only' })).toBeChecked();
    expect(await screen.findByRole('spinbutton', { name: 'Sharpen Stars' })).toBeDisabled();
    expect(screen.getByRole('spinbutton', { name: 'Tile Overlap' })).toBeEnabled();

    // Turning it off returns the pinned fields to the tool's defaults.
    fireEvent.click(screen.getByRole('checkbox', { name: 'Correct only' }));
    expect(onChange).toHaveBeenLastCalledWith({
      steps: [{ tool: 'bxt', parameters: { overlap: 0.3 } }],
    } satisfies RcAstroProcessing);
  });

  it('offers no Correct only when the schema lacks a pinned parameter', async () => {
    server.use(http.get('/api/tools/rc-astro', () => ok(capabilities)));
    render(
      <StackRcAstroControls label="Sh2 157 R" config={{ steps: [{ tool: 'bxt', parameters: {} }] }}
        disabled={false} onChange={() => {}} />,
      { wrapper: wrapper() }
    );
    await screen.findByRole('spinbutton', { name: 'Sharpen Stars' });
    expect(screen.queryByRole('checkbox', { name: 'Correct only' })).toBeNull();
  });
});
