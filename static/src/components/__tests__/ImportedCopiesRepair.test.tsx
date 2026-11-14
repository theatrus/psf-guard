import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import ImportedCopiesRepair from '../ImportedCopiesRepair';
import type { ImportedCopiesMerge, ImportedCopiesReport, RenamedLight } from '../../api/types';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });

const NAME = 'M 45 RGB_LIGHT_Luminance_30.00s_2026-10-04_03-13-11_Session_2026-10-03_0052.fits';

function light(id: number, file_name: string, target: string, grade = 0): RenamedLight {
  return { id, guid: `guid-${id}`, file_name, target, grade, reject_reason: null };
}

const report: ImportedCopiesReport = {
  groups: [{
    kept: light(7335, `C:\\Users\\astro\\Documents\\N.I.N.A\\${NAME}`, 'M45', 1),
    copies: [light(1252, `/capture/Session_2026-10-05/M 45 RGB/${NAME}`, 'M 45 RGB')],
    takes_grade: false,
  }],
  held: [{
    kept: light(9946, 'C:\\NINA\\Christmas Tree Cluster_LIGHT_SII_0071.fits', 'NGC 2264', 1),
    copies: [light(70, '/capture/Christmas Tree Cluster_LIGHT_SII_0071.fits', 'Christmas Tree Cluster', 2)],
    reason: 'the lights have different grades; grade them to match',
  }],
  projects: [
    { id: 5, name: 'M 45 RGB', targets: ['M 45 RGB'], exposure_plans: 4 },
    { id: 10, name: 'Veil Nebula Complex', targets: Array.from({ length: 8 }, (_, index) => `Panel ${index + 1}`), exposure_plans: 18 },
  ],
  targets: [{ id: 12, name: 'Veil Panel 3', project_id: 11, project: 'Veil', exposure_plans: 2 }],
  kept_projects: [{ id: 46, name: 'M 94', reason: 'a Director plan uses it' }],
  digest: 'digest-1',
  writable: true,
};

function mount(canManage: boolean, overrides: Partial<ImportedCopiesReport> = {}, outcome?: ImportedCopiesMerge) {
  const merge = vi.fn(async ({ request }: { request: Request }) => {
    const body = await request.json() as { digest: string };
    return ok(outcome ?? { ...report, state: 'merged', backup_path: `/data/telescope.sqlite.before-imported-copies-${body.digest}` });
  });
  server.use(
    http.get('/api/db/telescope/imported-copies', () => ok({ ...report, ...overrides })),
    http.post('/api/db/telescope/imported-copies/merge', merge),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...render(<ImportedCopiesRepair dbId="telescope" dbName="Telescope" canManage={canManage} />, { wrapper: Wrapper }), merge };
}

describe('imported copies repair', () => {
  it('lists the copies, the projects it deletes and what stays, then merges with the preview digest', async () => {
    const { merge } = mount(true);
    const note = await screen.findByRole('note', { name: 'Imported copies in Telescope' });
    expect(note).toHaveTextContent('2 lights are in the catalog twice: as Target Scheduler recorded them, and from an import of the same file.');
    expect(note).toHaveTextContent(`${NAME} in M45, imported again in M 45 RGB`);
    expect(note).toHaveTextContent('1 light stays as it is: Christmas Tree Cluster_LIGHT_SII_0071.fits (the lights have different grades; grade them to match).');
    expect(note).toHaveTextContent('The merge deletes these projects, which an import created and which will have no lights: M 45 RGB, Veil Nebula Complex (8 targets).');
    expect(note).toHaveTextContent('It also deletes this empty target: Veil Panel 3 in Veil.');
    expect(note).toHaveTextContent('1 empty imported project stays: M 94 (a Director plan uses it).');
    fireEvent.click(screen.getByRole('button', { name: 'Merge imported copies' }));
    const status = await screen.findByRole('status');
    expect(status).toHaveTextContent('Merged 1 light with imported copies in Telescope, and deleted 2 empty projects and 1 empty target.');
    expect(status).toHaveTextContent('telescope.sqlite.before-imported-copies-digest-1');
    expect(merge).toHaveBeenCalledTimes(1);
  });

  it('shows the new list when the database changed since the preview', async () => {
    const preview = { ...report, groups: [], projects: [], targets: [], kept_projects: [], digest: 'digest-2' };
    mount(true, {}, { state: 'changed', preview });
    await screen.findByRole('note', { name: 'Imported copies in Telescope' });
    fireEvent.click(screen.getByRole('button', { name: 'Merge imported copies' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Telescope changed since this list was made, so nothing was merged.');
    const note = screen.getByRole('note', { name: 'Imported copies in Telescope' });
    expect(note).toHaveTextContent('1 light is in the catalog twice');
    expect(screen.queryByRole('button', { name: 'Merge imported copies' })).not.toBeInTheDocument();
  });

  it('points a viewer at the command line instead', async () => {
    mount(false);
    expect(await screen.findByRole('note')).toHaveTextContent('psf-guard merge-imported-copies --db telescope --apply');
    expect(screen.queryByRole('button', { name: 'Merge imported copies' })).not.toBeInTheDocument();
  });

  it('offers the merge for empty projects an earlier merge left', async () => {
    mount(true, { groups: [], held: [], targets: [], kept_projects: [] });
    const note = await screen.findByRole('note');
    expect(note).toHaveTextContent('An earlier merge left projects or targets an import created with no lights.');
    expect(screen.getByRole('button', { name: 'Merge imported copies' })).toBeEnabled();
  });

  it('stays silent when nothing was imported twice', async () => {
    const { container } = mount(true, { groups: [], held: [], projects: [], targets: [], kept_projects: [] });
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(container).toBeEmptyDOMElement();
  });
});
