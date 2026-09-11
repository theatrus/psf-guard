import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import type { SchedulerFlatHistoryRecord } from '../../api/types';
import SchedulerFlatHistory from '../SchedulerFlatHistory';

function wrapper() {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  };
}

const row: SchedulerFlatHistoryRecord = {
  record_id: 'record-a', origin_id: 'origin', source_name: 'C925', source_row_id: 1,
  fingerprint: 'a'.repeat(64), target_guid: 'target', target_name: 'M31', profile_id: 'profile',
  light_session_date: 1789000000, light_session_id: 2, flats_taken_date: 1789040000,
  flats_type: 'sky', filter_name: 'L', gain: 100, offset: 30, bin: 1, readout_mode: 0,
  rotation: 90, roi: 1, state: 'recorded', reason: null, invalidated_at: null,
  last_seen: 1789041000, acknowledged_at: null, detail: null,
};

function serve(rows: SchedulerFlatHistoryRecord[], total = rows.length) {
  server.use(http.get('/api/db/demo/flat-history', () =>
    HttpResponse.json({ success: true, data: { records: rows, total } })));
}

describe('scheduler flat coverage', () => {
  it('requires a reason and submits exact selected records, including duplicates', async () => {
    serve([row, { ...row, record_id: 'record-b', source_row_id: 2 }]);
    let received: unknown;
    server.use(http.post('/api/db/demo/flat-history/invalidate', async ({ request }) => {
      received = await request.json();
      return HttpResponse.json({ success: true, data: { invalidated: 2 } });
    }));
    render(<SchedulerFlatHistory dbId="demo" canManage />, { wrapper: wrapper() });
    await screen.findAllByText('M31');
    fireEvent.click(screen.getByRole('checkbox', { name: 'Select recorded coverage on this page' }));
    const submit = screen.getByRole('button', { name: 'Invalidate coverage' });
    expect(submit).toBeDisabled();
    expect(screen.getByText(/Files and calibration masters are unchanged/)).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: '  Bright-star artifacts  ' } });
    fireEvent.click(submit);
    await waitFor(() => expect(received).toEqual({
      record_ids: ['record-a', 'record-b'], reason: 'Bright-star artifacts',
    }));
    expect(await screen.findByRole('status')).toHaveTextContent('2 coverage records awaiting sync.');
    expect(screen.queryByLabelText('Reason')).not.toBeInTheDocument();
  });

  it('shows failed invalidation and preserves the selection and reason for retry', async () => {
    serve([row]);
    server.use(http.post('/api/db/demo/flat-history/invalidate', () =>
      HttpResponse.json({ success: false, error: 'Coverage changed; refresh the list.' })));
    render(<SchedulerFlatHistory dbId="demo" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Select coverage 1 for M31' }));
    fireEvent.change(screen.getByLabelText('Reason'), { target: { value: 'Bad flats' } });
    fireEvent.click(screen.getByRole('button', { name: 'Invalidate coverage' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Coverage changed; refresh the list.');
    expect(screen.getByLabelText('Reason')).toHaveValue('Bad flats');
    expect(screen.getByRole('checkbox', { name: 'Select coverage 1 for M31' })).toBeChecked();
  });

  it('does not offer mutation controls to read-only users or already invalidated records', async () => {
    serve([{ ...row, state: 'conflict', reason: 'Artifacts', detail: 'Source row changed' }]);
    render(<SchedulerFlatHistory dbId="demo" canManage={false} />, { wrapper: wrapper() });
    expect(await screen.findByText('Source row changed')).toBeInTheDocument();
    expect(screen.getAllByText('Changed in NINA')).toHaveLength(2);
    expect(screen.queryByRole('checkbox')).not.toBeInTheDocument();
  });

  it('clears selection when searching or changing pages and passes server filters', async () => {
    const requests: URL[] = [];
    server.use(http.get('/api/db/demo/flat-history', ({ request }) => {
      requests.push(new URL(request.url));
      return HttpResponse.json({ success: true, data: { records: [row], total: 201 } });
    }));
    render(<SchedulerFlatHistory dbId="demo" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Select coverage 1 for M31' }));
    fireEvent.click(screen.getByRole('button', { name: 'Next coverage page' }));
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('offset')).toBe('100'));
    expect(screen.queryByLabelText('Reason')).not.toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Target, source or filter'), { target: { value: 'M31' } });
    fireEvent.click(screen.getByRole('button', { name: 'Search' }));
    await waitFor(() => expect(requests.at(-1)?.searchParams.get('q')).toBe('M31'));
    expect(requests.at(-1)?.searchParams.get('offset')).toBe('0');
    expect(requests.at(-1)?.searchParams.get('state')).toBe('recorded');
  });

  it('does not submit a stale selected row after refresh changes its status', async () => {
    serve([row]);
    render(<SchedulerFlatHistory dbId="demo" canManage />, { wrapper: wrapper() });
    fireEvent.click(await screen.findByRole('checkbox', { name: 'Select coverage 1 for M31' }));
    serve([{ ...row, state: 'superseded' }]);
    fireEvent.click(screen.getByRole('button', { name: 'Refresh coverage' }));
    await waitFor(() => expect(screen.getAllByText('Replaced')).toHaveLength(2));
    expect(screen.queryByRole('button', { name: 'Invalidate coverage' })).not.toBeInTheDocument();
  });

  it('returns to a valid page when a concurrent sync shrinks the result set', async () => {
    let shrunk = false;
    server.use(http.get('/api/db/demo/flat-history', ({ request }) => {
      const offset = new URL(request.url).searchParams.get('offset');
      if (offset === '100') shrunk = true;
      return HttpResponse.json({ success: true, data: {
        records: offset === '100' ? [] : [row], total: shrunk ? 1 : 101,
      } });
    }));
    render(<SchedulerFlatHistory dbId="demo" canManage />, { wrapper: wrapper() });
    await screen.findByText('1-1 of 101 records');
    fireEvent.click(screen.getByRole('button', { name: 'Next coverage page' }));
    expect(await screen.findByText('1-1 of 1 records')).toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Previous coverage page' })).toBeDisabled();
  });
});
