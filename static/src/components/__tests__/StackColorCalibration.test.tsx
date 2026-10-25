import { openProcessingFlow } from './processingFlowTestUtils';
import { render, screen } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactElement } from 'react';
import { afterEach, describe, expect, it, vi } from 'vitest';
import StackColorProcessingControls from '../StackColorProcessingControls';
import StackStretchControls from '../StackStretchControls';
import { calibrationOutdated } from '../stackCalibrationStale';
import StackColorCalibrationLine from '../StackColorCalibrationLine';
import { defaultColorProcessing } from '../stackColorProcessing';
import { colorSetupForRoles } from '../processingSetups';
import { OPEN_SETTINGS_EVENT, type OpenSettingsDetail } from '../../utils/settingsIntent';
import type { StackColorCalibrationOutcome } from '../../api/types';

function renderWithQueries(element: ReactElement) {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return render(<QueryClientProvider client={queryClient}>{element}</QueryClientProvider>);
}

const rgb = ['red', 'green', 'blue'] as const;

describe('color calibration defaults', () => {
  it('calibrates RGB and LRGB by default and never a narrowband palette', () => {
    expect(defaultColorProcessing([...rgb]).color_calibration).toEqual({
      enabled: true, neutralize_background: true, white_reference: 'solar',
    });
    expect(defaultColorProcessing(['luminance', ...rgb]).color_calibration?.enabled).toBe(true);
    expect(defaultColorProcessing(['ha', 'oiii', 'sii'])).not.toHaveProperty('color_calibration');
  });

  it('keeps a setup’s choice on RGB cards and drops it on narrowband ones', () => {
    const off = {
      ...defaultColorProcessing([...rgb]),
      color_calibration: { enabled: false, neutralize_background: true, white_reference: 'solar' as const },
    };
    expect(colorSetupForRoles(off, [...rgb]).color_calibration?.enabled).toBe(false);
    expect(colorSetupForRoles(off, ['ha', 'oiii'])).not.toHaveProperty('color_calibration');
    const narrowband = defaultColorProcessing(['ha', 'oiii']);
    expect(colorSetupForRoles(narrowband, [...rgb]).color_calibration?.enabled).toBe(true);
  });
});

describe('Color calibration switch', () => {
  it('is on for RGB and turning it off applies with calibration off', async () => {
    const user = userEvent.setup();
    const onApply = vi.fn();
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={[...rgb]}
      applied={defaultColorProcessing([...rgb])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={onApply} />);
    await openProcessingFlow();
    const toggle = screen.getByRole('switch', { name: 'Color calibration' });
    expect(toggle).toBeChecked();
    expect(screen.getByText('Gaia star colors')).toBeInTheDocument();

    await user.click(toggle);
    expect(toggle).not.toBeChecked();
    await user.click(screen.getByRole('button', { name: 'Apply processing stack' }));
    expect(onApply).toHaveBeenCalledWith(expect.objectContaining({
      color_calibration: { enabled: false, neutralize_background: true, white_reference: 'solar' },
    }));
  });

  it('reads an older RGB preview without settings as calibrated', async () => {
    const applied = defaultColorProcessing([...rgb]);
    delete applied.color_calibration;
    renderWithQueries(<StackColorProcessingControls label="RGB" roles={[...rgb]}
      applied={applied}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.getByRole('switch', { name: 'Color calibration' })).toBeChecked();
  });

  it('is absent for narrowband palettes', async () => {
    renderWithQueries(<StackColorProcessingControls label="SHO" roles={['ha', 'oiii', 'sii']}
      applied={defaultColorProcessing(['ha', 'oiii', 'sii'])}
      backgrounds={{}} protections={{}} fallbacks={{}} deconvolutions={{}}
      disabled={false} onApply={vi.fn()} />);
    await openProcessingFlow();
    expect(screen.queryByRole('switch', { name: 'Color calibration' })).not.toBeInTheDocument();
  });
});

