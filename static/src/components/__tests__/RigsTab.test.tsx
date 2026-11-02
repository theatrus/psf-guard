import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { MemoryRouter, useLocation } from 'react-router-dom';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import RigsTab from '../header/RigsTab';
import { liveSummary } from '../header/liveSummary';
import { withoutPlanningParams } from '../../hooks/useUrlState';
import type { DirectorRigStatusView } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const enabled = { protocol_version: 1, enabled: true, instance_id: '11111111-1111-4111-8111-111111111111', database_management: true };
const view = (name: string, state: DirectorRigStatusView['connectivity']['state'], phase: string | null, stale = false): DirectorRigStatusView => ({
  rig: { id: `id-${name}`, name, revision: 1 }, catalog_slug: name.toLowerCase(), catalog_name: name, checkins: [],
  status: phase ? { rig_id: `id-${name}`, session_id: 's', reported_at_ms: 1_700_000_000_000, received_at_ms: 1_700_000_000_001, payload: { phase } } : null,
  status_age_ms: 5000, status_stale: stale, contacts: { program_pull: null, check_in: null, status: null },
  connectivity: { state, last_contact_ms: 1_700_000_000_001, age_ms: 5000 }, assignments: [], pending_receipts: 0,
});

function Location() {
  const location = useLocation();
  return <output data-testid="location">{location.pathname}{location.search}</output>;
}

function mount(statuses: DirectorRigStatusView[], status: unknown = enabled, route = '/', current = false) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const statusCall = vi.fn(() => HttpResponse.json(ok(status)));
  const rigsCall = vi.fn(() => HttpResponse.json(ok(statuses)));
  server.use(http.get('/api/director/v1/status', statusCall), http.get('/api/director/v1/rigs/status', rigsCall));
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}><MemoryRouter initialEntries={[route]}>{children}<Location /></MemoryRouter></QueryClientProvider>;
  const opened = vi.fn();
  return { ...render(<RigsTab current={current} onClick={opened} />, { wrapper: Wrapper }), statusCall, rigsCall, opened };
}

describe('Live chip', () => {
  it('counts rigs and exposures, and alarms on a rig that went quiet', () => {
    const now = 1_700_000_100_000;
    expect(liveSummary([view('C925', 'online', 'exposing'), view('RC51', 'online', 'slewing')], now)).toMatchObject({ label: '2 rigs · 1 exposing', alert: false });
    expect(liveSummary([view('C925', 'online', 'idle'), view('RC51', 'stale', 'exposing', true)], now)).toMatchObject({ label: '2 rigs · 1 online', alert: true });
    expect(liveSummary([view('C925', 'never', null)], now)).toMatchObject({ label: '1 rig · 0 online', alert: false });
    expect(liveSummary([view('C925', 'online', 'exposing')], now).title).toContain('C925: exposing');
    const leased = view('C925', 'online', 'exposing');
    leased.status!.payload.fresh_for_ms = 15000;
    expect(liveSummary([leased], now).label).toBe('1 rig · 1 online');
  });

  it('carries the fleet on the Rigs tab, red when a rig went quiet', async () => {
    const { opened } = mount([view('C925', 'online', 'exposing'), view('RC51', 'offline', null)], enabled, '/grid?db=c925');
    const tab = await screen.findByRole('button', { name: 'Rigs: 2 rigs · 1 exposing' });
    expect(tab).toHaveClass('is-alert');
    expect(tab).toHaveTextContent('2 rigs · 1 exposing');
    expect(tab).toHaveAttribute('title', expect.stringContaining('C925: exposing'));
    expect(tab).not.toHaveAttribute('aria-current');
    fireEvent.click(tab);
    expect(opened).toHaveBeenCalledOnce();
    // The Sky's rig markers and the Rigs page's tab and night stay behind
    // when another view opens.
    expect(withoutPlanningParams('db=c925&live=1&rigsTab=nights&nightRig=r&night=2026-11-03').toString()).toBe('db=c925');
  });

  it('marks the Rigs page as the current view', async () => {
    mount([view('C925', 'online', 'idle')], enabled, '/rigs', true);
    expect(await screen.findByRole('button', { name: 'Rigs: 1 rig · 1 online' })).toHaveAttribute('aria-current', 'page');
  });

  it('is a plain tab before the first rig, and stays out of the header without Director', async () => {
    const empty = mount([]);
    await waitFor(() => expect(empty.rigsCall).toHaveBeenCalled());
    expect(await screen.findByRole('button', { name: 'Rigs' })).not.toHaveClass('is-alert');
    empty.unmount();
    const off = mount([view('C925', 'online', 'exposing')], { ...enabled, enabled: false });
    await waitFor(() => expect(off.statusCall).toHaveBeenCalled());
    await new Promise(resolve => setTimeout(resolve, 20));
    expect(off.rigsCall).not.toHaveBeenCalled();
    expect(screen.queryByRole('button', { name: /^Rigs/ })).not.toBeInTheDocument();
  });
});
