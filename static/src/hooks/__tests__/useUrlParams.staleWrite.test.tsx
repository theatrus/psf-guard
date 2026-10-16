import { useEffect } from 'react';
import { act, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, MemoryRouter, Route, Routes, RouterProvider, useLocation, useNavigate } from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { useUrlParams, type ParamUpdates } from '../useUrlState';

/**
 * PR #872 review. A write held by a view (a timer, an awaited request)
 * that fires after the person has moved to another view.
 */

let held: ((updates: ParamUpdates) => void) | null = null;
let go: ((to: string) => void) | null = null;

function GridView() {
  const { updateParams } = useUrlParams();
  useEffect(() => { held = updateParams; }, [updateParams]);
  return <p>grid</p>;
}

function Nav() {
  const navigate = useNavigate();
  go = (to) => { void navigate(to); };
  return null;
}

function Where() {
  const { pathname, search } = useLocation();
  return <output aria-label="where">{pathname + search}</output>;
}

const where = () => screen.getByRole('status', { name: 'where' }).textContent ?? '';

describe('a write held by a view that has since been left', () => {
  it('data router (the app): lands on the new view, merged into its search', async () => {
    const router = createMemoryRouter([
      { path: '/grid', element: <><GridView /><Nav /><Where /></> },
      { path: '/sequence', element: <><p>sequence</p><Where /></> },
    ], { initialEntries: ['/grid?db=a&project=1&target=5'] });
    render(<RouterProvider router={router} />);
    await screen.findByText('grid');
    await act(async () => { go!('/sequence?db=a&project=2'); });
    await screen.findByText('sequence');
    // The grid's timer fires now: it writes its cursor onto Sequence.
    act(() => held!({ current: 42 }));
    await waitFor(() => expect(where()).toContain('current=42'));
    // Same pathname as before the PR (router.navigate resolves `?` against
    // router.state.location); only the search it merges into differs.
    expect(where()).toBe('/sequence?db=a&project=2&current=42');
  });

  it('history router (most component tests): resolves against the pathname the hook rendered', async () => {
    render(
      <MemoryRouter initialEntries={['/grid?db=a&project=1']}>
        <Nav />
        <Routes>
          <Route path="/grid" element={<><GridView /><Where /></>} />
          <Route path="/sequence" element={<><p>sequence</p><Where /></>} />
        </Routes>
      </MemoryRouter>,
    );
    await screen.findByText('grid');
    // A push and a write in one handler, as PushThenWrite does but to
    // another path: the data router lands on /sequence, this one on /grid.
    act(() => {
      go!('/sequence?db=a&project=2');
      held!({ calRig: 'c925' });
    });
    await waitFor(() => expect(where()).toContain('calRig=c925'));
    expect(where()).toBe('/sequence?db=a&project=2&calRig=c925');
  });
});
