import type { ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import AstroBinExportDialog from '../AstroBinExportDialog';
import type { AstroBinExport } from '../../api/types';

function wrapper() {
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  return function Wrapper({ children }: { children: ReactNode }) {
    return <QueryClientProvider client={queryClient}>{children}</QueryClientProvider>;
  };
}

const ok = <T,>(data: T) => ({ success: true, data, error: null });

const exportFor = (detail: 'essentials' | 'full', mapped: boolean): AstroBinExport => ({
  filename: `astrobin-NGC-7023-${detail}.csv`,
  detail,
  scope: 'NGC 7023',
  rows: [
    {
      date: '2026-09-13',
      filter: 'L',
      filter_id: mapped ? 4049 : null,
      number: 77,
      duration: 300,
      binning: detail === 'full' ? 1 : null,
      gain: detail === 'full' ? 100 : null,
      sensor_cooling: detail === 'full' ? -3 : null,
      f_number: detail === 'full' ? 4.9 : null,
      darks: detail === 'full' ? 15 : null,
      flats: detail === 'full' ? 20 : null,
      flat_darks: detail === 'full' ? 0 : null,
      bias: detail === 'full' ? 33 : null,
      temperature: detail === 'full' ? 23.81 : null,
    },
  ],
  csv: `date,filter,number,duration\n2026-09-13,${mapped ? 4049 : ''},77,300\n`,
  frames: 77,
  nights: 1,
  total_exposure_seconds: 23100,
  unmapped_filters: mapped ? [] : ['L'],
  lights_missing_files: 0,
  notes: [],
});

const request = { dbId: 'alpha', scope: { target_id: 1 }, label: 'NGC 7023' };

describe('AstroBinExportDialog', () => {
  it('previews the rows, links the download, and widens with full detail', async () => {
    const seen: string[] = [];
    server.use(
      http.get('/api/settings/astrobin', () => HttpResponse.json(ok({ filter_ids: { L: 4049 } }))),
      http.get('/api/db/alpha/astrobin-export', ({ request: req }) => {
        const url = new URL(req.url);
        seen.push(url.search);
        const detail = url.searchParams.get('detail') === 'full' ? 'full' : 'essentials';
        return HttpResponse.json(ok(exportFor(detail, true)));
      })
    );
    render(<AstroBinExportDialog request={request} onClose={() => {}} />, {
      wrapper: wrapper(),
    });

    expect(await screen.findByText('2026-09-13')).toBeInTheDocument();
    expect(screen.getByText('#4049')).toBeInTheDocument();
    expect(screen.queryByText('Darks')).not.toBeInTheDocument();
    expect(seen[0]).toBe('?target_id=1&include_pending=true');

    const download = screen.getByRole('link', { name: 'Download CSV' });
    expect(download).toHaveAttribute(
      'href',
      '/api/db/alpha/astrobin-export.csv?target_id=1&include_pending=true'
    );

    fireEvent.click(screen.getByRole('radio', { name: /Full/ }));
    expect(await screen.findByText('Darks')).toBeInTheDocument();
    await waitFor(() =>
      expect(seen).toContain('?target_id=1&include_pending=true&detail=full')
    );
    expect(screen.getByText('23.81')).toBeInTheDocument();
    expect(screen.getByRole('link', { name: 'Download CSV' })).toHaveAttribute(
      'href',
      '/api/db/alpha/astrobin-export.csv?target_id=1&include_pending=true&detail=full'
    );

    fireEvent.click(screen.getByRole('checkbox', { name: /Count ungraded lights/ }));
    await waitFor(() => expect(seen).toContain('?target_id=1&detail=full'));
  });

  it('asks for the ids of unmapped filters and saves them alongside the known ones', async () => {
    let saved: unknown = null;
    let mapped = false;
    server.use(
      http.get('/api/settings/astrobin', () =>
        HttpResponse.json(ok({ filter_ids: { Ha: 4051 } }))
      ),
      http.put('/api/settings/astrobin', async ({ request: req }) => {
        saved = await req.json();
        mapped = true;
        return HttpResponse.json(ok({ filter_ids: { Ha: 4051, L: 4049 } }));
      }),
      http.get('/api/db/alpha/astrobin-export', () =>
        HttpResponse.json(ok(exportFor('essentials', mapped)))
      )
    );
    render(<AstroBinExportDialog request={request} onClose={() => {}} />, {
      wrapper: wrapper(),
    });

    const input = await screen.findByLabelText('AstroBin id for filter L');
    const save = screen.getByRole('button', { name: 'Save filter ids' });
    expect(save).toBeDisabled();
    fireEvent.change(input, { target: { value: '4049' } });
    expect(save).toBeEnabled();
    fireEvent.click(save);

    await waitFor(() => expect(saved).toEqual({ filter_ids: { Ha: 4051, L: 4049 } }));
    expect(await screen.findByText('#4049')).toBeInTheDocument();
    expect(screen.queryByLabelText('AstroBin id for filter L')).not.toBeInTheDocument();
  });
});
