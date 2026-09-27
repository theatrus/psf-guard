import { type ReactNode } from 'react';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { fireEvent, render, screen, waitFor } from '@testing-library/react';
import { http, HttpResponse } from 'msw';
import { describe, expect, it } from 'vitest';
import { server } from '../../test/msw-server';
import { AccessContext, useAccess } from '../../auth/access';
import ActivationPanel from '../director/ActivationPanel';
import type { DirectorActivationReport } from '../../api/directorTypes';

const ok = (data: unknown) => ({ success: true, data, error: null });
const rig = { id: 'aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa', name: 'RedCat', revision: 1 };
function report(applied: boolean): DirectorActivationReport {
  return {
    project: { id: 'project', name: 'Heart', revision: 1 }, framing_revision: 2, plan_revision: 3, panels: 2,
    rigs: [{ rig, catalog_slug: 'redcat', catalog_name: 'RedCat 61', profile_id: 'p', applied, warnings: [], changes: [
      { kind: 'project', action: 'create', name: 'Heart', detail: 'new Target Scheduler project' },
      { kind: 'target', action: 'create', name: 'Heart r1c1', detail: '02h 32m, +61° 27′' },
      { kind: 'target', action: 'create', name: 'Heart r2c1', detail: '02h 32m, +60° 03′' },
      { kind: 'plan', action: 'create', name: 'Heart r1c1 · Ha 300', detail: '72 frames of 300 s' },
      { kind: 'plan', action: 'create', name: 'Heart r2c1 · Ha 300', detail: '72 frames of 300 s' },
    ] }, { rig: { id: 'bbbbbbbb-bbbb-4bbb-8bbb-bbbbbbbbbbbb', name: 'Remote', revision: 1 }, catalog_slug: null, catalog_name: 'Remote', profile_id: null, applied: false, changes: [], warnings: ['Rig has no registered database on this server; push it through Sync later.'] }],
    warnings: [], preview_digest: 'd'.repeat(64), applied, activation_revision: applied ? 1 : null,
  };
}
function fixture(conflict = false) {
  const applies: Array<{ preview_digest: string }> = [];
  let previews = 0;
  server.use(
    http.get('/api/director/v1/projects/project/activation', () => HttpResponse.json(ok({ activation: null }))),
    http.post('/api/director/v1/projects/project/activation/preview', () => { previews++; return HttpResponse.json(ok(report(false))); }),
    http.post('/api/director/v1/projects/project/activation/apply', async ({ request }) => {
      applies.push(await request.json() as { preview_digest: string });
      if (conflict) return HttpResponse.json({ success: false, data: null, error: 'Director record conflicts with stored content; reload before retrying' }, { status: 409 });
      return HttpResponse.json(ok(report(true)));
    }),
  );
  return { applies, previewCount: () => previews };
}
function mount(canWrite = true) {
  const client = new QueryClient({ defaultOptions: { queries: { retry: false, gcTime: 0 } } });
  function Wrapper({ children }: { children: ReactNode }) {
    const access = useAccess();
    return <QueryClientProvider client={client}><AccessContext.Provider value={{ ...access, canWrite }}>{children}</AccessContext.Provider></QueryClientProvider>;
  }
  return render(<ActivationPanel projectId="project" />, { wrapper: Wrapper });
}

describe('Activation panel', () => {
  it('previews per rig, applies with the preview digest, and reports what landed', async () => {
    const { applies } = fixture(); mount();
    expect(screen.queryByRole('button', { name: /Apply to rig databases/ })).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole('button', { name: 'Preview activation' }));
    expect(await screen.findByText('Preview')).toBeInTheDocument();
    expect(screen.getByText('1 new', { selector: 'td' })).toBeInTheDocument();
    expect(screen.getAllByText('2 new', { selector: 'td' })).toHaveLength(2);
    expect(screen.getByText(/push it through Sync later/)).toBeInTheDocument();
    fireEvent.click(screen.getByRole('button', { name: 'Apply to rig databases' }));
    expect(await screen.findByText('Applied')).toBeInTheDocument();
    expect(applies).toEqual([{ preview_digest: 'd'.repeat(64) }]);
    expect(screen.getByText(/Activation revision 1/)).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Apply to rig databases' })).not.toBeInTheDocument();
  });

  it('drops a stale preview when apply is refused', async () => {
    const { previewCount } = fixture(true); mount();
    fireEvent.click(await screen.findByRole('button', { name: 'Preview activation' }));
    fireEvent.click(await screen.findByRole('button', { name: 'Apply to rig databases' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('Preview again before applying');
    await waitFor(() => expect(screen.queryByRole('button', { name: 'Apply to rig databases' })).not.toBeInTheDocument());
    expect(previewCount()).toBe(1);
  });

  it('offers nothing to a read-only account', async () => {
    fixture(); mount(false);
    expect(await screen.findByText('Read only')).toBeInTheDocument();
    expect(screen.queryByRole('button', { name: 'Preview activation' })).not.toBeInTheDocument();
  });
});
