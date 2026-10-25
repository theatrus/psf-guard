import { openProcessingFlow } from './processingFlowTestUtils';
import { render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { describe, expect, it, vi } from 'vitest';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { StackColorProcessing } from '../../api/types';
import StackColorProcessingControls from '../StackColorProcessingControls';
import { PROCESSING_SETUPS_QUERY_KEY } from '../defaultColorSetup';
import { ALL_COLOR_ROLES, defaultColorProcessing } from '../stackColorProcessing';

/** The setups list answers only once the test lets it. */
function slowDefault(processing: StackColorProcessing) {
  let release = () => {};
  const released = new Promise<void>((resolve) => { release = resolve; });
  server.use(http.get('/api/processing-setups', async () => {
    await released;
    return HttpResponse.json({ success: true, error: null, data: {
      schema_version: 1, setups: [], default_color: 'Faint', default_color_processing: processing,
    } });
  }));
  return release;
}

async function renderNewPreview() {
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(
    <QueryClientProvider client={queryClient}>
      <StackColorProcessingControls label="M 27 RGB" roles={['red', 'green', 'blue']}
        applied={null} backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
        channelProcessing={{}} disabled={false} onApply={vi.fn()} />
    </QueryClientProvider>
  );
  await openProcessingFlow();
  return {
    background: within(screen.getByRole('region', { name: 'Background extraction' }))
      .getByRole('checkbox', { name: 'Background extraction' }),
    calibration: within(screen.getByRole('region', { name: 'Color calibration' }))
      .getByRole('switch'),
    apply: screen.getByRole('button', { name: 'Apply processing stack' }),
    queryClient,
  };
}

describe('a new color preview\'s processing editor', () => {
  it('follows the default Color setup when it loads after the editor opens', async () => {
    const release = slowDefault({
      ...defaultColorProcessing(ALL_COLOR_ROLES),
      background_extraction: null,
    });
    const { background, apply } = await renderNewPreview();
    // The built-in pipeline until the server answers.
    expect(background).toBeChecked();
    release();
    await waitFor(() => expect(background).not.toBeChecked());
    expect(apply).toBeDisabled();
  });

  it('keeps an edit made before the default loads, and resets to the default', async () => {
    const release = slowDefault({
      ...defaultColorProcessing(ALL_COLOR_ROLES),
      color_calibration: { enabled: false, neutralize_background: true, white_reference: 'solar' },
    });
    const { background, calibration, apply, queryClient } = await renderNewPreview();
    await userEvent.click(background);
    release();
    await waitFor(() =>
      expect(queryClient.getQueryData(PROCESSING_SETUPS_QUERY_KEY)).toBeDefined()
    );
    // Rendered with the server's pipeline as the baseline, the edit stays.
    await waitFor(() => expect(screen.getByRole('region', { name: 'Color calibration' })).toBeVisible());
    expect(background).not.toBeChecked();
    expect(calibration).toBeChecked();
    expect(apply).toBeEnabled();

    await userEvent.click(screen.getByRole('button', { name: 'Reset defaults' }));
    expect(calibration).not.toBeChecked();
    expect(background).toBeChecked();
    expect(apply).toBeDisabled();
  });
});

it('applies RGB-star BlurXTerminator independently of narrowband color tools', async () => {
  server.use(http.get('/api/tools/rc-astro', () => HttpResponse.json({ success: true, data: {
    available: true, tools: [{ schema_version: 6, cli_version: '2.6.6', key: 'bxt', name: 'RC-Astro BlurXTerminator', licensed: true,
      parameters: [{ name: 'ss', flag: '--ss', label: 'Sharpen Stars', kind: { type: 'float', default: 0.5, min: 0, max: 0.7 } }] }],
  } })));
  const applied = vi.fn();
  const queryClient = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={queryClient}>
    <StackColorProcessingControls label="SNR G132 SHO" roles={['ha', 'oiii', 'sii']}
      applied={null} automaticNarrowband managesStars backgrounds={{}} protections={{}} fallbacks={{}}
      deconvolutions={{}} disabled={false} onApply={applied} />
  </QueryClientProvider>);
  await openProcessingFlow();
  const stars = await screen.findByRole('region', { name: 'RGB stars RC-Astro tools' });
  await userEvent.click(within(stars).getByRole('checkbox', { name: 'RC-Astro BlurXTerminator' }));
  await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
  expect(applied).toHaveBeenCalledWith(expect.objectContaining({ narrowband: expect.objectContaining({
    rgb_star_processing: { steps: [{ tool: 'bxt', parameters: {} }] },
  }) }));
  expect(applied.mock.calls[0][0].color_rc_astro).toBeUndefined();
});


it('offers Automatic, Manual and Off only on an explicit combined recipe', async () => {
  const applied = vi.fn();
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  render(<QueryClientProvider client={client}><StackColorProcessingControls label="SNR G132 RGB + emission"
    roles={['red','green','blue','ha','oiii']} combination="emission" applied={null}
    backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}} disabled={false} onApply={applied} /></QueryClientProvider>);
  await openProcessingFlow();
  expect(screen.getByRole('combobox', { name: 'Continuum mode' })).toHaveValue('automatic');
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Continuum mode' }), 'manual');
  await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
  expect(screen.getByRole('alert')).toHaveTextContent('Enter every reference weight');
  expect(applied).not.toHaveBeenCalled();
  await userEvent.selectOptions(screen.getByRole('combobox', { name: 'Continuum mode' }), 'off');
  await userEvent.click(screen.getByRole('button', { name: 'Apply processing stack' }));
  expect(applied).toHaveBeenCalledWith(expect.objectContaining({ emission: expect.objectContaining({ mode: 'off' }) }));
});
