import { act, render, screen } from '@testing-library/react';
import { createMemoryRouter, RouterProvider } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import BuildRefresh from '../BuildRefresh';
import { noteServerBuild, reloadForNewerBuild } from '../../updates/pageBuild';

vi.mock('../../updates/pageBuild', async importOriginal => ({
  ...await importOriginal<typeof import('../../updates/pageBuild')>(),
  reloadForNewerBuild: vi.fn(),
}));

describe('BuildRefresh', () => {
  afterEach(() => {
    document.head.querySelectorAll('meta[name="psf-guard-build"]').forEach(meta => meta.remove());
  });

  it('shows a newer build and loads it on the next change of view', async () => {
    const meta = document.createElement('meta');
    meta.name = 'psf-guard-build';
    meta.content = 'aaaa';
    document.head.append(meta);
    const router = createMemoryRouter([{ path: '*', element: <BuildRefresh /> }], { initialEntries: ['/grid'] });
    render(<RouterProvider router={router} />);
    expect(screen.queryByRole('status')).toBeNull();

    act(() => noteServerBuild('bbbb'));
    expect(screen.getByRole('status')).toHaveTextContent('PSF Guard was updated');
    expect(screen.getByRole('button', { name: 'Reload now' })).toBeInTheDocument();

    // Opening an image stays in the same view and keeps the page.
    await act(() => router.navigate('/detail/7'));
    expect(reloadForNewerBuild).not.toHaveBeenCalled();

    await act(() => router.navigate('/sequence'));
    expect(reloadForNewerBuild).toHaveBeenCalledTimes(1);
  });
});
