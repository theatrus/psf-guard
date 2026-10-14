import { act, renderHook, waitFor } from '@testing-library/react';
import type { PropsWithChildren } from 'react';
import { MemoryRouter, useLocation, useNavigate } from 'react-router-dom';
import { afterEach, describe, expect, it } from 'vitest';
import { useGridState } from '../../hooks/useUrlState';
import { getDisplayPreferences, setDisplayPreferences } from '../../hooks/useDisplayPreferences';
import { scopeForView } from '../viewScope';

const query = (search: string, from: string, to: string) =>
  Object.fromEntries(scopeForView(search, from, to));

describe('the scope the header carries', () => {
  const images = '?db=rig&project=1&target=2&order=newest&selected=5,6&current=5&size=450';

  it('leaves a view\'s thumbnail size behind when the view changes', () => {
    expect(query(images, '/grid', '/sequence')).toEqual({
      db: 'rig', project: '1', target: '2', order: 'newest', selected: '5,6', current: '5',
    });
    expect(query(images.replace('size=450', 'size=200'), '/sequence', '/grid')).not.toHaveProperty('size');
    // Library, Calibration and Stacks show no thumbnails of their own.
    expect(query(images, '/grid', '/')).not.toHaveProperty('size');
    expect(query(images, '/', '/grid')).not.toHaveProperty('size');
  });

  it('keeps it for the same view, and back from that view\'s detail', () => {
    expect(query(images, '/grid', '/grid')).toHaveProperty('size', '450');
    expect(query(images, '/detail/5', '/grid')).toHaveProperty('size', '450');
    expect(query(`${images}&returnTo=sequence`, '/detail/5', '/sequence')).toHaveProperty('size', '450');
    expect(query(`${images}&returnTo=sequence`, '/detail/5', '/grid')).not.toHaveProperty('size');
  });

  it('still leaves Planning\'s own keys behind', () => {
    expect(query('?db=rig&project=1&plan=rig:1', '/plan', '/grid')).toEqual({ db: 'rig', project: '1' });
  });
});

describe('each view keeps its own thumbnail size across the header', () => {
  const base = getDisplayPreferences();
  afterEach(() => setDisplayPreferences({ ...base }));

  /** Images or Sequence, reading its own size, with a header that moves
   *  between them as App's does. */
  function useView() {
    const location = useLocation();
    const navigate = useNavigate();
    const view = location.pathname === '/sequence' ? 'sequence' : 'grid';
    const grid = useGridState(view);
    const go = (path: string) => {
      const next = scopeForView(location.search, location.pathname, path).toString();
      navigate(next ? `${path}?${next}` : path);
    };
    return { view, size: grid.imageSize, setSize: grid.setImageSize, search: location.search, go };
  }

  it('Images → Sequence → Images shows each view\'s own size', async () => {
    setDisplayPreferences({ ...base, gridThumbnailSize: 300, sequenceThumbnailSize: 200 });
    const { result } = renderHook(useView, {
      wrapper: ({ children }: PropsWithChildren) => (
        <MemoryRouter initialEntries={['/grid?db=rig&project=1']}>{children}</MemoryRouter>
      ),
    });
    expect(result.current.size).toBe(300);
    act(() => result.current.setSize(450));
    await waitFor(() => expect(result.current.search).toContain('size=450'));

    act(() => result.current.go('/sequence'));
    await waitFor(() => expect(result.current.view).toBe('sequence'));
    expect(result.current.search).not.toContain('size=');
    expect(result.current.size).toBe(200);

    act(() => result.current.go('/grid'));
    await waitFor(() => expect(result.current.view).toBe('grid'));
    // The size chosen in Images was saved as Images' own.
    expect(result.current.size).toBe(450);
    expect(getDisplayPreferences().sequenceThumbnailSize).toBe(200);
  });
});
