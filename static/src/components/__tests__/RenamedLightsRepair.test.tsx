import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it, vi } from 'vitest';
import { server } from '../../test/msw-server';
import RenamedLightsRepair from '../RenamedLightsRepair';
import type { RenamedLight, RenamedLightsReport } from '../../api/types';

const ok = (data: unknown) => HttpResponse.json({ success: true, data, error: null, status: 'ready' });

function light(id: number, file_name: string, grade = 0): RenamedLight {
  return { id, guid: `guid-${id}`, file_name, target: 'Jellyfish Nebula SHO', grade, reject_reason: null };
}

const pair = {
  kept: light(51, '/data/Jellyfish_LIGHT_SII_Session_2026-10-27_0051.fits'),
  twin: light(812, '/data/Jellyfish_LIGHT_SII_Session_2026-10-27A_0051.fits', 1),
  takes_grade: true,
};
const held = {
  kept: light(71, '/data/Tree_LIGHT_SII_Session_2026-10-27_0071.fits', 2),
  twin: light(813, '/data/Tree_LIGHT_SII_Session_2026-10-27B_0071.fits', 1),
  reason: 'the two lights have different grades; grade one to match the other',
};

function mount(canManage: boolean, report: Partial<RenamedLightsReport> = {}) {
  const merge = vi.fn(() => ok({ merged: [pair], held: [held], backup_path: '/data/catalog.sqlite.before-renamed-merge-1790000000' }));
  server.use(
    http.get('/api/db/veil/renamed-lights', () => ok({ pairs: [pair], held: [held], writable: true, missing_folders: [], synced: null, ...report })),
    http.post('/api/db/veil/renamed-lights/merge', merge),
  );
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  const Wrapper = ({ children }: { children: ReactNode }) => <QueryClientProvider client={client}>{children}</QueryClientProvider>;
  return { ...render(<RenamedLightsRepair dbId="veil" dbName="Veil" canManage={canManage} />, { wrapper: Wrapper }), merge };
}

describe('renamed light repair', () => {
  it('lists the renamed lights and merges them after copying the database', async () => {
    const { merge } = mount(true);
    const note = await screen.findByRole('note', { name: 'Renamed lights in Veil' });
    expect(note).toHaveTextContent('2 lights were imported again after the file was renamed.');
    expect(note).toHaveTextContent('Jellyfish_LIGHT_SII_Session_2026-10-27_0051.fits → Jellyfish_LIGHT_SII_Session_2026-10-27A_0051.fits');
    expect(note).toHaveTextContent('1 pair stays as it is: Tree_LIGHT_SII_Session_2026-10-27B_0071.fits (the two lights have different grades');
    fireEvent.click(screen.getByRole('button', { name: 'Merge renamed lights' }));
    expect(await screen.findByRole('status')).toHaveTextContent('Merged 1 renamed light in Veil.');
    expect(screen.getByRole('status')).toHaveTextContent('catalog.sqlite.before-renamed-merge-1790000000');
    expect(merge).toHaveBeenCalledTimes(1);
  });

  it('points a viewer at the command line instead', async () => {
    mount(false);
    expect(await screen.findByRole('note')).toHaveTextContent('psf-guard merge-renamed-lights --db veil --apply');
    expect(screen.queryByRole('button', { name: 'Merge renamed lights' })).not.toBeInTheDocument();
  });

  it('offers no button when only pairs that stay are left', async () => {
    mount(true, { pairs: [] });
    expect(await screen.findByRole('note')).toHaveTextContent('1 light was imported again');
    expect(screen.queryByRole('button', { name: 'Merge renamed lights' })).not.toBeInTheDocument();
  });

  it('stays silent when no light was imported twice', async () => {
    const { container } = mount(true, { pairs: [], held: [] });
    await new Promise(resolve => setTimeout(resolve, 50));
    expect(container).toBeEmptyDOMElement();
  });
});
