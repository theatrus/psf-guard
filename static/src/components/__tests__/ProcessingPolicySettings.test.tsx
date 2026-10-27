import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import ProcessingPolicySettings from '../ProcessingPolicySettings';
import type { ProcessingPolicies, PolicyOverrides } from '../../api/processingPolicies';

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
function fixture() {
  const defaults = { analyze_new_frames: false, pair_calibrated_copies: true, scan_calibrated_copies: false };
  let data: ProcessingPolicies = { defaults, databases: [
    { id: 'c925', name: 'C925', overrides: { ...defaults }, effective: { ...defaults } },
    { id: 'new', name: 'New database', overrides: { analyze_new_frames: null, pair_calibrated_copies: null, scan_calibrated_copies: null }, effective: { ...defaults } },
  ] };
  const updates: Array<{ database_id?: string; values: Partial<PolicyOverrides> }> = [];
  server.use(
    http.get('/api/settings/processing', () => HttpResponse.json({ success: true, data })),
    http.put('/api/settings/processing', async ({ request }) => {
      const body = await request.json() as typeof updates[number]; updates.push(body);
      data = structuredClone(data);
      if (body.database_id) Object.assign(data.databases.find(entry => entry.id === body.database_id)!.overrides, body.values);
      else Object.assign(data.defaults, body.values);
      return HttpResponse.json({ success: true, data });
    }),
  );
  return updates;
}
describe('Shared processing settings', () => {
  it('saves only changed defaults and leaves database overrides intact', async () => {
    const updates = fixture();
    render(<ProcessingPolicySettings kind="quality" canManage />, { wrapper: wrapper() });
    const shared = await screen.findByRole('region', { name: 'Shared defaults' });
    fireEvent.click(within(shared).getByRole('checkbox', { name: 'Analyze new frames as they arrive' }));
    fireEvent.click(within(shared).getByRole('button', { name: 'Save shared defaults' }));
    await waitFor(() => expect(updates).toEqual([{ values: { analyze_new_frames: true } }]));
    const c925 = screen.getByRole('region', { name: 'C925 overrides', hidden: true });
    expect(within(c925).getByLabelText('Analyze new frames as they arrive')).not.toBeChecked();
  });
  it('restores inheritance for one database without changing other fields', async () => {
    const updates = fixture();
    render(<ProcessingPolicySettings kind="pairing" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByText('C925 · Has overrides'));
    const own = screen.getByRole('region', { name: 'C925 overrides' });
    fireEvent.click(within(own).getByRole('button', { name: /override global setting/ }));
    expect(within(own).getByText('· from shared defaults')).toBeInTheDocument();
    fireEvent.click(within(own).getByRole('button', { name: 'Save database overrides' }));
    await waitFor(() => expect(updates).toEqual([{ database_id: 'c925', values: { pair_calibrated_copies: null } }]));
    expect(await screen.findByText('C925 · Uses global settings')).toBeInTheDocument();
  });
  it('keeps inherited values visible but disables writes without database management', async () => {
    fixture();
    render(<ProcessingPolicySettings kind="pairing" canManage={false} />, { wrapper: wrapper() });
    const shared = await screen.findByRole('region', { name: 'Shared defaults' });
    expect(within(shared).getByRole('checkbox')).toBeDisabled();
    expect(within(shared).getByRole('button', { name: 'Save shared defaults' })).toBeDisabled();
  });
});
