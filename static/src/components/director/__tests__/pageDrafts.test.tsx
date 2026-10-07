import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, Link, RouterProvider } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DraftProvider, EditedMark, SaveBar } from '../pageDrafts';
import { leavesPage, samePageAs, useDraftSection, usePageDrafts, type Drafts } from '../pageDraftsState';

/** A section holding one value, saved by a stand-in for its API call. */
function Field({ id, label, order, saved: stored, saves, fail = false }: { id: string; label: string; order: number; saved: string; saves: string[]; fail?: boolean | string }) {
  const [saved, setSaved] = useState(stored);
  const [value, setValue] = useState(stored);
  useDraftSection(id, {
    label, order, unsaved: value !== saved, changes: value !== saved ? [`${saved} → ${value}`] : [],
    save: async () => { if (fail !== false) return fail === true ? false : fail; saves.push(`${label}=${value}`); setSaved(value); return true; },
    discard: () => setValue(saved),
  });
  return <input aria-label={label} value={value} onChange={event => setValue(event.target.value)} />;
}

function Page({ saves, failPlan = false, out }: { saves: string[]; failPlan?: boolean | string; out?: { current: Drafts | null } }) {
  const drafts = usePageDrafts();
  if (out) out.current = drafts;
  return <DraftProvider drafts={drafts}>
    {/* The plan named in the address is what the page edits. */}
    <SaveBar drafts={drafts} canWrite pageKeys={['plan']} />
    <h3>Plan<EditedMark drafts={drafts} id="plan" /></h3>
    {/* Registered out of order: saving still goes framing, then plan. */}
    <Field id="plan" label="Plan" order={2} saved="40" saves={saves} fail={failPlan} />
    <Field id="framing" label="Framing" order={1} saved="M 1" saves={saves} />
    <Link to="/elsewhere">Library</Link>
  </DraftProvider>;
}

function mount(failPlan: boolean | string = false, initial = '/') {
  const saves: string[] = [];
  const out: { current: Drafts | null } = { current: null };
  const router = createMemoryRouter([
    { path: '/', element: <Page saves={saves} failPlan={failPlan} out={out} /> },
    { path: '/elsewhere', element: <p>Library page</p> },
  ], { initialEntries: [initial] });
  render(<RouterProvider router={router} />);
  return { saves, router, drafts: out };
}

const bar = () => screen.queryByRole('region', { name: 'Unsaved changes' });

afterEach(() => vi.restoreAllMocks());

