import { act, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, RouterProvider, useLocation } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { useUrlParams, type ParamUpdates } from '../useUrlState';

/**
 * Review repro (PR #872, 27c6ae60): withoutBasename slices basename.length,
 * unlike React Router's stripBasename, so a basename written with a
 * trailing slash leaves a relative path ("grid"), which navigate resolves
 * against the current route.
 */
let write: ((updates: ParamUpdates) => void) | null = null;

function View() {
  const { updateParams } = useUrlParams();
  write = updateParams;
  const { pathname, search } = useLocation();
  return <output aria-label="where">{pathname + search}</output>;
}

const where = () => screen.getByRole('status', { name: 'where' }).textContent ?? '';

describe.each(['/app', '/app/'])('updateParams under basename %s', (basename) => {
  it('stays on the same path', async () => {
    const router = createMemoryRouter(
      [{ path: '/grid', element: <View /> }, { path: '*', element: <View /> }],
      { basename, initialEntries: ['/app/grid?db=a'] },
    );
    render(<RouterProvider router={router} />);
    await waitFor(() => expect(where()).toBe('/grid?db=a'));
    act(() => write!({ current: 42 }));
    await waitFor(() => expect(where()).toContain('current=42'));
    expect(router.state.location.pathname).toBe('/app/grid');
    expect(where()).toBe('/grid?db=a&current=42');
  });
});
