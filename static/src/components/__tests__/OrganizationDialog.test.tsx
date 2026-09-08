import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { fireEvent, render, screen, waitFor, within } from '@testing-library/react';
import userEvent from '@testing-library/user-event';
import { QueryClient, QueryClientProvider } from '@tanstack/react-query';
import { http, HttpResponse } from 'msw';
import { server } from '../../test/msw-server';
import type { OrganizationOperation, OrganizationPreview } from '../../api/types';
import OrganizationDialog, { type OrganizationScope } from '../OrganizationDialog';

const DB_ID = 'c925-review';
const moveScope: OrganizationScope = {
  kind: 'move_images',
  dbId: DB_ID,
  sourceTargetId: 11,
  sourceTargetName: 'Original field',
  sourceProjectId: 1,
  imageIds: [5, 6],
};

const projects = [
  { id: 1, profile_id: 'c925', name: 'Original import' },
  { id: 2, profile_id: 'c925', name: 'M 44 collection' },
  { id: 3, profile_id: 'ultracat', name: 'Another telescope' },
];

const targets = [
  { id: 11, project_id: 1, name: 'Original field', active: true, has_files: true },
  { id: 12, project_id: 1, name: 'Adjacent field', active: true, has_files: true },
  { id: 21, project_id: 2, name: 'M 44', active: true, has_files: true },
  { id: 31, project_id: 3, name: 'Other rig field', active: true, has_files: true },
];

const preview: OrganizationPreview = {
  fingerprint: 'preview-fingerprint',
  source_project_id: 1,
  source_project_name: 'Original import',
  source_target_id: 11,
  source_target_name: 'Original field',
  destination_project_id: 2,
  destination_project_name: 'M 44 collection',
  destination_target_id: 21,
  destination_target_name: 'M 44',
  images_moved: 2,
  exposure_plans_moved: 0,
  exposure_plans_created: 1,
  warnings: [],
};
const result = { project_id: 2, target_id: 21, images_moved: 2 };

function ok(data: unknown) {
  return HttpResponse.json({ success: true, data, error: null, status: 'ready' });
}

function deferred() {
  let resolve!: () => void;
  const promise = new Promise<void>(done => { resolve = done; });
  return { promise, resolve };
}

function openDialog(scope: OrganizationScope = moveScope) {
  const onClose = vi.fn();
  const onApplied = vi.fn();
  const queryClient = new QueryClient({
    defaultOptions: { queries: { retry: false, gcTime: 0 } },
  });
  render(
    <QueryClientProvider client={queryClient}>
      <OrganizationDialog scope={scope} onClose={onClose} onApplied={onApplied} />
    </QueryClientProvider>,
  );
  return { onClose, onApplied };
}

async function chooseExistingDestination() {
  await screen.findByRole('option', { name: 'M 44 collection' });
  await userEvent.selectOptions(screen.getByLabelText('Destination project'), '2');
  await userEvent.selectOptions(screen.getByLabelText('Destination target'), '21');
}

async function attemptClose() {
  await userEvent.click(screen.getByRole('button', { name: 'Close dialog' }));
  await userEvent.keyboard('{Escape}');
  fireEvent.mouseDown(screen.getByRole('presentation'));
  await userEvent.click(screen.getByRole('button', { name: 'Cancel' }));
}

beforeEach(() => {
  server.use(
    http.get('/api/db/:dbId/organization/destinations', () => ok({ projects, targets })),
    http.post('/api/db/:dbId/organization/preview', () => ok(preview)),
    http.post('/api/db/:dbId/organization/apply', () => ok(result)),
  );
});

afterEach(() => vi.restoreAllMocks());

