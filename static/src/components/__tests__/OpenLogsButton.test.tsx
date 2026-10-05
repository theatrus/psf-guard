import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { describe, expect, it, vi } from 'vitest';
import OpenLogsButton from '../OpenLogsButton';

const tauri = vi.hoisted(() => ({
  getLogFolder: vi.fn(),
  openLogFolder: vi.fn(),
}));
vi.mock('../../utils/tauri', () => ({ tauriFileSystem: tauri }));

function mount(onError = vi.fn()) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
  render(<QueryClientProvider client={client}><OpenLogsButton onError={onError} /></QueryClientProvider>);
  return onError;
}

describe('Open logs button', () => {
  it('shows where the log is and opens the folder', async () => {
    tauri.getLogFolder.mockResolvedValue('C:\\Users\\me\\AppData\\Local\\com.theatrus.psf-guard\\logs');
    tauri.openLogFolder.mockResolvedValue(undefined);
    mount();
    const button = screen.getByRole('button', { name: 'Open logs' });
    await waitFor(() => expect(button).toHaveAttribute('title', 'C:\\Users\\me\\AppData\\Local\\com.theatrus.psf-guard\\logs'));
    fireEvent.click(button);
    expect(tauri.openLogFolder).toHaveBeenCalledTimes(1);
  });

  it('says why the folder could not be opened', async () => {
    tauri.getLogFolder.mockResolvedValue(null);
    tauri.openLogFolder.mockRejectedValue(new Error('no file manager'));
    const onError = mount();
    fireEvent.click(screen.getByRole('button', { name: 'Open logs' }));
    await waitFor(() => expect(onError).toHaveBeenCalledWith('Failed to open the log folder: no file manager'));
  });
});
