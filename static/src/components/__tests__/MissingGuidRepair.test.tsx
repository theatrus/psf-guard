import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import MissingGuidRepair from '../MissingGuidRepair';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });
const gaps = [
  { table: 'project', missing: 23, total: 26 },
  { table: 'target', missing: 31, total: 34 },
  { table: 'acquiredimage', missing: 10448, total: 12057 },
];

function mount(canManage: boolean, missing = gaps, writable = true) {
  const fill = vi.fn(() => ok({ tables: missing, filled: 10502, backup_path: '/rigs/askar/schedulerdb.sqlite.before-guid-fill-1790000000' }));
  server.use(
    http.get('/api/db/askar/guids', () => ok({ tables: missing, missing: missing.reduce((sum, gap) => sum + gap.missing, 0), writable })),
    http.post('/api/db/askar/guids/fill', fill),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...render(<MissingGuidRepair dbId="askar" dbName="Askar107PHQ" canManage={canManage} />, { wrapper: Wrapper }), fill };
}

describe('missing GUID repair', () => {
  it('says what is missing and fills it in after copying the database', async () => {
    const { fill } = mount(true);
    const note = await screen.findByRole('note', { name: 'Missing GUIDs in Askar107PHQ' });
    expect(note).toHaveTextContent('10,502 rows have no Target Scheduler GUID');
    expect(note).toHaveTextContent('23 projects, 31 targets, 10,448 frames');
    fireEvent.click(screen.getByRole('button', { name: 'Fill in GUIDs' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Gave 10,502 rows in Askar107PHQ a GUID');
    expect(screen.getByRole('status')).toHaveTextContent('schedulerdb.sqlite.before-guid-fill-1790000000');
    expect(fill).toHaveBeenCalledTimes(1);
  });

  it('points a read-only server at the command line instead', async () => {
    mount(false);
    expect(await screen.findByRole('note')).toHaveTextContent('fill-guids');
    expect(screen.queryByRole('button', { name: 'Fill in GUIDs' })).not.toBeInTheDocument();
  });

  it('offers no button when this server cannot write the file', async () => {
    mount(true, gaps, false);
    expect(await screen.findByRole('note')).toHaveTextContent('This server cannot write the file.');
    expect(screen.queryByRole('button', { name: 'Fill in GUIDs' })).not.toBeInTheDocument();
  });

  it('stays silent when every row has a GUID', async () => {
    const { container } = mount(true, [{ table: 'project', missing: 0, total: 20 }]);
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(container).toBeEmptyDOMElement();
  });
});
