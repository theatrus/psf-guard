import { useEffect, useState, type ReactNode } from 'react';
import { flushSync } from 'react-dom';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import {
  createMemoryRouter,
  MemoryRouter,
  RouterProvider,
  useLocation,
  useNavigate,
} from 'react-router-dom';
import { describe, expect, it } from 'vitest';
import { useGridState, useUrlParams } from '../useUrlState';
import { useCalibrationUrlState } from '../../components/calibration/useCalibrationUrlState';

/**
 * Two URL writes in one moment must both land. Every write applies to the
 * URL as the router now holds it, so a write made earlier in the same tick,
 * by this hook or by another, survives the next one, before or after any
 * render in between.
 */

type Kind = 'data router' | 'memory router';

function Search() {
  return <output aria-label="search">{useLocation().search}</output>;
}

const search = () => new URLSearchParams(screen.getByRole('status', { name: 'search' }).textContent ?? '');

/** Mount `element` at `entry` behind `/start`, so a stray history entry
 *  shows when Back is taken. */
function mount(kind: Kind, element: ReactNode, entry = '/grid?db=a&project=1') {
  const page = <>{element}<Search /></>;
  if (kind === 'data router') {
    const router = createMemoryRouter(
      [{ path: '/start', element: <Search /> }, { path: '*', element: page }],
      { initialEntries: ['/start', entry], initialIndex: 1 },
    );
    render(<RouterProvider router={router} />);
    return { back: () => router.navigate(-1) };
  }
  let goBack: () => void = () => {};
  function Back() {
    const navigate = useNavigate();
    goBack = () => navigate(-1);
    return null;
  }
  render(
    <MemoryRouter initialEntries={['/start', entry]} initialIndex={1}>
      <Back />
      {page}
    </MemoryRouter>,
  );
  return { back: async () => goBack() };
}

