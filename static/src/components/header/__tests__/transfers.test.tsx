import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { transferItem, summarize, activityItems } from '../activityItems';
import { TransferControls } from '../../director/CollaborationTransfers';
import type { TransferJob } from '../../../api/transferTypes';

const job: TransferJob = { id: 'one', connection_id: 'connection', revision: 3, state: 'uploading', catalog: 'redcat', label: 'M31', total_files: 4, completed_files: 1, total_bytes: 1000, completed_bytes: 200, sent_bytes: 100, current_file: 'frame.fits', error: null, next_attempt_ms: 0, warnings: null, manifest: null };
describe('Transfers activity', () => {
  it('reports byte progress and keeps source scope', () => {
    expect(transferItem(job)).toMatchObject({ kind: 'transfer', title: 'Transfers', scope: 'redcat · M31', percent: 30, status: 'running', stateLabel: 'Uploading' });
  });
  it.each(['paused', 'review', 'retry', 'needs_review'] as const)('does not spin or average a %s job into running progress', state => {
    const item = transferItem({ ...job, state });
    expect(item.status).toBe('waiting'); expect(item.percent).toBeNull();
    expect(summarize([item]).percent).toBeNull();
  });
  it('keeps unfinished transfers visible, not completed history', () => {
    expect(activityItems([], [], undefined, undefined, {}, [job, { ...job, id: 'done', state: 'complete' }])).toHaveLength(1);
  });
  it('keeps transfer progress separate from waiting automatic work', () => {
    const items = activityItems([], [], undefined, undefined, { reapplies: [{ database_id: 'rig', project_id: 1, target_id: 2, label: 'M31 OIII', waiting_for: 'a stack build' }] }, [job, { ...job, id: 'paused', state: 'paused' }]);
    expect(summarize(items)).toMatchObject({ count: 3, running: 1, waiting: 2, queued: 0, percent: 30 });
    expect(items[0].kind).toBe('transfer');
    expect(items.find(i => i.key === 'transfer:paused')?.stateLabel).toBe('Paused');
  });
  it('pauses running work and resumes a paused job', () => {
    const run = vi.fn(); const view = render(<TransferControls job={job} busy={false} run={run} />);
    fireEvent.click(screen.getByRole('button', { name: 'Pause transfer' })); expect(run).toHaveBeenCalledWith('pause');
    view.rerender(<TransferControls job={{ ...job, state: 'paused' }} busy={false} run={run} />);
    fireEvent.click(screen.getByRole('button', { name: 'Resume transfer' })); expect(run).toHaveBeenCalledWith('resume');
  });
  it('requires an explicit approval for a reviewed package', () => {
    const run = vi.fn(); render(<TransferControls job={{ ...job, state: 'review' }} busy={false} run={run} />);
    fireEvent.click(screen.getByRole('button', { name: 'Upload reviewed files' })); expect(run).toHaveBeenCalledWith('approve');
  });
});