describe('page save bar', () => {
  it('shows nothing until something is edited, then names every section with edits', () => {
    mount();
    expect(bar()).toBeNull();
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    expect(bar()).toHaveTextContent('Unsaved changes in Plan.');
    expect(screen.getByText('edited')).toBeInTheDocument();
    fireEvent.change(screen.getByLabelText('Framing'), { target: { value: 'M 57' } });
    expect(bar()).toHaveTextContent('Unsaved changes in Framing and Plan.');
    // And what changed in each.
    const changed = screen.getByRole('list', { name: 'What changed' });
    expect(changed).toHaveTextContent('Framing: M 1 → M 57');
    expect(changed).toHaveTextContent('Plan: 40 → 120');
  });

  it('saves every section in order and says so', async () => {
    const { saves } = mount();
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    fireEvent.change(screen.getByLabelText('Framing'), { target: { value: 'M 57' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    expect(await screen.findByText('All changes saved.')).toBeInTheDocument();
    expect(saves).toEqual(['Framing=M 57', 'Plan=120']);
    expect(screen.queryByText('edited')).toBeNull();
  });

  it('stops at a section that cannot be saved and points at it', async () => {
    const { saves } = mount(true);
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    fireEvent.change(screen.getByLabelText('Framing'), { target: { value: 'M 57' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(bar()).toHaveTextContent('Plan was not saved; its tab shows why.'));
    expect(saves).toEqual(['Framing=M 57']);
  });

  it('says why a section could not be saved when it knows', async () => {
    mount('Invalid Director metadata request');
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('button', { name: 'Save changes' }));
    await waitFor(() => expect(bar()).toHaveTextContent('Plan was not saved: Invalid Director metadata request'));
  });

  it('discards every edit after asking', () => {
    mount();
    const confirm = vi.spyOn(window, 'confirm').mockReturnValueOnce(false).mockReturnValueOnce(true);
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(screen.getByLabelText('Plan')).toHaveValue('120');
    fireEvent.click(screen.getByRole('button', { name: 'Discard changes' }));
    expect(confirm).toHaveBeenLastCalledWith('Discard the unsaved changes in Plan?');
    expect(screen.getByLabelText('Plan')).toHaveValue('40');
    expect(bar()).toBeNull();
  });

  it('asks before leaving the page with edits, and can save on the way out', async () => {
    const { saves, router } = mount();
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    fireEvent.click(screen.getByRole('link', { name: 'Library' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Leave with unsaved changes in Plan?');
    fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
    expect(router.state.location.pathname).toBe('/');

    fireEvent.click(screen.getByRole('link', { name: 'Library' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Save and leave' }));
    expect(await screen.findByText('Library page')).toBeInTheDocument();
    expect(saves).toEqual(['Plan=120']);
  });

  it('asks before another plan opens in its place, but lets the same plan take a new address', async () => {
    const { router } = mount(false, '/?plan=a');
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    // The header's plan picker, Back, or a link: the path stays, the plan changes.
    await act(() => router.navigate('/?plan=b'));
    expect(await screen.findByRole('alert')).toHaveTextContent('Leave with unsaved changes in Plan?');
    expect(router.state.location.search).toBe('?plan=a');
    fireEvent.click(screen.getByRole('button', { name: 'Stay' }));
    // Another parameter is the same page.
    await act(() => router.navigate('/?plan=a&planTab=rigs'));
    expect(router.state.location.search).toBe('?plan=a&planTab=rigs');
    // The plan's key turning canonical (after a detach, an attach or a first
    // activation) rewrites this address and keeps the edits.
    await act(() => router.navigate('/?plan=guid', { replace: true, state: samePageAs('?plan=a&planTab=rigs') }));
    expect(router.state.location.search).toBe('?plan=guid');
    expect(screen.queryByRole('alert')).toBeNull();
    expect(screen.getByLabelText('Plan')).toHaveValue('120');
  });

  it('marks only a rewrite of the address it was made from as the same page', () => {
    const at = (search: string, state: unknown = null) => ({ pathname: '/plan', search, hash: '', state, key: search });
    expect(leavesPage(at('?plan=a'), at('?plan=b'), ['plan'])).toBe(true);
    expect(leavesPage(at('?plan=a'), at('?plan=a&planTab=rigs'), ['plan'])).toBe(false);
    expect(leavesPage(at('?plan=a'), at('?plan=guid', samePageAs('?plan=a')), ['plan'])).toBe(false);
    // Stepping forward from plan b onto that rewritten history entry still leaves b.
    expect(leavesPage(at('?plan=b'), at('?plan=guid', samePageAs('?plan=a')), ['plan'])).toBe(true);
    expect(leavesPage(at('?plan=a'), { ...at(''), pathname: '/' }, ['plan'])).toBe(true);
  });

  it('runs one save at a time: a second Save gets the first one\'s result', async () => {
    const { saves, drafts } = mount();
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    await waitFor(() => expect(drafts.current?.unsaved).toHaveLength(1));
    // The bar's Save, then activation's "Save and preview" before it ends.
    const [first, second] = await act(async () => {
      const both = [drafts.current!.saveAll(), drafts.current!.saveAll()];
      return Promise.all(both);
    });
    expect(first).toBeNull();
    expect(second).toBeNull();
    expect(saves).toEqual(['Plan=120']);
  });

  it('moves focus to the saved status once the Save button is gone', async () => {
    mount();
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    const save = screen.getByRole('button', { name: 'Save changes' });
    save.focus();
    fireEvent.click(save);
    const status = await screen.findByText('All changes saved.');
    await waitFor(() => expect(document.activeElement).toBe(status));
  });

  it('warns before the tab closes with edits', () => {
    mount();
    const quiet = new Event('beforeunload', { cancelable: true });
    act(() => { window.dispatchEvent(quiet); });
    expect(quiet.defaultPrevented).toBe(false);
    fireEvent.change(screen.getByLabelText('Plan'), { target: { value: '120' } });
    const leaving = new Event('beforeunload', { cancelable: true });
    act(() => { window.dispatchEvent(leaving); });
    expect(leaving.defaultPrevented).toBe(true);
  });
});
