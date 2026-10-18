import { useState, type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import HeaderMenu from '../header/HeaderMenu';
import ServerInfoPanel from '../ServerInfoPanel';

const catalog = {
  id: 'observatory', name: 'Observatory', database_path: '/catalogs/observatory.sqlite',
  image_directories: ['/images/first night', '/images/second night'],
};
const ok = (data: unknown) => HttpResponse.json({ success: true, data });
function wrapper({ children }: { children: ReactNode }) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  return <QueryClientProvider client={client}>{children}</QueryClientProvider>;
}
function Harness() {
  const [open, setOpen] = useState(false);
  return <>
    <HeaderMenu onWhatsNew={() => undefined} onShortcuts={() => undefined} onServerInfo={() => setOpen(true)} />
    {open && <ServerInfoPanel onClose={() => setOpen(false)} />}
  </>;
}
async function open() {
  const user = userEvent.setup();
  render(<Harness />, { wrapper });
  await user.click(screen.getByRole('button', { name: 'Menu' }));
  await user.click(screen.getByRole('menuitem', { name: 'Server information' }));
  return user;
}

describe('Server information', () => {
  it('groups each database file, ID and image folders under its name', async () => {
    server.use(http.get('/api/databases', () => ok([catalog])));
    await open();
    expect(screen.getByRole('dialog', { name: 'Server information' })).toBeInTheDocument();
    expect(await screen.findByText('/tmp/cache')).toBeInTheDocument();
    const databases = screen.getByRole('region', { name: 'Configured databases' });
    expect(await within(databases).findByRole('heading', { name: 'Databases (1)' })).toBeInTheDocument();
    const entry = within(databases).getByRole('heading', { name: 'Observatory' }).closest('li')!;
    expect(within(entry).getByText(catalog.id)).toBeInTheDocument();
    expect(within(entry).getByText(catalog.database_path)).toBeInTheDocument();
    expect(within(entry).getAllByRole('listitem').map(item => item.textContent)).toEqual(catalog.image_directories);
  });

  it('keeps the loading state distinct from an empty database list', async () => {
    let finish!: () => void;
    const ready = new Promise<void>(resolve => { finish = resolve; });
    server.use(http.get('/api/databases', async () => { await ready; return ok([]); }));
    await open();
    expect(screen.getByText('Loading databases...')).toBeInTheDocument();
    expect(screen.queryByText('No databases configured.')).not.toBeInTheDocument();
    finish();
    expect(await screen.findByText('No databases configured.')).toBeInTheDocument();
  });

  it('shows a database-list error without claiming the server has no databases, and retries', async () => {
    server.use(http.get('/api/databases', () => HttpResponse.json({ success: false, error: 'Offline' }, { status: 500 })));
    await open();
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load configured databases.');
    expect(screen.queryByText('No databases configured.')).not.toBeInTheDocument();
    expect(screen.getByText('/tmp/cache')).toBeInTheDocument();
    server.use(http.get('/api/databases', () => ok([])));
    fireEvent.click(screen.getByRole('button', { name: 'Retry databases' }));
    expect(await screen.findByText('No databases configured.')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('can load databases and retry server information independently', async () => {
    server.use(
      http.get('/api/info', () => HttpResponse.json({ success: false, error: 'Offline' }, { status: 500 })),
      http.get('/api/databases', () => ok([{ ...catalog, image_directories: [] }])),
    );
    await open();
    expect(await screen.findByRole('alert')).toHaveTextContent('Could not load server information.');
    expect(await screen.findByText('No image folders configured.')).toBeInTheDocument();
    server.use(http.get('/api/info', () => ok({ version: '0.12.0', cache_directory: '/restored/cache', allow_database_management: false })));
    fireEvent.click(screen.getByRole('button', { name: 'Retry server information' }));
    expect(await screen.findByText('/restored/cache')).toBeInTheDocument();
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it.each(['Done', 'Close dialog', 'Escape'])('closes with %s and restores focus to the menu', async method => {
    const user = await open();
    if (method === 'Escape') await user.keyboard('{Escape}');
    else await user.click(screen.getByRole('button', { name: method }));
    await waitFor(() => expect(screen.queryByRole('dialog')).not.toBeInTheDocument());
    expect(screen.getByRole('button', { name: 'Menu' })).toHaveFocus();
  });
});