describe('StackColorCalibrationLine', () => {
  const fit = { intercept: 0, slope: 0, scatter: 0.02, stars: 143 };
  const calibrated: StackColorCalibrationOutcome = {
    status: 'calibrated',
    gains: [1.1234, 1, 0.871],
    offsets: [0, 0, 0],
    background: [0.1, 0.1, 0.1],
    white_bp_rp: 0.82,
    red_fit: fit,
    blue_fit: { ...fit, stars: 150 },
    aperture_radius: 6,
    stars_offered: 400,
    stars_measured: 180,
    stars_used: 143,
  };

  afterEach(() => vi.restoreAllMocks());

  it('gives the stars and the red and blue gains', () => {
    render(<StackColorCalibrationLine outcome={calibrated} />);
    expect(screen.getByText('Calibrated on 143 stars · R ×1.12, B ×0.87')).toBeInTheDocument();
  });

  it('says when channels stretched one by one undid the balance', () => {
    render(<StackColorCalibrationLine outcome={{ ...calibrated, stretched_separately: true }} />);
    expect(screen.getByText(
      'Calibrated on 143 stars · R ×1.12, B ×0.87 · channels stretched separately'
    )).toHaveAttribute('data-calibration', 'separate');
  });

  it('says why it skipped in a few words', () => {
    const { rerender } = render(<StackColorCalibrationLine outcome={{
      status: 'skipped', reason: 'no_plate_solve', detail: 'The reference frame has no plate solve',
    }} />);
    expect(screen.getByText('Not color calibrated: reference frame not plate solved')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Install' })).not.toBeInTheDocument();

    rerender(<StackColorCalibrationLine outcome={{
      status: 'skipped', reason: 'too_few_stars', detail: 'only 12 stars survived the red colour fit',
    }} />);
    expect(screen.getByText('Not color calibrated: too few Gaia stars')).toBeInTheDocument();

    rerender(<StackColorCalibrationLine outcome={{
      status: 'skipped', reason: 'failed', detail: 'The reference stack records no sky layout; rebuild it',
    }} />);
    expect(screen.getByText(
      'Not color calibrated: The reference stack records no sky layout; rebuild it'
    )).toBeInTheDocument();
  });

  it('offers to install Gaia colors when they are missing', async () => {
    const user = userEvent.setup();
    const opened: OpenSettingsDetail[] = [];
    const listener = (event: Event) => opened.push((event as CustomEvent<OpenSettingsDetail>).detail);
    window.addEventListener(OPEN_SETTINGS_EVENT, listener);
    render(<StackColorCalibrationLine outcome={{
      status: 'skipped', reason: 'no_gaia_catalog', detail: 'Gaia colors are not installed',
    }} />);
    expect(screen.getByText('Not color calibrated: Gaia colors not installed')).toBeInTheDocument();
    await user.click(screen.getByRole('button', { name: 'Install' }));
    window.removeEventListener(OPEN_SETTINGS_EVENT, listener);
    expect(opened).toEqual([{ intent: 'catalogs', field: 'gaia_photometry' }]);
  });

  it('gives a viewer the reason without an Install button', () => {
    render(<StackColorCalibrationLine canInstall={false} outcome={{
      status: 'skipped', reason: 'no_gaia_catalog', detail: 'Gaia colors are not installed',
    }} />);
    expect(screen.getByText('Not color calibrated: Gaia colors not installed')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Install' })).not.toBeInTheDocument();
  });
});

describe('Color calibration of a one-shot-color stack', () => {
  const props = {
    apply: vi.fn(),
    onApplied: vi.fn(),
    onRevert: vi.fn().mockResolvedValue(undefined),
  };

  it('is on by default in a color stack\'s view processing, and turning it off applies with it off', async () => {
    const user = userEvent.setup();
    const apply = vi.fn().mockRejectedValue(new Error('not now'));
    renderWithQueries(<StackStretchControls label="Caldwell 23 L" channels={3} {...props} apply={apply} />);
    await user.click(screen.getByText('View processing'));
    const toggle = screen.getByRole('switch', { name: 'Color calibration' });
    expect(toggle).toBeChecked();
    await user.click(toggle);
    await user.click(screen.getByRole('button', { name: 'Apply processing' }));
    expect(apply).toHaveBeenCalledWith(
      expect.objectContaining({
        color_calibration: { enabled: false, neutralize_background: true, white_reference: 'solar' },
      }),
      expect.any(Function),
      expect.anything()
    );
  });

  it('is absent for a mono stack', async () => {
    const user = userEvent.setup();
    renderWithQueries(<StackStretchControls label="Ha" channels={1} {...props} />);
    await user.click(screen.getByText('View processing'));
    expect(screen.queryByRole('switch', { name: 'Color calibration' })).not.toBeInTheDocument();
  });
});

describe('an out-of-date color stack calibration', () => {
  const stale = 'Out of date — Gaia colors were installed; click Build current set to render the preview again';
  const reapply = 'Out of date — Gaia colors were installed; apply the processing again';

  it('comes from applied processing when there is some, else from the build preview', () => {
    const group = { output_channels: 3, color_calibration_outdated: stale };
    expect(calibrationOutdated(group, undefined)).toBe(stale);
    expect(calibrationOutdated(group, { color_calibration_outdated: reapply })).toBe(reapply);
    // Fresh processing hides a stale build preview: the card shows the
    // processing.
    expect(calibrationOutdated(group, { color_calibration_outdated: null })).toBeNull();
  });

  it('never applies to a mono stack', () => {
    expect(calibrationOutdated({ output_channels: 1, color_calibration_outdated: stale }, undefined))
      .toBeNull();
  });
});
