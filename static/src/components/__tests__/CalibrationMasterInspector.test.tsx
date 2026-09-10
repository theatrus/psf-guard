import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { apiClient } from '../../api/client';
import type { CalibrationMasterSource, StackCalibrationMaster } from '../../api/types';
import { __resetForTest } from '../../hooks/previewPoll';
import CalibrationMasterButton, { CalibrationMasterInspector } from '../CalibrationMasterInspector';

const source: CalibrationMasterSource = { kind: 'mono', jobId: 'job-old', groupIndex: 2, artifactRevision: 'revision-old' };
const flat: StackCalibrationMaster = {
  id: 'a'.repeat(64), kind: 'flat', label: 'Flat R - first night', available: true,
  unavailable_reason: null,
  usages: [{ channel: 'R', session: 1, lights: 10, estimated_pedestal_adu: null },
    { channel: 'R', session: 2, lights: 8, estimated_pedestal_adu: 201.5 }],
  preview_url: '/flat.png?revision=old', original_preview_url: '/flat-original.png?revision=old',
  fits_url: '/flat.fits?revision=old', source_count: 12, rejection_method: 'delta-sigma',
  masked_samples: 20, rejected_samples: 4, minimum_clean_samples: 2, maximum_clean_samples: 12,
};
const bias: StackCalibrationMaster = {
  ...flat, id: 'b'.repeat(64), kind: 'bias', label: 'Bias master',
  original_preview_url: '/bias-original.png?revision=old', fits_url: '/bias.fits?revision=old',
};

function wrapper({ children }: { children: React.ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false }, mutations: { retry: false } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}

function mountInspector() {
  return render(<CalibrationMasterInspector dbId="fixture" source={source} title="M42" label="R" onClose={vi.fn()} />, { wrapper });
}

function loadImage() {
  const image = screen.getByTestId('stack-inspector-image') as HTMLImageElement;
  Object.defineProperties(image, {
    naturalWidth: { configurable: true, value: 512 },
    naturalHeight: { configurable: true, value: 256 },
  });
  Object.defineProperty(image.parentElement, 'getBoundingClientRect', {
    configurable: true,
    value: () => ({ x: 0, y: 0, left: 0, top: 0, right: 256, bottom: 256, width: 256, height: 256 }),
  });
  fireEvent.load(image);
  return image;
}

beforeEach(() => {
  vi.spyOn(apiClient, 'getStackCalibrationMasters').mockResolvedValue({ masters: [bias, flat], notes: [] });
});

afterEach(() => {
  __resetForTest();
  vi.restoreAllMocks();
});

describe('CalibrationMasterInspector', () => {
  it('shows the exact master once with every session, diagnostics and raw FITS', async () => {
    mountInspector();
    expect(await screen.findByRole('combobox', { name: 'Master' })).toHaveValue(flat.id);
    expect(apiClient.getStackCalibrationMasters).toHaveBeenCalledWith('fixture', source);
    expect(screen.getAllByRole('option')).toHaveLength(2);
    expect(screen.getByText('R / Session 1: 10 light frames')).toBeInTheDocument();
    expect(screen.getByText('R / Session 2: 8 light frames / Estimated pedestal 201.5 ADU')).toBeInTheDocument();
    expect(screen.getByText('20 masked samples')).toBeInTheDocument();
    expect(screen.getByText('2-12 retained samples / pixel')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Download master FITS' })).toHaveAttribute('href', flat.fits_url);
    expect(screen.queryByRole('button', { name: 'Find source artifact' })).not.toBeInTheDocument();
  });

  it('retains unavailable masters without substituting or downloading a different file', async () => {
    vi.mocked(apiClient.getStackCalibrationMasters).mockResolvedValue({
      masters: [flat, { ...bias, available: false, unavailable_reason: 'The recorded file was removed.', original_preview_url: null, fits_url: null }],
      notes: [],
    });
    mountInspector();
    fireEvent.change(await screen.findByRole('combobox'), { target: { value: bias.id } });
    expect(screen.getByText('The recorded file was removed.')).toHaveAttribute('role', 'status');
    expect(screen.queryByTestId('stack-inspector-image')).not.toBeInTheDocument();
    expect(screen.queryByRole('link', { name: 'Download master FITS' })).not.toBeInTheDocument();
    expect(screen.getByRole('slider', { name: 'Master midtone' })).toBeDisabled();
  });

  it('surfaces catalog failures and can retry into a legacy/no-file explanation', async () => {
    vi.mocked(apiClient.getStackCalibrationMasters).mockRejectedValueOnce(new Error('Stack artifact changed.'))
      .mockResolvedValueOnce({ masters: [], notes: ['Estimated pedestal only; no bias master was applied.'] });
    mountInspector();
    expect(await screen.findByRole('alert')).toHaveTextContent('Stack artifact changed.');
    fireEvent.click(screen.getByRole('button', { name: 'Retry loading calibration masters' }));
    expect(await screen.findByText('Estimated pedestal only; no bias master was applied.')).toBeInTheDocument();
    expect(screen.getByRole('status')).toHaveTextContent('No calibration master files were recorded');
  });

  it('uses shared generation polling and reports terminal generation errors', async () => {
    const status = vi.spyOn(apiClient, 'getGenerationStatus').mockResolvedValue([{ state: 'ready' }]);
    mountInspector();
    const image = await screen.findByTestId('stack-inspector-image');
    fireEvent.error(image);
    await waitFor(() => expect(image.getAttribute('src')).toContain('&v=1'));
    expect(status).toHaveBeenCalledWith('fixture', [{
      kind: 'calibration_master', source, masterId: flat.id, size: 'original', midtone: 0.2, shadow: -2.8,
    }]);
    status.mockResolvedValue([{ state: 'error', error: 'The recorded master no longer exists.' }]);
    fireEvent.error(image);
    expect(await screen.findByRole('alert')).toHaveTextContent('The recorded master no longer exists.');
  });

  it('preserves the inspected pixels across display stretch changes and resets for another master', async () => {
    mountInspector();
    await screen.findByTestId('stack-inspector-image');
    const image = loadImage();
    const fittedTransform = image.style.transform;
    fireEvent.click(screen.getByRole('button', { name: '100%' }));
    const transform = image.style.transform;
    expect(transform).toContain('scale(1)');
    fireEvent.change(screen.getByRole('slider', { name: 'Master midtone' }), { target: { value: '0.35' } });
    await waitFor(() => expect(image.getAttribute('src')).toContain('midtone=0.35'));
    loadImage();
    expect(image.style.transform).toBe(transform);
    expect(screen.getByRole('link', { name: 'Download master FITS' })).toHaveAttribute('href', flat.fits_url);
    fireEvent.change(screen.getByRole('combobox'), { target: { value: bias.id } });
    loadImage();
    expect(image.style.transform).toBe(fittedTransform);
  });

  it('pins the action to the displayed artifact even if a rebuild replaces the card', async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    const element = (revision: string) => <QueryClientProvider client={client}>
      <CalibrationMasterButton dbId="fixture" source={{ ...source, artifactRevision: revision }} title="M42" label="R" />
    </QueryClientProvider>;
    const view = render(element('old'));
    fireEvent.click(screen.getByRole('button', { name: 'Inspect calibration masters for R' }));
    await screen.findByRole('combobox');
    view.rerender(element('new'));
    await act(async () => {});
    expect(apiClient.getStackCalibrationMasters).toHaveBeenCalledTimes(1);
    expect(apiClient.getStackCalibrationMasters).toHaveBeenLastCalledWith('fixture', { ...source, artifactRevision: 'old' });
  });
});