/** Writes `updates` once, when mounted. */
function WriteOnMount({ updates }: { updates: Record<string, string> }) {
  const { updateParams } = useUrlParams();
  useEffect(() => {
    updateParams(updates);
    // Once, on mount.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);
  return null;
}

/** Two effects of one component, each writing its own key. */
function TwoEffects() {
  const { updateParams } = useUrlParams();
  useEffect(() => updateParams({ calRig: 'c925' }), [updateParams]);
  useEffect(() => updateParams({ calSession: 's1' }), [updateParams]);
  return null;
}

/** Two buttons, each writing through its own hook in its own component. */
function TwoWriters({ immediate = false }: { immediate?: boolean }) {
  const first = useUrlParams();
  return (
    <>
      <button type="button" onClick={() => first.updateParams({ calRig: 'c925' })}>Write</button>
      <SecondWriter immediate={immediate} />
    </>
  );
}

function SecondWriter({ immediate }: { immediate: boolean }) {
  const { updateParams } = useUrlParams();
  return (
    <button type="button" onClick={() => updateParams({ calSession: 's1' }, { immediate })}
      data-testid="second">Second</button>
  );
}

/** One click: a write, an urgent render of this component before the
 *  router has shown the write, and another write from the same hook. */
function WriteRenderWrite() {
  const { updateParams } = useUrlParams();
  const [renders, setRenders] = useState(0);
  return (
    <button
      type="button"
      data-renders={renders}
      onClick={() => {
        updateParams({ calRig: 'c925' });
        flushSync(() => setRenders((count) => count + 1));
        updateParams({ calSession: 's1' });
      }}
    >
      Write
    </button>
  );
}

/** One click: another view pushed, then a write to the view it shows. */
function PushThenWrite() {
  const navigate = useNavigate();
  const { updateParams } = useUrlParams();
  return (
    <button
      type="button"
      onClick={() => {
        void navigate('/grid?db=a&project=2');
        updateParams({ calRig: 'c925' });
      }}
    >
      Write
    </button>
  );
}

/** A write in an effect, and one from a child's effect that runs first. */
function EffectsAcrossComponents() {
  return (
    <>
      <WriteOnMount updates={{ calSession: 's1' }} />
      <WriteOnMount updates={{ calRig: 'c925' }} />
    </>
  );
}

describe.each<Kind>(['data router', 'memory router'])('useUrlParams with a %s', (kind) => {
  it('keeps both writes from two effects of one component', async () => {
    mount(kind, <TwoEffects />);
    await waitFor(() => expect(search().get('calSession')).toBe('s1'));
    expect(search().get('calRig')).toBe('c925');
    expect(search().get('project')).toBe('1');
  });

  it('keeps both writes from effects in two components', async () => {
    mount(kind, <EffectsAcrossComponents />);
    await waitFor(() => expect(search().get('calRig')).toBe('c925'));
    expect(search().get('calSession')).toBe('s1');
  });

  it('keeps both writes from two hooks in one tick, before either renders', async () => {
    mount(kind, <TwoWriters />);
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'Write' }));
      fireEvent.click(screen.getByTestId('second'));
    });
    await waitFor(() => expect(search().get('calRig')).toBe('c925'));
    expect(search().get('calSession')).toBe('s1');
  });

  it('keeps a write that the next one renders at once', async () => {
    mount(kind, <TwoWriters immediate />);
    act(() => {
      fireEvent.click(screen.getByRole('button', { name: 'Write' }));
      fireEvent.click(screen.getByTestId('second'));
    });
    await waitFor(() => expect(search().get('calSession')).toBe('s1'));
    expect(search().get('calRig')).toBe('c925');
  });

  it('keeps a write across a render that still shows the URL before it', async () => {
    mount(kind, <WriteRenderWrite />);
    fireEvent.click(screen.getByRole('button', { name: 'Write' }));
    await waitFor(() => expect(search().get('calSession')).toBe('s1'));
    expect(search().get('calRig')).toBe('c925');
  });

  it('writes onto a view pushed in the same moment, and adds no entry of its own', async () => {
    const { back } = mount(kind, <PushThenWrite />);
    fireEvent.click(screen.getByRole('button', { name: 'Write' }));
    await waitFor(() => expect(search().get('calRig')).toBe('c925'));
    expect(search().get('project')).toBe('2');
    // The push is the one new entry: Back returns to the view it left.
    await act(async () => { await back(); });
    await waitFor(() => expect(search().get('project')).toBe('1'));
    expect(search().has('calRig')).toBe(false);
  });

  it('replaces the entry it writes to, however many writes compose', async () => {
    const { back } = mount(kind, <TwoEffects />);
    await waitFor(() => expect(search().get('calSession')).toBe('s1'));
    await act(async () => { await back(); });
    await waitFor(() => expect(screen.getByRole('status', { name: 'search' }).textContent).toBe(''));
  });

  it('hands an updater the URL as it stands, with writes not yet shown', async () => {
    mount(kind, <MoveAfterWrite />, '/grid?db=a&calFrame=f1');
    fireEvent.click(screen.getByRole('button', { name: 'Move' }));
    await waitFor(() => expect(search().get('calSessionFrame')).toBe('f2'));
    expect(search().has('calFrame')).toBe(false);
  });

  it('sets aside the frame just chosen when Calibration switches to Groups', async () => {
    mount(kind, <ChooseFrameThenGroups />, '/calibration?db=a&calFrame=f1');
    fireEvent.click(screen.getByRole('button', { name: 'Groups' }));
    await waitFor(() => expect(search().get('calBrowse')).toBe('groups'));
    expect(search().get('calSessionFrame')).toBe('f2');
  });

  it('keeps grid selections and expanded groups changed on either side of a render', async () => {
    mount(kind, <SelectRenderAdd />);
    fireEvent.click(screen.getByRole('button', { name: 'Select' }));
    await waitFor(() => expect(search().get('selected')).toBe('11,12'));
    expect(search().get('current')).toBe('11');
    expect(search().get('expanded')).toBe('a,b');
  });
});

/** One click: a frame chosen, then moved aside by an updater that reads it. */
function MoveAfterWrite() {
  const { updateParams } = useUrlParams();
  return (
    <button
      type="button"
      onClick={() => {
        updateParams({ calFrame: 'f2' });
        updateParams((latest) => ({ calSessionFrame: latest.get('calFrame'), calFrame: null }));
      }}
    >
      Move
    </button>
  );
}

/** One click: a session's frame chosen, then the Groups list. */
function ChooseFrameThenGroups() {
  const { selectFrame, setBrowse } = useCalibrationUrlState();
  return (
    <button
      type="button"
      onClick={() => {
        selectFrame('f2');
        setBrowse('groups');
      }}
    >
      Groups
    </button>
  );
}

/** One click: the grid's selection and expanded groups, each changed, then
 *  changed again after a render that still shows the URL before. */
function SelectRenderAdd() {
  const { setCurrentImageSelection, setSelectedImages, setExpandedGroups } = useGridState();
  const [renders, setRenders] = useState(0);
  return (
    <button
      type="button"
      data-renders={renders}
      onClick={() => {
        setCurrentImageSelection(11);
        setExpandedGroups((groups) => new Set(groups).add('a'));
        flushSync(() => setRenders((count) => count + 1));
        setSelectedImages((selected) => new Set(selected).add(12));
        setExpandedGroups((groups) => new Set(groups).add('b'));
      }}
    >
      Select
    </button>
  );
}
