import { useState } from 'react';
import { act, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { createMemoryRouter, Link, RouterProvider } from 'react-router-dom';
import { afterEach, describe, expect, it, vi } from 'vitest';
import { DraftProvider, EditedMark, SaveBar } from '../pageDrafts';
import { useDraftSection, usePageDrafts } from '../pageDraftsState';

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

function Page({ saves, failPlan = false }: { saves: string[]; failPlan?: boolean | string }) {
  const drafts = usePageDrafts();
  return <DraftProvider drafts={drafts}>
    <SaveBar drafts={drafts} canWrite />
    <h3>Plan<EditedMark drafts={drafts} id="plan" /></h3>
    {/* Registered out of order: saving still goes framing, then plan. */}
    <Field id="plan" label="Plan" order={2} saved="40" saves={saves} fail={failPlan} />
    <Field id="framing" label="Framing" order={1} saved="M 1" saves={saves} />
    <Link to="/elsewhere">Library</Link>
  </DraftProvider>;
}

function mount(failPlan: boolean | string = false) {
  const saves: string[] = [];
  const router = createMemoryRouter([
    { path: '/', element: <Page saves={saves} failPlan={failPlan} /> },
    { path: '/elsewhere', element: <p>Library page</p> },
  ]);
  render(<RouterProvider router={router} />);
  return { saves, router };
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
