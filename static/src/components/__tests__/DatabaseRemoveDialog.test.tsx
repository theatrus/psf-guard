import { describe, expect, it, vi, beforeEach } from 'vitest';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import type { ReactNode } from 'react';
import DatabaseRemoveDialog from '../DatabaseRemoveDialog';
import { apiClient } from '../../api/client';

vi.mock('../../api/client', () => ({
  apiClient: { getDirectorStatus: vi.fn(), getDirectorRigRemoval: vi.fn(), removeDatabase: vi.fn() },
}));
const api = vi.mocked(apiClient);

const rig = { id: 'rig', name: 'Redcat', revision: 1 };
const wrapper = ({ children }: { children: ReactNode }) => (
  <QueryClientProvider client={new QueryClient()}>{children}</QueryClientProvider>
);
const show = (onRemoved = vi.fn()) =>
  render(<DatabaseRemoveDialog database={{ id: 'redcat', name: 'Redcat' }} onClose={vi.fn()} onRemoved={onRemoved} />, { wrapper });

describe('DatabaseRemoveDialog', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    api.getDirectorStatus.mockResolvedValue({ enabled: true } as Awaited<ReturnType<typeof apiClient.getDirectorStatus>>);
  });

  it('lists what goes before removing the database', async () => {
    api.getDirectorRigRemoval.mockResolvedValue({ rig, plans: ['M31', 'M42'], plugins: ['NINA'], blocker: null, collaboration: false });
    api.removeDatabase.mockResolvedValue(true);
    const onRemoved = vi.fn();
    show(onRemoved);

    expect(await screen.findByText(/you cannot undo it/)).toBeInTheDocument();
    expect(screen.getByText(/this rig's part of M31 and M42/)).toBeInTheDocument();
    expect(screen.getByText(/the Director pairing with NINA/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Remove' }));

    await waitFor(() => expect(onRemoved).toHaveBeenCalledWith('Redcat'));
    expect(api.removeDatabase).toHaveBeenCalledWith('redcat');
  });

  it('explains open work and keeps Remove disabled', async () => {
    api.getDirectorRigRemoval.mockResolvedValue({ rig, plans: [], plugins: [], blocker: 'open_work', collaboration: false });
    show();

    expect(await screen.findByRole('alert')).toHaveTextContent('choose Close work on the Director tab');
    expect(screen.getByRole('button', { name: 'Remove' })).toBeDisabled();
  });

  it('waits for Director before enabling Remove', async () => {
    let answer: (value: Awaited<ReturnType<typeof apiClient.getDirectorStatus>>) => void = () => {};
    api.getDirectorStatus.mockReturnValue(new Promise(resolve => { answer = resolve; }));
    api.getDirectorRigRemoval.mockResolvedValue(null);
    show();

    expect(screen.getByRole('button', { name: 'Remove' })).toBeDisabled();
    answer({ enabled: true } as Awaited<ReturnType<typeof apiClient.getDirectorStatus>>);
    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove' })).toBeEnabled());
  });

  it('still removes the database when the rig cannot be read', async () => {
    api.getDirectorRigRemoval.mockRejectedValue(new Error('The store is busy.'));
    show();

    expect(await screen.findByRole('alert')).toHaveTextContent('could not list this rig');
    expect(screen.getByRole('button', { name: 'Remove' })).toBeEnabled();
  });

  it('keeps a collaborating rig and says so', async () => {
    api.getDirectorRigRemoval.mockResolvedValue({ rig, plans: ['M31'], plugins: [], blocker: null, collaboration: true });
    show();

    expect(await screen.findByText(/PSF Guard keeps the rig's settings, pairing and plans/)).toBeInTheDocument();
    expect(screen.queryByText(/you cannot undo it/)).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Remove' })).toBeEnabled();
  });

  it('removes a database with no rig without mentioning one', async () => {
    api.getDirectorRigRemoval.mockResolvedValue(null);
    show();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove' })).toBeEnabled());
    expect(screen.queryByText(/you cannot undo it/)).not.toBeInTheDocument();
  });

  it('works when Director is off', async () => {
    api.getDirectorStatus.mockResolvedValue({ enabled: false } as Awaited<ReturnType<typeof apiClient.getDirectorStatus>>);
    show();

    await waitFor(() => expect(screen.getByRole('button', { name: 'Remove' })).toBeEnabled());
    expect(api.getDirectorRigRemoval).not.toHaveBeenCalled();
  });
});
