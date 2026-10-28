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
    const shared = await screen.findByRole('region', { name: 'Global settings' });
    fireEvent.change(within(shared).getByRole('combobox', { name: 'Analyze new frames as they arrive' }), { target: { value: 'on' } });
    fireEvent.click(within(shared).getByRole('button', { name: 'Save global settings' }));
    await waitFor(() => expect(updates).toEqual([{ values: { analyze_new_frames: true } }]));
    const c925 = screen.getByRole('region', { name: 'C925 overrides', hidden: true });
    expect(within(c925).getByLabelText('Analyze new frames as they arrive')).toHaveValue('off');
  });
  it('restores inheritance for one database without changing other fields', async () => {
    const updates = fixture();
    render(<ProcessingPolicySettings kind="pairing" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: /^C925.*1 override$/ }));
    const own = screen.getByRole('region', { name: 'C925 overrides' });
    fireEvent.click(within(own).getByRole('button', { name: /override global setting/ }));
    expect(within(own).getByText('· global')).toBeInTheDocument();
    fireEvent.click(within(own).getByRole('button', { name: 'Save database' }));
    await waitFor(() => expect(updates).toEqual([{ database_id: 'c925', values: { pair_calibrated_copies: null } }]));
    expect(await screen.findByRole('button', { name: /^C925.*Uses global settings$/ })).toBeInTheDocument();
  });
  it('keeps inherited values visible but disables writes without database management', async () => {
    fixture();
    render(<ProcessingPolicySettings kind="pairing" canManage={false} />, { wrapper: wrapper() });
    const shared = await screen.findByRole('region', { name: 'Global settings' });
    expect(within(shared).getByRole('combobox')).toBeDisabled();
    expect(within(shared).getByRole('button', { name: 'Save global settings' })).toBeDisabled();
  });
  it('keeps a database draft when its override row is collapsed', async () => {
    const updates = fixture();
    render(<ProcessingPolicySettings kind="pairing" canManage />, { wrapper: wrapper() });
    const row = await screen.findByRole('button', { name: /^C925.*1 override$/ });
    fireEvent.click(row);
    const own = screen.getByRole('region', { name: 'C925 overrides' });
    fireEvent.change(within(own).getByRole('combobox'), { target: { value: 'off' } });
    fireEvent.click(row);
    expect(own).not.toBeVisible();
    fireEvent.click(row);
    expect(within(own).getByRole('combobox')).toHaveValue('off');
    fireEvent.click(within(own).getByRole('button', { name: 'Save database' }));
    await waitFor(() => expect(updates).toEqual([{ database_id: 'c925', values: { pair_calibrated_copies: false } }]));
  });
  it('resets all displayed quality overrides only when the database is saved', async () => {
    const updates = fixture();
    render(<ProcessingPolicySettings kind="quality" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('button', { name: /^C925.*2 overrides$/ }));
    const own = screen.getByRole('region', { name: 'C925 overrides' });
    fireEvent.click(within(own).getByRole('button', { name: 'Use global settings for all fields' }));
    expect(within(own).queryByRole('combobox')).not.toBeInTheDocument();
    expect(within(own).getAllByText('· global')).toHaveLength(2);
    expect(updates).toEqual([]);
    fireEvent.click(within(own).getByRole('button', { name: 'Save database' }));
    await waitFor(() => expect(updates).toEqual([{
      database_id: 'c925', values: { analyze_new_frames: null, scan_calibrated_copies: null },
    }]));
    expect(await screen.findByRole('button', { name: /^C925.*Uses global settings$/ })).toBeInTheDocument();
  });
});