describe('OrganizationDialog', () => {
  it('excludes other profiles and the source target, then applies only the scoped preview', async () => {
    const reads: string[] = [];
    const previews: Array<{ dbId: string; operation: unknown }> = [];
    const applies: Array<{ dbId: string; body: unknown }> = [];
    server.use(
      http.get('/api/db/:dbId/organization/destinations', ({ params }) => {
        reads.push(String(params.dbId));
        return ok({ projects, targets });
      }),
      http.post('/api/db/:dbId/organization/preview', async ({ params, request }) => {
        previews.push({ dbId: String(params.dbId), operation: await request.json() });
        return ok(preview);
      }),
      http.post('/api/db/:dbId/organization/apply', async ({ params, request }) => {
        applies.push({ dbId: String(params.dbId), body: await request.json() });
        return ok(result);
      }),
    );
    const { onClose, onApplied } = openDialog();
    expect(screen.queryByRole('button', { name: 'Apply move' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Preview move' })).toBeDisabled();
    await screen.findByRole('option', { name: 'Adjacent field' });
    expect(screen.queryByRole('option', { name: 'Another telescope' })).not.toBeInTheDocument();
    expect(screen.queryByRole('option', { name: 'Original field' })).not.toBeInTheDocument();
    await chooseExistingDestination();
    await userEvent.click(screen.getByRole('button', { name: 'Preview move' }));
    const apply = await screen.findByRole('button', { name: 'Apply move' });
    expect(applies).toEqual([]);
    await userEvent.click(apply);
    await waitFor(() => expect(onApplied).toHaveBeenCalledWith(result));
    expect(onClose).toHaveBeenCalledOnce();
    const operation = { kind: 'move_images', image_ids: [5, 6], destination: { target_id: 21 } };
    expect(previews).toEqual([{ dbId: DB_ID, operation }]);
    expect(applies).toEqual([{
      dbId: DB_ID,
      body: { operation, expected_fingerprint: preview.fingerprint },
    }]);
    expect(reads.length).toBeGreaterThanOrEqual(1);
    expect(reads.every(dbId => dbId === DB_ID)).toBe(true);
  });

  it('discards a preview when the destination name or project changes', async () => {
    const operations: OrganizationOperation[] = [];
    server.use(http.post('/api/db/:dbId/organization/preview', async ({ request }) => {
      operations.push(await request.json() as OrganizationOperation);
      return ok(preview);
    }));
    openDialog();
    await screen.findByRole('option', { name: 'Adjacent field' });
    await userEvent.selectOptions(screen.getByLabelText('Destination target'), 'new');
    const targetName = screen.getByLabelText('New target name');
    await userEvent.type(targetName, 'East panel');
    await userEvent.click(screen.getByRole('button', { name: 'Preview move' }));
    await screen.findByRole('button', { name: 'Apply move' });
    expect(screen.queryByLabelText('New target name')).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Edit destination' }));
    const editedTargetName = screen.getByLabelText('New target name');
    expect(editedTargetName).toHaveValue('East panel');
    await userEvent.clear(editedTargetName);
    await userEvent.type(editedTargetName, 'West panel');
    expect(screen.queryByRole('button', { name: 'Apply move' })).not.toBeInTheDocument();
    expect(screen.queryByRole('region', { name: 'Change preview' })).not.toBeInTheDocument();
    await userEvent.click(screen.getByRole('button', { name: 'Preview move' }));
    await screen.findByRole('button', { name: 'Apply move' });
    expect(operations).toEqual([
      { kind: 'move_images', image_ids: [5, 6], destination: { project_id: 1, new_target_name: 'East panel' } },
      { kind: 'move_images', image_ids: [5, 6], destination: { project_id: 1, new_target_name: 'West panel' } },
    ]);
    await userEvent.click(screen.getByRole('button', { name: 'Edit destination' }));
    await userEvent.selectOptions(screen.getByLabelText('Destination project'), '2');
    expect(screen.getByLabelText('Destination target')).toHaveValue('');
    expect(screen.queryByRole('button', { name: 'Apply move' })).not.toBeInTheDocument();
    expect(screen.getByRole('button', { name: 'Preview move' })).toBeDisabled();
  });

  it('shows the server apply error and requires a fresh preview before retrying', async () => {
    vi.spyOn(console, 'error').mockImplementation(() => undefined);
    let previewCount = 0;
    const fingerprints: string[] = [];
    server.use(
      http.post('/api/db/:dbId/organization/preview', () => {
        previewCount += 1;
        return ok({ ...preview, fingerprint: `preview-${previewCount}` });
      }),
      http.post('/api/db/:dbId/organization/apply', async ({ request }) => {
        const body = await request.json() as { expected_fingerprint: string };
        fingerprints.push(body.expected_fingerprint);
        return fingerprints.length === 1
          ? HttpResponse.json({ success: false, data: null, error: 'The catalog changed. Preview this move again.' }, { status: 409 })
          : ok(result);
      }),
    );
    const { onClose, onApplied } = openDialog();
    await chooseExistingDestination();
    await userEvent.click(screen.getByRole('button', { name: 'Preview move' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Apply move' }));
    expect(await screen.findByRole('alert')).toHaveTextContent('The catalog changed. Preview this move again.');
    expect(screen.queryByRole('button', { name: 'Apply move' })).not.toBeInTheDocument();
    expect(screen.getByLabelText('Destination project')).toHaveValue('2');
    expect(screen.getByLabelText('Destination target')).toHaveValue('21');
    expect(onClose).not.toHaveBeenCalled();
    expect(onApplied).not.toHaveBeenCalled();
    await userEvent.click(screen.getByRole('button', { name: 'Preview move' }));
    await userEvent.click(await screen.findByRole('button', { name: 'Apply move' }));
    await waitFor(() => expect(onApplied).toHaveBeenCalledOnce());
    expect(fingerprints).toEqual(['preview-1', 'preview-2']);
    expect(screen.queryByRole('alert')).not.toBeInTheDocument();
  });

  it('blocks duplicate requests, destination edits, and closing while preview or apply is pending', async () => {
    const pendingPreview = deferred();
    const pendingApply = deferred();
    let previewCount = 0;
    let applyCount = 0;
    server.use(
      http.post('/api/db/:dbId/organization/preview', async () => {
        previewCount += 1;
        await pendingPreview.promise;
        return ok(preview);
      }),
      http.post('/api/db/:dbId/organization/apply', async () => {
        applyCount += 1;
        await pendingApply.promise;
        return ok(result);
      }),
    );
    const { onClose, onApplied } = openDialog();
    await chooseExistingDestination();
    await userEvent.dblClick(screen.getByRole('button', { name: 'Preview move' }));
    await waitFor(() => expect(previewCount).toBe(1));
    expect(screen.getByRole('button', { name: 'Preparing preview...' })).toBeDisabled();
    expect(screen.getByLabelText('Destination project')).toBeDisabled();
    expect(screen.getByLabelText('Destination target')).toBeDisabled();
    await attemptClose();
    expect(onClose).not.toHaveBeenCalled();
    pendingPreview.resolve();
    await userEvent.dblClick(await screen.findByRole('button', { name: 'Apply move' }));
    await waitFor(() => expect(applyCount).toBe(1));
    expect(screen.getByRole('button', { name: 'Applying...' })).toBeDisabled();
    expect(screen.queryByLabelText('Destination project')).not.toBeInTheDocument();
    expect(screen.queryByLabelText('Destination target')).not.toBeInTheDocument();
    const back = screen.getByRole('button', { name: 'Edit destination' });
    expect(back).toBeDisabled();
    await userEvent.click(back);
    expect(screen.getByRole('button', { name: 'Applying...' })).toBeDisabled();
    await attemptClose();
    expect(onClose).not.toHaveBeenCalled();
    pendingApply.resolve();
    await waitFor(() => expect(onApplied).toHaveBeenCalledWith(result));
    expect(onClose).toHaveBeenCalledOnce();
    expect(previewCount).toBe(1);
    expect(applyCount).toBe(1);
  });

  it('merges into existing targets only and sends the source and destination IDs', async () => {
    const requests: unknown[] = [];
    server.use(http.post('/api/db/:dbId/organization/preview', async ({ request }) => {
      requests.push(await request.json());
      return ok({ ...preview, exposure_plans_created: 0, exposure_plans_moved: 1 });
    }));
    openDialog({
      kind: 'merge_targets',
      dbId: DB_ID,
      sourceProjectId: 1,
      sourceTargetId: 11,
      sourceTargetName: 'Original field',
    });
    const dialog = screen.getByRole('dialog', { name: 'Merge targets' });
    await screen.findByRole('option', { name: 'Adjacent field' });
    expect(within(dialog).queryByRole('option', { name: 'New project...' })).not.toBeInTheDocument();
    expect(within(dialog).queryByRole('option', { name: 'New target...' })).not.toBeInTheDocument();
    await chooseExistingDestination();
    await userEvent.click(screen.getByRole('button', { name: 'Preview merge' }));
    await screen.findByRole('button', { name: 'Apply merge' });
    expect(requests).toEqual([{ kind: 'merge_targets', source_target_id: 11, destination_target_id: 21 }]);
    expect(screen.getByRole('region', { name: 'Change preview' })).toHaveTextContent('The source target will be removed');
  });
});
