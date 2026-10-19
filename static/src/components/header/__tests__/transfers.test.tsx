import { describe, it, expect, vi } from 'vitest';
import { render, screen, fireEvent } from '@testing-library/react';
import { jobItem, summarize } from '../activityItems';
import { TransferControls } from '../../director/CollaborationTransfers';
import type { TransferJob } from '../../../api/transferTypes';

const job: TransferJob = { id: 'one', connection_id: 'connection', revision: 3, state: 'uploading', catalog: 'redcat', label: 'M31', total_files: 4, completed_files: 1, total_bytes: 1000, completed_bytes: 200, sent_bytes: 100, current_file: 'frame.fits', error: null, next_attempt_ms: 0, warnings: null, manifest: null };
describe('Transfers activity', () => {
  it('keeps the transfer on its row for the row controls', () => {
    const item = jobItem({
      key: 'transfer:one', kind: 'transfer', title: 'Transfers', scope: 'redcat · M31',
      detail: '1/4 files · 0.0 MiB', status: 'running', background: false, automatic: false,
      percent: 30, stoppable: false, state_label: 'Uploading', transfer: job,
    });
    expect(item).toMatchObject({ kind: 'transfer', scope: 'redcat · M31', percent: 30, stateLabel: 'Uploading' });
    expect(item.transfer).toBe(job);
    expect(summarize([item]).percent).toBe(30);
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
