import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import LiveChip from '../header/LiveChip';
import { liveSummary } from '../header/liveSummary';
import type { DirectorRigStatusView } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const enabled = { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', acquisition_available: false, database_management: true };
const view = (name: string, state: DirectorRigStatusView['connectivity']['state'], phase: string | null, stale = false): DirectorRigStatusView => ({
  rig: { id: `id-${name}`, name, revision: 1 }, catalog_slug: name.toLowerCase(), catalog_name: name, checkins: [],
  status: phase ? { rig_id: `id-${name}`, session_id: 's', reported_at_ms: 1_700_000_000_000, received_at_ms: 1_700_000_000_001, payload: { phase } } : null,
  status_age_ms: 5000, status_stale: stale, contacts: { program_pull: null, check_in: null, status: null },
  connectivity: { state, last_contact_ms: 1_700_000_000_001, age_ms: 5000 }, assignments: [], pending_receipts: 0,
});

function mount(statuses: DirectorRigStatusView[], status: unknown = enabled) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const statusCall = vi.fn(() => HttpResponse.json(ok(status)));
  const rigsCall = vi.fn(() => HttpResponse.json(ok(statuses)));
  server.use(http.get('/api/director/v1/status', statusCall), http.get('/api/director/v1/rigs/status', rigsCall));
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter>{children}</MemoryRouter></QueryClientProvider>;
  return { ...render(<LiveChip />, { wrapper: Wrapper }), statusCall, rigsCall };
}

describe('Live chip', () => {
  it('counts rigs and exposures, and alarms on a rig that went quiet', () => {
    const now = 1_700_000_100_000;
    expect(liveSummary([view('C925', 'online', 'exposing'), view('RC51', 'online', 'slewing')], now)).toMatchObject({ label: '2 rigs · 1 exposing', alert: false });
    expect(liveSummary([view('C925', 'online', 'idle'), view('RC51', 'stale', 'exposing', true)], now)).toMatchObject({ label: '2 rigs · 1 online', alert: true });
    expect(liveSummary([view('C925', 'never', null)], now)).toMatchObject({ label: '1 rig · 0 online', alert: false });
    expect(liveSummary([view('C925', 'online', 'exposing')], now).title).toContain('C925: exposing');
  });

  it('shows the fleet from any view and opens the dashboard in a drawer', async () => {
    mount([view('C925', 'online', 'exposing'), view('RC51', 'offline', null)]);
    const chip = await screen.findByRole('button', { name: 'Live rigs: 2 rigs · 1 exposing' });
    expect(chip).toHaveClass('is-alert');
    fireEvent.click(chip);
    const drawer = screen.getByRole('dialog', { name: 'Live rigs' });
    expect(drawer).toHaveAttribute('aria-modal', 'true');
    expect(drawer).toHaveTextContent('C925');
    expect(drawer).toHaveTextContent('Offline');
    // Focus moves in, Escape closes, focus returns to the chip.
    const closeButton = screen.getByRole('button', { name: 'Close live rigs' });
    expect(closeButton).toHaveFocus();
    fireEvent.keyDown(drawer, { key: 'Escape' });
    expect(screen.queryByRole('dialog')).not.toBeInTheDocument();
    expect(chip).toHaveFocus();
  });

  it('stays out of the header without rigs, and never asks for rigs without Director', async () => {
    const empty = mount([]);
    await waitFor(() => expect(empty.rigsCall).toHaveBeenCalled());
    await new Promise(resolve => setTimeout(resolve, 0));
    expect(empty.container).toBeEmptyDOMElement();
    empty.unmount();
    const off = mount([view('C925', 'online', 'exposing')], { ...enabled, enabled: false });
    await waitFor(() => expect(off.statusCall).toHaveBeenCalled());
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(off.rigsCall).not.toHaveBeenCalled();
    expect(off.container).toBeEmptyDOMElement();
  });
});
